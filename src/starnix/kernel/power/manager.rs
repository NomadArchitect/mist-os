// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::power::{listener, SuspendState, SuspendStats};
use crate::task::CurrentTask;
use crate::vfs::EpollKey;

use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex as StdMutex};

use anyhow::{anyhow, Context};
use fidl::endpoints::create_sync_proxy;
use fuchsia_component::client::connect_to_protocol_sync;
use fuchsia_inspect_contrib::nodes::BoundedListNode;
use fuchsia_zircon::{HandleBased, Peered};
use once_cell::sync::OnceCell;
use starnix_logging::{log_info, log_warn};
use starnix_sync::{Mutex, MutexGuard};
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error};
use {
    fidl_fuchsia_power_broker as fbroker, fidl_fuchsia_power_system as fsystem,
    fidl_fuchsia_session_power as fpower, fidl_fuchsia_starnix_runner as frunner,
    fuchsia_inspect as inspect, fuchsia_zircon as zx,
};

cfg_if::cfg_if! {
    if #[cfg(not(feature = "wake_locks"))] {
        use async_utils::hanging_get::client::HangingGetStream;
        use fidl_fuchsia_power_suspend as fsuspend;
        use fuchsia_component::client::connect_to_protocol;
        use futures::StreamExt;
        use starnix_logging::log_error;
    }
}

#[derive(Debug)]
struct PowerElement {
    element_proxy: fbroker::ElementControlSynchronousProxy,
    lessor_proxy: fbroker::LessorSynchronousProxy,
    level_proxy: Option<fbroker::CurrentLevelSynchronousProxy>,
}

// String keys used for various suspend events.  We should try to keep these
// keys in sync across binaries.
const SUSPEND_FAILED_AT: &str = "failed_at_ns";
const SUSPEND_ATTEMPTED_AT: &str = "attempted_at_ns";
const SUSPEND_RESUMED_AT: &str = "resumed_at_ns";
const SUSPEND_REQUESTED_STATE: &str = "requested_power_state";

/// Manager for suspend and resume.
#[derive(Default)]
pub struct SuspendResumeManager {
    /// Power Mode power element is owned and registered by Starnix kernel. This power element is
    /// added in the power topology as a dependent on Application Activity element that is owned by
    /// the SAG.
    ///
    /// After Starnix boots, a power-on lease will be created and retained.
    ///
    /// When it need to suspend, Starnix should create another lease for the suspend state and
    /// release the power-on lease.
    ///
    /// The power level will only be changed to the requested level when all elements in the
    /// topology can maintain the minimum power equilibrium in the lease.
    ///
    /// | Power Mode        | Level |
    /// | ----------------- | ----- |
    /// | On                | 4     |
    /// | Suspend-to-Idle   | 3     |
    /// | Standby           | 2     |
    /// | Suspend-to-RAM    | 1     |
    /// | Suspend-to-Disk   | 0     |
    ///
    /// Note that this `PowerElement` only represents the desires of user-space. The Starnix Kernel
    /// itself may hold wake leases which prevent System Activity Governor from suspending the
    /// system, despite this `PowerElement` lowering its level below `On`.
    power_mode: OnceCell<PowerElement>,

    // The mutable state of [SuspendResumeManager].
    inner: Mutex<SuspendResumeManagerInner>,
}
pub(super) static STARNIX_POWER_ON_LEVEL: fbroker::PowerLevel = 4;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum SuspendResult {
    /// Indicates suspension was successful.
    ///
    /// Note that a successful suspension result may be returned _after_ resuming
    /// from a suspend. Observers may not assume that this result will be observed
    /// before the system actually suspends.
    Success,
    Failure,
}

#[derive(Default)]
struct SuspendWaiter {
    cond_var: Condvar,
    result: StdMutex<Option<SuspendResult>>,
}

impl SuspendWaiter {
    fn new() -> Arc<Self> {
        Arc::new(SuspendWaiter::default())
    }

    fn wait(self: Arc<Self>) -> SuspendResult {
        let guard = self.result.lock().unwrap();
        self.cond_var
            .wait_while(guard, |result| result.is_none())
            .unwrap()
            .expect("result is set before being notified")
    }
}

