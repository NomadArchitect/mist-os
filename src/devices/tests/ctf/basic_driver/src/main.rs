// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashSet;

use anyhow::{anyhow, Error, Result};
use fidl::endpoints::{create_endpoints, create_proxy, DiscoverableProtocolMarker, ServiceMarker};
use fuchsia_component::client::{connect_to_protocol, connect_to_service_instance_at};
use fuchsia_component::server::ServiceFs;
use fuchsia_fs::directory::WatchEvent;
use futures::channel::mpsc;
use futures::prelude::*;
use realm_client::{extend_namespace, InstalledNamespace};
use tracing::info;
use {
    fidl_fuchsia_basicdriver_ctftest as ctf, fidl_fuchsia_driver_test as fdt,
    fidl_fuchsia_driver_testing as ftest, fidl_fuchsia_io as fio, fuchsia_async as fasync,
};

async fn run_waiter_server(mut stream: ctf::WaiterRequestStream, mut sender: mpsc::Sender<()>) {
    while let Some(ctf::WaiterRequest::Ack { .. }) = stream.try_next().await.expect("Stream failed")
    {
        info!("Received Ack request");
        sender.try_send(()).expect("Sender failed")
    }
}

async fn run_offers_server(
    offers_server: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
    sender: mpsc::Sender<()>,
) -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(move |stream: ctf::WaiterRequestStream| {
        fasync::Task::spawn(run_waiter_server(stream, sender.clone())).detach()
    });
    // Serve the outgoing services
    fs.serve_connection(offers_server)?;
    Ok(fs.collect::<()>().await)
}

async fn create_realm(options: ftest::RealmOptions) -> Result<InstalledNamespace> {
    let realm_factory = connect_to_protocol::<ftest::RealmFactoryMarker>()?;
    let (dict_client, dict_server) = create_endpoints();
    realm_factory
        .create_realm2(options, dict_server)
        .await?
        .map_err(realm_client::Error::OperationError)?;
    let ns = extend_namespace(realm_factory, dict_client).await?;
    Ok(ns)
}

#[fuchsia::test]
async fn test_basic_driver() -> Result<()> {
    // We need to resolve our test component manually. Eventually component framework could provide
    // an introspection way of resolving your own component.
    // This isn't exactly correct because if the test is running in ctf, the root package will not
    // be called "basic-driver-test-latest".
    let resolved = {
        let client = fuchsia_component::client::connect_to_protocol_at_path::<
            fidl_fuchsia_component_resolution::ResolverMarker,
        >("/svc/fuchsia.component.resolution.Resolver-hermetic")?;
        let root = client
            .resolve(
                "fuchsia-pkg://fuchsia.com/basic-driver-test-latest#meta/basic-driver-test-root.cm",
            )
            .await?
            .expect("Failed to resolve our root component.");

        client
            .resolve_with_context(
                "basic-driver-test#meta/test-suite.cm",
                &root.resolution_context.unwrap(),
            )
            .await?
            .expect("Failed to resolve our own component.")
    };

    let (offers_client, offers_server) = create_endpoints();
    let realm_options = ftest::RealmOptions {
        driver_test_realm_start_args: Some(fdt::RealmArgs {
            dtr_offers: Some(vec![fidl_fuchsia_component_test::Capability::Protocol(
                fidl_fuchsia_component_test::Protocol {
                    name: Some(ctf::WaiterMarker::PROTOCOL_NAME.to_string()),
                    ..Default::default()
                },
            )]),
            dtr_exposes: Some(vec![fidl_fuchsia_component_test::Capability::Service(
                fidl_fuchsia_component_test::Service {
                    name: Some(ctf::ServiceMarker::SERVICE_NAME.to_string()),
                    ..Default::default()
                },
            )]),
            test_component: Some(resolved),
            ..Default::default()
        }),
        offers_client: Some(offers_client),
        ..Default::default()
    };
    let test_ns = create_realm(realm_options).await?;
    info!("connected to the test realm!");

    // Setup our offers to provide the Waiter, and wait to receive the waiter ack event.
    let (sender, mut receiver) = mpsc::channel(1);
    let receiver_next = receiver.next().fuse();
    let offers_server = run_offers_server(offers_server, sender).fuse();
    futures::pin_mut!(receiver_next);
    futures::pin_mut!(offers_server);
    futures::select! {
        _ = receiver_next => {}
        _ = offers_server => { panic!("should not quit offers_server."); }
    }

    // Check to make sure our topological devfs connections is working.
    let (devfs_client, server) = create_proxy::<fio::DirectoryMarker>().unwrap();
    fdio::open_deprecated(
        &format!("{}/dev-topological", test_ns.prefix()),
        fio::OpenFlags::RIGHT_READABLE,
        server.into_channel(),
    )
    .unwrap();
    device_watcher::recursive_wait(&devfs_client, "sys/test").await?;

    // Connect to the device. We have already received an ack from the driver, but sometimes
    // seeing the item in the service directory doesn't happen immediately. So we wait for a
    // corresponding directory watcher event.
    let (service, server) = create_proxy::<fio::DirectoryMarker>().unwrap();
    fdio::open_deprecated(
        &format!("{}/{}", test_ns.prefix(), ctf::ServiceMarker::SERVICE_NAME),
        fio::OpenFlags::RIGHT_READABLE,
        server.into_channel(),
    )
    .unwrap();

    let mut watcher = fuchsia_fs::directory::Watcher::new(&service).await?;
    let mut instances: HashSet<String> = Default::default();
    let mut event = None;
    while instances.len() != 1 {
        event = watcher.next().await;
        let Some(Ok(ref message)) = event else { break };
        let filename = message.filename.as_path().to_str().unwrap().to_owned();
        if filename == "." {
            continue;
        }
        match message.event {
            WatchEvent::ADD_FILE | WatchEvent::EXISTING => _ = instances.insert(filename),
            WatchEvent::REMOVE_FILE => _ = instances.remove(&filename),
            WatchEvent::IDLE => {}
            WatchEvent::DELETED => break,
        }
    }
    if instances.len() != 1 {
        return Err(anyhow!(
            "Expected to find one instance within the service directory. \
            Last event: {event:?}. Instances: {instances:?}"
        ));
    }

    let service_instance = connect_to_service_instance_at::<ctf::ServiceMarker>(
        test_ns.prefix(),
        instances.iter().next().unwrap(),
    )?;
    let device = service_instance.connect_to_device()?;

    // Talk to the device!
    let pong = device.ping().await?;
    assert_eq!(42, pong);
    Ok(())
}
