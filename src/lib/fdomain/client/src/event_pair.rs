// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::handle::handle_type;
use crate::Handle;

/// An event pair handle in a remote FDomain.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Eventpair(pub(crate) Handle);
handle_type!(Eventpair EVENTPAIR peered);
