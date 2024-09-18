// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/hardware/block/driver/c/banjo.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fit/defer.h>
#include <string.h>
#include <unistd.h>

#include <zxtest/zxtest.h>

#include "block-device.h"
#include "server.h"
#include "src/devices/testing/mock-ddk/mock-device.h"
#include "test/stub-block-device.h"

namespace {

class ServerTest : public zxtest::Test {
 public:
  ServerTest() : client_(blkdev_.proto()) {}

  void TearDown() override {
    server_->Close();
    server_thread_.join();
  }

  void CreateServer() {
    zx::result server_or = Server::Create(&client_);
    ASSERT_OK(server_or);
    server_ = std::move(server_or.value());
    server_thread_ = std::thread([&server = server_]() { server->Serve(); });

    zx::result fifo_or = server_->GetFifo();
    ASSERT_OK(fifo_or);
    fifo_ = std::move(fifo_or.value());
  }

  void CreateServer(const block_info_t& block_info) {
    blkdev_.SetInfo(&block_info);
    CreateServer();
  }

  void AttachVmo(bool do_fill) {
    zx::vmo vmo;
    const size_t vmo_size = 8192;
    ASSERT_OK(zx::vmo::create(vmo_size, 0, &vmo));

    if (do_fill) {
      ASSERT_OK(FillVmo(vmo, vmo_size));
    }

    zx::result vmoid_or = server_->AttachVmo(std::move(vmo));
    ASSERT_OK(vmoid_or);
    vmoid_ = vmoid_or.value();
  }

  zx_status_t FillVmo(const zx::vmo& vmo, size_t size) {
    std::vector<uint8_t> buf(zx_system_get_page_size());
    memset(buf.data(), 0x44, zx_system_get_page_size());
    for (size_t i = 0; i < size; i += zx_system_get_page_size()) {
      size_t remain = size - i;
      if (remain > zx_system_get_page_size()) {
        remain = zx_system_get_page_size();
      }
      if (zx_status_t status = vmo.write(buf.data(), i, remain); status != ZX_OK) {
        return status;
      }
    }
    return ZX_OK;
  }

  void RequestOne(const block_fifo_request_t& request) {
    // Write request.
    size_t actual_count = 0;
    ASSERT_OK(fifo_.write(sizeof(request), &request, 1, &actual_count));
    ASSERT_EQ(actual_count, 1);
  }

  void RequestOneAndWaitResponse(const block_fifo_request_t& request, zx_status_t expected_status,
                                 uint32_t expected_response_count = 1) {
    // Write request.
    size_t actual_count = 0;
    ASSERT_OK(fifo_.write(sizeof(request), &request, 1, &actual_count));
    ASSERT_EQ(actual_count, 1);

    // Wait for response.
    zx_signals_t observed;
    ASSERT_OK(fifo_.wait_one(ZX_FIFO_READABLE, zx::time::infinite(), &observed));

    block_fifo_response_t response;
    ASSERT_OK(fifo_.read(sizeof(response), &response, 1, &actual_count));
    ASSERT_EQ(actual_count, 1);
    ASSERT_EQ(response.status, expected_status);
    ASSERT_EQ(request.reqid, response.reqid);
    ASSERT_EQ(response.count, expected_response_count);
  }

