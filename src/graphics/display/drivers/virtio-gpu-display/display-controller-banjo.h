// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_VIRTIO_GPU_DISPLAY_DISPLAY_CONTROLLER_BANJO_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_VIRTIO_GPU_DISPLAY_DISPLAY_CONTROLLER_BANJO_H_

#include <fuchsia/hardware/display/controller/cpp/banjo.h>
#include <lib/stdcompat/span.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <cstdint>

#include "src/graphics/display/drivers/virtio-gpu-display/display-coordinator-events-banjo.h"
#include "src/graphics/display/drivers/virtio-gpu-display/display-engine.h"

namespace virtio_display {

// Banjo <-> C++ bridge for the methods interface with the Display Coordinator.
//
// Instances are thread-safe, because Banjo does not make any threading
// guarantees.
class DisplayControllerBanjo : public ddk::DisplayEngineProtocol<DisplayControllerBanjo> {
 public:
  // `engine` and `coordinator_events` must not be null, and must outlive the
  // newly created instance.
  explicit DisplayControllerBanjo(DisplayEngine* engine,
                                  DisplayCoordinatorEventsBanjo* coordinator_events);

  DisplayControllerBanjo(const DisplayControllerBanjo&) = delete;
  DisplayControllerBanjo& operator=(const DisplayControllerBanjo&) = delete;

  ~DisplayControllerBanjo();

  // ddk::DisplayEngineProtocol
  void DisplayEngineRegisterDisplayEngineListener(
      const display_engine_listener_protocol_t* display_engine_listener);
  void DisplayEngineDeregisterDisplayEngineListener();
  zx_status_t DisplayEngineImportBufferCollection(uint64_t banjo_driver_buffer_collection_id,
                                                  zx::channel buffer_collection_token);
  zx_status_t DisplayEngineReleaseBufferCollection(uint64_t banjo_driver_buffer_collection_id);
  zx_status_t DisplayEngineImportImage(const image_metadata_t* banjo_image_metadata,
                                       uint64_t banjo_driver_buffer_collection_id, uint32_t index,
                                       uint64_t* out_image_handle);
  zx_status_t DisplayEngineImportImageForCapture(uint64_t banjo_driver_buffer_collection_id,
                                                 uint32_t index, uint64_t* out_capture_handle);
  void DisplayEngineReleaseImage(uint64_t banjo_image_handle);
  config_check_result_t DisplayEngineCheckConfiguration(
      const display_config_t* banjo_display_configs, size_t banjo_display_configs_count,
      client_composition_opcode_t* out_client_composition_opcodes_list,
      size_t out_client_composition_opcodes_size, size_t* out_client_composition_opcodes_actual);
  void DisplayEngineApplyConfiguration(const display_config_t* banjo_display_configs,
                                       size_t banjo_display_configs_count,
                                       const config_stamp_t* banjo_config_stamp);
  zx_status_t DisplayEngineSetBufferCollectionConstraints(
      const image_buffer_usage_t* banjo_image_buffer_usage,
      uint64_t banjo_driver_buffer_collection_id);
  zx_status_t DisplayEngineSetDisplayPower(uint64_t banjo_display_id, bool power_on);
  bool DisplayEngineIsCaptureSupported();
  zx_status_t DisplayEngineStartCapture(uint64_t capture_handle);
  zx_status_t DisplayEngineReleaseCapture(uint64_t capture_handle);
  bool DisplayEngineIsCaptureCompleted();
  zx_status_t DisplayEngineSetMinimumRgb(uint8_t minimum_rgb);

  display_engine_protocol_t GetProtocol();

 private:
  // This data member is thread-safe because it is immutable.
  DisplayEngine& engine_;

  // This data member is thread-safe because it is immutable.
  DisplayCoordinatorEventsBanjo& coordinator_events_;
};

}  // namespace virtio_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_VIRTIO_GPU_DISPLAY_DISPLAY_CONTROLLER_BANJO_H_