/// Manager for suspend and resume.
pub struct SuspendResumeManagerInner {
    /// The suspend counters and gauges.
    suspend_stats: SuspendStats,
    sync_on_suspend_enabled: bool,
    /// Lease control channel to hold the system power state as active.
    lease_control_channel: Option<zx::Channel>,

    suspend_waiter: Option<Arc<SuspendWaiter>>,
    inspect_node: BoundedListNode,

    /// The currently active wake locks in the system. If non-empty, this prevents
    /// the container from suspending.
    active_locks: HashSet<String>,

    /// The currently active EPOLLWAKEUPs in the system. If non-empty, this prevents
    /// the container from suspending.
    active_epolls: HashSet<EpollKey>,

    /// The event pair that is passed to the Starnix runner so it can observe whether
    /// or not any wake locks are active before completing a suspend operation.
    active_lock_reader: zx::EventPair,

    /// The event pair that is used by the Starnix kernel to signal when there are
    /// active wake locks in the container. Note that the peer of the writer is the
    /// object that is signaled.
    active_lock_writer: zx::EventPair,
}

/// The inspect node ring buffer will keep at most this many entries.
const INSPECT_RING_BUFFER_CAPACITY: usize = 128;

impl Default for SuspendResumeManagerInner {
    fn default() -> Self {
        let (active_lock_reader, active_lock_writer) = zx::EventPair::create();
        Self {
            inspect_node: BoundedListNode::new(
                inspect::component::inspector().root().create_child("suspend_events"),
                INSPECT_RING_BUFFER_CAPACITY,
            ),
            suspend_stats: Default::default(),
            sync_on_suspend_enabled: Default::default(),
            lease_control_channel: Default::default(),
            suspend_waiter: Default::default(),
            active_locks: Default::default(),
            active_epolls: Default::default(),
            active_lock_reader,
            active_lock_writer,
        }
    }
}

impl SuspendResumeManagerInner {
    /// Signals whether or not there are currently any active wake locks in the kernel.
    fn signal_wake_events(&mut self) {
        let (clear_mask, set_mask) =
            if self.active_locks.is_empty() && self.active_epolls.is_empty() {
                (zx::Signals::EVENT_SIGNALED, zx::Signals::empty())
            } else {
                (zx::Signals::empty(), zx::Signals::EVENT_SIGNALED)
            };
        self.active_lock_writer.signal_peer(clear_mask, set_mask).expect("Failed to signal peer");
    }
}

pub type SuspendResumeManagerHandle = Arc<SuspendResumeManager>;

