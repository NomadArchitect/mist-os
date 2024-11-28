// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod test_server;

use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use test_runners_lib::elf;
use test_server::TestServer;
use thiserror::Error;
use tracing::{error, info, warn};
use {fidl_fuchsia_component_runner as fcrunner, fuchsia_async as fasync};

#[fuchsia::main(logging_tags=["rust_test_runner"])]
async fn main() -> Result<(), anyhow::Error> {
    info!("started");
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    fuchsia_trace_provider::trace_provider_wait_for_init();

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(move |stream| {
        fasync::Task::local(
            async move { start_runner(stream).await.expect("failed to start runner.") },
        )
        .detach();
    });
    fs.take_and_serve_directory_handle()?;
    fs.collect::<()>().await;
    Ok(())
}

/// Error encountered by runner.
#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("Cannot read request: {:?}", _0)]
    RequestRead(fidl::Error),
}

async fn start_runner(
    mut request_stream: fcrunner::ComponentRunnerRequestStream,
) -> Result<(), RunnerError> {
    while let Some(event) = request_stream.try_next().await.map_err(RunnerError::RequestRead)? {
        match event {
            fcrunner::ComponentRunnerRequest::Start { start_info, controller, .. } => {
                let url = start_info.resolved_url.clone().unwrap_or("".to_owned());
                if let Err(e) = elf::start_component(
                    start_info,
                    controller,
                    get_new_test_server,
                    TestServer::validate_args,
                )
                .await
                {
                    warn!("Cannot start component '{}': {:?}", url, e)
                };
            }
            fcrunner::ComponentRunnerRequest::_UnknownMethod { ordinal, .. } => {
                warn!(%ordinal, "Unknown ComponentRunner request");
            }
        }
    }
    Ok(())
}

fn get_new_test_server() -> TestServer {
    TestServer::new()
}
