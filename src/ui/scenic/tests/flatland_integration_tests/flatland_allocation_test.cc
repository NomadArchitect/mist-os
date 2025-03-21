// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/sysmem/cpp/fidl.h>
#include <fuchsia/ui/composition/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>

#include <cstdint>
#include <memory>

#include <zxtest/zxtest.h>

#include "src/ui/scenic/lib/allocation/buffer_collection_import_export_tokens.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/tests/utils/blocking_present.h"
#include "src/ui/scenic/tests/utils/scenic_ctf_test_base.h"

namespace integration_tests {

using fuchsia::ui::composition::Allocator;
using fuchsia::ui::composition::Allocator_RegisterBufferCollection_Result;
using fuchsia::ui::composition::ContentId;
using fuchsia::ui::composition::Flatland;
using fuchsia::ui::composition::ImageProperties;
using fuchsia::ui::composition::RegisterBufferCollectionArgs;
using fuchsia::ui::composition::TransformId;

constexpr auto kDefaultSize = 128;
constexpr TransformId kRootTransform{.value = 1};

fuchsia::sysmem2::BufferCollectionConstraints GetDefaultBufferConstraints() {
  fuchsia::sysmem2::BufferCollectionConstraints constraints;
  auto& bmc = *constraints.mutable_buffer_memory_constraints();
  bmc.set_ram_domain_supported(true);
  bmc.set_cpu_domain_supported(true);
  constraints.mutable_usage()->set_cpu(fuchsia::sysmem2::CPU_USAGE_READ);
  constraints.set_min_buffer_count(1);
  auto& image_constraints = constraints.mutable_image_format_constraints()->emplace_back();
  image_constraints.set_pixel_format(fuchsia::images2::PixelFormat::B8G8R8A8);
  image_constraints.set_pixel_format_modifier(fuchsia::images2::PixelFormatModifier::LINEAR);
  image_constraints.mutable_color_spaces()->emplace_back(fuchsia::images2::ColorSpace::SRGB);
  image_constraints.set_required_min_size(
      fuchsia::math::SizeU{.width = kDefaultSize, .height = kDefaultSize});
  image_constraints.set_required_max_size(
      fuchsia::math::SizeU{.width = kDefaultSize, .height = kDefaultSize});
  return constraints;
}

// Test fixture that sets up an environment with a Scenic we can connect to.
class AllocationTest : public ScenicCtfTest {
 public:
  void SetUp() override {
    ScenicCtfTest::SetUp();

    auto context = sys::ComponentContext::Create();
    context->svc()->Connect(sysmem_allocator_.NewRequest());

    // Create a flatland display so render and cleanup loops happen.
    flatland_display_ = ConnectSyncIntoRealm<fuchsia::ui::composition::FlatlandDisplay>();

    // Create a root Flatland.
    root_flatland_ = ConnectAsyncIntoRealm<Flatland>();
    root_flatland_.set_error_handler([](zx_status_t status) {
      FX_LOGS(ERROR) << "Lost connection to Scenic: " << zx_status_get_string(status);
      FAIL();
    });

    // Attach |root_flatland_| as the only Flatland under |flatland_display_|.
    auto [child_token, parent_token] = scenic::ViewCreationTokenPair::New();
    fidl::InterfacePtr<fuchsia::ui::composition::ChildViewWatcher> child_view_watcher;
    flatland_display_->SetContent(std::move(parent_token), child_view_watcher.NewRequest());
    fidl::InterfacePtr<fuchsia::ui::composition::ParentViewportWatcher> parent_viewport_watcher;
    root_flatland_->CreateView2(std::move(child_token), scenic::NewViewIdentityOnCreation(), {},
                                parent_viewport_watcher.NewRequest());
    root_flatland_->CreateTransform(kRootTransform);
    root_flatland_->SetRootTransform(kRootTransform);
  }

  void TearDown() override {
    root_flatland_.Unbind();
    flatland_display_.Unbind();

    ScenicCtfTest::TearDown();
  }

