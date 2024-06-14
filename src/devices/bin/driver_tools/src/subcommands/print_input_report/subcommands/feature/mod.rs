// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use anyhow::{Context, Result};
use args::FeatureCommand;
use futures::lock::Mutex;
use std::io::Write;
use std::ops::DerefMut;
use std::sync::Arc;
use {fidl_fuchsia_io as fio, fuchsia_zircon_status as zx};

pub async fn feature(
    cmd: &FeatureCommand,
    writer: Arc<Mutex<impl Write + Send + Sync + 'static>>,
    dev: fio::DirectoryProxy,
) -> Result<()> {
    let input_device_proxy = super::connect_to_input_device(&dev, &cmd.device_path)
        .context("Failed to get input device proxy")?;
    let feature_report = input_device_proxy
        .get_feature_report()
        .await
        .context("Failed to send request to get feature report")?
        .map_err(|e| zx::Status::from_raw(e))
        .context("Failed to get feature report")?;
    let mut writer = writer.lock().await;
    writeln!(&mut writer, "Feature from file: {:?}", &cmd.device_path,)?;
    super::write_feature_report(writer.deref_mut(), &feature_report)
        .context("Failed to write feature report")?;
    Ok(())
}
