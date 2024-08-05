// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "examples/power/cpp/wake_lease.h"

#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>

#include <string>

namespace examples::power {

fpromise::promise<WakeLease, Error> WakeLease::Take(
    const fidl::Client<fuchsia_power_system::ActivityGovernor>& client, const std::string& name) {
  fpromise::bridge<WakeLease, Error> bridge;
  client->TakeWakeLease({name}).Then(
      [completer = std::move(bridge.completer)](auto& result) mutable {
        if (!result.is_ok()) {
          completer.complete_error("Failed to take wake lease");
          return;
        }
        completer.complete_ok(WakeLease(std::move(result->token())));
      });
  return bridge.consumer.promise();
}

}  // namespace examples::power
