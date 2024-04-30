// Copyright 2024 Mist Tecnologia LTDA. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_MISTOS_STARNIX_KERNEL_INCLUDE_LIB_MISTOS_STARNIX_RUNNER_CONFIG_H_
#define ZIRCON_KERNEL_LIB_MISTOS_STARNIX_KERNEL_INCLUDE_LIB_MISTOS_STARNIX_RUNNER_CONFIG_H_

#include <vector>

#include <fbl/string.h>

namespace starnix {

struct Config {
  std::vector<fbl::String> features;
  std::vector<fbl::String> init;
  fbl::String kernel_cmdline;
  std::vector<fbl::String> mounts;
  fbl::String name;
  fbl::String startup_file_path;
};

}  // namespace starnix

#endif  // ZIRCON_KERNEL_LIB_MISTOS_STARNIX_KERNEL_INCLUDE_LIB_MISTOS_STARNIX_RUNNER_CONFIG_H_
