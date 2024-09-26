// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::Deserialize;

/// Identifies which kind of declaration something is.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclType {
    Alias,
    Bits,
    Const,
    Enum,
    #[serde(rename = "experimental_resource")]
    Resource,
    NewType,
    Overlay,
    Protocol,
    Service,
    Struct,
    Table,
    Union,
}
