// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_POWER_TESTING_FAKE_HRTIMER_SRC_DEVICE_SERVER_H_
#define SRC_POWER_TESTING_FAKE_HRTIMER_SRC_DEVICE_SERVER_H_

#include <fidl/fuchsia.hardware.hrtimer/cpp/fidl.h>

#include "lib/fidl/cpp/channel.h"

namespace fake_hrtimer {

// Protocol served to client components over devfs.
class DeviceServer : public fidl::Server<fuchsia_hardware_hrtimer::Device> {
 public:
  explicit DeviceServer();
  void Start(StartRequest& request, StartCompleter::Sync& completer) override;
  void Stop(StopRequest& request, StopCompleter::Sync& completer) override;
  void GetTicksLeft(GetTicksLeftRequest& request, GetTicksLeftCompleter::Sync& completer) override;
  void SetEvent(SetEventRequest& request, SetEventCompleter::Sync& completer) override;
  void StartAndWait(StartAndWaitRequest& request, StartAndWaitCompleter::Sync& completer) override;
  void StartAndWait2(StartAndWait2Request& request,
                     StartAndWait2Completer::Sync& completer) override;
  void GetProperties(GetPropertiesCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_hrtimer::Device> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;
  void Serve(async_dispatcher_t* dispatcher,
             fidl::ServerEnd<fuchsia_hardware_hrtimer::Device> server);

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_hrtimer::Device> bindings_;
  std::optional<zx::event> event_;
  std::optional<fidl::ClientEnd<fuchsia_power_broker::ElementControl>> element_control_client_;
  std::optional<fidl::SyncClient<fuchsia_power_broker::CurrentLevel>> current_level_;
  std::optional<fidl::SyncClient<fuchsia_power_broker::RequiredLevel>> required_level_;
  std::optional<fidl::SyncClient<fuchsia_power_broker::Lessor>> lessor_;
};

}  // namespace fake_hrtimer

#endif  // SRC_POWER_TESTING_FAKE_HRTIMER_SRC_DEVICE_SERVER_H_
