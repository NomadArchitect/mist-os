// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "unit-lib.h"
#include "zxtest/zxtest.h"

namespace ufs {
using namespace ufs_mock_device;

class PowerTest : public UfsTest {
 public:
  void SetUp() override { InitMockDevice(); }
};

TEST_F(PowerTest, PowerSuspendResume) {
  libsync::Completion sleep_complete;
  libsync::Completion awake_complete;
  mock_device_.GetUicCmdProcessor().SetHook(
      UicCommandOpcode::kDmeHibernateEnter,
      [&](UfsMockDevice& mock_device, uint32_t ucmdarg1, uint32_t ucmdarg2, uint32_t ucmdarg3) {
        mock_device_.GetUicCmdProcessor().DefaultDmeHibernateEnterHandler(mock_device, ucmdarg1,
                                                                          ucmdarg2, ucmdarg3);
        sleep_complete.Signal();
      });
  mock_device_.GetUicCmdProcessor().SetHook(
      UicCommandOpcode::kDmeHibernateExit,
      [&](UfsMockDevice& mock_device, uint32_t ucmdarg1, uint32_t ucmdarg2, uint32_t ucmdarg3) {
        mock_device_.GetUicCmdProcessor().DefaultDmeHibernateExitHandler(mock_device, ucmdarg1,
                                                                         ucmdarg2, ucmdarg3);
        awake_complete.Signal();
      });

  ASSERT_NO_FATAL_FAILURE(StartDriver(/*supply_power_framework=*/true));

  scsi::BlockDevice* block_device;
  block_info_t info;
  uint64_t op_size;
  const auto& block_devs = dut_->block_devs();
  block_device = block_devs.at(0).at(0).get();
  block_device->BlockImplQuery(&info, &op_size);

  // 1. Initial power level is kPowerLevelOff.
  runtime_.PerformBlockingWork([&] { sleep_complete.Wait(); });

  // TODO(https://fxbug.dev/42075643): Check if suspend is enabled with inspect
  ASSERT_TRUE(!dut_->IsResumed());
  UfsPowerMode power_mode = UfsPowerMode::kSleep;
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerMode(), power_mode);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerCondition(),
            dut_->GetDeviceManager().GetPowerModeMap()[power_mode].first);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentLinkState(),
            dut_->GetDeviceManager().GetPowerModeMap()[power_mode].second);

  // 2. Issue request while power is suspended.
  awake_complete.Reset();
  sleep_complete.Reset();

  sync_completion_t done;
  auto callback = [](void* ctx, zx_status_t status, block_op_t* op) {
    EXPECT_OK(status);
    sync_completion_signal(static_cast<sync_completion_t*>(ctx));
  };

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmo));
  zx_vaddr_t vaddr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                       ufs_mock_device::kMockBlockSize, &vaddr));
  char* mapped_vaddr = reinterpret_cast<char*>(vaddr);
  std::strncpy(mapped_vaddr, "test", ufs_mock_device::kMockBlockSize);

  auto block_op = std::make_unique<uint8_t[]>(op_size);
  auto op = reinterpret_cast<block_op_t*>(block_op.get());
  *op = {
      .rw =
          {
              .command =
                  {
                      .opcode = BLOCK_OPCODE_WRITE,
                  },
              .vmo = vmo.get(),
              .length = 1,
              .offset_dev = 0,
              .offset_vmo = 0,
          },
  };
  block_device->BlockImplQueue(op, callback, &done);
  runtime_.PerformBlockingWork([&] { awake_complete.Wait(); });
  sync_completion_wait(&done, ZX_TIME_INFINITE);

  // Return the driver to the suspended state.
  incoming_.SyncCall([](IncomingNamespace* incoming) {
    incoming->power_broker.hardware_power_required_level_->required_level_ = Ufs::kPowerLevelOff;
  });
  runtime_.PerformBlockingWork([&] { sleep_complete.Wait(); });

  // TODO(https://fxbug.dev/42075643): Check if suspend is enabled with inspect
  ASSERT_TRUE(!dut_->IsResumed());
  power_mode = UfsPowerMode::kSleep;
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerMode(), power_mode);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerCondition(),
            dut_->GetDeviceManager().GetPowerModeMap()[power_mode].first);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentLinkState(),
            dut_->GetDeviceManager().GetPowerModeMap()[power_mode].second);

  // 3. Trigger power level change to kPowerLevelOn.
  awake_complete.Reset();
  incoming_.SyncCall([](IncomingNamespace* incoming) {
    incoming->power_broker.hardware_power_required_level_->required_level_ = Ufs::kPowerLevelOn;
  });
  runtime_.PerformBlockingWork([&] { awake_complete.Wait(); });

  // TODO(https://fxbug.dev/42075643): Check if suspend is enabled with inspect
  ASSERT_FALSE(!dut_->IsResumed());
  power_mode = UfsPowerMode::kActive;
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerMode(), power_mode);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerCondition(),
            dut_->GetDeviceManager().GetPowerModeMap()[power_mode].first);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentLinkState(),
            dut_->GetDeviceManager().GetPowerModeMap()[power_mode].second);

  // 4. Trigger power level change to kPowerLevelOff.
  sleep_complete.Reset();
  incoming_.SyncCall([](IncomingNamespace* incoming) {
    incoming->power_broker.hardware_power_required_level_->required_level_ = Ufs::kPowerLevelOff;
  });
  runtime_.PerformBlockingWork([&] { sleep_complete.Wait(); });

  // TODO(https://fxbug.dev/42075643): Check if suspend is enabled with inspect
  ASSERT_TRUE(!dut_->IsResumed());
  power_mode = UfsPowerMode::kSleep;
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerMode(), power_mode);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerCondition(),
            dut_->GetDeviceManager().GetPowerModeMap()[power_mode].first);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentLinkState(),
            dut_->GetDeviceManager().GetPowerModeMap()[power_mode].second);
}

}  // namespace ufs
