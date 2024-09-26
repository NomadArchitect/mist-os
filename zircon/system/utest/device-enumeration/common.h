// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_UTEST_DEVICE_ENUMERATION_COMMON_H_
#define ZIRCON_SYSTEM_UTEST_DEVICE_ENUMERATION_COMMON_H_

#include <fidl/fuchsia.driver.development/cpp/fidl.h>

#include <unordered_map>

#include <zxtest/zxtest.h>

#include "src/lib/fsl/io/device_watcher.h"

namespace device_enumeration {

void WaitForClassDeviceCount(const std::string& path_in_devfs, size_t count);

}  // namespace device_enumeration

class DeviceEnumerationTest : public zxtest::Test {
  void SetUp() override { ASSERT_NO_FATAL_FAILURE(RetrieveNodeInfo()); }

 protected:
  void VerifyNodes(cpp20::span<const char*> node_monikers);
  void VerifyOneOf(cpp20::span<const char*> node_monikers);

 private:
  void RetrieveNodeInfo();

  std::unordered_map<std::string, fuchsia_driver_development::NodeInfo> node_info_;
};

#endif  // ZIRCON_SYSTEM_UTEST_DEVICE_ENUMERATION_COMMON_H_
