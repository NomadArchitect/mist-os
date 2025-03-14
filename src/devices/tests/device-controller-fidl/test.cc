// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.sample/cpp/wire.h>
#include <fuchsia/driver/test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/lib.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fidl/cpp/synchronous_interface_ptr.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

class DeviceControllerFidl : public gtest::TestLoopFixture {};

TEST_F(DeviceControllerFidl, ControllerTest) {
  // Create and build the realm.
  auto realm_builder = component_testing::RealmBuilder::Create();
  driver_test_realm::Setup(realm_builder);
  auto realm = realm_builder.Build(dispatcher());

  // Start DriverTestRealm.
  fidl::SynchronousInterfacePtr<fuchsia::driver::test::Realm> driver_test_realm;
  ASSERT_EQ(ZX_OK, realm.component().Connect(driver_test_realm.NewRequest()));
  fuchsia::driver::test::Realm_Start_Result realm_result;
  auto args = fuchsia::driver::test::RealmArgs();
  ASSERT_EQ(ZX_OK, driver_test_realm->Start(std::move(args), &realm_result));
  ASSERT_FALSE(realm_result.is_err());

  fbl::unique_fd dev_topo_fd;
  {
    zx::channel dev_client, dev_server;
    ASSERT_EQ(zx::channel::create({}, &dev_client, &dev_server), ZX_OK);
    ASSERT_EQ(realm.component().exposed()->Open3("dev-topological", fuchsia::io::PERM_READABLE, {},
                                                 std::move(dev_server)),
              ZX_OK);
    ASSERT_EQ(fdio_fd_create(dev_client.release(), dev_topo_fd.reset_and_get_address()), ZX_OK);
  }

  // Wait for driver.
  zx::result dev_channel =
      device_watcher::RecursiveWaitForFile(dev_topo_fd.get(), "sys/test/sample_driver");
  ASSERT_EQ(dev_channel.status_value(), ZX_OK);

  fdio_cpp::UnownedFdioCaller dev_topo(dev_topo_fd);
  zx::result channel = component::ConnectAt<fuchsia_device::Controller>(
      dev_topo.directory(), "sys/test/sample_driver/device_controller");
  ASSERT_EQ(ZX_OK, channel.status_value());
  auto client = fidl::WireSyncClient(std::move(channel.value()));

  auto result = client->GetTopologicalPath();
  ASSERT_EQ(result->value()->path.get(), "/dev/sys/test/sample_driver");

  // Get the underlying device connection.
  {
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_sample::Echo>();
    ASSERT_EQ(client->ConnectToDeviceFidl(endpoints->server.TakeChannel()).status(), ZX_OK);

    auto echo = fidl::WireSyncClient(std::move(endpoints->client));

    std::string_view sent_string = "hello";
    auto result = echo->EchoString(fidl::StringView::FromExternal(sent_string));
    ASSERT_EQ(ZX_OK, result.status());
    ASSERT_EQ(sent_string, result.value().response.get());
  }

  // Check the Echo API through the device protocol connector.
  {
    zx::result channel = component::ConnectAt<fuchsia_hardware_sample::Echo>(
        dev_topo.directory(), "sys/test/sample_driver/device_protocol");
    ASSERT_EQ(ZX_OK, channel.status_value());

    auto echo = fidl::WireSyncClient(std::move(channel.value()));

    std::string_view sent_string = "hello";
    auto result = echo->EchoString(fidl::StringView::FromExternal(sent_string));
    ASSERT_EQ(ZX_OK, result.status());
    ASSERT_EQ(sent_string, result.value().response.get());
  }

  // Get the controller connection again.
  {
    auto endpoints = fidl::CreateEndpoints<fuchsia_device::Controller>();
    ASSERT_EQ(client->ConnectToController(std::move(endpoints->server)).status(), ZX_OK);

    auto result = fidl::WireCall(endpoints->client)->GetTopologicalPath();
    ASSERT_EQ(result->value()->path.get(), "/dev/sys/test/sample_driver");
  }
}
