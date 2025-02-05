// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use delivery_blob::compression::ChunkedArchiveError;
use delivery_blob::DeliveryBlobError;
use fxfs::errors::FxfsError;
use fxfs::log::*;
use zx::Status;

pub fn map_to_status(error: anyhow::Error) -> Status {
    if let Some(status) = error.root_cause().downcast_ref::<Status>() {
        status.clone()
    } else if let Some(fxfs_error) = error.root_cause().downcast_ref::<FxfsError>() {
        fxfs_error.clone().into()
    } else if let Some(delivery_blob_error) = error.root_cause().downcast_ref::<DeliveryBlobError>()
    {
        delivery_blob_error.clone().into()
    } else if let Some(_) = error.root_cause().downcast_ref::<ChunkedArchiveError>() {
        Status::IO_DATA_INTEGRITY
    } else {
        // Print the internal error if we re-map it because we will lose any context after this.
        warn!(error:?; "Internal error");
        Status::INTERNAL
    }
}
