// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/iterator/block_iterator.h"

#include <lib/syslog/cpp/macros.h>
#include <stdint.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <algorithm>
#include <memory>
#include <utility>

#include "src/storage/blobfs/iterator/extent_iterator.h"

namespace blobfs {

BlockIterator::BlockIterator(std::unique_ptr<ExtentIterator> iterator)
    : iterator_(std::move(iterator)) {}

bool BlockIterator::Done() const { return blocks_left_ == 0 && iterator_->Done(); }

uint64_t BlockIterator::BlockIndex() const { return iterator_->BlockIndex() - blocks_left_; }

zx_status_t BlockIterator::Next(uint64_t length, uint64_t* out_length, uint64_t* out_start) {
  ZX_DEBUG_ASSERT(!Done());

  // If there are no blocks left, refill the extent.
  if (!blocks_left_) {
    auto extent_or = iterator_->Next();
    if (extent_or.is_error()) {
      return extent_or.status_value();
    }
    extent_ = extent_or.value();
    blocks_left_ = extent_->Length();
  }

  // Return as many blocks as possible within this current extent.
  ZX_DEBUG_ASSERT(extent_ != nullptr);
  *out_length = std::min(blocks_left_, length);
  *out_start = (extent_->Start() + extent_->Length()) - blocks_left_;
  blocks_left_ -= *out_length;
  return ZX_OK;
}

zx_status_t IterateToBlock(BlockIterator* iter, uint64_t block_num) {
  while (!iter->Done() && block_num > iter->BlockIndex()) {
    uint64_t out_length = 0;
    uint64_t out_start = 0;
    uint64_t blocks_to_iterate_over = block_num - iter->BlockIndex();
    zx_status_t status = iter->Next(blocks_to_iterate_over, &out_length, &out_start);
    if (status != ZX_OK) {
      return status;
    }
  }
  if (iter->Done()) {
    return ZX_ERR_INVALID_ARGS;
  }
  return ZX_OK;
}

zx_status_t StreamBlocks(BlockIterator* iterator, uint64_t block_count, StreamFn stream) {
  while (block_count > 0) {
    if (iterator->Done()) {
      FX_LOGS(ERROR) << "Failed to access data (early exit)";
      return ZX_ERR_IO_DATA_INTEGRITY;
    }
    uint64_t local_offset = iterator->BlockIndex();
    uint64_t actual_length;
    uint64_t dev_offset;
    zx_status_t status = iterator->Next(block_count, &actual_length, &dev_offset);
    if (status != ZX_OK) {
      FX_LOGS(ERROR) << "Failed to iterate over blocks: " << status;
      return status;
    }
    status = stream(local_offset, dev_offset, actual_length);
    if (status != ZX_OK) {
      FX_LOGS(ERROR) << "Failed to enqueue blocks: " << status;
      return status;
    }
    block_count -= actual_length;
  }
  return ZX_OK;
}

}  // namespace blobfs
