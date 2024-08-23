// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/ld/fuchsia-debugdata.h>
#include <lib/ld/testing/mock-debugdata.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <string_view>

#include <gmock/gmock.h>

#include "load-tests.h"

namespace ld::testing {
namespace {

using namespace std::string_view_literals;

using ::testing::Ne;

TYPED_TEST(LdLoadTests, LlvmProfdata) {
  if constexpr (!TestFixture::kRunsLdStartup) {
    GTEST_SKIP() << "test only applies to startup dynamic linker";
  }

  ASSERT_NO_FATAL_FAILURE(this->Init());

  ASSERT_NO_FATAL_FAILURE(this->Load("llvm-profdata"));

  int64_t exit_code = this->Run();

  this->ExpectLog("");

  auto describe_exit_code = [exit_code]() -> std::string_view {
    switch (exit_code) {
      case ZX_TASK_RETCODE_EXCEPTION_KILL:
        return "CRASHED"sv;
      case 77:
        return "SKIPPED"sv;
      default:
        return zx_status_get_string(static_cast<zx_status_t>(exit_code));
    }
  };

  constexpr std::string_view kInProcess =
      std::is_same_v<ld::testing::LdStartupInProcessTests, TestFixture> ? "in-process: "sv
                                                                        : "test process: "sv;

  if (exit_code == 77) {
    GTEST_SKIP() << kInProcess << "test module not built with llvm-profdata instrumentation";
  }

  EXPECT_EQ(exit_code, 0) << kInProcess << describe_exit_code();

  // On successful exit, the test has written a message back on the bootstrap
  // channel.
  std::array<char, 64> buffer;
  zx::channel test_svc_server_end;
  uint32_t actual_bytes, actual_handles;
  zx_status_t status = this->bootstrap_sender().read(
      0, buffer.data(), test_svc_server_end.reset_and_get_address(),
      static_cast<uint32_t>(buffer.size()), 1, &actual_bytes, &actual_handles);
  ASSERT_EQ(status, ZX_OK) << kInProcess << zx_status_get_string(status);

  std::string_view bootstrap_reply_text(buffer.data(), actual_bytes);
  EXPECT_EQ(bootstrap_reply_text, "llvm-profdata"sv);

  // That delivered the /svc server end channel that the startup dynamic linker
  // passed as the third argument to the user entry point.
  ASSERT_EQ(actual_handles, 1u);
  ASSERT_TRUE(test_svc_server_end.is_valid());

  // Prime a mock fuchsia.debugdata service to expect the llvm-profdata dump.
  auto mock = std::make_unique<::testing::StrictMock<ld::testing::MockDebugdata>>();
  EXPECT_CALL(*mock, Publish(
                         // Expect the right sink name.
                         "llvm-profile"sv,
                         // There's nothing easy to assert about the dump
                         // other than that it's some VMO.
                         ld::testing::ObjKoidMatches(Ne(ZX_KOID_INVALID)),
                         // We could do a custom matcher to assert that the
                         // eventpair's peer is closed, but we only bother
                         // to assert that it's some eventpair.
                         ld::testing::ObjKoidMatches(Ne(ZX_KOID_INVALID))));

  // Prime a mock /svc to supply that service.
  ld::testing::MockSvcDirectory svc_dir;
  ASSERT_NO_FATAL_FAILURE(svc_dir.Init());
  ASSERT_NO_FATAL_FAILURE(svc_dir.AddEntry<fuchsia_debugdata::Publisher>(std::move(mock)));

  // Get a client end to that /svc.  This stands in for the /svc name table
  // entry in the normal process launching for a real program.
  fidl::ClientEnd<fuchsia_io::Directory> svc_client_end;
  ASSERT_NO_FATAL_FAILURE(svc_dir.Serve(svc_client_end));

  // Forward the startup dynamic linker's pipelined messages to /svc as the
  // libc startup code would in a normal process.
  auto result = ld::Debugdata::Forward(
      // Consume the channel at the end of the full expression.
      svc_client_end.TakeChannel().borrow(), std::move(test_svc_server_end));
  EXPECT_TRUE(result.is_ok()) << result.status_string();

  // Finally, drain the service message queues before expectations are checked.
  status = svc_dir.loop().RunUntilIdle();
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
}

}  // namespace
}  // namespace ld::testing