impl SuspendResumeManager {
    /// Locks and returns the inner state of the manager.
    pub fn lock(&self) -> MutexGuard<'_, SuspendResumeManagerInner> {
        self.inner.lock()
    }

    /// Power on the PowerMode element and start listening to the suspend stats updates.
    pub fn init(
        self: &SuspendResumeManagerHandle,
        system_task: &CurrentTask,
    ) -> Result<(), anyhow::Error> {
        let activity_governor = connect_to_protocol_sync::<fsystem::ActivityGovernorMarker>()?;
        let handoff = system_task
            .kernel()
            .connect_to_protocol_at_container_svc::<fpower::HandoffMarker>()?
            .into_sync_proxy();
        self.init_power_element(&activity_governor, &handoff, system_task)?;
        listener::init_listener(self, &activity_governor, system_task);
        #[cfg(not(feature = "wake_locks"))]
        self.init_stats_watcher(system_task);
        Ok(())
    }

    fn init_power_element(
        self: &SuspendResumeManagerHandle,
        activity_governor: &fsystem::ActivityGovernorSynchronousProxy,
        handoff: &fpower::HandoffSynchronousProxy,
        system_task: &CurrentTask,
    ) -> Result<(), anyhow::Error> {
        let topology = connect_to_protocol_sync::<fbroker::TopologyMarker>()?;

        // Create the PowerMode power element depending on the Application Activity of SAG.
        let power_elements = activity_governor
            .get_power_elements(zx::MonotonicTime::INFINITE)
            .context("cannot get Activity Governor element from SAG")?;
        if let Some(Some(application_activity_token)) = power_elements
            .application_activity
            .map(|application_activity| application_activity.assertive_dependency_token)
        {
            // TODO(https://fxbug.dev/316023943): also depend on execution_resume_latency after implemented.
            let power_levels: Vec<u8> = (0..=STARNIX_POWER_ON_LEVEL).collect();
            let (element_control, element_control_server_end) =
                create_sync_proxy::<fbroker::ElementControlMarker>();
            let (lessor, lessor_server_end) = create_sync_proxy::<fbroker::LessorMarker>();
            let (current_level, current_level_server_end) =
                create_sync_proxy::<fbroker::CurrentLevelMarker>();
            let (required_level, required_level_server_end) =
                create_sync_proxy::<fbroker::RequiredLevelMarker>();
            let level_control_channels = fbroker::LevelControlChannels {
                current: current_level_server_end,
                required: required_level_server_end,
            };
            topology
                .add_element(
                    fbroker::ElementSchema {
                        element_name: Some("starnix-power-mode".into()),
                        initial_current_level: Some(0),
                        valid_levels: Some(power_levels),
                        dependencies: Some(vec![fbroker::LevelDependency {
                            dependency_type: fbroker::DependencyType::Assertive,
                            dependent_level: STARNIX_POWER_ON_LEVEL,
                            requires_token: application_activity_token,
                            requires_level_by_preference: vec![
                                fsystem::ApplicationActivityLevel::Active.into_primitive(),
                            ],
                        }]),
                        element_control: Some(element_control_server_end),
                        lessor_channel: Some(lessor_server_end),
                        level_control_channels: Some(level_control_channels),
                        ..Default::default()
                    },
                    zx::MonotonicTime::INFINITE,
                )?
                .map_err(|e| anyhow!("PowerBroker::AddElementError({e:?})"))?;

            // Power on by holding a lease.
            let power_on_control = lessor
                .lease(STARNIX_POWER_ON_LEVEL, zx::MonotonicTime::INFINITE)?
                .map_err(|e| anyhow!("PowerBroker::LeaseError({e:?})"))?
                .into_channel();
            self.lock().lease_control_channel = Some(power_on_control);

            self.power_mode
                .set(PowerElement {
                    element_proxy: element_control,
                    lessor_proxy: lessor,
                    level_proxy: Some(current_level),
                })
                .expect("Power Mode should be uninitialized");

            let self_ref = self.clone();
            system_task.kernel().kthreads.spawn(move |_, _| {
                while let Ok(Ok(level)) = required_level.watch(zx::MonotonicTime::INFINITE) {
                    if let Err(e) = self_ref
                        .power_mode()
                        .expect("Starnix should have a power mode")
                        .level_proxy
                        .as_ref()
                        .expect("Starnix power mode should have a current level proxy")
                        .update(level, zx::MonotonicTime::INFINITE)
                    {
                        log_warn!("Failed to update current level: {e:?}");
                        break;
                    }
                }
            });

            // We may not have a session manager to take a lease from in tests.
            match handoff.take(zx::MonotonicTime::INFINITE) {
                Ok(parent_lease) => {
                    let parent_lease = parent_lease.map_err(|e| {
                        anyhow!("Failed to take lessor and lease from parent: {e:?}")
                    })?;
                    drop(parent_lease)
                }
                Err(e) => {
                    if e.is_closed() {
                        log_warn!("Failed to send the fuchsia.session.power/Handoff.Take request. Assuming no Handoff protocol exists and moving on...");
                    } else {
                        return Err(e).context("Handoff::Take");
                    }
                }
            }
        };

        Ok(())
    }

    /// Adds a wake lock `name` to the active wake locks.
    pub fn add_lock(&self, name: String) {
        let mut state = self.lock();
        state.active_locks.insert(name);
        state.signal_wake_events();
    }

    /// Removes a wake lock `name` from the active wake locks.
    pub fn remove_lock(&self, name: &str) {
        let mut state = self.lock();
        state.active_locks.remove(name);
        state.signal_wake_events();
    }

    /// Adds a wake lock `key` to the active epoll wake locks.
    pub fn add_epoll(&self, key: EpollKey) {
        let mut state = self.lock();
        state.active_epolls.insert(key);
        state.signal_wake_events();
    }

    /// Removes a wake lock `key` from the active epoll wake locks.
    pub fn remove_epoll(&self, key: EpollKey) {
        let mut state = self.lock();
        state.active_epolls.remove(&key);
        state.signal_wake_events();
    }

    /// Returns a duplicate handle to the `EventPair` that is signaled when wake
    /// locks are active.
    pub fn duplicate_lock_event(&self) -> zx::EventPair {
        let state = self.lock();
        state
            .active_lock_reader
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .expect("Failed to duplicate handle")
    }

    #[cfg(not(feature = "wake_locks"))]
    fn update_stats(&self, stats: fsuspend::SuspendStats) {
        let stats_guard = &mut self.lock().suspend_stats;

        // Only update the stats if the new stats moves forward.
        let success_count = stats.success_count.unwrap_or_default();
        if stats_guard.success_count > success_count {
            return;
        }
        let fail_count = stats.fail_count.unwrap_or_default();
        if stats_guard.fail_count > fail_count {
            return;
        }

        stats_guard.success_count = stats.success_count.unwrap_or_default();
        stats_guard.fail_count = stats.fail_count.unwrap_or_default();
        stats_guard.last_time_in_sleep =
            zx::Duration::from_millis(stats.last_time_in_suspend.unwrap_or_default());
        stats_guard.last_time_in_suspend_operations =
            zx::Duration::from_millis(stats.last_time_in_suspend_operations.unwrap_or_default());
    }

    #[cfg(not(feature = "wake_locks"))]
    fn init_stats_watcher(self: &SuspendResumeManagerHandle, system_task: &CurrentTask) {
        let self_ref = self.clone();
        system_task.kernel().kthreads.spawn_future(async move {
            // Start listening to the suspend stats updates
            let stats_proxy = connect_to_protocol::<fsuspend::StatsMarker>()
                .expect("connection to fuchsia.power.suspend.Stats");
            let mut stats_stream = HangingGetStream::new(stats_proxy, fsuspend::StatsProxy::watch);
            while let Some(stats) = stats_stream.next().await {
                match stats {
                    Ok(stats) => self_ref.update_stats(stats),
                    Err(e) => {
                        log_error!("stats watcher got an error: {}", e);
                        break;
                    }
                }
            }
        });
    }

    fn power_mode(&self) -> Result<&PowerElement, Errno> {
        match self.power_mode.get() {
            Some(p) => Ok(p),
            None => error!(EAGAIN, "power-mode element is not initialized"),
        }
    }

    /// Gets the suspend statistics.
    pub fn suspend_stats(&self) -> SuspendStats {
        self.lock().suspend_stats.clone()
    }

    pub fn update_suspend_stats<UpdateFn>(&self, update: UpdateFn)
    where
        UpdateFn: FnOnce(&mut SuspendStats),
    {
        let stats_guard = &mut self.lock().suspend_stats;
        update(stats_guard);
    }

    /// Get the contents of the power "sync_on_suspend" file in the power
    /// filesystem.  True will cause `1` to be reported, and false will cause
    /// `0` to be reported.
    pub fn sync_on_suspend_enabled(&self) -> bool {
        self.lock().sync_on_suspend_enabled.clone()
    }

    /// Get the contents of the power "sync_on_suspend" file in the power
    /// filesystem.  See also [sync_on_suspend_enabled].
    pub fn set_sync_on_suspend(&self, enable: bool) {
        self.lock().sync_on_suspend_enabled = enable;
    }

    /// Returns the supported suspend states.
    pub fn suspend_states(&self) -> HashSet<SuspendState> {
        // TODO(b/326470421): Remove the hardcoded supported state.
        HashSet::from([SuspendState::Ram, SuspendState::Idle])
    }

    /// Sets the power level to `level`.
    pub(super) fn update_power_level(&self, level: fbroker::PowerLevel) -> Result<(), Errno> {
        let power_mode = self.power_mode()?;
        // Before the old lease is dropped, a new lease must be created to transit to the
        // new level. This ensures a smooth transition without going back to the initial
        // power level.
        match power_mode.lessor_proxy.lease(level, zx::MonotonicTime::INFINITE) {
            Ok(Ok(lease_client)) => {
                // Wait until the lease is satisfied.
                let lease_control = lease_client.into_sync_proxy();
                let mut lease_status = fbroker::LeaseStatus::Unknown;
                while lease_status != fbroker::LeaseStatus::Satisfied {
                    lease_status = lease_control
                        .watch_status(lease_status, zx::MonotonicTime::INFINITE)
                        .map_err(|_| errno!(EINVAL))?;
                }
                self.lock().lease_control_channel = Some(lease_control.into_channel());
            }
            Ok(Err(err)) => {
                return error!(EINVAL, format!("power broker lease error {:?}", err));
            }
            Err(err) => {
                return error!(EINVAL, format!("power broker lease fidl error {err}"));
            }
        }

        match power_mode
            .level_proxy
            .as_ref()
            .expect("Starnix PowerMode should have power level proxy")
            .update(level, zx::MonotonicTime::INFINITE)
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(err)) => error!(EINVAL, format!("power level update error {:?}", err)),
            Err(err) => error!(EINVAL, format!("power level update fidl error {err}")),
        }
    }

    fn wait_for_power_level(&self, level: fbroker::PowerLevel) -> Result<(), Errno> {
        // Create power element status stream
        let (element_status, element_status_server) = create_sync_proxy::<fbroker::StatusMarker>();
        self.power_mode()?
            .element_proxy
            .open_status_channel(element_status_server)
            .map_err(|e| errno!(EINVAL, format!("Status channel failed to open: {e}")))?;
        while element_status
            .watch_power_level(zx::MonotonicTime::INFINITE)
            .map_err(|err| errno!(EINVAL, format!("power element status watch error {err}")))?
            .map_err(|err| {
                errno!(EINVAL, format!("power element status watch fidl error {:?}", err))
            })?
            != level
        {}
        Ok(())
    }

    /// Notify all waiters of the suspension `result`.
    pub(super) fn notify_suspension(&self, result: SuspendResult) {
        let waiter = std::mem::take(&mut self.lock().suspend_waiter);
        waiter.map(|waiter| {
            let mut guard = waiter.result.lock().unwrap();
            let prev = guard.replace(result);
            debug_assert_eq!(prev, None, "waiter should only be notified once");
            // We should only have a single thread blocked per waiter.
            waiter.cond_var.notify_one();
        });
    }

    /// Executed on suspend.
    pub fn suspend(&self, state: SuspendState) -> Result<(), Errno> {
        log_info!(target=?state, "Initiating suspend");
        self.lock().inspect_node.add_entry(|node| {
            node.record_int(SUSPEND_ATTEMPTED_AT, zx::MonotonicTime::get().into_nanos());
            node.record_string(SUSPEND_REQUESTED_STATE, state.to_str());
        });

        let waiter = SuspendWaiter::new();
        let prev = self.lock().suspend_waiter.replace(Arc::clone(&waiter));
        debug_assert!(prev.is_none(), "Should not have concurrent suspend attempts");

        self.update_power_level(state.into()).inspect_err(|_| {
            // If `update_power_level()` fails, drop the `suspend_waiter`,
            // to indicate that there is no longer a suspend in progress.
            self.lock().suspend_waiter.take();
        })?;

        // Starnix will wait here on suspend.
        let suspend_result = waiter.wait();

        #[cfg(not(feature = "wake_locks"))]
        {
            // Synchronously update the stats after performing suspend so that a later
            // query of stats is guaranteed to reflect the current suspend operation.
            let stats_proxy = connect_to_protocol_sync::<fsuspend::StatsMarker>()
                .expect("connection to fuchsia.power.suspend.Stats");
            match stats_proxy.watch(zx::MonotonicTime::INFINITE) {
                Ok(stats) => self.update_stats(stats),
                Err(e) => log_warn!("failed to update stats after suspend: {e:?}"),
            }
        }

        // Use the same "now" for all subsequent stats.
        let now = zx::MonotonicTime::get();

        match suspend_result {
            SuspendResult::Success => self.wait_for_power_level(STARNIX_POWER_ON_LEVEL)?,
            SuspendResult::Failure => {
                self.lock().inspect_node.add_entry(|node| {
                    node.record_int(SUSPEND_FAILED_AT, now.into_nanos());
                });
                return error!(EINVAL, format!("failed to suspend at ns: {}", &now.into_nanos()));
            }
        }

        self.lock().inspect_node.add_entry(|node| {
            node.record_int(SUSPEND_RESUMED_AT, now.into_nanos());
        });
        log_info!(state=?state, "Resumed from suspend");

        Ok(())
    }
}

