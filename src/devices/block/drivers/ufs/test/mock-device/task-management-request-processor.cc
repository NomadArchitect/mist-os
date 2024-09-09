// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/drivers/ufs/test/mock-device/task-management-request-processor.h"

#include <lib/ddk/debug.h>

#include "safemath/safe_conversions.h"
#include "ufs-mock-device.h"

namespace ufs {
namespace ufs_mock_device {

void TaskManagementRequestProcessor::HandleTaskManagementRequest(
    TaskManagementRequestDescriptor &descriptor) {
  UpiuHeader *request_upiu_header =
      reinterpret_cast<UpiuHeader *>(&descriptor.GetRequestData()->header);

  UpiuHeader *response_upiu_header =
      reinterpret_cast<UpiuHeader *>(&descriptor.GetResponseData()->header);
  std::memcpy(response_upiu_header, request_upiu_header, sizeof(UpiuHeader));
  response_upiu_header->set_trans_code(request_upiu_header->trans_code() | (1 << 5));

  TaskManagementFunction function =
      static_cast<TaskManagementFunction>(request_upiu_header->function);
  zx_status_t status = ZX_OK;
  if (auto it = handlers_.find(function); it != handlers_.end()) {
    status = (it->second)(mock_device_, descriptor);
  } else {
    status = ZX_ERR_NOT_SUPPORTED;
    FDF_LOG(ERROR, "UFS MOCK: task management function: 0x%x is not supported",
            static_cast<uint8_t>(function));
  }

  if (status == ZX_OK) {
    descriptor.set_overall_command_status(OverallCommandStatus::kSuccess);
  } else {
    FDF_LOG(ERROR, "UFS MOCK: Failed to handle task management request: %s",
            zx_status_get_string(status));
    descriptor.set_overall_command_status(OverallCommandStatus::kInvalid);
  }

  if ((descriptor.overall_command_status() == OverallCommandStatus::kSuccess &&
       descriptor.interrupt()) ||
      descriptor.overall_command_status() != OverallCommandStatus::kSuccess) {
    InterruptStatusReg::Get()
        .ReadFrom(mock_device_.GetRegisters())
        .set_utp_task_management_request_completion_status(true)
        .WriteTo(mock_device_.GetRegisters());
    if (InterruptEnableReg::Get()
            .ReadFrom(mock_device_.GetRegisters())
            .utp_task_management_request_completion_enable()) {
      mock_device_.TriggerInterrupt();
    }
  }
}

zx_status_t TaskManagementRequestProcessor::DefaultAbortTaskHandler(
    UfsMockDevice &mock_device, TaskManagementRequestDescriptor &descriptor) {
  TaskManagementRequestUpiuData *request_upiu = descriptor.GetRequestData();
  TaskManagementResponseUpiuData *response_upiu = descriptor.GetResponseData();

  uint8_t lun = safemath::checked_cast<uint8_t>(request_upiu->input_param1);
  if (mock_device.GetLogicalUnit(lun).GetUnitDesc().bLUEnable == 0) {
    return ZX_ERR_UNAVAILABLE;
  }

  // It is difficult to implement task management on the mock device, so it does nothing.

  response_upiu->output_param1 =
      static_cast<uint32_t>(TaskManagementServiceResponse::kTaskManagementFunctionSucceeded);
  response_upiu->output_param2 = 0;
  return ZX_OK;
}

zx_status_t TaskManagementRequestProcessor::DefaultQueryTaskHandler(
    UfsMockDevice &mock_device, TaskManagementRequestDescriptor &descriptor) {
  TaskManagementRequestUpiuData *request_upiu = descriptor.GetRequestData();
  TaskManagementResponseUpiuData *response_upiu = descriptor.GetResponseData();

  uint8_t lun = safemath::checked_cast<uint8_t>(request_upiu->input_param1);
  if (mock_device.GetLogicalUnit(lun).GetUnitDesc().bLUEnable == 0) {
    return ZX_ERR_UNAVAILABLE;
  }

  // It is difficult to implement task management on the mock device, so it does nothing.

  response_upiu->output_param1 =
      static_cast<uint32_t>(TaskManagementServiceResponse::kTaskManagementFunctionSucceeded);
  response_upiu->output_param2 = 0;
  return ZX_OK;
}

zx_status_t TaskManagementRequestProcessor::DefaultLogicalUnitResetHandler(
    UfsMockDevice &mock_device, TaskManagementRequestDescriptor &descriptor) {
  TaskManagementRequestUpiuData *request_upiu = descriptor.GetRequestData();
  TaskManagementResponseUpiuData *response_upiu = descriptor.GetResponseData();

  uint8_t lun = safemath::checked_cast<uint8_t>(request_upiu->input_param1);
  if (mock_device.GetLogicalUnit(lun).GetUnitDesc().bLUEnable == 0) {
    return ZX_ERR_UNAVAILABLE;
  }

  // It is difficult to implement task management on the mock device, so it does nothing.

  response_upiu->output_param1 =
      static_cast<uint32_t>(TaskManagementServiceResponse::kTaskManagementFunctionSucceeded);
  response_upiu->output_param2 = 0;
  return ZX_OK;
}

}  // namespace ufs_mock_device
}  // namespace ufs