 protected:
  StubBlockDevice blkdev_;
  ddk::BlockProtocolClient client_;
  std::unique_ptr<Server> server_;
  std::thread server_thread_;
  zx::fifo fifo_;
  vmoid_t vmoid_;
};

TEST_F(ServerTest, Create) { CreateServer(); }

TEST_F(ServerTest, AttachVmo) {
  CreateServer();
  AttachVmo(/*do_fill=*/false);
}

TEST_F(ServerTest, CloseVMO) {
  CreateServer();
  AttachVmo(/*do_fill=*/false);

  // Request close VMO.
  block_fifo_request_t req = {
      .command = {.opcode = BLOCK_OPCODE_CLOSE_VMO, .flags = 0},
      .reqid = 0x100,
      .group = 0,
      .vmoid = vmoid_,
      .length = 0,
      .vmo_offset = 0,
      .dev_offset = 0,
  };
  RequestOneAndWaitResponse(req, ZX_OK);
}

TEST_F(ServerTest, ReadSingleTest) {
  CreateServer();
  AttachVmo(/*do_fill=*/true);

  // Request close VMO.
  block_fifo_request_t req = {
      .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
      .reqid = 0x100,
      .group = 0,
      .vmoid = vmoid_,
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };
  RequestOneAndWaitResponse(req, ZX_OK);
}

TEST_F(ServerTest, ReadManyBlocksHasOneResponse) {
  // Restrict max_transfer_size so that the server has to split up our request.
  block_info_t block_info = {
      .block_count = kBlockCount, .block_size = kBlockSize, .max_transfer_size = kBlockSize};
  CreateServer(block_info);
  AttachVmo(/*do_fill=*/true);

  block_fifo_request_t reqs[2] = {
      {
          .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
          .reqid = 0x100,
          .group = 0,
          .vmoid = vmoid_,
          .length = 4,
          .vmo_offset = 0,
          .dev_offset = 0,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
          .reqid = 0x101,
          .group = 0,
          .vmoid = vmoid_,
          .length = 1,
          .vmo_offset = 0,
          .dev_offset = 0,
      },
  };

  // Write requests.
  size_t actual_count = 0;
  ASSERT_OK(fifo_.write(sizeof(reqs[0]), reqs, 2, &actual_count));
  ASSERT_EQ(actual_count, 2);

  // Wait for first response.
  zx_signals_t observed;
  ASSERT_OK(zx_object_wait_one(fifo_.get(), ZX_FIFO_READABLE, ZX_TIME_INFINITE, &observed));

  block_fifo_response_t res;
  ASSERT_OK(fifo_.read(sizeof(res), &res, 1, &actual_count));
  ASSERT_EQ(actual_count, 1);
  ASSERT_OK(res.status);
  ASSERT_EQ(reqs[0].reqid, res.reqid);
  ASSERT_EQ(res.count, 1);

  // Wait for second response.
  ASSERT_OK(zx_object_wait_one(fifo_.get(), ZX_FIFO_READABLE, ZX_TIME_INFINITE, &observed));

  ASSERT_OK(fifo_.read(sizeof(res), &res, 1, &actual_count));
  ASSERT_EQ(actual_count, 1);
  ASSERT_OK(res.status);
  ASSERT_EQ(reqs[1].reqid, res.reqid);
  ASSERT_EQ(res.count, 1);
}

TEST_F(ServerTest, TestLargeGroupedTransaction) {
  // Restrict max_transfer_size so that the server has to split up our request.
  block_info_t block_info = {
      .block_count = kBlockCount, .block_size = kBlockSize, .max_transfer_size = kBlockSize};
  CreateServer(block_info);
  AttachVmo(/*do_fill=*/true);

  block_fifo_request_t reqs[2] = {
      {
          .command = {.opcode = BLOCK_OPCODE_READ, .flags = BLOCK_IO_FLAG_GROUP_ITEM},
          .reqid = 0x101,
          .group = 0,
          .vmoid = vmoid_,
          .length = 4,
          .vmo_offset = 0,
          .dev_offset = 0,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ,
                      .flags = BLOCK_IO_FLAG_GROUP_ITEM | BLOCK_IO_FLAG_GROUP_LAST},
          .reqid = 0x101,
          .group = 0,
          .vmoid = vmoid_,
          .length = 1,
          .vmo_offset = 0,
          .dev_offset = 0,
      },
  };

  // Write requests.
  size_t actual_count = 0;
  ASSERT_OK(fifo_.write(sizeof(reqs[0]), reqs, 2, &actual_count));
  ASSERT_EQ(actual_count, 2);

  // Wait for first response.
  zx_signals_t observed;
  ASSERT_OK(zx_object_wait_one(fifo_.get(), ZX_FIFO_READABLE, ZX_TIME_INFINITE, &observed));

  block_fifo_response_t res;
  ASSERT_OK(fifo_.read(sizeof(res), &res, 1, &actual_count));
  ASSERT_EQ(actual_count, 1);
  ASSERT_OK(res.status);
  ASSERT_EQ(reqs[0].reqid, res.reqid);
  ASSERT_EQ(res.count, 2);
  ASSERT_EQ(res.group, 0);
}

