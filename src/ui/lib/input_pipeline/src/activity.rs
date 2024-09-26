// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#![cfg(fuchsia_api_level_at_least = "HEAD")]
use anyhow::{Context, Error};
use async_utils::hanging_get::server::{HangingGet, Publisher};
use fidl_fuchsia_input_interaction::{
    NotifierRequest, NotifierRequestStream, NotifierWatchStateResponder, State,
};
use fidl_fuchsia_input_interaction_observation::{
    AggregatorRequest, AggregatorRequestStream, HandoffWakeError,
};
use fidl_fuchsia_power_system::{ActivityGovernorMarker, ActivityGovernorProxy};
use fuchsia_async::{Task, Timer};
use fuchsia_component::client::connect_to_protocol;
use fuchsia_zircon as zx;
use futures::StreamExt;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct LeaseHolder {
    activity_governor: ActivityGovernorProxy,
    wake_lease: Option<zx::EventPair>,
}

impl LeaseHolder {
    async fn new(activity_governor: ActivityGovernorProxy) -> Result<Self, Error> {
        let wake_lease = activity_governor
            .take_wake_lease("scene_manager")
            .await
            .context("cannot get wake lease from SAG")?;
        tracing::info!("Activity Manager created a wake lease during initialization.");

        Ok(Self { activity_governor, wake_lease: Some(wake_lease) })
    }

    async fn create_lease(&mut self) -> Result<(), Error> {
        if self.wake_lease.is_some() {
            tracing::warn!("Activity Manager already held a wake lease when trying to create one, please investigate.");
            return Ok(());
        }

        let wake_lease = self
            .activity_governor
            .take_wake_lease("scene_manager")
            .await
            .context("cannot get wake lease from SAG")?;
        self.wake_lease = Some(wake_lease);
        tracing::info!("Activity Manager created a wake lease due to receiving recent user input.");

        Ok(())
    }

    fn drop_lease(&mut self) {
        if let Some(lease) = self.wake_lease.take() {
            tracing::info!("Activity Manager is dropping the wake lease due to not receiving any recent user input.");
            std::mem::drop(lease);
        } else {
            tracing::warn!("Activity Manager was not holding a wake lease when trying to drop one, please investigate.");
        }
    }

    #[cfg(test)]
    fn is_holding_lease(&self) -> bool {
        self.wake_lease.is_some()
    }
}

type NotifyFn = Box<dyn Fn(&State, NotifierWatchStateResponder) -> bool>;
type InteractionHangingGet = HangingGet<State, NotifierWatchStateResponder, NotifyFn>;
type StatePublisher = Publisher<State, NotifierWatchStateResponder, NotifyFn>;

struct StateTransitioner {
    idle_threshold_ms: zx::Duration,
    idle_transition_task: Cell<Option<Task<()>>>,
    last_event_time: RefCell<zx::MonotonicTime>,

    // To support power management, the caller must provide `Some` value for
    // `lease_holder`. The existence of a `LeaseHolder` implies power framework
    // availability in the platform.
    lease_holder: Option<Rc<RefCell<LeaseHolder>>>,
    state_publisher: StatePublisher,
}

impl StateTransitioner {
    pub fn new(
        initial_timestamp: zx::MonotonicTime,
        idle_threshold_ms: zx::Duration,
        state_publisher: StatePublisher,
        lease_holder: Option<Rc<RefCell<LeaseHolder>>>,
    ) -> Self {
        tracing::info!(
            "Activity Manager is initialized with idle_threshold_ms: {:?}",
            idle_threshold_ms.into_millis()
        );

        let task = Self::create_idle_transition_task(
            initial_timestamp + idle_threshold_ms,
            state_publisher.clone(),
            lease_holder.clone(),
        );
        Self {
            idle_threshold_ms,
            idle_transition_task: Cell::new(Some(task)),
            last_event_time: RefCell::new(initial_timestamp),
            lease_holder,
            state_publisher,
        }
    }

