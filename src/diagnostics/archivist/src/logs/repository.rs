// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::events::router::EventConsumer;
use crate::events::types::{Event, EventPayload, LogSinkRequestedPayload};
use crate::identity::ComponentIdentity;
use crate::logs::budget::BudgetManager;
use crate::logs::container::LogsArtifactsContainer;
use crate::logs::debuglog::{DebugLog, DebugLogBridge, KERNEL_IDENTITY};
use crate::logs::multiplex::{Multiplexer, MultiplexerHandle};
use crate::severity_filter::KlogSeverityFilter;
use anyhow::format_err;
use diagnostics_data::{LogsData, Severity};
use fidl_fuchsia_diagnostics::{
    LogInterestSelector, Selector, Severity as FidlSeverity, StreamMode,
};
use flyweights::FlyStr;
use fuchsia_sync::{Mutex, RwLock};
use fuchsia_url::boot_url::BootUrl;
use fuchsia_url::AbsoluteComponentUrl;
use futures::channel::mpsc;
use futures::prelude::*;
use moniker::{ExtendedMoniker, Moniker};
use selectors::SelectorExt;
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use tracing::{debug, error};
use {fuchsia_async as fasync, fuchsia_inspect as inspect, fuchsia_trace as ftrace};

// LINT.IfChange
#[derive(Ord, PartialOrd, Eq, PartialEq)]
pub struct ComponentInitialInterest {
    /// The URL or moniker for the component which should receive the initial interest.
    component: UrlOrMoniker,
    /// The log severity the initial interest should specify.
    log_severity: Severity,
}
// LINT.ThenChange(/src/lib/assembly/config_schema/src/platform_config/diagnostics_config.rs)

impl FromStr for ComponentInitialInterest {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut split = s.rsplitn(2, ":");
        match (split.next(), split.next()) {
            (Some(severity), Some(url_or_moniker)) => {
                let Ok(url_or_moniker) = UrlOrMoniker::from_str(url_or_moniker) else {
                    return Err(format_err!("invalid url or moniker"));
                };
                let Ok(severity) = Severity::from_str(severity) else {
                    return Err(format_err!("invalid severity"));
                };
                Ok(ComponentInitialInterest { log_severity: severity, component: url_or_moniker })
            }
            _ => Err(format_err!("invalid interest")),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Ord, PartialOrd)]
pub enum UrlOrMoniker {
    /// An absolute fuchsia url to a component.
    Url(FlyStr),
    /// The absolute moniker for a component.
    Moniker(ExtendedMoniker),
}

impl FromStr for UrlOrMoniker {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if AbsoluteComponentUrl::from_str(s).is_ok() || BootUrl::parse(s).is_ok() {
            Ok(UrlOrMoniker::Url(s.into()))
        } else if let Ok(moniker) = Moniker::from_str(s) {
            Ok(UrlOrMoniker::Moniker(ExtendedMoniker::ComponentInstance(moniker)))
        } else {
            Err(())
        }
    }
}

static INTEREST_CONNECTION_ID: AtomicUsize = AtomicUsize::new(0);
static ARCHIVIST_MONIKER: LazyLock<Moniker> =
    LazyLock::new(|| Moniker::parse_str("bootstrap/archivist").unwrap());

/// LogsRepository holds all diagnostics data and is a singleton wrapped by multiple
/// [`pipeline::Pipeline`]s in a given Archivist instance.
pub struct LogsRepository {
    log_sender: RwLock<mpsc::UnboundedSender<fasync::Task<()>>>,
    mutable_state: Mutex<LogsRepositoryState>,
    /// Processes removal of components emitted by the budget handler. This is done to prevent a
    /// deadlock. It's behind a mutex, but it's only set once. Holds an Arc<Self>.
    component_removal_task: Mutex<Option<fasync::Task<()>>>,
}