TEST_F(ServerTest, FuaWrite) {
  block_info_t block_info = {
      .block_count = kBlockCount, .block_size = kBlockSize, .max_transfer_size = kBlockSize};
  CreateServer(block_info);
  AttachVmo(/*do_fill=*/true);

  block_fifo_request_t req = {
      .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = BLOCK_IO_FLAG_FORCE_ACCESS},  // Write FUA
      .reqid = 0x100,
      .group = 0,
      .vmoid = vmoid_,
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };
  RequestOneAndWaitResponse(req, ZX_OK);

  auto commands = blkdev_.GetCommandSequence();
  ASSERT_EQ(commands.size(), 2);
  ASSERT_EQ(commands[0].opcode, BLOCK_OPCODE_WRITE);
  ASSERT_EQ(commands[0].flags, 0);                    // FUA flag is removed
  ASSERT_EQ(commands[1].opcode, BLOCK_OPCODE_FLUSH);  // Post flush
  ASSERT_EQ(commands[1].flags, 0);
}

TEST_F(ServerTest, FuaWriteWithFua) {
  block_info_t block_info = {.block_count = kBlockCount,
                             .block_size = kBlockSize,
                             .max_transfer_size = kBlockSize,
                             .flags = FLAG_FUA_SUPPORT};
  CreateServer(block_info);
  AttachVmo(/*do_fill=*/true);

  block_fifo_request_t req = {
      .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = BLOCK_IO_FLAG_FORCE_ACCESS},  // Write FUA
      .reqid = 0x100,
      .group = 0,
      .vmoid = vmoid_,
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };
  RequestOneAndWaitResponse(req, ZX_OK);

  auto commands = blkdev_.GetCommandSequence();
  ASSERT_EQ(commands.size(), 1);
  ASSERT_EQ(commands[0].opcode, BLOCK_OPCODE_WRITE);
  ASSERT_EQ(commands[0].flags, BLOCK_IO_FLAG_FORCE_ACCESS);  // FUA write
}

TEST_F(ServerTest, Postflush) {
  block_info_t block_info = {
      .block_count = kBlockCount, .block_size = kBlockSize, .max_transfer_size = kBlockSize};
  CreateServer(block_info);
  AttachVmo(/*do_fill=*/true);

  block_fifo_request_t req = {
      .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = BLOCK_IO_FLAG_FORCE_ACCESS},  // FUA
      .reqid = 0x100,
      .group = 0,
      .vmoid = vmoid_,
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };
  RequestOneAndWaitResponse(req, ZX_OK);

  // If the device has a volatile write cache but FUA command is not supported, the post flush
  // commands are delivered.
  auto commands = blkdev_.GetCommandSequence();
  ASSERT_EQ(commands.size(), 2);
  ASSERT_EQ(commands[0].opcode, BLOCK_OPCODE_WRITE);
  ASSERT_EQ(commands[0].flags, 0);                    // FUA flag is removed
  ASSERT_EQ(commands[1].opcode, BLOCK_OPCODE_FLUSH);  // Post flush
  ASSERT_EQ(commands[1].flags, 0);
}

