// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::BlockDevice;
use crate::environment::{Container, Filesystem, FilesystemLauncher, FxfsContainer};
use anyhow::{Context, Error, Result};
use fidl_fuchsia_device::ControllerProxy;
use fs_management::filesystem::ServingMultiVolumeFilesystem;
use fs_management::format::DiskFormat;
use fs_management::partition::{find_partition, PartitionMatcher};
use fuchsia_zircon::{self as zx, Duration};
use futures::StreamExt;
use std::sync::Arc;
use vfs::service;

/// Make a new vfs service node that implements fuchsia.update.verify.BlobfsVerifier
pub fn blobfs_verifier_service() -> Arc<service::Service> {
    service::host(
        move |mut stream: fidl_fuchsia_update_verify::BlobfsVerifierRequestStream| async move {
            while let Some(request) = stream.next().await {
                match request {
                    Ok(fidl_fuchsia_update_verify::BlobfsVerifierRequest::Verify {
                        responder,
                        ..
                    }) => {
                        // TODO(https://fxbug.dev/42077105): Implement by calling out to Fxfs' blob volume.
                        responder.send(Ok(())).unwrap_or_else(|e| {
                            tracing::error!("failed to send Verify response. error: {:?}", e);
                        });
                    }
                    Err(e) => {
                        tracing::error!("BlobfsVerifier server failed: {:?}", e);
                        return;
                    }
                }
            }
        },
    )
}

const FIND_PARTITION_DURATION: Duration = Duration::from_seconds(10);

async fn find_fxblob_partition(ramdisk_prefix: Option<String>) -> Result<ControllerProxy, Error> {
    let matcher = PartitionMatcher {
        detected_disk_formats: Some(vec![DiskFormat::Fxfs]),
        ignore_prefix: ramdisk_prefix,
        ..Default::default()
    };

    find_partition(matcher, FIND_PARTITION_DURATION).await.context("failed to find Fxfs")
}

/// Mounts (or formats) the data volume in Fxblob.  Assumes the partition is already formatted.
pub async fn mount_or_format_data(
    ramdisk_prefix: Option<String>,
    launcher: &FilesystemLauncher,
) -> Result<(ServingMultiVolumeFilesystem, Filesystem), Error> {
    let partition_controller = find_fxblob_partition(ramdisk_prefix).await?;
    let partition_path = partition_controller
        .get_topological_path()
        .await
        .context("get_topo_path transport error")?
        .map_err(zx::Status::from_raw)
        .context("get_topo_path returned error")?;
    tracing::info!(%partition_path, "Found Fxblob partition");
    let mut device = Box::new(
        BlockDevice::from_proxy(partition_controller, &partition_path)
            .await
            .context("failed to make new device")?,
    );
    let mut container = Box::new(FxfsContainer::new(
        launcher.serve_fxblob(device.as_mut()).await.context("serving Fxblob")?,
    ));
    let data = container.serve_data(&launcher).await.context("serving data from Fxblob")?;

    Ok((container.into_fs(), data))
}
