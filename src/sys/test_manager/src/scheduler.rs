// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::debug_data_processor::DebugDataSender;
use crate::self_diagnostics::DiagnosticNode;
use crate::test_suite::{self, Suite};
use async_trait::async_trait;
use futures::channel::oneshot;
use futures::stream::{self, StreamExt};
use std::sync::atomic::{AtomicU16, Ordering};

#[async_trait]
pub(crate) trait Scheduler {
    /// This function schedules and executes the provided collection
    /// of test suites. This allows objects that implement the
    /// Scheduler trait to define their own test suite scheduling
    /// algorithm. Inputs:
    ///     - &self
    ///     - suites: a collection of suites to schedule and execute
    ///     - stop_recv: Receiving end of a channel that receives messages to attempt to stop the
    ///                  test run. Scheduler::execute should check for stop messages over
    ///                  this channel and try to terminate the test run gracefully.
    ///     - run_id: an id that identifies the test run.
    ///     - debug_data_sender: used to send debug data VMOs for processing
    async fn execute(
        &self,
        suites: Vec<test_suite::Suite>,
        diagnostics: DiagnosticNode,
        stop_recv: &mut oneshot::Receiver<()>,
        debug_data_sender: DebugDataSender,
    );
}

#[async_trait]
pub(crate) trait RunSuiteFn {
    /// This function allows us to specify what function we want the
    /// parallel scheduler to invoke to run a single suite.
    /// This trait was added for testing purposes, specifically to add the
    /// ability to mock test_suite::run_single_suite in test::parallel_executor_test.
    async fn run_suite(
        &self,
        suite: Suite,
        debug_data_sender: DebugDataSender,
        diagnostics: DiagnosticNode,
    );
}

pub struct SerialScheduler {}

#[async_trait]
impl Scheduler for SerialScheduler {
    async fn execute(
        &self,
        suites: Vec<test_suite::Suite>,
        diagnostics: DiagnosticNode,
        stop_recv: &mut oneshot::Receiver<()>,
        debug_data_sender: DebugDataSender,
    ) {
        // run test suites serially for now
        for (suite_idx, suite) in suites.into_iter().enumerate() {
            // only check before running the test. We should complete the test run for
            // running tests, if stop is called.
            if let Ok(Some(())) = stop_recv.try_recv() {
                break;
            }
            let instance_name = format!("suite-{:?}", suite_idx);
            let suite_node = diagnostics.child(instance_name);
            suite_node.set_property("url", suite.test_url.clone());
            test_suite::run_single_suite(suite, debug_data_sender.clone(), suite_node).await;
        }
    }
}

pub(crate) struct ParallelScheduler<T: RunSuiteFn> {
    pub suite_runner: T,
    pub max_parallel_suites: u16,
}

pub(crate) struct RunSuiteObj {}

#[async_trait]
impl RunSuiteFn for RunSuiteObj {
    async fn run_suite(
        &self,
        suite: Suite,
        debug_data_sender: DebugDataSender,
        diagnostics: DiagnosticNode,
    ) {
        test_suite::run_single_suite(suite, debug_data_sender, diagnostics).await;
    }
}

