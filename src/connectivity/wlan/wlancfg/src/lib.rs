// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// The complexity of a separate struct doesn't seem universally better than having many arguments
#![allow(clippy::too_many_arguments)]
// The readability of redundant closures is preferred over the small compiler optimization.
#![allow(clippy::redundant_closure)]

pub mod access_point;
pub mod client;
pub mod config_management;
pub mod legacy;
pub mod mode_management;
pub mod regulatory_manager;
pub mod telemetry;
#[cfg(test)]
mod tests;
pub mod util;