/// A power lease to keep the system awake.
///
/// The lease is armed when the `activate` method is called, and it is released/transferred when
/// the `take_lease` method is called.
///
/// The wake-lease PE is a dependency of the SAG `WakeHandling` PE that is responsible for keeping
/// the system awake.
///
/// This is useful for syscalls that need to keep the system awake for a period of time, such as
/// `EPOLLWAKEUP` event in epoll.
pub struct WakeLease {
    name: String,
    lease: Mutex<Option<zx::EventPair>>,
}

impl WakeLease {
    pub fn new(name: &str) -> Self {
        Self { name: format!("starnix-wake-lock-{}", name), lease: Default::default() }
    }

    pub fn activate(&self) -> Result<(), Errno> {
        let mut guard = self.lease.lock();
        if guard.is_none() {
            let activity_governor = connect_to_protocol_sync::<fsystem::ActivityGovernorMarker>()
                .map_err(|_| errno!(EINVAL, "Failed to connect to SAG"))?;
            *guard = Some(
                activity_governor
                    .take_wake_lease(&self.name, zx::MonotonicTime::INFINITE)
                    .map_err(|_| errno!(EINVAL, "Failed to take wake lease"))?,
            );
        }
        Ok(())
    }
}

impl WakeLeaseInterlockOps for WakeLease {
    fn take_lease(&self) -> Option<zx::EventPair> {
        self.lease.lock().take()
    }
}

