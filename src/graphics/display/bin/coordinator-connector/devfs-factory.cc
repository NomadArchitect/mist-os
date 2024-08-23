// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/bin/coordinator-connector/devfs-factory.h"

#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include <cstdint>

#include "src/lib/fsl/io/device_watcher.h"

namespace display {

static const std::string kDisplayDir = "/dev/class/display-coordinator";

zx::result<> DevFsCoordinatorFactory::CreateAndPublishService(
    component::OutgoingDirectory& outgoing, async_dispatcher_t* dispatcher) {
  return outgoing.AddProtocol<fuchsia_hardware_display::Provider>(
      std::make_unique<DevFsCoordinatorFactory>(dispatcher));
}

DevFsCoordinatorFactory::DevFsCoordinatorFactory(async_dispatcher_t* dispatcher)
    : dispatcher_(dispatcher) {}

zx_status_t DevFsCoordinatorFactory::OpenCoordinatorWithListenerForPrimaryOnDevice(
    const fidl::ClientEnd<fuchsia_io::Directory>& dir, const std::string& filename,
    fidl::ServerEnd<fuchsia_hardware_display::Coordinator> coordinator_server,
    fidl::ClientEnd<fuchsia_hardware_display::CoordinatorListener> listener_client) {
  zx::result client = component::ConnectAt<fuchsia_hardware_display::Provider>(dir, filename);
  if (client.is_error()) {
    FX_PLOGS(ERROR, client.error_value())
        << "Failed to open display_controller at path: " << kDisplayDir << '/' << filename;

    // We could try to match the value of the C "errno" macro to the closest ZX error, but
    // this would give rise to many corner cases.  We never expect this to fail anyway, since
    // |filename| is given to us by the device watcher.
    return ZX_ERR_INTERNAL;
  }

  // TODO(https://fxbug.dev/42135096): Pass an async completer asynchronously into
  // OpenCoordinator(), rather than blocking on a synchronous call.
  fidl::Arena arena;
  auto request =
      fuchsia_hardware_display::wire::ProviderOpenCoordinatorWithListenerForPrimaryRequest::Builder(
          arena)
          .coordinator(std::move(coordinator_server))
          .coordinator_listener(std::move(listener_client))
          .Build();
  fidl::WireResult result =
      fidl::WireCall(client.value())->OpenCoordinatorWithListenerForPrimary(std::move(request));
  if (!result.ok()) {
    FX_PLOGS(ERROR, result.status()) << "Failed to call service handle";

    // There's not a clearly-better value to return here.  Returning the FIDL error would be
    // somewhat unexpected, since the caller wouldn't receive it as a FIDL status, rather as
    // the return value of a "successful" method invocation.
    return ZX_ERR_INTERNAL;
  }
  if (result.value().is_error()) {
    FX_PLOGS(ERROR, result.value().error_value()) << "Failed to open display coordinator";
    return result.value().error_value();
  }

  return ZX_OK;
}

void DevFsCoordinatorFactory::OpenCoordinatorWithListenerForVirtcon(
    OpenCoordinatorWithListenerForVirtconRequest& request,
    OpenCoordinatorWithListenerForVirtconCompleter::Sync& completer) {
  completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
}

void DevFsCoordinatorFactory::OpenCoordinatorWithListenerForPrimary(
    OpenCoordinatorWithListenerForPrimaryRequest& request,
    OpenCoordinatorWithListenerForPrimaryCompleter::Sync& completer) {
  const int64_t id = next_display_client_id_++;

  // Watcher's lifetime needs to be at most as long as the lifetime of |this|,
  // and otherwise as long as the lifetime of |callback|.  |this| will own
  // the references to outstanding watchers, and each watcher will notify |this|
  // when it is done, so that |this| can remove a reference to it.
  std::unique_ptr<fsl::DeviceWatcher> watcher = fsl::DeviceWatcher::Create(
      kDisplayDir,
      [this, id, request = std::move(request), async_completer = completer.ToAsync()](
          const fidl::ClientEnd<fuchsia_io::Directory>& dir, const std::string& filename) mutable {
        FX_LOGS(INFO) << "Found display controller at path: " << kDisplayDir << '/' << filename
                      << '.';
        zx_status_t open_coordinator_status = OpenCoordinatorWithListenerForPrimaryOnDevice(
            dir, filename, std::move(*request.coordinator()),
            std::move(*request.coordinator_listener()));
        if (open_coordinator_status != ZX_OK) {
          async_completer.Reply(fit::error(open_coordinator_status));
          return;
        }
        async_completer.Reply(fit::ok());
        // We no longer need |this| to store this closure, remove it. Do not do
        // any work after this point.
        pending_device_watchers_.erase(id);
      },
      dispatcher_);
  pending_device_watchers_[id] = std::move(watcher);
}

}  // namespace display
