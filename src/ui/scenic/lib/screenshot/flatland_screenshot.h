// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_SCREENSHOT_FLATLAND_SCREENSHOT_H_
#define SRC_UI_SCENIC_LIB_SCREENSHOT_FLATLAND_SCREENSHOT_H_

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.compression.internal/cpp/fidl.h>
#include <fuchsia/images2/cpp/fidl.h>
#include <fuchsia/io/cpp/fidl.h>

#include <optional>
#include <string>

#include "src/lib/fxl/memory/weak_ptr.h"
#include "src/ui/scenic/lib/allocation/allocator.h"
#include "src/ui/scenic/lib/screen_capture/screen_capture.h"
#include "src/ui/scenic/lib/screenshot/util.h"

namespace screenshot {

namespace test {
class FlatlandScreenshotTest;
}  // namespace test

using allocation::Allocator;
using screen_capture::ScreenCapture;

class FlatlandScreenshot : public fidl::Server<fuchsia_ui_composition::Screenshot> {
 public:
  FlatlandScreenshot(std::unique_ptr<ScreenCapture> screen_capturer,
                     std::shared_ptr<Allocator> allocator, fuchsia::math::SizeU display_size,
                     int display_rotation,
                     fidl::Client<fuchsia_ui_compression_internal::ImageCompressor> client,
                     fit::function<void(FlatlandScreenshot*)> destroy_instance_function);
  ~FlatlandScreenshot() override = default;

  void AllocateBuffers();

  // |fuchsia_ui_composition::Screenshot|
  void Take(TakeRequest& request, TakeCompleter::Sync& completer) override;
  void Take(fuchsia_ui_composition::ScreenshotTakeRequest params,
            fit::function<void(fuchsia_ui_composition::ScreenshotTakeResponse)> callback);

  // |fuchsia_ui_composition::Screenshot|
  void TakeFile(TakeFileRequest& request, TakeFileCompleter::Sync& completer) override;
  void TakeFile(fuchsia_ui_composition::ScreenshotTakeFileRequest params,
                fit::function<void(fuchsia_ui_composition::ScreenshotTakeFileResponse)> callback);

 private:
  friend class test::FlatlandScreenshotTest;

  void FinishTake(zx::vmo response_vmo);
  void FinishTakeFile(zx::vmo response_vmo);
  zx::vmo HandleFrameRender();
  void GetNextFrame();

  std::unique_ptr<screen_capture::ScreenCapture> screen_capturer_;
  fuchsia::sysmem2::AllocatorPtr sysmem_allocator_;
  std::shared_ptr<Allocator> flatland_allocator_;

  fuchsia::math::SizeU display_size_;

  // Angle in degrees by which the display is rotated in the clockwise direction.
  int display_rotation_ = 0;

  // Preferred raw pixel format by Screenshot's client, updated if a new raw format is specified
  // in a subsequent Take() request.
  fuchsia_ui_composition::ScreenshotFormat raw_format_ =
      fuchsia_ui_composition::ScreenshotFormat::kBgraRaw;

  // Maps buffer collections where the display can be rendered into, based on preferred pixel
  // format.
  std::map<fuchsia_ui_composition::ScreenshotFormat, fuchsia::sysmem2::BufferCollectionInfo>
      buffer_collection_info_;

  fidl::Client<fuchsia_ui_compression_internal::ImageCompressor> client_;

  // Called when this instance should be destroyed.
  fit::function<void(FlatlandScreenshot*)> destroy_instance_function_;

  // The client-supplied callback to be fired after the screenshot occurs.
  fit::function<void(fuchsia_ui_composition::ScreenshotTakeResponse)> take_callback_ = nullptr;
  fit::function<void(fuchsia_ui_composition::ScreenshotTakeFileResponse)> take_file_callback_ =
      nullptr;

  zx::event render_event_;

  std::shared_ptr<async::WaitOnce> render_wait_;

  // Used to ensure that the first Take() call happens after the asynchronous sysmem buffer
  // allocation.
  zx::event init_event_;
  std::shared_ptr<async::WaitOnce> init_wait_;

  size_t served_screenshots_next_id_ = 0;
  std::unordered_map<size_t,
                     std::pair<std::unique_ptr<vfs::VmoFile>, std::unique_ptr<async::WaitOnce>>>
      served_screenshots_;

  size_t NumCurrentServedScreenshots() { return served_screenshots_.size(); }

  // Should be last.
  fxl::WeakPtrFactory<FlatlandScreenshot> weak_factory_;
};

}  // namespace screenshot

#endif  // SRC_UI_SCENIC_LIB_SCREENSHOT_FLATLAND_SCREENSHOT_H_
