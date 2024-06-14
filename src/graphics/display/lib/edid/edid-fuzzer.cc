// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <climits>
#include <cstddef>
#include <cstdint>

#include "src/graphics/display/lib/edid/edid.h"

// fuzz_target.cc
extern "C" int LLVMFuzzerTestOneInput(const uint8_t* data, size_t size) {
  edid::Edid edid;
  if (size > UINT16_MAX) {
    return 0;
  }

  fit::result<const char*> result = edid.Init(cpp20::span(data, size));
  if (!result.is_ok()) {
    return 0;
  }

  // Use a static variable to introduce optimization-preventing side-effects.
  [[maybe_unused]] static size_t count = 0;
  count += edid.is_hdmi() ? 0 : 1;
  for (auto it = edid::timing_iterator(&edid); it.is_valid(); ++it) {
    count++;
  }
  edid.Print([](const char* str) {});

  return 0;
}
