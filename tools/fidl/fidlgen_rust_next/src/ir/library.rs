// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;

use serde::Deserialize;

use crate::de::Index;

use super::{CompIdent, DeclType, TypeShape};

#[derive(Debug, Deserialize)]
pub struct Library {
    pub name: String,
    pub declarations: HashMap<CompIdent, ExternalDeclaration>,
}

impl Index for Library {
    type Key = String;

    fn key(&self) -> &Self::Key {
        &self.name
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ExternalDeclaration {
    pub kind: DeclType,
    #[serde(rename = "resource", default)]
    #[expect(dead_code)]
    pub is_resouce: bool,
    #[serde(rename = "type_shape_v2")]
    pub shape: Option<TypeShape>,
}
