// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async-testing/test_loop.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"

namespace {

using ::testing::_;

// Simple vnode implementation that provides a way to query whether the vfs pointer is set.
class TestNode : public fs::Vnode {
 public:
  // Vnode implementation:
  fuchsia_io::NodeProtocolKinds GetProtocols() const override {
    return fuchsia_io::NodeProtocolKinds::kFile;
  }

 private:
  friend fbl::internal::MakeRefCountedHelper<TestNode>;
  friend fbl::RefPtr<TestNode>;

  ~TestNode() override = default;
};

}  // namespace

// ManagedVfs always sets the dispatcher in its constructor, and trying to change it using
// Vfs::SetDispatcher should fail.
TEST(ManagedVfs, CantSetDispatcher) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  fs::ManagedVfs vfs(loop.dispatcher());
  ASSERT_DEATH(vfs.SetDispatcher(loop.dispatcher()), _);
}

TEST(SynchronousVfs, CanOnlySetDispatcherOnce) {
  fs::SynchronousVfs vfs;
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  vfs.SetDispatcher(loop.dispatcher());

  ASSERT_DEATH(vfs.SetDispatcher(loop.dispatcher()), _);
}

static void CheckClosesConnection(fs::FuchsiaVfs* vfs, async::TestLoop* loop) {
  zx::result a = fidl::CreateEndpoints<fuchsia_io::Directory>();
  zx::result b = fidl::CreateEndpoints<fuchsia_io::Directory>();
  ASSERT_EQ(a.status_value(), ZX_OK);
  ASSERT_EQ(b.status_value(), ZX_OK);

  auto dir_a = fbl::MakeRefCounted<fs::PseudoDir>();
  auto dir_b = fbl::MakeRefCounted<fs::PseudoDir>();
  ASSERT_EQ(vfs->ServeDirectory(dir_a, std::move(a->server)), ZX_OK);
  ASSERT_EQ(vfs->ServeDirectory(dir_b, std::move(b->server)), ZX_OK);
  bool callback_called = false;
  vfs->CloseAllConnectionsForVnode(*dir_a, [&callback_called]() { callback_called = true; });
  loop->RunUntilIdle();
  zx_signals_t signals;
  ASSERT_EQ(a->client.channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), &signals),
            ZX_OK);
  ASSERT_TRUE(signals & ZX_CHANNEL_PEER_CLOSED);
  ASSERT_EQ(ZX_ERR_TIMED_OUT,
            b->client.channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time(0), &signals));
  ASSERT_TRUE(callback_called);
}

TEST(ManagedVfs, CloseAllConnections) {
  async::TestLoop loop;
  fs::ManagedVfs vfs(loop.dispatcher());
  CheckClosesConnection(&vfs, &loop);
  loop.RunUntilIdle();
}

TEST(SynchronousVfs, CloseAllConnections) {
  async::TestLoop loop;
  fs::SynchronousVfs vfs(loop.dispatcher());
  CheckClosesConnection(&vfs, &loop);
  loop.RunUntilIdle();
}

TEST(ManagedVfs, CloseAllConnectionsForVnodeWithoutAnyConnections) {
  async::TestLoop loop;
  fs::ManagedVfs vfs(loop.dispatcher());
  auto dir = fbl::MakeRefCounted<fs::PseudoDir>();
  bool closed = false;
  vfs.CloseAllConnectionsForVnode(*dir, [&closed]() { closed = true; });
  loop.RunUntilIdle();
  ASSERT_TRUE(closed);
}

TEST(SynchronousVfs, CloseAllConnectionsForVnodeWithoutAnyConnections) {
  async::TestLoop loop;
  fs::SynchronousVfs vfs(loop.dispatcher());
  auto dir = fbl::MakeRefCounted<fs::PseudoDir>();
  bool closed = false;
  vfs.CloseAllConnectionsForVnode(*dir, [&closed]() { closed = true; });
  loop.RunUntilIdle();
  ASSERT_TRUE(closed);
}
