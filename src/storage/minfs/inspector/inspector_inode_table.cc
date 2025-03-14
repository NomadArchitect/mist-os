// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/minfs/inspector/inspector_inode_table.h"

#include <zircon/assert.h>

#include <cstddef>
#include <cstdint>
#include <memory>

#include <safemath/safe_conversions.h>

#include "src/storage/lib/disk_inspector/disk_inspector.h"
#include "src/storage/minfs/format.h"
#include "src/storage/minfs/inspector/inspector_inode.h"

namespace minfs {

void InodeTableObject::GetValue(const void** out_buffer, size_t* out_buffer_size) const {
  ZX_DEBUG_ASSERT_MSG(false, "Invalid GetValue call for non primitive data type.");
}

std::unique_ptr<disk_inspector::DiskObject> InodeTableObject::GetElementAt(uint32_t index) const {
  if (index >= allocated_inode_count_) {
    return nullptr;
  }
  return GetInode(index);
}

std::unique_ptr<disk_inspector::DiskObject> InodeTableObject::GetInode(
    uint32_t element_index) const {
  Inode inode_obj;
  auto inode_index = safemath::strict_cast<ino_t>(allocated_inode_indices[element_index]);
  inode_table_->Load(inode_index, &inode_obj);
  return std::make_unique<InodeObject>(element_index, inode_index, inode_obj);
}

void InodeTableObject::SetupAllocatedInodeIndex() {
  for (uint32_t i = 0; i < inode_count_; ++i) {
    if (inode_table_->CheckAllocated(i))
      allocated_inode_indices.push_back(i);
  }
}

}  // namespace minfs