impl LogsRepository {
    pub fn new(
        logs_max_cached_original_bytes: u64,
        initial_interests: impl Iterator<Item = ComponentInitialInterest>,
        parent: &fuchsia_inspect::Node,
    ) -> Arc<Self> {
        let (remover_snd, remover_rcv) = mpsc::unbounded();
        let logs_budget = BudgetManager::new(logs_max_cached_original_bytes as usize, remover_snd);
        let (log_sender, log_receiver) = mpsc::unbounded();
        let this = Arc::new(LogsRepository {
            mutable_state: Mutex::new(LogsRepositoryState::new(
                logs_budget,
                log_receiver,
                parent,
                initial_interests,
            )),
            log_sender: RwLock::new(log_sender),
            component_removal_task: Mutex::new(None),
        });
        *this.component_removal_task.lock() = Some(fasync::Task::spawn(
            Self::process_removal_of_components(remover_rcv, Arc::clone(&this)),
        ));
        this
    }

    /// Drain the kernel's debug log. The returned future completes once
    /// existing messages have been ingested.
    pub fn drain_debuglog<K>(&self, klog_reader: K)
    where
        K: DebugLog + Send + Sync + 'static,
    {
        let mut mutable_state = self.mutable_state.lock();
        // We can only have one klog reader, if this is already set, it means we are already
        // draining klog.
        if mutable_state.drain_klog_task.is_some() {
            return;
        }

        let container = mutable_state.get_log_container(KERNEL_IDENTITY.clone());
        mutable_state.drain_klog_task = Some(fasync::Task::spawn(async move {
            debug!("Draining debuglog.");
            let mut kernel_logger =
                DebugLogBridge::create(klog_reader, Arc::clone(&container.stats));
            let mut messages = match kernel_logger.existing_logs() {
                Ok(messages) => messages,
                Err(e) => {
                    error!(%e, "failed to read from kernel log, important logs may be missing");
                    return;
                }
            };
            messages.sort_by_key(|m| m.timestamp());
            for message in messages {
                container.ingest_message(message);
            }

            let res = kernel_logger
                .listen()
                .try_for_each(|message| async {
                    container.ingest_message(message);
                    Ok(())
                })
                .await;
            if let Err(e) = res {
                error!(%e, "failed to drain kernel log, important logs may be missing");
            }
        }));
    }

