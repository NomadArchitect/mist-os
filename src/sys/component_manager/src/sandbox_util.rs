// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::component::{
    ComponentInstance, ExtendedInstance, WeakComponentInstance, WeakExtendedInstance,
};
use ::routing::capability_source::CapabilitySource;
use ::routing::component_instance::ComponentInstanceInterface;
use ::routing::error::{ComponentInstanceError, RoutingError};
use ::routing::policy::GlobalPolicyChecker;
use ::routing::{WeakInstanceTokenExt, WithDefault};
use async_trait::async_trait;
use cm_util::WeakTaskGroup;
use fidl::endpoints::{ProtocolMarker, RequestStream};
use fidl::epitaph::ChannelEpitaphExt;
use fidl::AsyncChannel;
use futures::future::BoxFuture;
use futures::FutureExt;
use router_error::{Explain, RouterError};
use sandbox::{Capability, Connectable, Connector, Message, Request, Routable, Router};
use std::fmt::Debug;
use std::sync::Arc;
use tracing::warn;
use vfs::directory::entry::OpenRequest;
use vfs::execution_scope::ExecutionScope;
use vfs::path::Path;
use vfs::ToObjectRequest;
use zx::AsHandleRef;
use {fidl_fuchsia_io as fio, fuchsia_async as fasync, zx};

pub fn take_handle_as_stream<P: ProtocolMarker>(channel: zx::Channel) -> P::RequestStream {
    let channel = AsyncChannel::from_channel(channel);
    P::RequestStream::from_channel(channel)
}

/// Waits for a new message on a receiver, and launches a new async task on a `WeakTaskGroup` to
/// handle each new message from the receiver.
#[derive(Clone)]
pub struct LaunchTaskOnReceive {
    capability_source: CapabilitySource,
    task_to_launch: Arc<
        dyn Fn(zx::Channel, WeakComponentInstance) -> BoxFuture<'static, Result<(), anyhow::Error>>
            + Sync
            + Send
            + 'static,
    >,
    // Note that we explicitly need a `WeakTaskGroup` because if our `run` call is scheduled on the
    // same task group as we'll be launching tasks on then if we held a strong reference we would
    // inadvertently give the task group a strong reference to itself and make it un-droppable.
    task_group: WeakTaskGroup,
    policy: Option<GlobalPolicyChecker>,
    task_name: String,
}

impl std::fmt::Debug for LaunchTaskOnReceive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchTaskOnReceive").field("task_name", &self.task_name).finish()
    }
}

fn cm_unexpected() -> RouterError {
    RoutingError::from(ComponentInstanceError::ComponentManagerInstanceUnexpected {}).into()
}

impl LaunchTaskOnReceive {
    pub fn new(
        capability_source: CapabilitySource,
        task_group: WeakTaskGroup,
        task_name: impl Into<String>,
        policy: Option<GlobalPolicyChecker>,
        task_to_launch: Arc<
            dyn Fn(
                    zx::Channel,
                    WeakComponentInstance,
                ) -> BoxFuture<'static, Result<(), anyhow::Error>>
                + Sync
                + Send
                + 'static,
        >,
    ) -> Self {
        Self { capability_source, task_to_launch, task_group, policy, task_name: task_name.into() }
    }

    pub fn into_sender(self: Arc<Self>, target: WeakComponentInstance) -> Connector {
        #[derive(Debug)]
        struct TaskAndTarget {
            task: Arc<LaunchTaskOnReceive>,
            target: WeakComponentInstance,
        }

        impl Connectable for TaskAndTarget {
            fn send(&self, message: Message) -> Result<(), ()> {
                self.task.launch_task(message.channel, self.target.clone());
                Ok(())
            }
        }

        Connector::new_sendable(TaskAndTarget { task: self, target })
    }