#[async_trait]
impl<T: RunSuiteFn + std::marker::Sync + std::marker::Send> Scheduler for ParallelScheduler<T> {
    async fn execute(
        &self,
        suites: Vec<test_suite::Suite>,
        diagnostics: DiagnosticNode,
        _stop_recv: &mut oneshot::Receiver<()>,
        debug_data_sender: DebugDataSender,
    ) {
        const MAX_PARALLEL_SUITES_DEFAULT: usize = 8;
        let mut max_parallel_suites = self.max_parallel_suites as usize;

        // This logic is necessary due to the defined behavior in the RunOptions
        // fidl. We promise clients that if they use the WithSchedulingOptions
        // method, and they set max_parallel_suites in SchedulingOptions to 0,
        // the parallel scheduler implementation will choose a default
        // max_parallel_suites value.
        max_parallel_suites =
            if max_parallel_suites > 0 { max_parallel_suites } else { MAX_PARALLEL_SUITES_DEFAULT };
        let suite_idx = AtomicU16::new(0);
        let suite_idx_ref = &suite_idx;
        let debug_data_sender_ref = &debug_data_sender;
        let diagnostics_ref = &diagnostics;
        stream::iter(suites)
            .for_each_concurrent(max_parallel_suites, |suite| async move {
                let suite_idx_local = suite_idx_ref.fetch_add(1, Ordering::Relaxed);
                let instance_name = format!("suite-{:?}", suite_idx_local);
                let suite_node = diagnostics_ref.child(instance_name);
                suite_node.set_property("url", suite.test_url.clone());
                self.suite_runner.run_suite(suite, debug_data_sender_ref.clone(), suite_node).await;
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug_data_processor::{DebugDataDirectory, DebugDataProcessor};
    use crate::{facet, AboveRootCapabilitiesForTest};
    use async_trait::async_trait;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_test_manager::{RunOptions, SuiteControllerMarker};
    use std::sync::{Arc, Mutex};
    use {fidl_fuchsia_component_resolution as fresolution, fidl_fuchsia_pkg as fpkg};

    async fn create_fake_suite(test_url: String) -> Suite {
        let (_controller_proxy, controller_stream) =
            create_proxy_and_stream::<SuiteControllerMarker>();
        let (resolver_proxy, _resolver_stream) =
            create_proxy_and_stream::<fresolution::ResolverMarker>();
        let (pkg_resolver_proxy, _pkg_resolver_stream) =
            create_proxy_and_stream::<fpkg::PackageResolverMarker>();
        let resolver_proxy = Arc::new(resolver_proxy);
        let pkg_resolver_proxy = Arc::new(pkg_resolver_proxy);
        let routing_info = Arc::new(AboveRootCapabilitiesForTest::new_empty_for_tests());
        Suite {
            realm: None,
            test_url,
            options: RunOptions {
                parallel: None,
                arguments: None,
                run_disabled_tests: Some(false),
                timeout: None,
                case_filters_to_run: None,
                log_iterator: None,
                ..Default::default()
            },
            controller: controller_stream,
            resolver: resolver_proxy,
            pkg_resolver: pkg_resolver_proxy,
            above_root_capabilities_for_test: routing_info,
            facets: facet::ResolveStatus::Unresolved,
        }
    }

    struct RunSuiteObjForTests {
        test_vec: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl RunSuiteFn for &RunSuiteObjForTests {
        async fn run_suite(
            &self,
            suite: Suite,
            _debug_data_sender: DebugDataSender,
            _diagnostics: DiagnosticNode,
        ) {
            let suite_url = suite.test_url;
            self.test_vec.clone().lock().expect("expected locked").push(suite_url);
        }
    }

    #[fuchsia::test]
    async fn parallel_executor_runs_all_tests() {
        let suite_1 = create_fake_suite("suite_1".to_string()).await;
        let suite_2 = create_fake_suite("suite_2".to_string()).await;
        let suite_3 = create_fake_suite("suite_3".to_string()).await;
        let suite_vec = vec![suite_1, suite_2, suite_3];

        let test_vec = Arc::new(Mutex::new(vec![]));
        let suite_runner = RunSuiteObjForTests { test_vec };
        let parallel_executor =
            ParallelScheduler { suite_runner: &suite_runner, max_parallel_suites: 8 };

        let diagnostics = DiagnosticNode::new(
            "root",
            Arc::new(fuchsia_inspect::component::inspector().root().clone_weak()),
        );

        let sender =
            DebugDataProcessor::new_for_test(DebugDataDirectory::Isolated { parent: "/tmp" })
                .sender;

        let (_stop_sender, mut stop_recv) = oneshot::channel::<()>();

        parallel_executor.execute(suite_vec, diagnostics, &mut stop_recv, sender).await;

        assert!(suite_runner
            .test_vec
            .lock()
            .expect("expected locked")
            .contains(&"suite_1".to_string()));
        assert!(suite_runner
            .test_vec
            .lock()
            .expect("expected locked")
            .contains(&"suite_2".to_string()));
        assert!(suite_runner
            .test_vec
            .lock()
            .expect("expected locked")
            .contains(&"suite_3".to_string()));
    }
}
