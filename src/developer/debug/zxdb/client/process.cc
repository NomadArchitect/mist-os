// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/process.h"

namespace zxdb {

Process::Process(Session* session, StartType start_type)
    : ClientObject(session), start_type_(start_type), weak_factory_(this) {}
Process::~Process() = default;

fxl::WeakPtr<Process> Process::GetWeakPtr() { return weak_factory_.GetWeakPtr(); }

}  // namespace zxdb

std::ostream& operator<<(std::ostream& os, const zxdb::Process& process) {
  os << "process '" << process.GetName() << "' (" << process.GetKoid() << ")";
  return os;
}
