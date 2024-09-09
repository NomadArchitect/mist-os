// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/framebuffer-display/framebuffer-display-driver.h"

#include <fidl/fuchsia.hardware.sysmem/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/mmio/mmio-buffer.h>

#include <memory>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/display/cpp/bind.h>
#include <fbl/alloc_checker.h>

#include "src/graphics/display/lib/framebuffer-display/framebuffer-display.h"

namespace framebuffer_display {

FramebufferDisplayDriver::FramebufferDisplayDriver(
    std::string_view device_name, fdf::DriverStartArgs start_args,
    fdf::UnownedSynchronizedDispatcher driver_dispatcher)
    : fdf::DriverBase(device_name, std::move(start_args), std::move(driver_dispatcher)) {}

FramebufferDisplayDriver::~FramebufferDisplayDriver() = default;

zx::result<> FramebufferDisplayDriver::Start() {
  zx::result<> configure_hardware_result = ConfigureHardware();
  if (configure_hardware_result.is_error()) {
    FDF_LOG(ERROR, "Failed to configure hardware: %s", configure_hardware_result.status_string());
    return configure_hardware_result.take_error();
  }

  zx::result<std::unique_ptr<FramebufferDisplay>> framebuffer_display_result =
      CreateAndInitializeFramebufferDisplay();
  if (framebuffer_display_result.is_error()) {
    FDF_LOG(ERROR, "Failed to create and initialize FramebufferDisplay: %s",
            framebuffer_display_result.status_string());
    return framebuffer_display_result.take_error();
  }
  framebuffer_display_ = std::move(framebuffer_display_result).value();

  zx::result<> add_banjo_server_node_result = InitializeBanjoServerNode();
  if (add_banjo_server_node_result.is_error()) {
    FDF_LOG(ERROR, "Failed to add banjo server node: %s",
            add_banjo_server_node_result.status_string());
    return add_banjo_server_node_result.take_error();
  }

  return zx::ok();
}

void FramebufferDisplayDriver::Stop() {}

zx::result<std::unique_ptr<FramebufferDisplay>>
FramebufferDisplayDriver::CreateAndInitializeFramebufferDisplay() {
  zx::result<fdf::MmioBuffer> frame_buffer_mmio_result = GetFrameBufferMmioBuffer();
  if (frame_buffer_mmio_result.is_error()) {
    FDF_LOG(ERROR, "Failed to get frame buffer mmio buffer: %s",
            frame_buffer_mmio_result.status_string());
    return frame_buffer_mmio_result.take_error();
  }
  fdf::MmioBuffer frame_buffer_mmio = std::move(frame_buffer_mmio_result).value();

  zx::result<DisplayProperties> display_properties_result = GetDisplayProperties();
  if (display_properties_result.is_error()) {
    FDF_LOG(ERROR, "Failed to get display properties: %s",
            display_properties_result.status_string());
    return display_properties_result.take_error();
  }
  DisplayProperties display_properties = std::move(display_properties_result).value();

  zx::result<fidl::ClientEnd<fuchsia_hardware_sysmem::Sysmem>> hardware_sysmem_result =
      incoming()->Connect<fuchsia_hardware_sysmem::Sysmem>();
  if (hardware_sysmem_result.is_error()) {
    FDF_LOG(ERROR, "Failed to get hardware sysmem protocol: %s",
            hardware_sysmem_result.status_string());
    return hardware_sysmem_result.take_error();
  }
  fidl::WireSyncClient hardware_sysmem{std::move(hardware_sysmem_result).value()};

  zx::result<fidl::ClientEnd<fuchsia_sysmem2::Allocator>> sysmem_result =
      incoming()->Connect<fuchsia_sysmem2::Allocator>();
  if (sysmem_result.is_error()) {
    FDF_LOG(ERROR, "Failed to get fuchsia.sysmem2.Allocator protocol: %s",
            sysmem_result.status_string());
    return sysmem_result.take_error();
  }
  fidl::WireSyncClient sysmem(std::move(sysmem_result).value());

  zx::result<fdf::SynchronizedDispatcher> create_dispatcher_result =
      fdf::SynchronizedDispatcher::Create(fdf::SynchronizedDispatcher::Options::kAllowSyncCalls,
                                          "framebuffer-display-dispatcher",
                                          [](fdf_dispatcher_t*) {});
  if (create_dispatcher_result.is_error()) {
    FDF_LOG(ERROR, "Failed to create framebuffer display dispatcher: %s",
            create_dispatcher_result.status_string());
    return create_dispatcher_result.take_error();
  }
  framebuffer_display_dispatcher_ = std::move(create_dispatcher_result).value();

  fbl::AllocChecker alloc_checker;
  auto framebuffer_display = fbl::make_unique_checked<FramebufferDisplay>(
      &alloc_checker, std::move(hardware_sysmem), std::move(sysmem), std::move(frame_buffer_mmio),
      std::move(display_properties), framebuffer_display_dispatcher_.async_dispatcher());
  if (!alloc_checker.check()) {
    FDF_LOG(ERROR, "Failed to allocate memory for FramebufferDisplay");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result<> framebuffer_display_initialize_result = framebuffer_display->Initialize();
  if (framebuffer_display_initialize_result.is_error()) {
    FDF_LOG(ERROR, "Failed to initialize FramebufferDisplay: %s",
            framebuffer_display_initialize_result.status_string());
    return framebuffer_display_initialize_result.take_error();
  }

  return zx::ok(std::move(framebuffer_display));
}

zx::result<> FramebufferDisplayDriver::InitializeBanjoServerNode() {
  ZX_DEBUG_ASSERT(framebuffer_display_ != nullptr);
  display_engine_protocol_t protocol = framebuffer_display_->GetProtocol();

  // Serves the [`fuchsia.hardware.display.controller/ControllerImpl`] protocol
  // over the compatibility server.
  banjo_server_ = compat::BanjoServer(ZX_PROTOCOL_DISPLAY_ENGINE, protocol.ctx, protocol.ops);
  compat::DeviceServer::BanjoConfig banjo_config;
  banjo_config.callbacks[ZX_PROTOCOL_DISPLAY_ENGINE] = banjo_server_->callback();
  zx::result<> compat_server_init_result =
      compat_server_.Initialize(incoming(), outgoing(), node_name(), name(),
                                /*forward_metadata=*/compat::ForwardMetadata::None(),
                                /*banjo_config=*/std::move(banjo_config));
  if (compat_server_init_result.is_error()) {
    return compat_server_init_result.take_error();
  }

  const std::vector<fuchsia_driver_framework::NodeProperty> node_properties = {
      fdf::MakeProperty(bind_fuchsia::PROTOCOL, bind_fuchsia_display::BIND_PROTOCOL_ENGINE),
  };
  const std::vector<fuchsia_driver_framework::Offer> node_offers = compat_server_.CreateOffers2();
  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> controller_client_result =
      AddChild(name(), node_properties, node_offers);
  if (controller_client_result.is_error()) {
    FDF_LOG(ERROR, "Failed to add child node: %s", controller_client_result.status_string());
    return controller_client_result.take_error();
  }
  controller_ = fidl::WireSyncClient(std::move(controller_client_result).value());

  return zx::ok();
}

}  // namespace framebuffer_display