    pub fn into_router(self) -> Router {
        #[derive(Debug)]
        struct LaunchTaskRouter {
            inner: Arc<LaunchTaskOnReceive>,
        }
        #[async_trait]
        impl Routable for LaunchTaskRouter {
            async fn route(
                &self,
                request: Option<Request>,
                debug: bool,
            ) -> Result<Capability, RouterError> {
                let request = request.ok_or_else(|| RouterError::InvalidArgs)?;
                let WeakExtendedInstance::Component(target) = request.target.to_instance() else {
                    return Err(cm_unexpected());
                };
                let cap = self.inner.clone().into_sender(target).into();
                if !debug {
                    Ok(cap)
                } else {
                    Ok(self
                        .inner
                        .capability_source
                        .clone()
                        .try_into()
                        .expect("failed to convert capability source to dictionary"))
                }
            }
        }
        Router::new(LaunchTaskRouter { inner: Arc::new(self) })
    }

    fn launch_task(&self, channel: zx::Channel, instance: WeakComponentInstance) {
        if let Some(policy_checker) = &self.policy {
            if let Err(_e) =
                policy_checker.can_route_capability(&self.capability_source, &instance.moniker)
            {
                // The `can_route_capability` function above will log an error, so we don't
                // have to.
                let _ = channel.close_with_epitaph(zx::Status::ACCESS_DENIED);
                return;
            }
        }

        let fut = (self.task_to_launch)(channel, instance);
        let task_name = self.task_name.clone();
        self.task_group.spawn(async move {
            if let Err(error) = fut.await {
                warn!(%error, "{} failed", task_name);
            }
        });
    }

    // Create a new LaunchTaskOnReceive that represents a framework hook task.
    // The task that this launches finds the components internal provider and will
    // open that.
    pub fn new_hook_launch_task(
        component: &Arc<ComponentInstance>,
        capability_source: CapabilitySource,
    ) -> LaunchTaskOnReceive {
        let weak_component = WeakComponentInstance::new(component);
        LaunchTaskOnReceive::new(
            capability_source.clone(),
            component.nonblocking_task_group().as_weak(),
            "framework hook dispatcher",
            Some(component.context.policy().clone()),
            Arc::new(move |channel, target| {
                let weak_component = weak_component.clone();
                let capability_source = capability_source.clone();
                async move {
                    if let Ok(target) = target.upgrade() {
                        if let Ok(component) = weak_component.upgrade() {
                            if let Some(provider) = target
                                .context
                                .find_internal_provider(&capability_source, target.as_weak())
                                .await
                            {
                                let mut object_request =
                                    fio::OpenFlags::empty().to_object_request(channel);
                                provider
                                    .open(
                                        component.nonblocking_task_group(),
                                        OpenRequest::new(
                                            component.execution_scope.clone(),
                                            fio::OpenFlags::empty(),
                                            Path::dot(),
                                            &mut object_request,
                                        ),
                                    )
                                    .await?;
                                return Ok(());
                            }
                        }

                        let _ = channel.close_with_epitaph(zx::Status::UNAVAILABLE);
                    }
                    Ok(())
                }
                .boxed()
            }),
        )
    }
}

/// Porcelain methods on [`Routable`] objects.
pub trait RoutableExt: Routable {
    /// Returns a router that resolves with a [`sandbox::Connector`] that watches for
    /// the channel to be readable, then delegates to the current router. The wait
    /// is performed in the provided `scope`.
    fn on_readable(self, scope: ExecutionScope) -> Router;
}