    pub async fn transition_to_active(
        state_publisher: &StatePublisher,
        lease_holder: &Option<Rc<RefCell<LeaseHolder>>>,
    ) {
        if let Some(holder) = lease_holder {
            if let Err(e) = holder.borrow_mut().create_lease().await {
                tracing::warn!(
                    "Unable to create lease, system may incorrectly go into suspend: {:?}",
                    e
                );
            };
        }
        state_publisher.set(State::Active);
    }

    pub fn create_idle_transition_task(
        timeout: zx::MonotonicTime,
        state_publisher: StatePublisher,
        lease_holder: Option<Rc<RefCell<LeaseHolder>>>,
    ) -> Task<()> {
        Task::local(async move {
            Timer::new(timeout).await;
            lease_holder.and_then(|holder| Some(holder.borrow_mut().drop_lease()));
            state_publisher.set(State::Idle);
        })
    }

    pub async fn transition_to_idle_after_new_time(&self, event_time: zx::MonotonicTime) {
        if *self.last_event_time.borrow() > event_time {
            return;
        }

        *self.last_event_time.borrow_mut() = event_time;
        if let Some(t) = self.idle_transition_task.take() {
            // If the task returns a completed output, we can assume the
            // state has transitioned to Idle.
            if let Some(()) = t.cancel().await {
                Self::transition_to_active(&self.state_publisher, &self.lease_holder).await;
            }
        }

        self.idle_transition_task.set(Some(Self::create_idle_transition_task(
            event_time + self.idle_threshold_ms,
            self.state_publisher.clone(),
            self.lease_holder.clone(),
        )));
    }

    #[cfg(test)]
    fn is_holding_lease(&self) -> bool {
        if let Some(holder) = &self.lease_holder {
            return holder.borrow().is_holding_lease();
        }

        false
    }
}

/// An [`ActivityManager`] tracks the state of user input interaction activity.
pub struct ActivityManager {
    state_transitioner: StateTransitioner,
    interaction_hanging_get: RefCell<InteractionHangingGet>,
    suspend_enabled: bool,
}

impl ActivityManager {
    /// Creates a new [`ActivityManager`] that listens for user input
    /// input interactions and notifies clients of activity state changes.
    pub async fn new(idle_threshold_ms: zx::Duration, suspend_enabled: bool) -> Rc<Self> {
        let lease_holder = match suspend_enabled {
            true => {
                let activity_governor = connect_to_protocol::<ActivityGovernorMarker>()
                    .expect("connect to fuchsia.power.system.ActivityGovernor");
                match LeaseHolder::new(activity_governor).await {
                    Ok(holder) => Some(Rc::new(RefCell::new(holder))),
                    Err(e) => {
                        tracing::error!("Unable to integrate with power, system may incorrectly enter suspend: {:?}", e);
                        None
                    }
                }
            }
            false => None,
        };

        Self::new_internal(
            idle_threshold_ms,
            zx::MonotonicTime::get(),
            suspend_enabled,
            lease_holder,
        )
        .await
    }

    #[cfg(test)]
    /// Sets the initial idleness timer relative to fake time at 0 for tests.
    async fn new_for_test(
        idle_threshold_ms: zx::Duration,
        suspend_enabled: bool,
        lease_holder: Option<Rc<RefCell<LeaseHolder>>>,
    ) -> Rc<Self> {
        fuchsia_async::TestExecutor::advance_to(zx::MonotonicTime::ZERO.into()).await;
        Self::new_internal(
            idle_threshold_ms,
            zx::MonotonicTime::ZERO,
            suspend_enabled,
            lease_holder,
        )
        .await
    }

