// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl_fuchsia_power_system as fsystem;

pub struct WakeLease {
    _token: fsystem::WakeLeaseToken,
}

impl WakeLease {
    pub async fn take(
        activity_governor: fsystem::ActivityGovernorProxy,
        name: String,
    ) -> Result<Self> {
        let _token = activity_governor.take_wake_lease(&name).await?;
        Ok(Self { _token })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use fidl::endpoints::create_proxy_and_stream;
    use futures::channel::mpsc;
    use futures::prelude::*;
    use {fuchsia_async as fasync, fuchsia_zircon as zx};

    struct FakeActivityGovernor {
        // Sends updates in server wake lease state (active, inactive).
        wake_lease_active: mpsc::UnboundedSender<bool>,
    }

    impl FakeActivityGovernor {
        async fn run(&self, stream: fsystem::ActivityGovernorRequestStream) -> Result<()> {
            stream
                .map(|request| request.context("failed request"))
                .try_for_each(|request| async {
                    match request {
                        fsystem::ActivityGovernorRequest::TakeWakeLease {
                            name: _name,
                            responder,
                        } => {
                            let (server_token, client_token) = fsystem::WakeLeaseToken::create();
                            let wake_lease_active = self.wake_lease_active.clone();
                            assert!(wake_lease_active.unbounded_send(true).is_ok());

                            // Listen for the client dropping its wake lease token.
                            fasync::Task::local(async move {
                                let _ = fasync::OnSignals::new(
                                    server_token,
                                    zx::Signals::EVENTPAIR_PEER_CLOSED,
                                )
                                .await;
                                wake_lease_active
                                    .unbounded_send(false)
                                    .expect("server dropping wake lease status");
                            })
                            .detach();

                            responder.send(client_token).context("send failed")
                        }
                        _ => unreachable!(),
                    }
                })
                .await
        }
    }

    #[fasync::run_until_stalled(test)]
    async fn test_acquire_then_release() -> Result<()> {
        let (client, stream) = create_proxy_and_stream::<fsystem::ActivityGovernorMarker>()?;
        let (wake_lease_active_tx, mut wake_lease_active_rx) = mpsc::unbounded::<bool>();

        // Create a FakeActivityGovernor server and run it in the background.
        fasync::Task::local(async move {
            let server = FakeActivityGovernor { wake_lease_active: wake_lease_active_tx };
            server.run(stream).await.expect("FakeActivityGovernor server completion");
        })
        .detach();

        // Create and acquire a wake lease.
        let wake_lease = WakeLease::take(client, "example_wake_lease".to_string()).await?;

        // Check that the server was notified about the acquired wake lease.
        assert!(wake_lease_active_rx.next().await.expect("server wake lease call"));

        // Release the wake lease and check that the server was notified.
        drop(wake_lease);
        assert!(!wake_lease_active_rx.next().await.expect("server wake lease call"));

        Ok(())
    }
}
