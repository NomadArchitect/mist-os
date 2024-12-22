// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "startup-symbols.h"

// This module defines initializers and finalizer functions that are combined
// into a single DT_INIT/DT_FINI entry point. They expect a global variable to
// be in a certain state before it is updated.

extern "C" [[gnu::retain]] void _init() {
  if (gInitFiniState == 0) {
    gInitFiniState = 101;
  } else {
    gInitFiniState = -101;
  }
}

extern "C" [[gnu::retain]] void _fini() {
  if (gInitFiniState == 101) {
    gInitFiniState = 102;
  } else {
    gInitFiniState = -102;
  }
}
