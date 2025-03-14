// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/paver/validation.h"

#include <lib/arch/zbi.h>
#include <lib/zbi-format/zbi.h>
#include <zircon/errors.h>

#include <memory>
#include <span>
#include <vector>

#include <fbl/array.h>
#include <zxtest/zxtest.h>

#include "src/storage/lib/paver/device-partitioner.h"
#include "src/storage/lib/paver/test/test-utils.h"

namespace paver {
namespace {

// Allocate header and data following it, and give it some basic defaults.
//
// If "span" is non-null, it will be initialized with a span covering
// the allocated data.
//
// If "result_header" is non-null, it will point to the beginning of the
// uint8_t. It must not outlive the returned fbl::Array object.
fbl::Array<uint8_t> CreateZbiHeader(Arch arch, size_t payload_size,
                                    arch::ZbiKernelImage** result_header,
                                    std::span<uint8_t>* span = nullptr) {
  // Allocate raw memory.
  const size_t data_size = sizeof(arch::ZbiKernelImage) + payload_size;
  auto data = fbl::Array<uint8_t>(new uint8_t[data_size], data_size);
  memset(data.get(), 0xee, data_size);

  // Set up header for outer ZBI header.
  auto header = reinterpret_cast<arch::ZbiKernelImage*>(data.get());
  header->hdr_file.type = ZBI_TYPE_CONTAINER;
  header->hdr_file.extra = ZBI_CONTAINER_MAGIC;
  header->hdr_file.magic = ZBI_ITEM_MAGIC;
  header->hdr_file.flags = ZBI_FLAGS_VERSION;
  header->hdr_file.crc32 = ZBI_ITEM_NO_CRC32;
  header->hdr_file.length =
      static_cast<uint32_t>(sizeof(zbi_header_t) + sizeof(zbi_kernel_t) + payload_size);

  // Set up header for inner ZBI header.
  header->hdr_kernel.type = (arch == Arch::kX64) ? ZBI_TYPE_KERNEL_X64 : ZBI_TYPE_KERNEL_ARM64;
  header->hdr_kernel.magic = ZBI_ITEM_MAGIC;
  header->hdr_kernel.flags = ZBI_FLAGS_VERSION;
  header->hdr_kernel.crc32 = ZBI_ITEM_NO_CRC32;
  header->hdr_kernel.length = static_cast<uint32_t>(sizeof(zbi_kernel_t) + payload_size);

  if (span != nullptr) {
    *span = std::span<uint8_t>(data.get(), data_size);
  }
  if (result_header != nullptr) {
    *result_header = reinterpret_cast<arch::ZbiKernelImage*>(data.get());
  }

  return data;
}

TEST(IsValidKernelZbi, EmptyData) {
  ASSERT_FALSE(IsValidKernelZbi(Arch::kX64, std::span<uint8_t>()));
}

TEST(IsValidKernelZbi, MinimalValid) {
  std::span<uint8_t> data;
  arch::ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 0, &header, &data);
  ASSERT_TRUE(IsValidKernelZbi(Arch::kX64, data));
}

TEST(IsValidKernelZbi, DataTooSmall) {
  std::span<uint8_t> data;
  arch::ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 1024, &header, &data);
  header->hdr_file.length += 1;
  ASSERT_FALSE(IsValidKernelZbi(Arch::kX64, data));
}

TEST(IsValidKernelZbi, DataTooBig) {
  std::span<uint8_t> data;
  arch::ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 1024, &header, &data);
  header->hdr_file.length = 0xffff'ffffu;
  ASSERT_FALSE(IsValidKernelZbi(Arch::kX64, data));
}

TEST(IsValidKernelZbi, KernelDataTooSmall) {
  std::span<uint8_t> data;
  arch::ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 1024, &header, &data);
  header->hdr_kernel.length += 1;
  ASSERT_FALSE(IsValidKernelZbi(Arch::kX64, data));
}

TEST(IsValidKernelZbi, ValidWithPayload) {
  std::span<uint8_t> data;
  arch::ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 1024, &header, &data);
  ASSERT_TRUE(IsValidKernelZbi(Arch::kX64, data));
}

TEST(IsValidKernelZbi, InvalidArch) {
  std::span<uint8_t> data;
  arch::ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 0, &header, &data);
  ASSERT_FALSE(IsValidKernelZbi(Arch::kArm64, data));
}

TEST(IsValidKernelZbi, InvalidMagic) {
  std::span<uint8_t> data;
  arch::ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 0, &header, &data);
  header->hdr_file.magic = 0;
  ASSERT_FALSE(IsValidKernelZbi(Arch::kX64, data));
}

TEST(IsValidKernelZbi, ValidCrc) {
  std::span<uint8_t> data;
  arch::ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 0, &header, &data);
  header->hdr_kernel.flags |= ZBI_FLAGS_CRC32;
  header->data_kernel.entry = 0x1122334455667788;
  header->data_kernel.reserve_memory_size = 0xaabbccdd12345678;
  header->hdr_kernel.crc32 = 0x8b8e6cfc;  // == crc32({header->data_kernel})
  ASSERT_TRUE(IsValidKernelZbi(Arch::kX64, data));
}

TEST(IsValidKernelZbi, InvalidCrc) {
  std::span<uint8_t> data;
  arch::ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 0, &header, &data);
  header->hdr_kernel.flags |= ZBI_FLAGS_CRC32;
  header->data_kernel.entry = 0x1122334455667788;
  header->data_kernel.reserve_memory_size = 0xaabbccdd12345678;
  header->hdr_kernel.crc32 = 0xffffffff;
  ASSERT_FALSE(IsValidKernelZbi(Arch::kX64, data));
}

static std::span<const uint8_t> StringToSpan(const std::string& data) {
  return std::span<const uint8_t>(reinterpret_cast<const uint8_t*>(data.data()), data.size());
}

TEST(IsValidChromeOsKernel, TooSmall) {
  EXPECT_FALSE(IsValidChromeOsKernel(StringToSpan("")));
  EXPECT_FALSE(IsValidChromeOsKernel(StringToSpan("C")));
  EXPECT_FALSE(IsValidChromeOsKernel(StringToSpan("CHROMEO")));
}

TEST(IsValidChromeOsKernel, IncorrectMagic) {
  EXPECT_FALSE(IsValidChromeOsKernel(StringToSpan("CHROMEOX")));
}

TEST(IsValidChromeOsKernel, MinimalValid) {
  EXPECT_TRUE(IsValidChromeOsKernel(StringToSpan("CHROMEOS")));
}

TEST(IsValidChromeOsKernel, ExcessData) {
  EXPECT_TRUE(IsValidChromeOsKernel(StringToSpan("CHROMEOS-1234")));
}

TEST(IsValidAndroidKernel, Validity) {
  EXPECT_TRUE(IsValidAndroidKernel(StringToSpan("ANDROID!")));
  EXPECT_FALSE(IsValidAndroidKernel(StringToSpan("VNDRBOOT")));
}

TEST(IsValidAndroidVendorKernel, Validity) {
  EXPECT_TRUE(IsValidAndroidVendorKernel(StringToSpan("VNDRBOOT")));
  EXPECT_FALSE(IsValidAndroidVendorKernel(StringToSpan("ANDROID!")));
}

}  // namespace
}  // namespace paver