 protected:
  fuchsia::sysmem2::BufferCollectionInfo SetConstraintsAndAllocateBuffer(
      fuchsia::sysmem2::BufferCollectionTokenSyncPtr token,
      fuchsia::sysmem2::BufferCollectionConstraints constraints) {
    fuchsia::sysmem2::BufferCollectionSyncPtr buffer_collection;
    fuchsia::sysmem2::AllocatorBindSharedCollectionRequest bind_shared_request;
    bind_shared_request.set_token(std::move(token));
    bind_shared_request.set_buffer_collection_request(buffer_collection.NewRequest());
    auto status = sysmem_allocator_->BindSharedCollection(std::move(bind_shared_request));
    FX_CHECK(status == ZX_OK);

    uint32_t constraints_min_buffer_count = constraints.min_buffer_count();

    fuchsia::sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
    set_constraints_request.set_constraints(std::move(constraints));
    status = buffer_collection->SetConstraints(std::move(set_constraints_request));
    FX_CHECK(status == ZX_OK);
    zx_status_t allocation_status = ZX_OK;

    fuchsia::sysmem2::BufferCollection_WaitForAllBuffersAllocated_Result wait_result;
    status = buffer_collection->WaitForAllBuffersAllocated(&wait_result);
    FX_CHECK(status == ZX_OK);
    FX_CHECK(!wait_result.is_framework_err());
    FX_CHECK(!wait_result.is_err());
    FX_CHECK(wait_result.is_response());
    auto buffer_collection_info =
        std::move(*wait_result.response().mutable_buffer_collection_info());
    EXPECT_EQ(constraints_min_buffer_count, buffer_collection_info.buffers().size());
    FX_CHECK(buffer_collection->Release() == ZX_OK);
    return buffer_collection_info;
  }

  fuchsia::sysmem2::AllocatorSyncPtr sysmem_allocator_;
  fuchsia::ui::composition::FlatlandPtr root_flatland_;