    async fn new_internal(
        idle_threshold_ms: zx::Duration,
        initial_timestamp: zx::MonotonicTime,
        suspend_enabled: bool,
        lease_holder: Option<Rc<RefCell<LeaseHolder>>>,
    ) -> Rc<Self> {
        let initial_state = State::Active;
        let interaction_hanging_get = ActivityManager::init_hanging_get(initial_state);
        let state_publisher = interaction_hanging_get.new_publisher();

        Rc::new(Self {
            interaction_hanging_get: RefCell::new(interaction_hanging_get),
            state_transitioner: StateTransitioner::new(
                initial_timestamp,
                idle_threshold_ms,
                state_publisher,
                lease_holder,
            ),
            suspend_enabled,
        })
    }

    /// Handles the request stream for
    /// fuchsia.input.interaction.observation.Aggregator.
    ///
    /// # Parameters
    /// `stream`: The `AggregatorRequestStream` to be handled.
    pub async fn handle_interaction_aggregator_request_stream(
        self: Rc<Self>,
        mut stream: AggregatorRequestStream,
    ) -> Result<(), Error> {
        while let Some(aggregator_request) = stream.next().await {
            match aggregator_request {
                Ok(AggregatorRequest::ReportDiscreteActivity { event_time, responder }) => {
                    // Clamp the time to now so that clients cannot send events far off
                    // in the future to keep the system always active.
                    // Note: We use the global executor to get the current time instead
                    // of the kernel so that we do not unnecessarily clamp
                    // test-injected times.
                    let event_time = zx::MonotonicTime::from_nanos(event_time)
                        .clamp(zx::MonotonicTime::ZERO, fuchsia_async::Time::now().into_zx());

                    self.state_transitioner.transition_to_idle_after_new_time(event_time).await;

                    let _: Result<(), fidl::Error> = responder.send();
                }
                Ok(AggregatorRequest::HandoffWake { responder }) => {
                    if self.suspend_enabled {
                        let event_time = fuchsia_async::Time::now().into_zx();
                        self.state_transitioner.transition_to_idle_after_new_time(event_time).await;

                        if let Err(e) = responder.send(Ok(())) {
                            tracing::warn!("Error sending a response to HandoffWake: {:?}", e);
                        }
                    } else {
                        if let Err(e) = responder.send(Err(HandoffWakeError::PowerNotAvailable)) {
                            tracing::warn!(
                                "Error sending an error response to HandoffWake: {:?}",
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Error serving fuchsia.input.interaction.observation.Aggregator: {:?}",
                        e
                    );
                }
            }
        }

        Ok(())
    }

    /// Handles the request stream for fuchsia.input.interaction.Notifier.
    ///
    /// # Parameters
    /// `stream`: The `NotifierRequestStream` to be handled.
    pub async fn handle_interaction_notifier_request_stream(
        self: Rc<Self>,
        mut stream: NotifierRequestStream,
    ) -> Result<(), Error> {
        let subscriber = self.interaction_hanging_get.borrow_mut().new_subscriber();

        while let Some(notifier_request) = stream.next().await {
            let NotifierRequest::WatchState { responder } = notifier_request?;
            subscriber.register(responder)?;
        }

        Ok(())
    }

    fn init_hanging_get(initial_state: State) -> InteractionHangingGet {
        let notify_fn: NotifyFn = Box::new(|state, responder| {
            if responder.send(*state).is_err() {
                tracing::info!("Failed to send user input interaction state");
            }

            true
        });

        InteractionHangingGet::new(initial_state, notify_fn)
    }

    #[cfg(test)]
    fn is_holding_lease(&self) -> bool {
        self.state_transitioner.is_holding_lease()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use async_utils::hanging_get::client::HangingGetStream;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_input_interaction::{NotifierMarker, NotifierProxy};
    use fidl_fuchsia_input_interaction_observation::{AggregatorMarker, AggregatorProxy};
    use fidl_fuchsia_power_system::{ActivityGovernorMarker, ActivityGovernorRequest};
    use fuchsia_async::TestExecutor;
    use futures::pin_mut;
    use std::task::Poll;
    use test_case::test_case;

    const ACTIVITY_TIMEOUT: zx::Duration = zx::Duration::from_millis(5000);

    async fn create_activity_manager(suspend_enabled: bool) -> Rc<ActivityManager> {
        let lease_holder = match suspend_enabled {
            true => {
                let holder = LeaseHolder::new(fake_activity_governor_server())
                    .await
                    .expect("create lease holder for test");
                Some(Rc::new(RefCell::new(holder)))
            }
            false => None,
        };

        ActivityManager::new_for_test(ACTIVITY_TIMEOUT, suspend_enabled, lease_holder).await
    }

    fn create_interaction_aggregator_proxy(
        activity_manager: Rc<ActivityManager>,
    ) -> AggregatorProxy {
        let (aggregator_proxy, aggregator_stream) = create_proxy_and_stream::<AggregatorMarker>()
            .expect("Failed to create aggregator proxy");

        Task::local(async move {
            if activity_manager
                .handle_interaction_aggregator_request_stream(aggregator_stream)
                .await
                .is_err()
            {
                panic!("Failed to handle aggregator request stream");
            }
        })
        .detach();

        aggregator_proxy
    }

    fn create_interaction_notifier_proxy(activity_manager: Rc<ActivityManager>) -> NotifierProxy {
        let (notifier_proxy, notifier_stream) =
            create_proxy_and_stream::<NotifierMarker>().expect("Failed to create notifier proxy");

        let stream_fut =
            activity_manager.clone().handle_interaction_notifier_request_stream(notifier_stream);

        Task::local(async move {
            if stream_fut.await.is_err() {
                panic!("Failed to handle notifier request stream");
            }
        })
        .detach();

        notifier_proxy
    }

    fn fake_activity_governor_server() -> ActivityGovernorProxy {
        let (proxy, mut stream) = create_proxy_and_stream::<ActivityGovernorMarker>()
            .expect("Failed to create activity governor proxy");
        Task::local(async move {
            while let Some(request) = stream.next().await {
                match request {
                    Ok(ActivityGovernorRequest::TakeWakeLease { responder, .. }) => {
                        let (_, fake_wake_lease) = zx::EventPair::create();
                        responder.send(fake_wake_lease).expect("failed to send fake wake lease");
                    }
                    Ok(unexpected) => {
                        tracing::warn!(
                            "Unexpected request {unexpected:?} serving fuchsia.power.system.ActivityGovernor"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Error serving fuchsia.power.system.ActivityGovernor: {:?}",
                            e
                        );
                    }
                }
            }
        })
        .detach();

        proxy
    }

    #[test_case(true; "Suspend enabled")]
    #[test_case(false; "Suspend disabled")]
    #[fuchsia::test(allow_stalls = false)]
    async fn aggregator_reports_activity(suspend_enabled: bool) {
        let activity_manager = create_activity_manager(suspend_enabled).await;
        let proxy = create_interaction_aggregator_proxy(activity_manager.clone());
        proxy.report_discrete_activity(0).await.expect("Failed to report activity");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn aggregator_handoff_wake_ok_when_suspend_enabled() {
        let activity_manager = create_activity_manager(/* suspend_enabled */ true).await;
        let proxy = create_interaction_aggregator_proxy(activity_manager.clone());
        assert_matches!(proxy.handoff_wake().await, Ok(Ok(())));
        assert_eq!(activity_manager.is_holding_lease(), true);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn aggregator_handoff_wake_error_when_suspend_disabled() {
        let activity_manager = create_activity_manager(/* suspend_enabled */ false).await;
        let proxy = create_interaction_aggregator_proxy(activity_manager.clone());
        assert_matches!(proxy.handoff_wake().await, Ok(Err(HandoffWakeError::PowerNotAvailable)));
        assert_eq!(activity_manager.is_holding_lease(), false);
    }

    #[test_case(true; "Suspend enabled")]
    #[test_case(false; "Suspend disabled")]
    #[fuchsia::test(allow_stalls = false)]
    async fn notifier_sends_initial_state(suspend_enabled: bool) {
        let activity_manager = create_activity_manager(suspend_enabled).await;
        let notifier_proxy = create_interaction_notifier_proxy(activity_manager.clone());
        let state = notifier_proxy.watch_state().await.expect("Failed to get interaction state");
        assert_eq!(state, State::Active);
        assert_eq!(activity_manager.is_holding_lease(), suspend_enabled);
    }

    #[test_case(true; "Suspend enabled")]
    #[test_case(false; "Suspend disabled")]
    #[fuchsia::test]
    fn notifier_sends_idle_state_after_timeout(suspend_enabled: bool) -> Result<(), Error> {
        let mut executor = TestExecutor::new_with_fake_time();

        let activity_manager_fut = create_activity_manager(suspend_enabled);
        pin_mut!(activity_manager_fut);
        let activity_manager_res = executor.run_until_stalled(&mut activity_manager_fut);
        let activity_manager = match activity_manager_res {
            Poll::Ready(manager) => manager,
            _ => panic!("Unable to create activity manager"),
        };

        let notifier_proxy = create_interaction_notifier_proxy(activity_manager.clone());

        // Initial state is active.
        let mut watch_state_stream =
            HangingGetStream::new(notifier_proxy, NotifierProxy::watch_state);
        let state_fut = watch_state_stream.next();
        pin_mut!(state_fut);
        let initial_state = executor.run_until_stalled(&mut state_fut);
        assert_matches!(initial_state, Poll::Ready(Some(Ok(State::Active))));
        assert_eq!(activity_manager.is_holding_lease(), suspend_enabled);

        // Skip ahead by the activity timeout.
        executor.set_fake_time(fuchsia_async::Time::after(ACTIVITY_TIMEOUT));

        // State transitions to Idle.
        let idle_state_fut = watch_state_stream.next();
        pin_mut!(idle_state_fut);
        let initial_state = executor.run_until_stalled(&mut idle_state_fut);
        assert_matches!(initial_state, Poll::Ready(Some(Ok(State::Idle))));
        assert_eq!(activity_manager.is_holding_lease(), false);

        Ok(())
    }

    #[test_case(true; "Suspend enabled")]
    #[test_case(false; "Suspend disabled")]
    #[fuchsia::test]
    fn notifier_sends_active_state_with_report_discrete_activity(
        suspend_enabled: bool,
    ) -> Result<(), Error> {
        let mut executor = TestExecutor::new_with_fake_time();

        let activity_manager_fut = create_activity_manager(suspend_enabled);
        pin_mut!(activity_manager_fut);
        let activity_manager_res = executor.run_until_stalled(&mut activity_manager_fut);
        let activity_manager = match activity_manager_res {
            Poll::Ready(manager) => manager,
            _ => panic!("Unable to create activity manager"),
        };

        let notifier_proxy = create_interaction_notifier_proxy(activity_manager.clone());

        // Initial state is active.
        let mut watch_state_stream =
            HangingGetStream::new(notifier_proxy, NotifierProxy::watch_state);
        let state_fut = watch_state_stream.next();
        pin_mut!(state_fut);
        let initial_state = executor.run_until_stalled(&mut state_fut);
        assert_matches!(initial_state, Poll::Ready(Some(Ok(State::Active))));
        assert_eq!(activity_manager.is_holding_lease(), suspend_enabled);

        // Skip ahead by the activity timeout.
        executor.set_fake_time(fuchsia_async::Time::after(ACTIVITY_TIMEOUT));

        // State transitions to Idle.
        let idle_state_fut = watch_state_stream.next();
        pin_mut!(idle_state_fut);
        let initial_state = executor.run_until_stalled(&mut idle_state_fut);
        assert_matches!(initial_state, Poll::Ready(Some(Ok(State::Idle))));
        assert_eq!(activity_manager.is_holding_lease(), false);

        // Send an activity.
        let proxy = create_interaction_aggregator_proxy(activity_manager.clone());
        let report_fut = proxy.report_discrete_activity(ACTIVITY_TIMEOUT.into_nanos());
        pin_mut!(report_fut);
        assert!(executor.run_until_stalled(&mut report_fut).is_ready());

        // State transitions to Active.
        let active_state_fut = watch_state_stream.next();
        pin_mut!(active_state_fut);
        let initial_state = executor.run_until_stalled(&mut active_state_fut);
        assert_matches!(initial_state, Poll::Ready(Some(Ok(State::Active))));
        assert_eq!(activity_manager.is_holding_lease(), suspend_enabled);

        Ok(())
    }

    #[fuchsia::test]
    fn notifier_sends_active_state_with_handoff_wake_suspend_enabled() -> Result<(), Error> {
        let mut executor = TestExecutor::new_with_fake_time();

        let activity_manager_fut = create_activity_manager(/* suspend_enabled */ true);
        pin_mut!(activity_manager_fut);
        let activity_manager_res = executor.run_until_stalled(&mut activity_manager_fut);
        let activity_manager = match activity_manager_res {
            Poll::Ready(manager) => manager,
            _ => panic!("Unable to create activity manager"),
        };

        let notifier_proxy = create_interaction_notifier_proxy(activity_manager.clone());

        // Initial state is active.
        let mut watch_state_stream =
            HangingGetStream::new(notifier_proxy, NotifierProxy::watch_state);
        let state_fut = watch_state_stream.next();
        pin_mut!(state_fut);
        let initial_state = executor.run_until_stalled(&mut state_fut);
        assert_matches!(initial_state, Poll::Ready(Some(Ok(State::Active))));
        assert_eq!(activity_manager.is_holding_lease(), true);

        // Skip ahead by the activity timeout.
        executor.set_fake_time(fuchsia_async::Time::after(ACTIVITY_TIMEOUT));

        // State transitions to Idle.
        let idle_state_fut = watch_state_stream.next();
        pin_mut!(idle_state_fut);
        let initial_state = executor.run_until_stalled(&mut idle_state_fut);
        assert_matches!(initial_state, Poll::Ready(Some(Ok(State::Idle))));
        assert_eq!(activity_manager.is_holding_lease(), false);

        // Send an activity.
        let proxy = create_interaction_aggregator_proxy(activity_manager.clone());
        let handoff_fut = proxy.handoff_wake();
        pin_mut!(handoff_fut);
        let handoff_response = executor.run_until_stalled(&mut handoff_fut);
        assert_matches!(handoff_response, Poll::Ready(Ok(Ok(()))));

        // State transitions to Active.
        let active_state_fut = watch_state_stream.next();
        pin_mut!(active_state_fut);
        let initial_state = executor.run_until_stalled(&mut active_state_fut);
        assert_matches!(initial_state, Poll::Ready(Some(Ok(State::Active))));
        assert_eq!(activity_manager.is_holding_lease(), true);

        Ok(())
    }

    #[fuchsia::test]
    fn notifier_sends_nothing_with_handoff_wake_suspend_disabled() -> Result<(), Error> {
        let mut executor = TestExecutor::new_with_fake_time();

        let activity_manager_fut = create_activity_manager(/* suspend_enabled */ false);
        pin_mut!(activity_manager_fut);
        let activity_manager_res = executor.run_until_stalled(&mut activity_manager_fut);
        let activity_manager = match activity_manager_res {
            Poll::Ready(manager) => manager,
            _ => panic!("Unable to create activity manager"),
        };

        let notifier_proxy = create_interaction_notifier_proxy(activity_manager.clone());

        // Initial state is active.
        let mut watch_state_stream =
            HangingGetStream::new(notifier_proxy, NotifierProxy::watch_state);
        let state_fut = watch_state_stream.next();
        pin_mut!(state_fut);
        let initial_state = executor.run_until_stalled(&mut state_fut);
        assert_matches!(initial_state, Poll::Ready(Some(Ok(State::Active))));
        assert_eq!(activity_manager.is_holding_lease(), false);

        // Skip ahead by the activity timeout.
        executor.set_fake_time(fuchsia_async::Time::after(ACTIVITY_TIMEOUT));

        // State transitions to Idle.
        let idle_state_fut = watch_state_stream.next();
        pin_mut!(idle_state_fut);
        let initial_state = executor.run_until_stalled(&mut idle_state_fut);
        assert_matches!(initial_state, Poll::Ready(Some(Ok(State::Idle))));
        assert_eq!(activity_manager.is_holding_lease(), false);

        // Send an activity.
        let proxy = create_interaction_aggregator_proxy(activity_manager.clone());
        let handoff_fut = proxy.handoff_wake();
        pin_mut!(handoff_fut);
        let handoff_response = executor.run_until_stalled(&mut handoff_fut);
        assert_matches!(
            handoff_response,
            Poll::Ready(Ok(Err(HandoffWakeError::PowerNotAvailable)))
        );
        assert_eq!(activity_manager.is_holding_lease(), false);

        // Idle state does not change.
        let watch_state_fut = watch_state_stream.next();
        pin_mut!(watch_state_fut);
        let watch_state_res = executor.run_until_stalled(&mut watch_state_fut);
        assert_matches!(watch_state_res, Poll::Pending);

        Ok(())
    }

    #[test_case(true; "Suspend enabled")]
    #[test_case(false; "Suspend disabled")]
    #[fuchsia::test]
    fn activity_manager_drops_first_timer_on_activity(suspend_enabled: bool) -> Result<(), Error> {
        // This test does the following:
        //   - Start an activity manager, whose initial timeout is set to
        //     ACTIVITY_TIMEOUT.
        //   - Send an activity at time ACTIVITY_TIMEOUT / 2.
        //   - Observe that after ACTIVITY_TIMEOUT transpires, the initial
        //     timeout to transition to idle state _does not_ fire, as we
        //     expect it to be replaced by a new timeout in response to the
        //     injected activity.
        //   - Observe that after ACTIVITY_TIMEOUT * 1.5 transpires, the second
        //     timeout to transition to idle state _does_ fire.
        // Because division will round to 0, odd-number timeouts could cause an
        // incorrect implementation to still pass the test. In order to catch
        // these cases, we first assert that ACTIVITY_TIMEOUT is an even number.
        assert_eq!(ACTIVITY_TIMEOUT.into_nanos() % 2, 0);

        let mut executor = TestExecutor::new_with_fake_time();

        let activity_manager_fut = create_activity_manager(suspend_enabled);
        pin_mut!(activity_manager_fut);
        let activity_manager_res = executor.run_until_stalled(&mut activity_manager_fut);
        let activity_manager = match activity_manager_res {
            Poll::Ready(manager) => manager,
            _ => panic!("Unable to create activity manager"),
        };
        let notifier_proxy = create_interaction_notifier_proxy(activity_manager.clone());

        // Initial state is active.
        let mut watch_state_stream =
            HangingGetStream::new(notifier_proxy, NotifierProxy::watch_state);
        let state_fut = watch_state_stream.next();
        pin_mut!(state_fut);
        let initial_state = executor.run_until_stalled(&mut state_fut);
        assert_matches!(initial_state, Poll::Ready(Some(Ok(State::Active))));
        assert_eq!(activity_manager.is_holding_lease(), suspend_enabled);

        // Skip ahead by half the activity timeout.
        executor.set_fake_time(fuchsia_async::Time::after(ACTIVITY_TIMEOUT / 2));

        // Send an activity, replacing the initial idleness timer.
        let proxy = create_interaction_aggregator_proxy(activity_manager.clone());
        let report_fut = proxy.report_discrete_activity((ACTIVITY_TIMEOUT / 2).into_nanos());
        pin_mut!(report_fut);
        assert!(executor.run_until_stalled(&mut report_fut).is_ready());

        // Skip ahead by half the activity timeout.
        executor.set_fake_time(fuchsia_async::Time::after(ACTIVITY_TIMEOUT / 2));

        // Initial state does not change.
        let watch_state_fut = watch_state_stream.next();
        pin_mut!(watch_state_fut);
        let watch_state_res = executor.run_until_stalled(&mut watch_state_fut);
        assert_matches!(watch_state_res, Poll::Pending);
        assert_eq!(activity_manager.is_holding_lease(), suspend_enabled);

        // Skip ahead by half the activity timeout.
        executor.set_fake_time(fuchsia_async::Time::after(ACTIVITY_TIMEOUT / 2));

        // Activity state does change.
        let watch_state_res = executor.run_until_stalled(&mut watch_state_fut);
        assert_matches!(watch_state_res, Poll::Ready(Some(Ok(State::Idle))));
        assert_eq!(activity_manager.is_holding_lease(), false);

        Ok(())
    }

    #[test_case(true; "Suspend enabled")]
    #[test_case(false; "Suspend disabled")]
    #[fuchsia::test]
    fn activity_manager_drops_late_activities(suspend_enabled: bool) -> Result<(), Error> {
        let mut executor = TestExecutor::new_with_fake_time();

        let activity_manager_fut = create_activity_manager(suspend_enabled);
        pin_mut!(activity_manager_fut);
        let activity_manager_res = executor.run_until_stalled(&mut activity_manager_fut);
        let activity_manager = match activity_manager_res {
            Poll::Ready(manager) => manager,
            _ => panic!("Unable to create activity manager"),
        };
        let notifier_proxy = create_interaction_notifier_proxy(activity_manager.clone());

        // Initial state is active.
        let mut watch_state_stream =
            HangingGetStream::new(notifier_proxy, NotifierProxy::watch_state);
        let state_fut = watch_state_stream.next();
        pin_mut!(state_fut);
        let watch_state_res = executor.run_until_stalled(&mut state_fut);
        assert_matches!(watch_state_res, Poll::Ready(Some(Ok(State::Active))));
        assert_eq!(activity_manager.is_holding_lease(), suspend_enabled);

        // Skip ahead by half the activity timeout.
        executor.set_fake_time(fuchsia_async::Time::after(ACTIVITY_TIMEOUT / 2));

        // Send an activity, replacing the initial idleness timer.
        let proxy = create_interaction_aggregator_proxy(activity_manager.clone());
        let report_fut = proxy.report_discrete_activity((ACTIVITY_TIMEOUT / 2).into_nanos());
        pin_mut!(report_fut);
        assert!(executor.run_until_stalled(&mut report_fut).is_ready());

        // Skip ahead by half the activity timeout.
        executor.set_fake_time(fuchsia_async::Time::after(ACTIVITY_TIMEOUT / 2));

        // Send an activity with an earlier event time.
        let proxy = create_interaction_aggregator_proxy(activity_manager.clone());
        let report_fut = proxy.report_discrete_activity(0);
        pin_mut!(report_fut);
        assert!(executor.run_until_stalled(&mut report_fut).is_ready());

        // Initial task does not transition to idle, nor does one from the
        // "earlier" activity that was received later.
        let watch_state_fut = watch_state_stream.next();
        pin_mut!(watch_state_fut);
        let initial_state = executor.run_until_stalled(&mut watch_state_fut);
        assert_matches!(initial_state, Poll::Pending);
        assert_eq!(activity_manager.is_holding_lease(), suspend_enabled);

        // Skip ahead by half the activity timeout.
        executor.set_fake_time(fuchsia_async::Time::after(ACTIVITY_TIMEOUT / 2));

        // Activity state does change.
        let watch_state_res = executor.run_until_stalled(&mut watch_state_fut);
        assert_matches!(watch_state_res, Poll::Ready(Some(Ok(State::Idle))));
        assert_eq!(activity_manager.is_holding_lease(), false);

        Ok(())
    }
}
