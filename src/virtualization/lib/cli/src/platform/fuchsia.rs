// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::platform::PlatformServices;
use anyhow::{Context, Result};
use async_trait::async_trait;
use fidl_fuchsia_virtualization::{
    GuestManagerMarker, GuestManagerProxy, LinuxManagerMarker, LinuxManagerProxy,
};
use fuchsia_component::client::{connect_to_protocol, connect_to_protocol_at_path};
use guest_cli_args::GuestType;

pub struct FuchsiaPlatformServices;

impl FuchsiaPlatformServices {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait(?Send)]
impl PlatformServices for FuchsiaPlatformServices {
    async fn connect_to_manager(&self, guest_type: GuestType) -> Result<GuestManagerProxy> {
        let manager = connect_to_protocol_at_path::<GuestManagerMarker>(
            format!("/svc/{}", guest_type.guest_manager_interface()).as_str(),
        )
        .context("Failed to connect to manager service")?;
        Ok(manager)
    }

    async fn connect_to_linux_manager(&self) -> Result<LinuxManagerProxy> {
        connect_to_protocol::<LinuxManagerMarker>()
            .context("Failed to connect to linux manager service")
    }
}