impl<T: Routable + 'static> RoutableExt for T {
    fn on_readable(self, scope: ExecutionScope) -> Router {
        #[derive(Debug)]
        struct OnReadableRouter {
            router: Router,
            scope: ExecutionScope,
        }

        #[async_trait]
        impl Routable for OnReadableRouter {
            async fn route(
                &self,
                request: Option<Request>,
                debug: bool,
            ) -> Result<Capability, RouterError> {
                if debug {
                    return self.router.route(request, debug).await;
                }
                let request = request.ok_or_else(|| RouterError::InvalidArgs)?;

                let ExtendedInstance::Component(target) =
                    request.target.clone().to_instance().upgrade().map_err(RoutingError::from)?
                else {
                    return Err(cm_unexpected());
                };
                let router = self.router.clone().with_default(request);

                // Wrap the router in something that will wait until the channel is readable.
                #[derive(Debug)]
                struct OnReadable {
                    scope: ExecutionScope,
                    target: Arc<ComponentInstance>,
                    router: Router,
                }
                impl Connectable for OnReadable {
                    fn send(&self, message: Message) -> Result<(), ()> {
                        let router = self.router.clone();
                        let target = self.target.clone();
                        self.scope.spawn(async move {
                            let Message { channel } = message;
                            match Self::send_inner(&router, &target, &channel).await {
                                Ok(conn) => {
                                    // We're in an async task, and the original function already
                                    // returned Ok. There's nothing we can do with this result.
                                    let _ = conn.send(Message { channel });
                                }
                                Err(e) => {
                                    let _ = channel.close_with_epitaph(e);
                                }
                            }
                        });
                        Ok(())
                    }
                }
                impl OnReadable {
                    async fn send_inner(
                        router: &Router,
                        target: &Arc<ComponentInstance>,
                        channel: &fidl::Channel,
                    ) -> Result<Connector, zx::Status> {
                        let signals = fasync::OnSignalsRef::new(
                            channel.as_handle_ref(),
                            fidl::Signals::OBJECT_READABLE | fidl::Signals::CHANNEL_PEER_CLOSED,
                        )
                        .await
                        .unwrap();
                        if !signals.contains(fidl::Signals::OBJECT_READABLE) {
                            return Err(zx::Status::PEER_CLOSED);
                        }
                        let conn = match router.route(None, false).await.and_then(|c| {
                            if let Capability::Connector(c) = c {
                                Ok(c)
                            } else {
                                Err(RoutingError::BedrockWrongCapabilityType {
                                    actual: c.debug_typename().into(),
                                    expected: "Connector".into(),
                                    moniker: target.moniker.clone().into(),
                                }
                                .into())
                            }
                        }) {
                            Ok(c) => c,
                            Err(err) => {
                                // TODO(https://fxbug.dev/319754472): Improve the fidelity of error
                                // logging. This should log into the component's log sink using the
                                // proper `report_routing_failure`, but that function requires a
                                // legacy `RouteRequest` at the moment.
                                target
                                    .with_logger_as_default(|| {
                                        warn!(
                                        "Request was not available for target component `{}`: `{}`",
                                        target.moniker, err
                                    );
                                    })
                                    .await;
                                return Err(err.as_zx_status());
                            }
                        };
                        Ok(conn)
                    }
                }

                let on_readable = OnReadable { scope: self.scope.clone(), router, target };
                Ok(Capability::from(Connector::new_sendable(on_readable)))
            }
        }

        let router = Router::new(self);
        Router::new(OnReadableRouter { router, scope })
    }
}

#[cfg(test)]
pub mod tests {
    use crate::model::context::ModelContext;
    use crate::model::environment::Environment;

    use super::*;
    use assert_matches::assert_matches;
    use cm_rust::Availability;
    use cm_types::RelativePath;
    use fuchsia_async::TestExecutor;
    use moniker::Moniker;
    use router_error::DowncastErrorForTest;
    use routing::availability::AvailabilityMetadata;
    use routing::bedrock::structured_dict::ComponentInput;
    use routing::{test_invalid_instance_token, DictExt, LazyGet};
    use sandbox::{Data, Dict, RemotableCapability, WeakInstanceToken};
    use std::pin::pin;
    use std::sync::Weak;
    use std::task::Poll;

