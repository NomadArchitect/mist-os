// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use ffx_core as _;
use ffx_lib_args::FfxBuiltIn;

pub fn ffx_plugin_is_machine_supported() -> bool {
    unreachable!("This is a side effect needed for the jinja template")
}

pub fn ffx_plugin_has_schema() -> bool {
    unreachable!("This is a side effect needed for the jinja template")
}

pub async fn ffx_plugin_impl(
    _: &std::sync::Arc<dyn ffx_core::Injector>,
    _: FfxBuiltIn,
) -> Result<()> {
    unreachable!("This is a side effect needed for the jinja template")
}
