// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/power/cpp/wake-lease.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/clock.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace fdf_power {

WakeLease::WakeLease(async_dispatcher_t* dispatcher, std::string_view lease_name,
                     fidl::ClientEnd<fuchsia_power_system::ActivityGovernor> sag_client,
                     inspect::Node* parent_node, bool log)
    : dispatcher_(dispatcher), lease_name_(lease_name), log_(log) {
  if (sag_client) {
    sag_client_.Bind(std::move(sag_client));

    auto [client_end, server_end] =
        fidl::Endpoints<fuchsia_power_system::ActivityGovernorListener>::Create();
    fidl::Arena arena;
    fidl::WireResult result = sag_client_->RegisterListener(
        fuchsia_power_system::wire::ActivityGovernorRegisterListenerRequest::Builder(arena)
            .listener(std::move(client_end))
            .Build());
    if (!result.ok()) {
      if (log_) {
        fdf::warn("Failed to register for sag state listener: {}", result.error());
      }
      ResetSagClient();
    } else {
      listener_binding_.emplace(dispatcher_, std::move(server_end), this,
                                [this](fidl::UnbindInfo unused) { ResetSagClient(); });
    }
  }

  if (parent_node) {
    total_lease_acquisitions_ = parent_node->CreateUint("Total Lease Acquisitions", 0);
    wake_lease_held_ = parent_node->CreateBool("Wake Lease Held", false);
    wake_lease_grabbable_ = parent_node->CreateBool("Wake Lease Grabbable", sag_client_.is_valid());
    wake_lease_last_acquired_timestamp_ =
        parent_node->CreateUint("Wake Lease Last Acquired Timestamp (ns)", 0);
    wake_lease_last_refreshed_timestamp_ =
        parent_node->CreateUint("Wake Lease Last Refreshed Timestamp (ns)", 0);
  }
}

bool WakeLease::HandleInterrupt(zx::duration timeout) {
  // Only acquire a wake lease if the system state is appropriate.
  if (!system_suspended_) {
    return false;
  }
  return AcquireWakeLease(timeout);
}

bool WakeLease::AcquireWakeLease(zx::duration timeout) {
  if (!sag_client_) {
    return false;
  }

  if (lease_) {
    // If already holding a lease, cancel the current timeout.
    lease_task_.Cancel();
    wake_lease_last_refreshed_timestamp_.Set(zx::clock::get_monotonic().get());
  } else {
    // If not holding a lease, take one.
    auto result_lease = sag_client_->TakeWakeLease(fidl::StringView::FromExternal(lease_name_));
    if (!result_lease.ok()) {
      if (log_) {
        fdf::warn(
            "Failed to take wake lease, system may incorrectly enter suspend: {}. Will not attempt again.",
            result_lease.error());
      }
      ResetSagClient();
      return false;
    }

    lease_ = std::move(result_lease->token);
    if (log_) {
      fdf::info("Created a wake lease due to recent wake event.");
    }
    auto now = zx::clock::get_monotonic().get();
    wake_lease_last_acquired_timestamp_.Set(now);
    wake_lease_last_refreshed_timestamp_.Set(now);
    total_lease_acquisitions_.Add(1);
    wake_lease_held_.Set(true);
  }

  if (lease_) {
    lease_task_.PostDelayed(dispatcher_, timeout);
  }
  return true;
}

void WakeLease::DepositWakeLease(zx::eventpair wake_lease, zx::time timeout_deadline) {
  if (lease_) {
    if (timeout_deadline < lease_task_.last_deadline()) {
      // If the current least out lives the new one, don't need to do anything.
      return;
    }
    // If already holding a lease, cancel the current timeout.
    lease_task_.Cancel();
  }

  lease_ = std::move(wake_lease);
  wake_lease_last_refreshed_timestamp_.Set(zx::clock::get_monotonic().get());
  wake_lease_held_.Set(true);
  lease_task_.PostForTime(dispatcher_, timeout_deadline);
}

zx::eventpair WakeLease::TakeWakeLease() {
  lease_task_.Cancel();
  wake_lease_held_.Set(false);
  return std::move(lease_);
}

void WakeLease::OnResume(OnResumeCompleter::Sync& completer) {
  system_suspended_ = false;
  completer.Reply();
}

void WakeLease::OnSuspendStarted(OnSuspendStartedCompleter::Sync& completer) {
  system_suspended_ = true;
  completer.Reply();
}

void WakeLease::OnSuspendFail(OnSuspendFailCompleter::Sync& completer) {
  system_suspended_ = false;
  completer.Reply();
}

void WakeLease::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_power_system::ActivityGovernorListener> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  if (log_) {
    fdf::warn("Encountered unexpected method: {}", metadata.method_ordinal);
  }
}

void WakeLease::HandleTimeout() {
  if (log_) {
    fdf::info("Dropping the wake lease due to not receiving any wake events.");
  }
  lease_.reset();
  wake_lease_held_.Set(false);
}

void WakeLease::ResetSagClient() {
  sag_client_ = {};
  wake_lease_grabbable_.Set(false);
}

}  // namespace fdf_power

#endif
