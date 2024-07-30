// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>

#include <soc/aml-s905d2/s905d2-gpio.h>
#include <soc/aml-s905d2/s905d2-hw.h>

#include "astro.h"

namespace astro {
namespace fpbus = fuchsia_hardware_platform_bus;

static const std::vector<fpbus::Mmio> astro_canvas_mmios{
    {{
        .base = S905D2_DMC_BASE,
        .length = S905D2_DMC_LENGTH,
    }},
};

static const std::vector<fpbus::Bti> astro_canvas_btis{
    {{
        .iommu_index = 0,
        .bti_id = BTI_CANVAS,
    }},
};

static const fpbus::Node canvas_dev = []() {
  fpbus::Node dev = {};
  dev.name() = "canvas";
  dev.vid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC;
  dev.pid() = bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC;
  dev.did() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_CANVAS;
  dev.mmio() = astro_canvas_mmios;
  dev.bti() = astro_canvas_btis;
  return dev;
}();

zx_status_t Astro::CanvasInit() {
  fidl::Arena<> fidl_arena;
  fdf::Arena arena('CANV');
  auto result = pbus_.buffer(arena)->NodeAdd(fidl::ToWire(fidl_arena, canvas_dev));
  if (!result.ok()) {
    zxlogf(ERROR, "%s: NodeAdd Canvas(canvas_dev) request failed: %s", __func__,
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "%s: NodeAdd Canvas(canvas_dev) failed: %s", __func__,
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  return ZX_OK;
}

}  // namespace astro
