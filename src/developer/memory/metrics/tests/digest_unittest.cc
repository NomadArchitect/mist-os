// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/metrics/digest.h"

#include <gtest/gtest.h>

#include "src/developer/memory/metrics/tests/test_utils.h"

namespace memory {
namespace test {

using DigestUnitTest = testing::Test;

struct ExpectedBucket {
  std::string name;
  uint64_t size;
};

void ConfirmBuckets(const Digest& digest, const std::vector<ExpectedBucket>& expected_buckets) {
  std::vector<Bucket> buckets_copy = digest.buckets();
  for (const auto& expected_bucket : expected_buckets) {
    bool found = false;
    for (size_t j = 0; j < buckets_copy.size(); ++j) {
      const auto& bucket = buckets_copy.at(j);

      if (expected_bucket.name == bucket.name()) {
        EXPECT_EQ(expected_bucket.size, bucket.size())
            << "Bucket name='" << expected_bucket.name << "' has an unexpected value";
        buckets_copy.erase(buckets_copy.begin() + j);
        found = true;
        break;
      }
    }
    EXPECT_TRUE(found) << "Bucket name='" << expected_bucket.name << "' is missing";
  }
  for (const auto& unmatched_bucket : buckets_copy) {
    EXPECT_TRUE(false) << "Bucket name='" << unmatched_bucket.name() << "' is unexpected";
  }
}

TEST_F(DigestUnitTest, VMONames) {
  Capture c;
  TestUtils::CreateCapture(&c, {
                                   .vmos =
                                       {
                                           {.koid = 1,
                                            .name = "a1",
                                            .committed_bytes = 100,
                                            .committed_fractional_scaled_bytes = UINT64_MAX},
                                           {.koid = 2,
                                            .name = "b1",
                                            .committed_bytes = 200,
                                            .committed_fractional_scaled_bytes = UINT64_MAX},
                                       },
                                   .processes =
                                       {
                                           {.koid = 1, .name = "p1", .vmos = {1}},
                                           {.koid = 2, .name = "q1", .vmos = {2}},
                                       },
                               });

  Digester digester({{"A", "", "a.*"}, {"B", ".*", "b.*"}});
  Digest d(c, &digester);
  ConfirmBuckets(d, {{"B", 200U}, {"A", 100U}});
  EXPECT_EQ(0U, d.undigested_vmos().size());
}  // namespace test

TEST_F(DigestUnitTest, ProcessNames) {
  Capture c;
  TestUtils::CreateCapture(&c, {
                                   .vmos =
                                       {
                                           {.koid = 1,
                                            .name = "a1",
                                            .committed_bytes = 100,
                                            .committed_fractional_scaled_bytes = UINT64_MAX},
                                           {.koid = 2,
                                            .name = "b1",
                                            .committed_bytes = 200,
                                            .committed_fractional_scaled_bytes = UINT64_MAX},
                                       },
                                   .processes =
                                       {
                                           {.koid = 1, .name = "p1", .vmos = {1}},
                                           {.koid = 2, .name = "q1", .vmos = {2}},
                                       },
                               });

  Digester digester({{"P", "p.*", ""}, {"Q", "q.*", ".*"}});
  Digest d(c, &digester);
  ConfirmBuckets(d, {{"Q", 200U}, {"P", 100U}});
  EXPECT_EQ(0U, d.undigested_vmos().size());
}

TEST_F(DigestUnitTest, Undigested) {
  Capture c;
  TestUtils::CreateCapture(&c, {
                                   .vmos =
                                       {
                                           {.koid = 1,
                                            .name = "a1",
                                            .committed_bytes = 100,
                                            .committed_fractional_scaled_bytes = UINT64_MAX},
                                           {.koid = 2,
                                            .name = "b1",
                                            .committed_bytes = 200,
                                            .committed_fractional_scaled_bytes = UINT64_MAX},
                                       },
                                   .processes =
                                       {
                                           {.koid = 1, .name = "p1", .vmos = {1}},
                                           {.koid = 2, .name = "q1", .vmos = {2}},
                                       },
                               });

  Digester digester({{"A", ".*", "a.*"}});
  Digest d(c, &digester);
  ASSERT_EQ(1U, d.undigested_vmos().size());
  ASSERT_NE(d.undigested_vmos().end(), d.undigested_vmos().find(2U));
  ConfirmBuckets(d, {{"A", 100U}, {"Undigested", 200U}});
}  // namespace test

TEST_F(DigestUnitTest, Kernel) {
  // Test kernel stats.
  Capture c;
  TestUtils::CreateCapture(&c, {
                                   .kmem =
                                       {
                                           .total_bytes = 1000,
                                           .free_bytes = 100,
                                           .wired_bytes = 10,
                                           .total_heap_bytes = 20,
                                           .mmu_overhead_bytes = 30,
                                           .ipc_bytes = 40,
                                           .other_bytes = 50,
                                       },
                               });
  Digester digester({});
  Digest d(c, &digester);
  EXPECT_EQ(0U, d.undigested_vmos().size());
  ConfirmBuckets(d, {{"Kernel", 150U}, {"Free", 100U}});
}

TEST_F(DigestUnitTest, Orphaned) {
  // Test kernel stats.
  Capture c;
  TestUtils::CreateCapture(&c, {
                                   .kmem =
                                       {
                                           .total_bytes = 1000,
                                           .vmo_bytes = 300,
                                       },
                                   .vmos =
                                       {
                                           {.koid = 1,
                                            .name = "a1",
                                            .committed_bytes = 100,
                                            .committed_fractional_scaled_bytes = UINT64_MAX},
                                       },
                                   .processes =
                                       {
                                           {.koid = 1, .name = "p1", .vmos = {1}},
                                       },
                               });
  Digester digester({{"A", ".*", "a.*"}});
  Digest d(c, &digester);
  EXPECT_EQ(0U, d.undigested_vmos().size());
  ConfirmBuckets(d, {{.name = "A", .size = 100U},
                     {.name = "Orphaned", .size = 200U},
                     {.name = "Kernel", .size = 0U},
                     {.name = "Free", .size = 0U}});
}

TEST_F(DigestUnitTest, DefaultBuckets) {
  // Test kernel stats.
  Capture c;
  TestUtils::CreateCapture(
      &c, {.vmos =
               {
                   {.koid = 1,
                    .name = "uncompressed-bootfs",
                    .committed_bytes = 1,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 2,
                    .name = "magma_create_buffer",
                    .committed_bytes = 2,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 3,
                    .name = "SysmemAmlogicProtectedPool",
                    .committed_bytes = 3,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 4,
                    .name = "SysmemContiguousPool",
                    .committed_bytes = 4,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 5,
                    .name = "test",
                    .committed_bytes = 5,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 6,
                    .name = "test",
                    .committed_bytes = 6,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 7,
                    .name = "test",
                    .committed_bytes = 7,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 8,
                    .name = "dart",
                    .committed_bytes = 8,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 9,
                    .name = "test",
                    .committed_bytes = 9,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 10,
                    .name = "test",
                    .committed_bytes = 10,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 11,
                    .name = "test",
                    .committed_bytes = 11,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 12,
                    .name = "test",
                    .committed_bytes = 12,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 13,
                    .name = "test",
                    .committed_bytes = 13,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 14,
                    .name = "test",
                    .committed_bytes = 14,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 15,
                    .name = "test",
                    .committed_bytes = 15,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 16,
                    .name = "test",
                    .committed_bytes = 16,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 17,
                    .name = "test",
                    .committed_bytes = 17,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 18,
                    .name = "test",
                    .committed_bytes = 18,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 19,
                    .name = "test",
                    .committed_bytes = 19,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 20,
                    .name = "test",
                    .committed_bytes = 20,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 21,
                    .name = "test",
                    .committed_bytes = 21,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 22,
                    .name = "test",
                    .committed_bytes = 22,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 23,
                    .name = "inactive-blob-123",
                    .committed_bytes = 23,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 24,
                    .name = "blob-abc",
                    .committed_bytes = 24,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 25,
                    .name = "Mali JIT memory",
                    .committed_bytes = 25,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 26,
                    .name = "MagmaProtectedSysmem",
                    .committed_bytes = 26,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 27,
                    .name = "ImagePipe2Surface:0",
                    .committed_bytes = 27,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 28,
                    .name = "GFXBufferCollection:1",
                    .committed_bytes = 28,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 29,
                    .name = "ScenicImageMemory",
                    .committed_bytes = 29,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 30,
                    .name = "Display:0",
                    .committed_bytes = 30,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 31,
                    .name = "Display-Protected:0",
                    .committed_bytes = 31,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 32,
                    .name = "CompactImage:0",
                    .committed_bytes = 32,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
                   {.koid = 33,
                    .name = "GFX Device Memory CPU Uncached",
                    .committed_bytes = 33,
                    .committed_fractional_scaled_bytes = UINT64_MAX},
               },
           .processes = {
               {.koid = 1, .name = "bin/bootsvc", .vmos = {1}},
               {.koid = 2, .name = "test", .vmos = {2, 25, 26}},
               {.koid = 3, .name = "driver_host", .vmos = {3, 4}},
               {.koid = 4, .name = "fshost.cm", .vmos = {5}},
               {.koid = 5, .name = "/boot/bin/minfs", .vmos = {6}},
               {.koid = 6, .name = "/boot/bin/blobfs", .vmos = {7, 23, 24}},
               {.koid = 7, .name = "io.flutter.product_runner.aot", .vmos = {8, 9, 28, 29}},
               {.koid = 10, .name = "kronk.cm", .vmos = {10}},
               {.koid = 8, .name = "web_engine_exe:renderer", .vmos = {11}},
               {.koid = 9, .name = "web_engine_exe:gpu", .vmos = {12, 27, 32, 33}},
               {.koid = 11, .name = "scenic.cm", .vmos = {13, 27, 28, 29, 30, 31}},
               {.koid = 12, .name = "driver_host", .vmos = {14}},
               {.koid = 13, .name = "netstack.cm", .vmos = {15}},
               {.koid = 14, .name = "pkgfs", .vmos = {16}},
               {.koid = 15, .name = "cast_agent.cm", .vmos = {17}},
               {.koid = 16, .name = "archivist.cm", .vmos = {18}},
               {.koid = 17, .name = "cobalt.cm", .vmos = {19}},
               {.koid = 18, .name = "audio_core.cm", .vmos = {20}},
               {.koid = 19, .name = "context_provider.cm", .vmos = {21}},
               {.koid = 20, .name = "new", .vmos = {22}},
           }});

  const std::vector<BucketMatch> bucket_matches = {
      {"ZBI Buffer", ".*", "uncompressed-bootfs"},
      // Memory used with the GPU or display hardware.
      {"Graphics", ".*",
       "magma_create_buffer|Mali "
       ".*|Magma.*|ImagePipe2Surface.*|GFXBufferCollection.*|ScenicImageMemory|Display.*|"
       "CompactImage.*|GFX Device Memory.*"},
      // Unused protected pool memory.
      {"ProtectedPool", "driver_host", "SysmemAmlogicProtectedPool"},
      // Unused contiguous pool memory.
      {"ContiguousPool", "driver_host", "SysmemContiguousPool"},
      {"Fshost", "fshost.cm", ".*"},
      {"Minfs", ".*minfs", ".*"},
      {"BlobfsInactive", ".*blobfs", "inactive-blob-.*"},
      {"Blobfs", ".*blobfs", ".*"},
      {"FlutterApps", "io\\.flutter\\..*", "dart.*"},
      {"Flutter", "io\\.flutter\\..*", ".*"},
      {"Web", "web_engine_exe:.*", ".*"},
      {"Kronk", "kronk.cm", ".*"},
      {"Scenic", "scenic.cm", ".*"},
      {"Amlogic", "driver_host", ".*"},
      {"Netstack", "netstack.cm", ".*"},
      {"Pkgfs", "pkgfs", ".*"},
      {"Cast", "cast_agent.cm", ".*"},
      {"Archivist", "archivist.cm", ".*"},
      {"Cobalt", "cobalt.cm", ".*"},
      {"Audio", "audio_core.cm", ".*"},
      {"Context", "context_provider.cm", ".*"},
  };

  Digester digester(bucket_matches);
  Digest d(c, &digester);
  EXPECT_EQ(1U, d.undigested_vmos().size());

  ConfirmBuckets(d, {
                        {"Web", 23U},
                        {"Context", 21U},
                        {"Audio", 20U},
                        {"Cobalt", 19U},
                        {"Archivist", 18U},
                        {"Cast", 17U},
                        {"Pkgfs", 16U},
                        {"Netstack", 15U},
                        {"Amlogic", 14U},
                        {"Scenic", 13U},
                        {"Kronk", 10U},
                        {"Flutter", 9U},
                        {"FlutterApps", 8U},
                        {"Blobfs", 31U},
                        {"Minfs", 6U},
                        {"Fshost", 5U},
                        {"ContiguousPool", 4U},
                        {"ProtectedPool", 3U},
                        {"Graphics", 2U + 25U + 26U + 27U + 28U + 29U + 30U + 31U + 32U + 33U},
                        {"ZBI Buffer", 1U},
                        {"BlobfsInactive", 23U},
                        {"Undigested", 22U},
                    });
}

}  // namespace test
}  // namespace memory