    pub fn logs_cursor(
        &self,
        mode: StreamMode,
        selectors: Option<Vec<Selector>>,
        parent_trace_id: ftrace::Id,
    ) -> impl Stream<Item = Arc<LogsData>> + Send + 'static {
        let mut repo = self.mutable_state.lock();
        let substreams = repo.logs_data_store.iter().map(|(identity, c)| {
            let cursor = c.cursor(mode, parent_trace_id);
            (Arc::clone(identity), cursor)
        });
        let (mut merged, mpx_handle) = Multiplexer::new(parent_trace_id, selectors, substreams);
        repo.logs_multiplexers.add(mode, mpx_handle);
        merged.set_on_drop_id_sender(repo.logs_multiplexers.cleanup_sender());
        merged
    }

    pub fn get_log_container(
        &self,
        identity: Arc<ComponentIdentity>,
    ) -> Arc<LogsArtifactsContainer> {
        self.mutable_state.lock().get_log_container(identity)
    }

    /// Stop accepting new messages, ensuring that pending Cursors return Poll::Ready(None) after
    /// consuming any messages received before this call.
    pub async fn wait_for_termination(&self) {
        let receiver = self.mutable_state.lock().log_receiver.take().unwrap();
        receiver.for_each_concurrent(None, |rx| rx).await;
        // Process messages from log sink.
        debug!("Log ingestion stopped.");
        let mut repo = self.mutable_state.lock();
        for container in repo.logs_data_store.values() {
            container.terminate();
        }
        repo.logs_multiplexers.terminate();

        debug!("Terminated logs");
        repo.logs_budget.terminate();
    }

    /// Closes the connection in which new logger draining tasks are sent. No more logger tasks
    /// will be accepted when this is called and we'll proceed to terminate logs.
    pub fn stop_accepting_new_log_sinks(&self) {
        self.log_sender.write().disconnect();
    }

    /// Returns an id to use for a new interest connection. Used by both LogSettings and Log, to
    /// ensure shared uniqueness of their connections.
    pub fn new_interest_connection(&self) -> usize {
        INTEREST_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
    }

    /// Updates log selectors associated with an interest connection.
    pub fn update_logs_interest(&self, connection_id: usize, selectors: Vec<LogInterestSelector>) {
        self.mutable_state.lock().update_logs_interest(connection_id, selectors);
    }

    /// Indicates that the connection associated with the given ID is now done.
    pub fn finish_interest_connection(&self, connection_id: usize) {
        self.mutable_state.lock().finish_interest_connection(connection_id);
    }

    async fn process_removal_of_components(
        mut removal_requests: mpsc::UnboundedReceiver<Arc<ComponentIdentity>>,
        logs_repo: Arc<LogsRepository>,
    ) {
        while let Some(identity) = removal_requests.next().await {
            let mut repo = logs_repo.mutable_state.lock();
            if !repo.is_live(&identity) {
                debug!(%identity, "Removing component from repository.");
                repo.remove(&identity);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn default() -> Arc<Self> {
        LogsRepository::new(
            crate::constants::LEGACY_DEFAULT_MAXIMUM_CACHED_LOGS_BYTES,
            std::iter::empty(),
            &Default::default(),
        )
    }
}

impl EventConsumer for LogsRepository {
    fn handle(self: Arc<Self>, event: Event) {
        match event.payload {
            EventPayload::LogSinkRequested(LogSinkRequestedPayload {
                component,
                request_stream,
            }) => {
                debug!(identity = %component, "LogSink requested.");
                let container = self.get_log_container(component);
                container.handle_log_sink(request_stream, self.log_sender.read().clone());
            }
            _ => unreachable!("Archivist state just subscribes to log sink requested"),
        }
    }
}

pub struct LogsRepositoryState {
    logs_data_store: HashMap<Arc<ComponentIdentity>, Arc<LogsArtifactsContainer>>,
    inspect_node: inspect::Node,

    /// Receives the logger tasks. This will be taken once in wait for termination hence why it's
    /// an option.
    log_receiver: Option<mpsc::UnboundedReceiver<fasync::Task<()>>>,

    /// A reference to the budget manager, kept to be passed to containers.
    logs_budget: BudgetManager,
    /// BatchIterators for logs need to be made aware of new components starting and their logs.
    logs_multiplexers: MultiplexerBroker,

    /// Interest registrations that we have received through fuchsia.logger.Log/ListWithSelectors
    /// or through fuchsia.logger.LogSettings/SetInterest.
    interest_registrations: BTreeMap<usize, Vec<LogInterestSelector>>,

    /// The task draining klog and routing to syslog.
    drain_klog_task: Option<fasync::Task<()>>,

    /// The initial log interests with which archivist was configured.
    initial_interests: BTreeMap<UrlOrMoniker, Severity>,
}

impl LogsRepositoryState {
    fn new(
        logs_budget: BudgetManager,
        log_receiver: mpsc::UnboundedReceiver<fasync::Task<()>>,
        parent: &fuchsia_inspect::Node,
        initial_interests: impl Iterator<Item = ComponentInitialInterest>,
    ) -> Self {
        Self {
            inspect_node: parent.create_child("log_sources"),
            logs_data_store: HashMap::new(),
            logs_budget,
            log_receiver: Some(log_receiver),
            logs_multiplexers: MultiplexerBroker::new(),
            interest_registrations: BTreeMap::new(),
            drain_klog_task: None,
            initial_interests: initial_interests
                .map(|ComponentInitialInterest { component, log_severity }| {
                    (component, log_severity)
                })
                .collect(),
        }
    }

    /// Returns a container for logs artifacts, constructing one and adding it to the trie if
    /// necessary.
    pub fn get_log_container(
        &mut self,
        identity: Arc<ComponentIdentity>,
    ) -> Arc<LogsArtifactsContainer> {
        match self.logs_data_store.get(&identity) {
            None => {
                let initial_interest = self.get_initial_interest(identity.as_ref());
                let container = Arc::new(LogsArtifactsContainer::new(
                    Arc::clone(&identity),
                    self.interest_registrations.values().flat_map(|s| s.iter()),
                    initial_interest,
                    &self.inspect_node,
                    self.logs_budget.handle(),
                ));
                self.logs_budget.add_container(Arc::clone(&container));
                self.logs_data_store.insert(identity, Arc::clone(&container));
                self.logs_multiplexers.send(&container);
                container
            }
            Some(existing) => Arc::clone(existing),
        }
    }

    fn get_initial_interest(&self, identity: &ComponentIdentity) -> Option<FidlSeverity> {
        match (
            self.initial_interests.get(&UrlOrMoniker::Url(identity.url.clone())),
            self.initial_interests.get(&UrlOrMoniker::Moniker(identity.moniker.clone())),
        ) {
            (None, None) => None,
            (Some(severity), None) | (None, Some(severity)) => Some(FidlSeverity::from(*severity)),
            (Some(s1), Some(s2)) => Some(FidlSeverity::from(std::cmp::min(*s1, *s2))),
        }
    }

    fn is_live(&self, identity: &Arc<ComponentIdentity>) -> bool {
        match self.logs_data_store.get(identity) {
            Some(container) => container.should_retain(),
            None => false,
        }
    }

    /// Updates our own log interest if we are the root Archivist and logging
    /// to klog.
    fn maybe_update_own_logs_interest(
        &mut self,
        selectors: &[LogInterestSelector],
        clear_interest: bool,
    ) {
        tracing::dispatcher::get_default(|dispatcher| {
            let Some(publisher) = dispatcher.downcast_ref::<KlogSeverityFilter>() else {
                return;
            };
            let lowest_selector = selectors
                .iter()
                .filter(|selector| {
                    ARCHIVIST_MONIKER
                        .matches_component_selector(&selector.selector)
                        .unwrap_or(false)
                })
                .min_by_key(|selector| {
                    selector.interest.min_severity.unwrap_or(FidlSeverity::Info)
                });
            if let Some(selector) = lowest_selector {
                if clear_interest {
                    publisher.set_severity(FidlSeverity::Info);
                } else {
                    publisher
                        .set_severity(selector.interest.min_severity.unwrap_or(FidlSeverity::Info));
                }
            }
        });
    }

    fn update_logs_interest(&mut self, connection_id: usize, selectors: Vec<LogInterestSelector>) {
        self.maybe_update_own_logs_interest(&selectors, false);
        let previous_selectors =
            self.interest_registrations.insert(connection_id, selectors).unwrap_or_default();
        // unwrap safe, we just inserted.
        let new_selectors = self.interest_registrations.get(&connection_id).unwrap();
        for logs_data in self.logs_data_store.values() {
            logs_data.update_interest(new_selectors.iter(), &previous_selectors);
        }
    }

    pub fn finish_interest_connection(&mut self, connection_id: usize) {
        let selectors = self.interest_registrations.remove(&connection_id);
        if let Some(selectors) = selectors {
            self.maybe_update_own_logs_interest(&selectors, true);
            for logs_data in self.logs_data_store.values() {
                logs_data.reset_interest(&selectors);
            }
        }
    }

    pub fn remove(&mut self, identity: &Arc<ComponentIdentity>) {
        self.logs_data_store.remove(identity);
    }
}

type LiveIteratorsMap = HashMap<usize, (StreamMode, MultiplexerHandle<Arc<LogsData>>)>;

/// Ensures that BatchIterators get access to logs from newly started components.
pub struct MultiplexerBroker {
    live_iterators: Arc<Mutex<LiveIteratorsMap>>,
    cleanup_sender: mpsc::UnboundedSender<usize>,
    _live_iterators_cleanup_task: fasync::Task<()>,
}

impl MultiplexerBroker {
    fn new() -> Self {
        let (cleanup_sender, mut receiver) = mpsc::unbounded();
        let live_iterators = Arc::new(Mutex::new(HashMap::new()));
        let live_iterators_clone = Arc::clone(&live_iterators);
        Self {
            live_iterators,
            cleanup_sender,
            _live_iterators_cleanup_task: fasync::Task::spawn(async move {
                while let Some(id) = receiver.next().await {
                    live_iterators_clone.lock().remove(&id);
                }
            }),
        }
    }

    fn cleanup_sender(&self) -> mpsc::UnboundedSender<usize> {
        self.cleanup_sender.clone()
    }

    /// A new BatchIterator has been created and must be notified when future log containers are
    /// created.
    fn add(&mut self, mode: StreamMode, recipient: MultiplexerHandle<Arc<LogsData>>) {
        match mode {
            // snapshot streams only want to know about what's currently available
            StreamMode::Snapshot => recipient.close(),
            StreamMode::SnapshotThenSubscribe | StreamMode::Subscribe => {
                self.live_iterators.lock().insert(recipient.multiplexer_id(), (mode, recipient));
            }
        }
    }

    /// Notify existing BatchIterators of a new logs container so they can include its messages
    /// in their results.
    pub fn send(&mut self, container: &Arc<LogsArtifactsContainer>) {
        self.live_iterators.lock().retain(|_, (mode, recipient)| {
            recipient.send(
                Arc::clone(&container.identity),
                container.cursor(*mode, recipient.parent_trace_id()),
            )
        });
    }

    /// Notify all multiplexers to terminate their streams once sub streams have terminated.
    fn terminate(&mut self) {
        for (_, (_, recipient)) in self.live_iterators.lock().drain() {
            recipient.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::stored_message::StoredMessage;
    use diagnostics_log_encoding::encode::{Encoder, EncoderOpts};
    use diagnostics_log_encoding::{Argument, Record, Severity as StreamSeverity, Value};
    use fidl_fuchsia_logger::LogSinkMarker;

    use moniker::ExtendedMoniker;
    use selectors::FastError;
    use std::io::Cursor;
    use std::time::Duration;

    #[fuchsia::test]
    async fn data_repo_filters_logs_by_selectors() {
        let repo = LogsRepository::default();
        let foo_container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./foo").unwrap(),
            "fuchsia-pkg://foo",
        )));
        let bar_container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./bar").unwrap(),
            "fuchsia-pkg://bar",
        )));

        foo_container.ingest_message(make_message("a", zx::BootInstant::from_nanos(1)));
        bar_container.ingest_message(make_message("b", zx::BootInstant::from_nanos(2)));
        foo_container.ingest_message(make_message("c", zx::BootInstant::from_nanos(3)));

        let stream = repo.logs_cursor(StreamMode::Snapshot, None, ftrace::Id::random());

        let results =
            stream.map(|value| value.msg().unwrap().to_string()).collect::<Vec<_>>().await;
        assert_eq!(results, vec!["a".to_string(), "b".to_string(), "c".to_string()]);

        let filtered_stream = repo.logs_cursor(
            StreamMode::Snapshot,
            Some(vec![selectors::parse_selector::<FastError>("foo:root").unwrap()]),
            ftrace::Id::random(),
        );

        let results =
            filtered_stream.map(|value| value.msg().unwrap().to_string()).collect::<Vec<_>>().await;
        assert_eq!(results, vec!["a".to_string(), "c".to_string()]);
    }

    #[fuchsia::test]
    async fn multiplexer_broker_cleanup() {
        let repo = LogsRepository::default();
        let stream =
            repo.logs_cursor(StreamMode::SnapshotThenSubscribe, None, ftrace::Id::random());

        assert_eq!(repo.mutable_state.lock().logs_multiplexers.live_iterators.lock().len(), 1);

        // When the multiplexer goes away it must be forgotten by the broker.
        drop(stream);
        loop {
            fasync::Timer::new(Duration::from_millis(100)).await;
            if repo.mutable_state.lock().logs_multiplexers.live_iterators.lock().len() == 0 {
                break;
            }
        }
    }

    #[fuchsia::test]
    async fn data_repo_correctly_sets_initial_interests() {
        let repo = LogsRepository::new(
            1000,
            [
                ComponentInitialInterest {
                    component: UrlOrMoniker::Url("fuchsia-pkg://bar".into()),
                    log_severity: Severity::Info,
                },
                ComponentInitialInterest {
                    component: UrlOrMoniker::Url("fuchsia-pkg://baz".into()),
                    log_severity: Severity::Warn,
                },
                ComponentInitialInterest {
                    component: UrlOrMoniker::Moniker("core/bar".try_into().unwrap()),
                    log_severity: Severity::Error,
                },
                ComponentInitialInterest {
                    component: UrlOrMoniker::Moniker("core/foo".try_into().unwrap()),
                    log_severity: Severity::Debug,
                },
            ]
            .into_iter(),
            &fuchsia_inspect::Node::default(),
        );

        // We have the moniker configured, use the associated severity.
        let container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("core/foo").unwrap(),
            "fuchsia-pkg://foo",
        )));
        expect_initial_interest(Some(FidlSeverity::Debug), container).await;

        // We have the URL configure, use the associated severity.
        let container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("core/baz").unwrap(),
            "fuchsia-pkg://baz",
        )));
        expect_initial_interest(Some(FidlSeverity::Warn), container).await;

        // We have both a URL and a moniker in the config. Pick the minimium one, in this case Info
        // for the URL over Error for the moniker.
        let container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("core/bar").unwrap(),
            "fuchsia-pkg://bar",
        )));
        expect_initial_interest(Some(FidlSeverity::Info), container).await;

        // Neither the moniker nor the URL have an associated severity, therefore, the minimum
        // severity isn't set.
        let container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("core/quux").unwrap(),
            "fuchsia-pkg://quux",
        )));
        expect_initial_interest(None, container).await;
    }

    async fn expect_initial_interest(
        expected_severity: Option<FidlSeverity>,
        container: Arc<LogsArtifactsContainer>,
    ) {
        let (sender, _recv) = mpsc::unbounded();
        let (log_sink, stream) =
            fidl::endpoints::create_proxy_and_stream::<LogSinkMarker>().expect("create log sink");
        container.handle_log_sink(stream, sender);
        let initial_interest = log_sink.wait_for_interest_change().await.unwrap().unwrap();
        assert_eq!(initial_interest.min_severity, expected_severity);
    }

    fn make_message(msg: &str, timestamp: zx::BootInstant) -> StoredMessage {
        let record = Record {
            timestamp: timestamp.into_nanos(),
            severity: StreamSeverity::Debug.into_primitive(),
            arguments: vec![
                Argument { name: "pid".to_string(), value: Value::UnsignedInt(1) },
                Argument { name: "tid".to_string(), value: Value::UnsignedInt(2) },
                Argument { name: "message".to_string(), value: Value::Text(msg.to_string()) },
            ],
        };
        let mut buffer = Cursor::new(vec![0u8; 1024]);
        let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
        encoder.write_record(&record).unwrap();
        let encoded = &buffer.get_ref()[..buffer.position() as usize];
        StoredMessage::new(encoded.to_vec().into(), &Default::default()).unwrap()
    }
}
