// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_bluetooth_sys::{PairingRequest, PairingRequestStream};
use futures::stream::TryStreamExt;
use log::{info, warn};

use crate::host_dispatcher::HostDispatcher;

pub mod pairing_dispatcher;
pub mod pairing_requests;

pub async fn run(hd: HostDispatcher, mut stream: PairingRequestStream) -> Result<(), Error> {
    while let Some(request) = stream.try_next().await? {
        handler(hd.clone(), request).await;
    }
    Ok(())
}

async fn handler(hd: HostDispatcher, request: PairingRequest) {
    match request {
        PairingRequest::SetPairingDelegate { input, output, delegate, control_handle: _ } => {
            info!("fuchsia.bluetooth.sys.Pairing.SetPairingDelegate({:?}, {:?})", input, output);
            // Attempt to set the pairing delegate for the HostDispatcher. The
            // HostDispatcher will reject if there is currently an active delegate; in this
            // case `proxy` will be dropped, closing the channel.
            if let Err(e) = hd.set_pairing_delegate(delegate.into_proxy(), input, output) {
                warn!("Couldn't set PairingDelegate: {e:?}");
            }
        }
        PairingRequest::SetDelegate { .. } => {
            warn!("sys.Pairing.SetDelegate received, unimplemented, ignoring (will drop delegate)");
        }
    }
}