    #[fuchsia::test]
    async fn get_capability() {
        let sub_dict = Dict::new();
        sub_dict
            .insert("bar".parse().unwrap(), Capability::Dictionary(Dict::new()))
            .expect("dict entry already exists");
        let (_, sender) = Connector::new();
        sub_dict.insert("baz".parse().unwrap(), sender.into()).expect("dict entry already exists");

        let test_dict = Dict::new();
        test_dict
            .insert("foo".parse().unwrap(), Capability::Dictionary(sub_dict))
            .expect("dict entry already exists");

        assert!(test_dict.get_capability(&RelativePath::dot()).is_some());
        assert!(test_dict.get_capability(&RelativePath::new("nonexistent").unwrap()).is_none());
        assert!(test_dict.get_capability(&RelativePath::new("foo").unwrap()).is_some());
        assert!(test_dict.get_capability(&RelativePath::new("foo/bar").unwrap()).is_some());
        assert!(test_dict.get_capability(&RelativePath::new("foo/nonexistent").unwrap()).is_none());
        assert!(test_dict.get_capability(&RelativePath::new("foo/baz").unwrap()).is_some());
    }

    #[fuchsia::test]
    async fn insert_capability() {
        let test_dict = Dict::new();
        assert!(test_dict
            .insert_capability(&RelativePath::new("foo/bar").unwrap(), Dict::new().into())
            .is_ok());
        assert!(test_dict.get_capability(&RelativePath::new("foo/bar").unwrap()).is_some());

        let (_, sender) = Connector::new();
        assert!(test_dict
            .insert_capability(&RelativePath::new("foo/baz").unwrap(), sender.into())
            .is_ok());
        assert!(test_dict.get_capability(&RelativePath::new("foo/baz").unwrap()).is_some());
    }

    #[fuchsia::test]
    async fn remove_capability() {
        let test_dict = Dict::new();
        assert!(test_dict
            .insert_capability(&RelativePath::new("foo/bar").unwrap(), Dict::new().into())
            .is_ok());
        assert!(test_dict.get_capability(&RelativePath::new("foo/bar").unwrap()).is_some());

        test_dict.remove_capability(&RelativePath::new("foo/bar").unwrap());
        assert!(test_dict.get_capability(&RelativePath::new("foo/bar").unwrap()).is_none());
        assert!(test_dict.get_capability(&RelativePath::new("foo").unwrap()).is_some());

        test_dict.remove_capability(&RelativePath::new("foo").unwrap());
        assert!(test_dict.get_capability(&RelativePath::new("foo").unwrap()).is_none());
    }

    #[fuchsia::test]
    async fn get_with_request_ok() {
        let bar = Dict::new();
        let data = Data::String("hello".to_owned());
        assert!(bar.insert_capability(&RelativePath::new("data").unwrap(), data.into()).is_ok());
        // Put bar behind a few layers of Router for good measure.
        let bar_router = Router::new_ok(bar);
        let bar_router = Router::new_ok(bar_router);
        let bar_router = Router::new_ok(bar_router);

        let foo = Dict::new();
        assert!(foo
            .insert_capability(&RelativePath::new("bar").unwrap(), bar_router.into())
            .is_ok());
        let foo_router = Router::new_ok(foo);

        let dict = Dict::new();
        assert!(dict
            .insert_capability(&RelativePath::new("foo").unwrap(), foo_router.into())
            .is_ok());

        let metadata = Dict::new();
        metadata.set_availability(Availability::Required);
        let cap = dict
            .get_with_request(
                Moniker::root(),
                &RelativePath::new("foo/bar/data").unwrap(),
                Some(Request {
                    target: test_invalid_instance_token::<ComponentInstance>(),
                    metadata,
                }),
                false,
            )
            .await;
        assert_matches!(
            cap,
            Ok(Some(Capability::Data(Data::String(str)))) if str == "hello"
        );
    }

    #[fuchsia::test]
    async fn get_with_request_error() {
        let dict = Dict::new();
        let foo =
            Router::new_error(RoutingError::SourceCapabilityIsVoid { moniker: Moniker::root() });
        assert!(dict.insert_capability(&RelativePath::new("foo").unwrap(), foo.into()).is_ok());
        let metadata = Dict::new();
        metadata.set_availability(Availability::Required);
        let cap = dict
            .get_with_request(
                Moniker::root(),
                &RelativePath::new("foo/bar").unwrap(),
                Some(Request {
                    target: test_invalid_instance_token::<ComponentInstance>(),
                    metadata,
                }),
                false,
            )
            .await;
        assert_matches!(
            cap,
            Err(RouterError::NotFound(err))
            if matches!(
                err.downcast_for_test::<RoutingError>(),
                RoutingError::SourceCapabilityIsVoid { .. }
            )
        );
    }

