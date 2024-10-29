// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod binding_stubs;
pub mod mem;
pub mod props;
pub mod storage;

// Must be called by the runtime as a prologue to TA_CreateEntryPoint() for
// each TA to set up Fuchsia-specific state.
pub fn on_entrypoint_creation() {
    storage::on_entrypoint_creation()
}

// Must be called by the runtime as an epilogue to TA_DestroyEntryPoint() for
// each TA to tear down Fuchsia-specific state.
pub fn on_entrypoint_destruction() {
    storage::on_entrypoint_destruction()
}

pub fn panic(code: u32) {
    std::process::exit(code as i32)
}
