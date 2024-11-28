// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Constants use throughout assembly including file destinations and kernel arguments.

mod files;
mod kernel_args;

pub use files::{
    BlobfsCompiledPackageDestination, BootfsCompiledPackageDestination, BootfsDestination,
    BootfsPackageDestination, CompiledPackageDestination, Destination, FileEntry,
    PackageDestination, PackageSetDestination, TestCompiledPackageDestination,
};
pub use kernel_args::{Arm64DebugDapSoc, KernelArg, ZeroPageScanCount};
