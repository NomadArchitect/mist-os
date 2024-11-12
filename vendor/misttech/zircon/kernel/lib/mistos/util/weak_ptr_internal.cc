// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/mistos/memory/weak_ptr_internal.h"

#include <zircon/assert.h>

namespace mtl::internal {

WeakPtrFlag::WeakPtrFlag() : is_valid_(true) {}

WeakPtrFlag::~WeakPtrFlag() {
  // Should be invalidated before destruction.
  Guard<fbl::Mutex> guard{lock()};
  ZX_DEBUG_ASSERT(!is_valid_);
}

void WeakPtrFlag::Invalidate() {
  // Invalidation should happen exactly once.
  Guard<fbl::Mutex> guard{lock()};
  ZX_DEBUG_ASSERT(is_valid_);
  is_valid_ = false;
}

}  // namespace mtl::internal
