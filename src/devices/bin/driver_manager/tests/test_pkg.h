// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_TEST_PKG_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_TEST_PKG_H_

#include <fidl/fuchsia.io/cpp/test_base.h>
#include <fuchsia/io/cpp/fidl_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/binding.h>
#include <zircon/errors.h>

#include <gtest/gtest.h>

namespace test_utils {

class TestFile : public fuchsia::io::testing::File_TestBase {
 public:
  explicit TestFile(std::string_view path) : path_(std::move(path)) {}

 private:
  void GetBackingMemory(fuchsia::io::VmoFlags flags, GetBackingMemoryCallback callback) override {
    EXPECT_EQ(fuchsia::io::VmoFlags::READ | fuchsia::io::VmoFlags::EXECUTE |
                  fuchsia::io::VmoFlags::PRIVATE_CLONE,
              flags);
    auto endpoints = fidl::Endpoints<fuchsia_io::File>::Create();
    EXPECT_EQ(ZX_OK, fdio_open(path_.data(),
                               static_cast<uint32_t>(fuchsia::io::OpenFlags::RIGHT_READABLE |
                                                     fuchsia::io::OpenFlags::RIGHT_EXECUTABLE),
                               endpoints.server.channel().release()));

    fidl::WireSyncClient<fuchsia_io::File> file(std::move(endpoints.client));
    fidl::WireResult result = file->GetBackingMemory(fuchsia_io::wire::VmoFlags(uint32_t(flags)));
    EXPECT_TRUE(result.ok()) << result.FormatDescription();
    auto* res = result.Unwrap();
    if (res->is_error()) {
      callback(fuchsia::io::File_GetBackingMemory_Result::WithErr(std::move(res->error_value())));
      return;
    }
    callback(fuchsia::io::File_GetBackingMemory_Result::WithResponse(
        fuchsia::io::File_GetBackingMemory_Response(std::move(res->value()->vmo))));
  }

  void NotImplemented_(const std::string& name) override {
    printf("Not implemented: File::%s\n", name.data());
  }

  std::string path_;
};

class TestDirectory : public fuchsia::io::testing::Directory_TestBase {
 public:
  using OpenHandler = fit::function<void(fuchsia::io::OpenFlags flags, std::string path,
                                         fidl::InterfaceRequest<fuchsia::io::Node> object)>;

  void SetOpenHandler(OpenHandler open_handler) { open_handler_ = std::move(open_handler); }

 private:
  void Open(fuchsia::io::OpenFlags flags, fuchsia::io::ModeType mode, std::string path,
            fidl::InterfaceRequest<fuchsia::io::Node> object) override {
    open_handler_(flags, std::move(path), std::move(object));
  }

  void NotImplemented_(const std::string& name) override {
    printf("Not implemented: Directory::%s\n", name.data());
  }

  OpenHandler open_handler_;
};

// Implementation of a /pkg directory that can be passed as a component namespace entry
// for the started driver host or driver component.
class TestPkg {
 public:
  struct Config {
    // Where the module is located in the test's package. e.g.
    // /pkg/bin/driver_host2.
    std::string_view module_test_pkg_path;
    // The path that will be requested to the /pkg open
    // handler for the module. e.g. bin/driver_host2.
    std::string_view module_open_path;
    // The names of the libraries that are needed by the module.
    // This list will be used to construct the test files that the driver host runner
    // or driver runner expects to be present in the "/pkg/libs" dir that will be passed
    // to the dynamic linker. No additional validation is done on the strings in |expected_libs|.
    std::vector<std::string_view> expected_libs;
  };

  // |server| is the channel that will be served by |TestPkg|.
  //
  // |module_test_pkg_path| is where the module is located in the test's package. e.g.
  // /pkg/bin/driver_host2.
  //
  // |module_open_path| is the path that will be requested to the /pkg open
  // handler for the module. e.g. bin/driver_host2.
  //
  // |expected_libs| holds that names of the libraries that are needed by the module.
  // This list will be used to construct the test files that the driver host runner
  // or driver runner expects to be present in the "/pkg/libs" dir that will be passed
  // to the dynamic linker. No additional validation is done on the strings in |expected_libs|.
  TestPkg(fidl::ServerEnd<fuchsia_io::Directory> server, std::string_view module_test_pkg_path,
          std::string_view module_open_path, const std::vector<std::string_view> expected_libs);

  TestPkg(fidl::ServerEnd<fuchsia_io::Directory> server, Config config)
      : TestPkg(std::move(server), config.module_test_pkg_path, config.module_open_path,
                config.expected_libs) {}

  ~TestPkg() {
    loop_.Quit();
    loop_.JoinThreads();
  }

 private:
  static constexpr std::string_view kLibPathPrefix = "/pkg/lib/";

  async::Loop loop_{&kAsyncLoopConfigNoAttachToCurrentThread};

  TestDirectory pkg_dir_;
  fidl::Binding<fuchsia::io::Directory> pkg_binding_{&pkg_dir_};

  TestDirectory lib_dir_;
  fidl::Binding<fuchsia::io::Directory> lib_dir_binding_{&lib_dir_};
  std::map<std::string, TestFile> libname_to_file_;
  std::vector<std::unique_ptr<fidl::Binding<fuchsia::io::File>>> lib_file_bindings_;

  TestFile module_;
  fidl::Binding<fuchsia::io::File> module_binding_{&module_};
};

}  // namespace test_utils

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_TEST_PKG_H_