/// `WakeLeaseInterlockOps` is a trait that defines the interface for handling a wake lease in a
/// interlock manner.
///
/// Interlock mechanism is used to ensure that the successor lease is activated before the
/// predecessor lease is dropped. This is important to ensure that any common dependencies of the
/// predecessor and successor leases remain actively claimed across a transfer of flow control.
pub trait WakeLeaseInterlockOps {
    /// Transfer the active wake lease to the caller.
    ///
    /// Ignoring the returned Channel means dropping the wake lease.
    fn take_lease(&self) -> Option<zx::EventPair>;
}

pub trait OnWakeOps: Send + Sync {
    fn on_wake(&self, current_task: &CurrentTask, baton_lease: &zx::Channel);
}

/// Creates a proxy between `remote_channel` and the returned `zx::Channel`.
///
/// The proxying is done by the Starnix runner, and allows messages on the channel to wake
/// the container.
pub fn create_proxy_for_wake_events(remote_channel: zx::Channel) -> (zx::Channel, zx::EventPair) {
    let (local_proxy, kernel_channel) = zx::Channel::create();
    let (resume_event, local_resume_event) = zx::EventPair::create();

    let manager =
        fuchsia_component::client::connect_to_protocol::<frunner::ManagerMarker>().expect("failed");
    manager
        .proxy_wake_channel(frunner::ManagerProxyWakeChannelRequest {
            container_job: Some(
                fuchsia_runtime::job_default()
                    .duplicate(zx::Rights::SAME_RIGHTS)
                    .expect("Failed to dup handle"),
            ),
            container_channel: Some(kernel_channel),
            remote_channel: Some(remote_channel),
            resume_event: Some(resume_event),
            ..Default::default()
        })
        .expect("Failed to create proxy");

    (local_proxy, local_resume_event)
}
