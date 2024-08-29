// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SYSMEM_DRIVERS_SYSMEM_MACROS_H_
#define SRC_DEVICES_SYSMEM_DRIVERS_SYSMEM_MACROS_H_

#include "src/devices/sysmem/drivers/sysmem/logging.h"

#define LOG(severity, fmt, ...)                                                              \
  ::sysmem_service::Log(::fuchsia_logging::LOG_##severity, __FILE__, __LINE__, nullptr, fmt, \
                        ##__VA_ARGS__)

#endif  // SRC_DEVICES_SYSMEM_DRIVERS_SYSMEM_MACROS_H_