TEST_F(ServerTest, PostflushException) {
  block_info_t block_info = {
      .block_count = kBlockCount, .block_size = kBlockSize, .max_transfer_size = kBlockSize};
  CreateServer(block_info);
  AttachVmo(/*do_fill=*/true);

  block_fifo_request_t req = {
      .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = BLOCK_IO_FLAG_FORCE_ACCESS},  // FUA
      .reqid = 0x100,
      .group = 0,
      .vmoid = vmoid_,
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  // If the device has a volatile write cache but FUA command is not supported, the post flush
  // commands are delivered.
  // (I/O sequence = Write -> Post flush)
  auto& commands = blkdev_.GetCommandSequence();
  {
    // I/O error occurs on write
    blkdev_.set_callback([&](const block_op_t& block_op) {
      if (commands.size() == 1 && block_op.command.opcode == BLOCK_OPCODE_WRITE) {
        return ZX_ERR_IO;
      }
      return ZX_OK;
    });
    RequestOneAndWaitResponse(req, ZX_ERR_IO);
    ASSERT_EQ(commands.size(), 1);  // Error is reported after write transfered
    ASSERT_EQ(commands[0].opcode, BLOCK_OPCODE_WRITE);
    ASSERT_EQ(commands[0].flags, 0);  // FUA flag is removed
    commands.clear();
  }
  {
    // I/O error occurs on postflush
    blkdev_.set_callback([&](const block_op_t& block_op) {
      if (commands.size() == 2 && block_op.command.opcode == BLOCK_OPCODE_FLUSH) {
        return ZX_ERR_IO;
      }
      return ZX_OK;
    });
    RequestOneAndWaitResponse(req, ZX_ERR_IO);
    ASSERT_EQ(commands.size(), 2);  // Error is reported after postflush transfered
    ASSERT_EQ(commands[0].opcode, BLOCK_OPCODE_WRITE);
    ASSERT_EQ(commands[0].flags, 0);                    // FUA flag is removed
    ASSERT_EQ(commands[1].opcode, BLOCK_OPCODE_FLUSH);  // Post flush
    ASSERT_EQ(commands[1].flags, 0);
    commands.clear();
  }
}

TEST_F(ServerTest, FuaWriteWithLargeGroupedTransaction) {
  // Restrict max_transfer_size so that the server has to split up our request.
  block_info_t block_info = {.block_count = kBlockCount,
                             .block_size = kBlockSize,
                             .max_transfer_size = kBlockSize,
                             .flags = FLAG_FUA_SUPPORT};
  CreateServer(block_info);
  AttachVmo(/*do_fill=*/true);

  block_fifo_request_t req = {
      .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = BLOCK_IO_FLAG_FORCE_ACCESS},
      .reqid = 0x100,
      .group = 0,
      .vmoid = vmoid_,
      .length = 5,
      .vmo_offset = 0,
      .dev_offset = 0,
  };
  RequestOneAndWaitResponse(req, ZX_OK);

  // If the device has a volatile write cache but FUA command is not supported, the post flush
  // commands are delivered.
  auto commands = blkdev_.GetCommandSequence();
  ASSERT_EQ(commands.size(), 5);
  for (int i = 0; i < 5; ++i) {
    ASSERT_EQ(commands[i].opcode, BLOCK_OPCODE_WRITE);
    ASSERT_EQ(commands[i].flags, BLOCK_IO_FLAG_FORCE_ACCESS);  // FUA write
  }
}

TEST_F(ServerTest, PostflushWithLargeGroupedTransaction) {
  // Restrict max_transfer_size so that the server has to split up our request.
  block_info_t block_info = {
      .block_count = kBlockCount, .block_size = kBlockSize, .max_transfer_size = kBlockSize};
  CreateServer(block_info);
  AttachVmo(/*do_fill=*/true);

  block_fifo_request_t req = {
      .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = BLOCK_IO_FLAG_FORCE_ACCESS},
      .reqid = 0x100,
      .group = 0,
      .vmoid = vmoid_,
      .length = 5,
      .vmo_offset = 0,
      .dev_offset = 0,
  };
  RequestOneAndWaitResponse(req, ZX_OK);

  // If the device has a volatile write cache but FUA command is not supported, the post flush
  // commands are delivered.
  auto commands = blkdev_.GetCommandSequence();
  ASSERT_EQ(commands.size(), 6);
  for (int i = 0; i < 5; ++i) {
    ASSERT_EQ(commands[i].opcode, BLOCK_OPCODE_WRITE);
    ASSERT_EQ(commands[i].flags, 0);  // FUA flag is removed
  }
  ASSERT_EQ(commands[5].opcode, BLOCK_OPCODE_FLUSH);  // Post flush
  ASSERT_EQ(commands[5].flags, 0);
}

