// Copyright 2024 Mist Tecnologia LTDA. All rights reserved.
// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/mistos/starnix/kernel/vfs/fd_numbers.h"

// clang-format off
#include <linux/fcntl.h>
// clang-format on

namespace starnix {

const FdNumber FdNumber::_AT_FDCWD = {AT_FDCWD};

}

// #define AT_FDCWD AT_FDCWD_TMP
