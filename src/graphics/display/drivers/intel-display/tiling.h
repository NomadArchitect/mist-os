// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_TILING_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_TILING_H_

#include <fuchsia/hardware/display/controller/c/banjo.h>
#include <fuchsia/hardware/intelgpucore/c/banjo.h>

#include <cinttypes>

#include <fbl/algorithm.h>

namespace intel_display {

constexpr int get_tile_byte_width(image_tiling_type_t tiling) {
  switch (tiling) {
    case IMAGE_TILING_TYPE_LINEAR:
      return 64;
    case IMAGE_TILING_TYPE_X_TILED:
      return 512;
    case IMAGE_TILING_TYPE_Y_LEGACY_TILED:
      return 128;
    case IMAGE_TILING_TYPE_YF_TILED:
      // TODO(https://fxbug.dev/42076787): For 1-byte-per-pixel formats (e.g. R8), the
      // tile width is 64. We need to check the pixel format once we support
      // importing such formats.
      return 128;
    default:
      assert(false);
      return 0;
  }
}

constexpr int get_tile_byte_size(image_tiling_type_t tiling) {
  return tiling == IMAGE_TILING_TYPE_LINEAR ? 64 : 4096;
}

constexpr int get_tile_px_height(image_tiling_type_t tiling) {
  return get_tile_byte_size(tiling) / get_tile_byte_width(tiling);
}

constexpr uint32_t width_in_tiles(image_tiling_type_t tiling, int width, int bytes_per_pixel) {
  int tile_width = get_tile_byte_width(tiling);
  return ((width * bytes_per_pixel) + tile_width - 1) / tile_width;
}

constexpr uint32_t height_in_tiles(image_tiling_type_t tiling, int height) {
  int tile_height = get_tile_px_height(tiling);
  return (height + tile_height - 1) / tile_height;
}

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_TILING_H_
