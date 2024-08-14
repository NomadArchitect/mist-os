// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use block_client::{BlockClient, BufferSlice, MutableBufferSlice, RemoteBlockClient};
use fidl_fuchsia_hardware_block::BlockMarker;
use fidl_fuchsia_hardware_block_volume::{VolumeMarker, VolumeProxy};
use fuchsia_component::client::connect_to_protocol_at_path;
use fuchsia_zircon as zx;
use storage_stress_test_utils::fvm::{get_volume_path, Guid};

fn fidl_to_status(result: Result<i32, fidl::Error>) -> Result<(), zx::Status> {
    match result {
        Ok(code) => zx::Status::ok(code),
        Err(e) => {
            if e.is_closed() {
                Err(zx::Status::PEER_CLOSED)
            } else {
                panic!("Unrecoverable connection error: {}", e);
            }
        }
    }
}

fn anyhow_to_status(result: Result<(), anyhow::Error>) -> Result<(), zx::Status> {
    match result {
        Ok(()) => Ok(()),
        Err(e) => match e.downcast::<zx::Status>() {
            Ok(s) => Err(s),
            Err(e) => panic!("Unrecoverable connection error: {:?}", e),
        },
    }
}

/// Represents a connection to a particular FVM volume.
/// Using this struct one can perform I/O, extend, shrink or destroy the volume.
pub struct VolumeConnection {
    volume_proxy: VolumeProxy,
    block_device: RemoteBlockClient,
    slice_size: u64,
}

impl VolumeConnection {
    pub async fn new(volume_guid: &Guid, slice_size: u64) -> Self {
        let volume_path = get_volume_path(volume_guid).await;
        let volume_path = volume_path.to_str().unwrap();

        let volume_proxy = connect_to_protocol_at_path::<VolumeMarker>(volume_path).unwrap();
        let block_proxy = connect_to_protocol_at_path::<BlockMarker>(volume_path).unwrap();
        let block_device = RemoteBlockClient::new(block_proxy).await.unwrap();

        Self { volume_proxy, block_device, slice_size }
    }

    // Writes a slice worth of data at the given offset.
    pub async fn write_slice_at(&self, data: &[u8], slice_offset: u64) -> Result<(), zx::Status> {
        let offset = slice_offset * self.slice_size;
        assert_eq!(data.len() as u64, self.slice_size);

        let buffer_slice = BufferSlice::from(data);
        let result = self.block_device.write_at(buffer_slice, offset).await;
        return anyhow_to_status(result);
    }

    // Reads a slice worth of data from the given offset.
    pub async fn read_slice_at(&self, slice_offset: u64) -> Result<Vec<u8>, zx::Status> {
        let mut data: Vec<u8> = Vec::with_capacity(self.slice_size as usize);
        data.resize(self.slice_size as usize, 0);

        let offset = slice_offset * self.slice_size;
        assert_eq!(data.len() as u64, self.slice_size);

        let buffer_slice = MutableBufferSlice::from(data.as_mut_slice());
        let result = self.block_device.read_at(buffer_slice, offset).await;
        let result = anyhow_to_status(result);
        let result = result.map(|_| data);

        return result;
    }

    // Adds slices to the volume at a given offset.
    pub async fn extend(&self, start_slice: u64, slice_count: u64) -> Result<(), zx::Status> {
        let result = self.volume_proxy.extend(start_slice, slice_count).await;
        return fidl_to_status(result);
    }

    // Removes slices from the volume at a given offset.
    pub async fn shrink(&self, start_slice: u64, slice_count: u64) -> Result<(), zx::Status> {
        let result = self.volume_proxy.shrink(start_slice, slice_count).await;
        return fidl_to_status(result);
    }

    // Destroys the volume, returning all slices to the volume manager.
    pub async fn destroy(&self) -> Result<(), zx::Status> {
        let result = self.volume_proxy.destroy().await;
        return fidl_to_status(result);
    }

    pub fn slice_size(&self) -> u64 {
        self.slice_size
    }
}