    #[fuchsia::test]
    async fn get_with_request_missing() {
        let dict = Dict::new();
        let metadata = Dict::new();
        metadata.set_availability(Availability::Required);
        let cap = dict
            .get_with_request(
                Moniker::root(),
                &RelativePath::new("foo/bar").unwrap(),
                Some(Request {
                    target: test_invalid_instance_token::<ComponentInstance>(),
                    metadata,
                }),
                false,
            )
            .await;
        assert_matches!(cap, Ok(None));
    }

    #[fuchsia::test]
    async fn get_with_request_missing_deep() {
        let dict = Dict::new();

        let foo = Dict::new();
        let foo = Router::new_ok(foo);
        assert!(dict.insert_capability(&RelativePath::new("foo").unwrap(), foo.into()).is_ok());

        let metadata = Dict::new();
        metadata.set_availability(Availability::Required);
        let cap = dict
            .get_with_request(
                Moniker::root(),
                &RelativePath::new("foo").unwrap(),
                Some(Request {
                    target: test_invalid_instance_token::<ComponentInstance>(),
                    metadata,
                }),
                false,
            )
            .await;
        assert_matches!(cap, Ok(Some(Capability::Dictionary(_))));

        let metadata = Dict::new();
        metadata.set_availability(Availability::Required);
        let cap = dict
            .get_with_request(
                Moniker::root(),
                &RelativePath::new("foo/bar").unwrap(),
                Some(Request {
                    target: test_invalid_instance_token::<ComponentInstance>(),
                    metadata,
                }),
                false,
            )
            .await;
        assert_matches!(cap, Ok(None));
    }

    #[derive(Debug, Clone)]
    struct RouteCounter {
        capability: Arc<Capability>,
        counter: Arc<test_util::Counter>,
    }

    impl RouteCounter {
        fn new(capability: Capability) -> Self {
            Self { capability: Arc::new(capability), counter: Arc::new(test_util::Counter::new(0)) }
        }

        fn count(&self) -> usize {
            self.counter.get()
        }
    }

