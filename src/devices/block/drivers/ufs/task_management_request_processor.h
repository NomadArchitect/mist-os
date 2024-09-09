// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_TASK_MANAGEMENT_REQUEST_PROCESSOR_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_TASK_MANAGEMENT_REQUEST_PROCESSOR_H_

#include <lib/trace/event.h>

#include "request_processor.h"
#include "src/devices/block/drivers/ufs/registers.h"
#include "src/devices/block/drivers/ufs/task_management_request_descriptor.h"
#include "src/devices/block/drivers/ufs/upiu/scsi_commands.h"
#include "src/devices/block/drivers/ufs/upiu/upiu_transactions.h"

namespace ufs {

constexpr uint8_t kMaxTaskManagementRequestListSize = 8;

// Owns and processes the UTP task management request list.
class TaskManagementRequestProcessor : public RequestProcessor {
 public:
  static zx::result<std::unique_ptr<TaskManagementRequestProcessor>> Create(
      Ufs &ufs, zx::unowned_bti bti, const fdf::MmioView mmio, uint8_t entry_count) {
    if (entry_count > kMaxTaskManagementRequestListSize) {
      FDF_LOG(ERROR, "Request list size exceeded the maximum size of %d.",
              kMaxTaskManagementRequestListSize);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    return RequestProcessor::Create<TaskManagementRequestProcessor,
                                    TaskManagementRequestDescriptor>(ufs, std::move(bti), mmio,
                                                                     entry_count);
  }
  explicit TaskManagementRequestProcessor(RequestList request_list, Ufs &ufs, zx::unowned_bti bti,
                                          const fdf::MmioView mmio, uint32_t slot_count)
      : RequestProcessor(std::move(request_list), ufs, std::move(bti), mmio, slot_count) {}
  ~TaskManagementRequestProcessor() override = default;

  zx::result<> Init() override;

  uint32_t IoRequestCompletion() override;

  // |SendTaskManagementRequest| allocates a slot for request UPIU and calls
  // FillDescriptorAndSendRequest.
  zx::result<TaskManagementResponseUpiu> SendTaskManagementRequest(
      TaskManagementRequestUpiu &request);

 private:
  friend class UfsTest;

  zx::result<> FillDescriptorAndSendRequest(uint8_t slot, TaskManagementRequestUpiu &request);

  void SetDoorBellRegister(uint8_t slot_num) override {
    UtmrListDoorBellReg::Get().FromValue(1 << slot_num).WriteTo(&register_);
  }
};

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_TASK_MANAGEMENT_REQUEST_PROCESSOR_H_
