// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::CapabilityBound;
use fidl_fuchsia_component_sandbox as fsandbox;
use std::fmt::Debug;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unit;

impl From<Unit> for fsandbox::Unit {
    fn from(_unit: Unit) -> Self {
        Self {}
    }
}

impl From<Unit> for fsandbox::Capability {
    fn from(unit: Unit) -> Self {
        Self::Unit(unit.into())
    }
}

impl CapabilityBound for Unit {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_into_fidl() {
        let unit = Unit::default();
        let fidl_capability: fsandbox::Capability = unit.into();
        assert_eq!(fidl_capability, fsandbox::Capability::Unit(fsandbox::Unit {}));
    }
}
