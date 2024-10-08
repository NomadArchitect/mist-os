// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod system_activity_governor;

use crate::system_activity_governor::SystemActivityGovernor;
use anyhow::Result;
use fuchsia_async::{DurationExt, TimeoutExt};
use fuchsia_component::client::{connect_to_protocol, connect_to_service_instance, open_service};
use fuchsia_inspect::health::Reporter;
use futures::{TryFutureExt, TryStreamExt};
use sag_config::Config;
use zx::Duration;
use {fidl_fuchsia_hardware_suspend as fhsuspend, fidl_fuchsia_power_broker as fbroker};

const SUSPEND_DEVICE_TIMEOUT: Duration = Duration::from_seconds(10);

async fn connect_to_suspender() -> Result<fhsuspend::SuspenderProxy> {
    let service_dir =
        open_service::<fhsuspend::SuspendServiceMarker>().expect("failed to open service dir");

    let mut watcher = fuchsia_fs::directory::Watcher::new(&service_dir)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create watcher: {:?}", e))?;

    // Connect to the first suspend service instance that is discovered.
    let filename = loop {
        let next = watcher
            .try_next()
            .map_err(|e| anyhow::anyhow!("Failed to get next watch message: {e:?}"))
            .on_timeout(SUSPEND_DEVICE_TIMEOUT.after_now(), || {
                Err(anyhow::anyhow!("Timeout waiting for next watcher message."))
            })
            .await?;

        if let Some(watch_msg) = next {
            let filename = watch_msg.filename.as_path().to_str().unwrap().to_owned();
            if filename != "." {
                if watch_msg.event == fuchsia_fs::directory::WatchEvent::ADD_FILE
                    || watch_msg.event == fuchsia_fs::directory::WatchEvent::EXISTING
                {
                    break Ok(filename);
                }
            }
        } else {
            break Err(anyhow::anyhow!("Suspend service watcher returned None entry."));
        }
    }?;

    let svc_inst =
        connect_to_service_instance::<fhsuspend::SuspendServiceMarker>(filename.as_str())?;

    svc_inst
        .connect_to_suspender()
        .map_err(|e| anyhow::anyhow!("Failed to connect to suspender: {:?}", e))
}

#[fuchsia::main]
async fn main() -> Result<()> {
    tracing::info!("started");
    fuchsia_trace_provider::trace_provider_create_with_fdio();

    let inspector = fuchsia_inspect::component::inspector();
    let _inspect_server_task =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default());
    fuchsia_inspect::component::health().set_starting_up();

    let config = Config::take_from_startup_handle();
    inspector.root().record_child("config", |config_node| config.record_inspect(config_node));

    // Set up the SystemActivityGovernor.
    let use_suspender = config.use_suspender;
    tracing::info!("use_suspender={use_suspender}");
    let suspender = if use_suspender {
        // TODO(https://fxbug.dev/361403498): Re-attempt to connect to suspender indefinitely once
        // dependents have aligned on the use of structured config for SAG.
        tracing::info!("Attempting to connect to suspender...");
        match connect_to_suspender().await {
            Ok(s) => {
                tracing::info!("Connected to suspender");
                Some(s)
            }
            Err(e) => {
                tracing::warn!("Unable to connect to suspender protocol: {e:?}");
                None
            }
        }
    } else {
        tracing::info!("Skipping connecting to suspender.");
        None
    };

    let wait_for_suspending_token = config.wait_for_suspending_token;
    tracing::info!("wait_for_suspending_token={wait_for_suspending_token}");

    let sag = SystemActivityGovernor::new(
        &connect_to_protocol::<fbroker::TopologyMarker>()?,
        inspector.root().clone_weak(),
        suspender,
    )
    .await?;

    fuchsia_inspect::component::health().set_ok();

    // This future should never complete.
    let result = sag.run().await;
    tracing::error!(?result, "Unexpected exit");
    fuchsia_inspect::component::health().set_unhealthy(&format!("Unexpected exit: {:?}", result));
    result
}