TEST_F(ServerTest, PostflushWithLargeGroupedTransactionException) {
  // Restrict max_transfer_size so that the server has to split up our request.
  block_info_t block_info = {
      .block_count = kBlockCount, .block_size = kBlockSize, .max_transfer_size = kBlockSize};
  CreateServer(block_info);
  AttachVmo(/*do_fill=*/true);

  block_fifo_request_t req = {
      .command = {.opcode = BLOCK_OPCODE_WRITE,
                  .flags = BLOCK_IO_FLAG_FORCE_ACCESS},  // Write flush and FUA
      .reqid = 0x100,
      .group = 0,
      .vmoid = vmoid_,
      .length = 5,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  // If the device has a volatile write cache but FUA command is not supported, the post flush
  // commands are delivered.
  // (I/O Sequence = Write -> Post flush)
  auto& commands = blkdev_.GetCommandSequence();
  {
    // I/O error occurs on write
    blkdev_.set_callback([&](const block_op_t& block_op) {
      if (commands.size() == 1 && block_op.command.opcode == BLOCK_OPCODE_WRITE) {
        return ZX_ERR_IO;
      }
      return ZX_OK;
    });
    RequestOneAndWaitResponse(req, ZX_ERR_IO);
    ASSERT_EQ(commands.size(), 5);  // Error is reported after write transfered
    for (int i = 0; i < 5; ++i) {
      ASSERT_EQ(commands[i].opcode, BLOCK_OPCODE_WRITE);
      ASSERT_EQ(commands[i].flags, 0);  // FUA flag is removed
    }
    commands.clear();
  }
  {
    // I/O error occurs on postflush
    blkdev_.set_callback([&](const block_op_t& block_op) {
      if (commands.size() == 6 && block_op.command.opcode == BLOCK_OPCODE_FLUSH) {
        return ZX_ERR_IO;
      }
      return ZX_OK;
    });
    RequestOneAndWaitResponse(req, ZX_ERR_IO);
    ASSERT_EQ(commands.size(), 6);  // Error is reported after postflush transfered
    for (int i = 0; i < 5; ++i) {
      ASSERT_EQ(commands[i].opcode, BLOCK_OPCODE_WRITE);
      ASSERT_EQ(commands[i].flags, 0);  // FUA flag is removed
    }
    ASSERT_EQ(commands[5].opcode, BLOCK_OPCODE_FLUSH);  // Post flush
    ASSERT_EQ(commands[5].flags, 0);
    commands.clear();
  }
}

TEST_F(ServerTest, PostflushMustBeIssuedOnlyAfterGroupLast) {
  // Restrict max_transfer_size so that the server has to split up our request.
  block_info_t block_info = {
      .block_count = kBlockCount, .block_size = kBlockSize, .max_transfer_size = kBlockSize};
  CreateServer(block_info);
  AttachVmo(/*do_fill=*/true);

  block_fifo_request_t reqs[2] = {
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE,
                      .flags = BLOCK_IO_FLAG_GROUP_ITEM |
                               BLOCK_IO_FLAG_FORCE_ACCESS},  // FUA flag must be ignored
          .reqid = 0x101,
          .group = 0,
          .vmoid = vmoid_,
          .length = 4,
          .vmo_offset = 0,
          .dev_offset = 0,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE,
                      .flags = BLOCK_IO_FLAG_GROUP_ITEM | BLOCK_IO_FLAG_GROUP_LAST |
                               BLOCK_IO_FLAG_FORCE_ACCESS},
          .reqid = 0x101,
          .group = 0,
          .vmoid = vmoid_,
          .length = 1,
          .vmo_offset = 0,
          .dev_offset = 0,
      },
  };
  RequestOne(reqs[0]);
  RequestOneAndWaitResponse(reqs[1], ZX_OK, /*expected_response_count=*/2);

  // If the device has a volatile write cache but FUA command is not supported, the post flush
  // commands are delivered.
  auto commands = blkdev_.GetCommandSequence();
  ASSERT_EQ(commands.size(), 6);
  for (int i = 0; i < 4; ++i) {
    ASSERT_EQ(commands[i].opcode, BLOCK_OPCODE_WRITE);
    ASSERT_EQ(commands[i].flags, 0);  // FUA flag is ignored
  }
  ASSERT_EQ(commands[4].opcode, BLOCK_OPCODE_WRITE);
  ASSERT_EQ(commands[4].flags, 0);  // BLOCK_IO_FLAG_GROUP_LAST, FUA flag is removed
  ASSERT_EQ(commands[5].opcode, BLOCK_OPCODE_FLUSH);  // Post flush
  ASSERT_EQ(commands[5].flags, 0);
}

}  // namespace