    #[async_trait]
    impl Routable for RouteCounter {
        async fn route(&self, _: Option<Request>, _: bool) -> Result<Capability, RouterError> {
            self.counter.inc();
            Ok(self.capability.try_clone().unwrap())
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn router_on_readable_client_writes() {
        let (receiver, sender) = Connector::new();
        let scope = ExecutionScope::new();
        let (client_end, server_end) = zx::Channel::create();

        let route_counter = RouteCounter::new(sender.into());
        let router = route_counter.clone().on_readable(scope.clone());

        let mut receive = pin!(receiver.receive());
        assert_matches!(TestExecutor::poll_until_stalled(&mut receive).await, Poll::Pending);

        let component = ComponentInstance::new_root(
            ComponentInput::default(),
            Environment::empty(),
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "test:///root".parse().unwrap(),
        )
        .await;
        let metadata = Dict::new();
        metadata.set_availability(Availability::Required);
        let capability = router
            .route(Some(Request { target: component.as_weak().into(), metadata }), false)
            .await
            .unwrap();

        assert_matches!(TestExecutor::poll_until_stalled(&mut receive).await, Poll::Pending);
        assert_eq!(route_counter.count(), 0);

        let mut object_request = fio::OpenFlags::empty().to_object_request(server_end);
        capability
            .try_into_directory_entry()
            .unwrap()
            .open_entry(OpenRequest::new(
                scope.clone(),
                fio::OpenFlags::empty(),
                Path::dot(),
                &mut object_request,
            ))
            .unwrap();

        assert_matches!(TestExecutor::poll_until_stalled(&mut receive).await, Poll::Pending);
        assert_eq!(route_counter.count(), 0);

        client_end.write(&[0], &mut []).unwrap();
        assert_matches!(TestExecutor::poll_until_stalled(&mut receive).await, Poll::Ready(Some(_)));
        scope.wait().await;
        assert_eq!(route_counter.count(), 1);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn router_on_readable_client_closes() {
        let (receiver, sender) = Connector::new();
        let scope = ExecutionScope::new();
        let (client_end, server_end) = zx::Channel::create();

        let route_counter = RouteCounter::new(sender.into());
        let router = route_counter.clone().on_readable(scope.clone());

        let mut receive = pin!(receiver.receive());
        assert_matches!(TestExecutor::poll_until_stalled(&mut receive).await, Poll::Pending);

        let component = ComponentInstance::new_root(
            ComponentInput::default(),
            Environment::empty(),
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "test:///root".parse().unwrap(),
        )
        .await;
        let metadata = Dict::new();
        metadata.set_availability(Availability::Required);
        let capability = router
            .route(Some(Request { target: component.as_weak().into(), metadata }), false)
            .await
            .unwrap();

        let mut object_request = fio::OpenFlags::empty().to_object_request(server_end);
        capability
            .try_into_directory_entry()
            .unwrap()
            .open_entry(OpenRequest::new(
                scope.clone(),
                fio::OpenFlags::empty(),
                Path::dot(),
                &mut object_request,
            ))
            .unwrap();

        assert_matches!(TestExecutor::poll_until_stalled(&mut receive).await, Poll::Pending);
        assert_matches!(
            TestExecutor::poll_until_stalled(Box::pin(scope.clone().wait())).await,
            Poll::Pending
        );
        assert_eq!(route_counter.count(), 0);

        drop(client_end);
        assert_matches!(TestExecutor::poll_until_stalled(&mut receive).await, Poll::Pending);
        scope.wait().await;
        assert_eq!(route_counter.count(), 0);
    }

    #[fuchsia::test]
    async fn router_on_readable_debug() {
        let scope = ExecutionScope::new();

        let source_moniker: Moniker = "source".try_into().unwrap();
        let mut source = WeakComponentInstance::invalid();
        source.moniker = source_moniker;
        let source = WeakExtendedInstance::Component(source);
        let source2 = source.clone();
        let debug_router = Router::new(move |request: Option<Request>, debug: bool| {
            let source2 = source2.clone();
            async move {
                let _ = request.unwrap();
                assert!(debug);
                let res: Result<Capability, RouterError> =
                    Ok(Capability::Instance(WeakInstanceToken::from(source2.clone())));
                res
            }
            .boxed()
        });
        let router = debug_router.clone().on_readable(scope.clone());

        let target = ComponentInstance::new_root(
            ComponentInput::default(),
            Environment::empty(),
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "test:///target".parse().unwrap(),
        )
        .await;
        let metadata = Dict::new();
        metadata.set_availability(Availability::Required);
        let capability = router
            .route(Some(Request { target: target.as_weak().into(), metadata }), true)
            .await
            .unwrap();
        assert_matches!(
            capability,
            Capability::Instance(c) if <WeakInstanceToken as WeakInstanceTokenExt<ComponentInstance>>::moniker(&c) == source.extended_moniker()
        );
    }

    #[fuchsia::test]
    async fn lazy_get() {
        let source = Capability::Data(Data::String("hello".to_string()));
        let dict1 = Dict::new();
        dict1.insert("source".parse().unwrap(), source).expect("dict entry already exists");

        let base_router = Router::new_ok(dict1);
        let downscoped_router = base_router.lazy_get(
            RelativePath::new("source").unwrap(),
            RoutingError::BedrockMemberAccessUnsupported { moniker: Moniker::root().into() },
        );

        let metadata = Dict::new();
        metadata.set_availability(Availability::Optional);
        let capability = downscoped_router
            .route(
                Some(Request {
                    target: test_invalid_instance_token::<ComponentInstance>(),
                    metadata,
                }),
                false,
            )
            .await
            .unwrap();
        let capability = match capability {
            Capability::Data(d) => d,
            c => panic!("Bad enum {:#?}", c),
        };
        assert_eq!(capability, Data::String("hello".to_string()));
    }

    #[fuchsia::test]
    async fn lazy_get_deep() {
        let source = Capability::Data(Data::String("hello".to_string()));
        let dict1 = Dict::new();
        dict1.insert("source".parse().unwrap(), source).expect("dict entry already exists");
        let dict2 = Dict::new();
        dict2
            .insert("dict1".parse().unwrap(), Capability::Dictionary(dict1))
            .expect("dict entry already exists");
        let dict3 = Dict::new();
        dict3
            .insert("dict2".parse().unwrap(), Capability::Dictionary(dict2))
            .expect("dict entry already exists");
        let dict4 = Dict::new();
        dict4
            .insert("dict3".parse().unwrap(), Capability::Dictionary(dict3))
            .expect("dict entry already exists");

        let base_router = Router::new_ok(dict4);
        let downscoped_router = base_router.lazy_get(
            RelativePath::new("dict3/dict2/dict1/source").unwrap(),
            RoutingError::BedrockMemberAccessUnsupported { moniker: Moniker::root().into() },
        );

        let metadata = Dict::new();
        metadata.set_availability(Availability::Optional);
        let capability = downscoped_router
            .route(
                Some(Request {
                    target: test_invalid_instance_token::<ComponentInstance>(),
                    metadata,
                }),
                false,
            )
            .await
            .unwrap();
        let capability = match capability {
            Capability::Data(d) => d,
            c => panic!("Bad enum {:#?}", c),
        };
        assert_eq!(capability, Data::String("hello".to_string()));
    }

    #[fuchsia::test]
    async fn get_router_or_not_found() {
        let source = Router::new(Capability::Data(Data::String("hello".to_string())));
        let dict1 = Dict::new();
        dict1.insert("source".parse().unwrap(), source.into()).expect("dict entry already exists");

        let router = dict1.get_router_or_not_found(
            &RelativePath::new("source").unwrap(),
            RoutingError::BedrockMemberAccessUnsupported { moniker: Moniker::root().into() },
        );

        let metadata = Dict::new();
        metadata.set_availability(Availability::Optional);
        let capability = router.route(None, false).await.unwrap();
        let capability = match capability {
            Capability::Data(d) => d,
            c => panic!("Bad enum {:#?}", c),
        };
        assert_eq!(capability, Data::String("hello".to_string()));
    }

    #[fuchsia::test]
    async fn get_router_or_not_found_deep() {
        let source = Capability::Data(Data::String("hello".to_string()));
        let dict1 = Dict::new();
        dict1.insert("source".parse().unwrap(), source.into()).expect("dict entry already exists");
        let dict2 = Dict::new();
        dict2
            .insert("dict1".parse().unwrap(), Capability::Dictionary(dict1))
            .expect("dict entry already exists");
        let dict3 = Dict::new();
        dict3
            .insert("dict2".parse().unwrap(), Capability::Dictionary(dict2))
            .expect("dict entry already exists");
        let dict4 = Dict::new();
        dict4
            .insert("dict3".parse().unwrap(), Router::new(dict3).into())
            .expect("dict entry already exists");

        let router = dict4.get_router_or_not_found(
            &RelativePath::new("dict3/dict2/dict1/source").unwrap(),
            RoutingError::BedrockMemberAccessUnsupported { moniker: Moniker::root().into() },
        );

        let metadata = Dict::new();
        metadata.set_availability(Availability::Optional);
        let capability = router.route(None, false).await.unwrap();
        let capability = match capability {
            Capability::Data(d) => d,
            c => panic!("Bad enum {:#?}", c),
        };
        assert_eq!(capability, Data::String("hello".to_string()));
    }
}
