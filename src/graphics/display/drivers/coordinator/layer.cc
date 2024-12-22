// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/layer.h"

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <fuchsia/hardware/display/controller/c/banjo.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <optional>
#include <utility>

#include <fbl/auto_lock.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/ref_ptr.h>

#include "src/graphics/display/drivers/coordinator/fence.h"
#include "src/graphics/display/drivers/coordinator/image.h"
#include "src/graphics/display/lib/api-types/cpp/config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer-id.h"
#include "src/graphics/display/lib/api-types/cpp/event-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/rectangle.h"

namespace fhdt = fuchsia_hardware_display_types;

namespace display_coordinator {

namespace {

// Removes and invokes EarlyRetire on all entries before end.
static void EarlyRetireUpTo(Image::DoublyLinkedList& list, Image::DoublyLinkedList::iterator end) {
  while (list.begin() != end) {
    fbl::RefPtr<Image> image = list.pop_front();
    image->EarlyRetire();
  }
}

}  // namespace

Layer::Layer(display::DriverLayerId id) {
  this->id = id;
  memset(&pending_layer_, 0, sizeof(layer_t));
  memset(&current_layer_, 0, sizeof(layer_t));
  config_change_ = false;
  pending_node_.layer = this;
  current_node_.layer = this;
  current_display_id_ = display::kInvalidDisplayId;
  is_skipped_ = false;
}

Layer::~Layer() {
  if (pending_image_) {
    pending_image_->DiscardAcquire();
  }
  EarlyRetireUpTo(waiting_images_, waiting_images_.end());
  if (displayed_image_) {
    fbl::AutoLock lock(displayed_image_->mtx());
    displayed_image_->StartRetire();
  }
}

bool Layer::ResolvePendingLayerProperties() {
  // If the layer's image configuration changed, get rid of any current images
  if (pending_image_config_gen_ != current_image_config_gen_) {
    current_image_config_gen_ = pending_image_config_gen_;

    if (pending_image_ == nullptr) {
      FDF_LOG(ERROR, "Tried to apply configuration with missing image");
      return false;
    }

    EarlyRetireUpTo(waiting_images_, waiting_images_.end());
    if (displayed_image_ != nullptr) {
      {
        fbl::AutoLock lock(displayed_image_->mtx());
        displayed_image_->StartRetire();
      }
      displayed_image_ = nullptr;
    }
  }
  return true;
}

bool Layer::ResolvePendingImage(FenceCollection* fences, display::ConfigStamp stamp) {
  if (pending_image_) {
    auto wait_fence = fences->GetFence(pending_wait_event_id_);
    if (wait_fence && wait_fence->InContainer()) {
      FDF_LOG(ERROR, "Tried to wait with a busy event");
      return false;
    }
    pending_image_->PrepareFences(std::move(wait_fence));
    {
      fbl::AutoLock lock(pending_image_->mtx());
      waiting_images_.push_back(std::move(pending_image_));
    }
  }

  if (!waiting_images_.is_empty()) {
    waiting_images_.back().set_latest_client_config_stamp(stamp);
  }
  return true;
}

void Layer::ApplyChanges(const display_mode_t& mode) {
  if (!config_change_) {
    return;
  }

  current_layer_ = pending_layer_;
  config_change_ = false;

  if (displayed_image_) {
    current_layer_.image_handle = ToBanjoDriverImageId(displayed_image_->driver_id());
  } else {
    current_layer_.image_handle = INVALID_DISPLAY_ID;
  }
}

void Layer::DiscardChanges() {
  pending_image_config_gen_ = current_image_config_gen_;
  if (pending_image_) {
    pending_image_->DiscardAcquire();
    pending_image_ = nullptr;
  }
  if (config_change_) {
    pending_layer_ = current_layer_;
    config_change_ = false;
  }
}

bool Layer::CleanUpAllImages() {
  RetirePendingImage();

  // Retire all waiting images.
  EarlyRetireUpTo(waiting_images_, waiting_images_.end());

  return RetireDisplayedImage();
}

bool Layer::CleanUpImage(const Image& image) {
  if (pending_image_.get() == &image) {
    RetirePendingImage();
  }

  RetireWaitingImage(image);

  if (displayed_image_.get() == &image) {
    return RetireDisplayedImage();
  }
  return false;
}

std::optional<display::ConfigStamp> Layer::GetCurrentClientConfigStamp() const {
  if (displayed_image_ != nullptr) {
    return displayed_image_->latest_client_config_stamp();
  }
  return std::nullopt;
}

bool Layer::ActivateLatestReadyImage() {
  if (waiting_images_.is_empty()) {
    return false;
  }

  // Find the most recent (i.e. the most behind) waiting image that is ready.
  auto it = waiting_images_.end();
  bool found_ready_image = false;
  do {
    --it;
    if (it->IsReady()) {
      found_ready_image = true;
      break;
    }
  } while (it != waiting_images_.begin());

  if (!found_ready_image) {
    return false;
  }

  // Retire the last active image
  if (displayed_image_ != nullptr) {
    fbl::AutoLock lock(displayed_image_->mtx());
    displayed_image_->StartRetire();
  }

  // Retire the waiting images that were never presented.
  EarlyRetireUpTo(waiting_images_, /*end=*/it);
  displayed_image_ = waiting_images_.pop_front();

  current_layer_.image_handle = ToBanjoDriverImageId(displayed_image_->driver_id());
  return true;
}

bool Layer::AppendToConfig(fbl::DoublyLinkedList<LayerNode*>* list) {
  if (pending_node_.InContainer()) {
    return false;
  }

  list->push_front(&pending_node_);
  return true;
}

void Layer::SetPrimaryConfig(fhdt::wire::ImageMetadata image_metadata) {
  pending_layer_.image_handle = INVALID_DISPLAY_ID;
  pending_layer_.image_metadata = display::ImageMetadata(image_metadata).ToBanjo();
  const rect_u_t image_area = {.x = 0,
                               .y = 0,
                               .width = image_metadata.dimensions.width,
                               .height = image_metadata.dimensions.height};
  pending_layer_.fallback_color = {
      .format = static_cast<uint32_t>(fuchsia_images2::wire::PixelFormat::kR8G8B8A8),
      .bytes = {0, 0, 0, 0, 0, 0, 0, 0}};
  pending_layer_.image_source = image_area;
  pending_layer_.display_destination = image_area;
  pending_image_config_gen_++;
  pending_image_ = nullptr;
  config_change_ = true;
}

void Layer::SetPrimaryPosition(fhdt::wire::CoordinateTransformation image_source_transformation,
                               fuchsia_math::wire::RectU image_source,
                               fuchsia_math::wire::RectU display_destination) {
  pending_layer_.image_source = display::Rectangle::From(image_source).ToBanjo();
  pending_layer_.display_destination = display::Rectangle::From(display_destination).ToBanjo();
  pending_layer_.image_source_transformation = static_cast<uint8_t>(image_source_transformation);

  config_change_ = true;
}

void Layer::SetPrimaryAlpha(fhdt::wire::AlphaMode mode, float val) {
  static_assert(static_cast<alpha_t>(fhdt::wire::AlphaMode::kDisable) == ALPHA_DISABLE,
                "Bad constant");
  static_assert(static_cast<alpha_t>(fhdt::wire::AlphaMode::kPremultiplied) == ALPHA_PREMULTIPLIED,
                "Bad constant");
  static_assert(static_cast<alpha_t>(fhdt::wire::AlphaMode::kHwMultiply) == ALPHA_HW_MULTIPLY,
                "Bad constant");

  pending_layer_.alpha_mode = static_cast<alpha_t>(mode);
  pending_layer_.alpha_layer_val = val;

  config_change_ = true;
}

void Layer::SetColorConfig(fuchsia_hardware_display_types::wire::Color color) {
  // Increase the size of the static array when large color formats are introduced
  static_assert(decltype(color.bytes)::size() == sizeof(pending_layer_.fallback_color.bytes));

  ZX_DEBUG_ASSERT(!color.format.IsUnknown());
  pending_layer_.fallback_color.format =
      static_cast<fuchsia_images2_pixel_format_enum_value_t>(color.format);
  std::ranges::copy(color.bytes, pending_layer_.fallback_color.bytes);

  pending_layer_.image_metadata = {.dimensions = {.width = 0, .height = 0},
                                   .tiling_type = IMAGE_TILING_TYPE_LINEAR};
  pending_layer_.image_source = {.x = 0, .y = 0, .width = 0, .height = 0};
  pending_layer_.display_destination = {.x = 0, .y = 0, .width = 0, .height = 0};

  pending_image_ = nullptr;
  config_change_ = true;
}

void Layer::SetImage(fbl::RefPtr<Image> image, display::EventId wait_event_id) {
  if (pending_image_) {
    pending_image_->DiscardAcquire();
  }

  pending_image_ = std::move(image);
  pending_wait_event_id_ = wait_event_id;
}

void Layer::RetirePendingImage() {
  if (pending_image_) {
    pending_image_->DiscardAcquire();
    pending_image_ = nullptr;
  }
}

void Layer::RetireWaitingImage(const Image& image) {
  auto it = waiting_images_.find_if([&image](const Image& node) { return &node == &image; });
  if (it != waiting_images_.end()) {
    fbl::RefPtr<Image> to_retire = waiting_images_.erase(it);
    ZX_DEBUG_ASSERT(to_retire);
    to_retire->EarlyRetire();
  }
}

bool Layer::RetireDisplayedImage() {
  if (!displayed_image_) {
    return false;
  }

  {
    fbl::AutoLock lock(displayed_image_->mtx());
    displayed_image_->StartRetire();
  }
  displayed_image_ = nullptr;

  return current_node_.InContainer();
}

}  // namespace display_coordinator
