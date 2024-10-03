// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::Deserialize;

use super::r#type::Type;
use super::{Attributes, CompIdent};
use crate::de::Index;

#[derive(Clone, Debug, Deserialize)]
pub struct Struct {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: CompIdent,
    pub members: Vec<StructMember>,
    #[serde(rename = "resource")]
    pub is_resource: bool,
}

impl Index for Struct {
    type Key = CompIdent;

    fn key(&self) -> &Self::Key {
        &self.name
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct StructMember {
    #[expect(dead_code)]
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: String,
    #[serde(rename = "type")]
    pub ty: Type,
}
