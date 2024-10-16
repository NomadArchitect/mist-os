// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_MEMORY_PRESSURE_SIGNALER_DEBUGGER_H_
#define SRC_DEVELOPER_MEMORY_PRESSURE_SIGNALER_DEBUGGER_H_

#include <fuchsia/memory/debug/cpp/fidl.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/sys/cpp/component_context.h>

#include "src/developer/memory/pressure_signaler/pressure_notifier.h"

namespace pressure_signaler {

class MemoryDebugger : public fuchsia::memory::debug::MemoryPressure {
 public:
  MemoryDebugger(sys::ComponentContext *context, PressureNotifier *notifier);

  // Signals registered watchers of the fuchsia.memorypressure service with the
  // specified memory pressure `level`.
  void Signal(fuchsia::memorypressure::Level level) final;

 private:
  fidl::BindingSet<fuchsia::memory::debug::MemoryPressure> bindings_;
  PressureNotifier *const notifier_;
};

}  // namespace pressure_signaler

#endif  // SRC_DEVELOPER_MEMORY_PRESSURE_SIGNALER_DEBUGGER_H_
