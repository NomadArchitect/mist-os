// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "startup-symbols.h"

// This is a startup module that initializes this global variable, which will be
// accessed and modified by init/fini functions of dlopen-ed modules and are
// checked for correctness in libdl tests.
int gInitFiniState = -1;
