// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef EXAMPLES_DRIVERS_TRANSPORT_DRIVER_V2_PARENT_DRIVER_H_
#define EXAMPLES_DRIVERS_TRANSPORT_DRIVER_V2_PARENT_DRIVER_H_

#include <fidl/fuchsia.hardware.i2cimpl/cpp/driver/wire.h>
#include <lib/driver/component/cpp/driver_base.h>

namespace driver_transport {

// A driver that implements and serves the fuchsia.hardware.i2cimpl FIDL protocol.
class ParentTransportDriver : public fdf::DriverBase,
                              public fdf::WireServer<fuchsia_hardware_i2cimpl::Device> {
 public:
  ParentTransportDriver(fdf::DriverStartArgs start_args,
                        fdf::UnownedSynchronizedDispatcher driver_dispatcher)
      : DriverBase("transport-parent", std::move(start_args), std::move(driver_dispatcher)) {}

  zx::result<> Start() override;

  void GetMaxTransferSize(fdf::Arena& arena, GetMaxTransferSizeCompleter::Sync& completer) override;
  void SetBitrate(SetBitrateRequestView request, fdf::Arena& arena,
                  SetBitrateCompleter::Sync& completer) override;
  void Transact(TransactRequestView request, fdf::Arena& arena,
                TransactCompleter::Sync& completer) override;

  // Only required for open protocols.
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_i2cimpl::Device> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  fidl::WireClient<fuchsia_driver_framework::NodeController> controller_;

  uint32_t bitrate_;

  fdf::ServerBindingGroup<fuchsia_hardware_i2cimpl::Device> server_bindings_;
};

}  // namespace driver_transport

#endif  // EXAMPLES_DRIVERS_TRANSPORT_DRIVER_V2_PARENT_DRIVER_H_