 private:
  fuchsia::ui::composition::FlatlandDisplaySyncPtr flatland_display_;
};

TEST_F(AllocationTest, CreateAndReleaseImage) {
  auto flatland_allocator = ConnectSyncIntoRealm<Allocator>();

  auto [local_token, scenic_token] = utils::CreateSysmemTokens(sysmem_allocator_.get());

  // Send one token to Flatland Allocator.
  allocation::BufferCollectionImportExportTokens bc_tokens =
      allocation::BufferCollectionImportExportTokens::New();
  RegisterBufferCollectionArgs rbc_args = {};
  rbc_args.set_export_token(std::move(bc_tokens.export_token));
  rbc_args.set_buffer_collection_token2(std::move(scenic_token));
  Allocator_RegisterBufferCollection_Result result;
  flatland_allocator->RegisterBufferCollection(std::move(rbc_args), &result);
  ASSERT_FALSE(result.is_err());

  // Use the local token to set constraints.
  auto info =
      SetConstraintsAndAllocateBuffer(std::move(local_token), GetDefaultBufferConstraints());

  ImageProperties image_properties = {};
  image_properties.set_size({.width = kDefaultSize, .height = kDefaultSize});
  const ContentId kImageContentId{.value = 1};

  root_flatland_->CreateImage(kImageContentId, std::move(bc_tokens.import_token), 0,
                              std::move(image_properties));
  root_flatland_->SetContent(kRootTransform, kImageContentId);
  BlockingPresent(this, root_flatland_);

  // Release image and remove content to actually deallocate.
  root_flatland_->ReleaseImage(kImageContentId);
  root_flatland_->SetContent(kRootTransform, {0});
  BlockingPresent(this, root_flatland_);
}

TEST_F(AllocationTest, CreateAndReleaseMultipleImages) {
  const auto kImageCount = 3;
  auto flatland_allocator = ConnectSyncIntoRealm<Allocator>();

  for (uint64_t i = 1; i <= kImageCount; ++i) {
    auto [local_token, scenic_token] = utils::CreateSysmemTokens(sysmem_allocator_.get());

    // Send one token to root_flatland_ Allocator.
    allocation::BufferCollectionImportExportTokens bc_tokens =
        allocation::BufferCollectionImportExportTokens::New();
    RegisterBufferCollectionArgs rbc_args = {};
    rbc_args.set_export_token(std::move(bc_tokens.export_token));
    rbc_args.set_buffer_collection_token2(std::move(scenic_token));
    Allocator_RegisterBufferCollection_Result result;
    flatland_allocator->RegisterBufferCollection(std::move(rbc_args), &result);
    ASSERT_FALSE(result.is_err());

    // Use the local token to set constraints.
    auto info =
        SetConstraintsAndAllocateBuffer(std::move(local_token), GetDefaultBufferConstraints());

    ImageProperties image_properties = {};
    image_properties.set_size({.width = kDefaultSize, .height = kDefaultSize});
    const ContentId kImageContentId{.value = i};
    root_flatland_->CreateImage(kImageContentId, std::move(bc_tokens.import_token), 0,
                                std::move(image_properties));
    const TransformId kImageTransformId{.value = i + 1};
    root_flatland_->CreateTransform(kImageTransformId);
    root_flatland_->SetContent(kImageTransformId, kImageContentId);
    root_flatland_->AddChild(kRootTransform, kImageTransformId);
  }
  BlockingPresent(this, root_flatland_);

  for (uint64_t i = 1; i <= kImageCount; ++i) {
    // Release image and remove content to actually deallocate.
    const ContentId kImageContentId{.value = i};
    root_flatland_->ReleaseImage(kImageContentId);
    const TransformId kImageTransformId{.value = i + 1};
    root_flatland_->RemoveChild(kRootTransform, kImageTransformId);
    root_flatland_->ReleaseTransform(kImageTransformId);
  }
  BlockingPresent(this, root_flatland_);
}

TEST_F(AllocationTest, MultipleClientsCreateAndReleaseImages) {
  const auto kClientCount = 16;

  // Add Viewports for as many as kClientCount.
  std::vector<fuchsia::ui::views::ViewCreationToken> view_creation_tokens;
  for (uint64_t i = 1; i <= kClientCount; ++i) {
    auto [child_token, parent_token] = scenic::ViewCreationTokenPair::New();
    view_creation_tokens.emplace_back(std::move(child_token));
    fidl::InterfacePtr<fuchsia::ui::composition::ChildViewWatcher> child_view_watcher;
    fuchsia::ui::composition::ViewportProperties properties;
    properties.set_logical_size({.width = kDefaultSize, .height = kDefaultSize});
    const ContentId kViewportContentId{.value = i};
    root_flatland_->CreateViewport(kViewportContentId, std::move(parent_token),
                                   std::move(properties), child_view_watcher.NewRequest());
    const TransformId kViewportTransformId{.value = i + 1};
    root_flatland_->CreateTransform(kViewportTransformId);
    root_flatland_->AddChild(kRootTransform, kViewportTransformId);
  }
  BlockingPresent(this, root_flatland_);

  std::vector<std::shared_ptr<async::Loop>> loops;
  for (uint64_t i = 0; i < kClientCount; ++i) {
    auto loop = std::make_shared<async::Loop>(&kAsyncLoopConfigNeverAttachToThread);
    loops.push_back(loop);
    auto status = loop->StartThread();
    EXPECT_EQ(status, ZX_OK);
    status = async::PostTask(loop->dispatcher(), [this, i, loop, &view_creation_tokens]() mutable {
      LoggingEventLoop present_loop;
      auto flatland_allocator = ConnectSyncIntoRealm<Allocator>();

      auto [local_token, scenic_token] = utils::CreateSysmemTokens(sysmem_allocator_.get());

      // Send one token to Flatland Allocator.
      allocation::BufferCollectionImportExportTokens bc_tokens =
          allocation::BufferCollectionImportExportTokens::New();
      RegisterBufferCollectionArgs rbc_args = {};
      rbc_args.set_export_token(std::move(bc_tokens.export_token));
      rbc_args.set_buffer_collection_token2(std::move(scenic_token));
      Allocator_RegisterBufferCollection_Result result;
      flatland_allocator->RegisterBufferCollection(std::move(rbc_args), &result);
      ASSERT_FALSE(result.is_err());

      // Use the local token to set constraints.
      auto info =
          SetConstraintsAndAllocateBuffer(std::move(local_token), GetDefaultBufferConstraints());

      auto flatland = ConnectAsyncIntoRealm<Flatland>();
      flatland.set_error_handler([](zx_status_t status) {
        FX_LOGS(ERROR) << "Lost connection to Scenic: " << zx_status_get_string(status);
        FAIL();
      });
      fidl::InterfacePtr<fuchsia::ui::composition::ParentViewportWatcher> parent_viewport_watcher;
      flatland->CreateView(std::move(view_creation_tokens[i]),
                           parent_viewport_watcher.NewRequest());
      flatland->CreateTransform(kRootTransform);
      flatland->SetRootTransform(kRootTransform);

      ImageProperties image_properties;
      image_properties.set_size({.width = kDefaultSize, .height = kDefaultSize});
      const ContentId kImageContentId{.value = 1};
      flatland->CreateImage(kImageContentId, std::move(bc_tokens.import_token), 0,
                            std::move(image_properties));
      // Make each overlapping child slightly smaller, so all Images are visible.
      const auto size = kDefaultSize - static_cast<uint32_t>(i);
      flatland->SetImageDestinationSize(kImageContentId, {.width = size, .height = size});
      flatland->SetContent(kRootTransform, kImageContentId);
      BlockingPresent(&present_loop, flatland);

      // Release image and remove content to actually deallocate.
      flatland->ReleaseImage(kImageContentId);
      flatland->SetContent(kRootTransform, {0});
      BlockingPresent(&present_loop, flatland);

      flatland.Unbind();
      loop->Quit();
    });
    EXPECT_EQ(status, ZX_OK);
  }
  for (uint64_t i = 0; i < kClientCount; ++i) {
    loops[i]->JoinThreads();
  }
}

}  // namespace integration_tests