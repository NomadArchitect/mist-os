// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_DRIVERS_HID_INPUT_REPORT_DRIVER_H_
#define SRC_UI_INPUT_DRIVERS_HID_INPUT_REPORT_DRIVER_H_

#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/devfs/cpp/connector.h>

#include "src/ui/input/drivers/hid-input-report/input-report.h"

namespace hid_input_report_dev {

const std::string kDeviceName = "InputReport";

class InputReportDriver : public fdf::DriverBase {
 public:
  InputReportDriver(fdf::DriverStartArgs start_args, fdf::UnownedSynchronizedDispatcher dispatcher)
      : DriverBase(kDeviceName, std::move(start_args), std::move(dispatcher)),
        devfs_connector_(fit::bind_member<&InputReportDriver::Serve>(this)) {}

  zx::result<> Start() override;
  void PrepareStop(fdf::PrepareStopCompleter completer) override;

  // Public for testing
  hid_input_report_dev::InputReport& input_report_for_testing() { return *input_report_; }

 private:
  zx::result<> CreateDevfsNode();

  void Serve(fidl::ServerEnd<fuchsia_input_report::InputDevice> server) {
    input_report_bindings_.AddBinding(dispatcher(), std::move(server), input_report_.get(),
                                      fidl::kIgnoreBindingClosure);
  }

  std::unique_ptr<hid_input_report_dev::InputReport> input_report_;
  fidl::ServerBindingGroup<fuchsia_input_report::InputDevice> input_report_bindings_;

  fidl::WireSyncClient<fuchsia_driver_framework::Node> node_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
  driver_devfs::Connector<fuchsia_input_report::InputDevice> devfs_connector_;
};

}  // namespace hid_input_report_dev

#endif  // SRC_UI_INPUT_DRIVERS_HID_INPUT_REPORT_DRIVER_H_
