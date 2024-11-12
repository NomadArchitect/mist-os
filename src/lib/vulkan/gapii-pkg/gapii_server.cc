// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/io.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/remote_dir.h>
#include <zircon/processargs.h>

// Serve /pkg as /pkg in the outgoing directory.
int main(int argc, const char* const* argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  zx::channel client_end, server_end;
  zx_status_t status = zx::channel::create(0, &client_end, &server_end);
  if (status != ZX_OK) {
    fprintf(stderr, "Couldn't create channel, %d\n", status);
    return -1;
  }
  status = fdio_open3(
      "/pkg", static_cast<uint64_t>(fuchsia::io::PERM_READABLE | fuchsia::io::PERM_EXECUTABLE),
      server_end.release());
  if (status != ZX_OK) {
    fprintf(stderr, "Failed to open /pkg");
    return -1;
  }

  vfs::PseudoDir root_dir;
  root_dir.AddEntry("pkg", std::make_unique<vfs::RemoteDir>(std::move(client_end)));

  status = root_dir.Serve(
      fuchsia::io::OpenFlags::RIGHT_READABLE | fuchsia::io::OpenFlags::RIGHT_EXECUTABLE,
      zx::channel(zx_take_startup_handle(PA_DIRECTORY_REQUEST)));

  if (status != ZX_OK) {
    fprintf(stderr, "Failed to serve outgoing.");
    return -1;
  }

  loop.Run();
  return 0;
}
