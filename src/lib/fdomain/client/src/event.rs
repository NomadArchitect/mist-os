// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::handle::handle_type;
use crate::Handle;

/// An event handle in a remote FDomain.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Event(pub(crate) Handle);
handle_type!(Event);
