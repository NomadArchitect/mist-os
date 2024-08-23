// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/f2fs.h"

namespace f2fs {

Writer::Writer(BcacheMapper *bcache_mapper, size_t capacity) : bcache_mapper_(bcache_mapper) {
  write_buffer_ =
      std::make_unique<StorageBuffer>(bcache_mapper, capacity, kBlockSize, "WriteBuffer");
}

Writer::~Writer() {
  sync_completion_t completion;
  ScheduleWriteBlocks(&completion);
  ZX_ASSERT(sync_completion_wait(&completion, ZX_TIME_INFINITE) == ZX_OK);
  executor_.Terminate();
  writeback_executor_.Terminate();
}

StorageOperations Writer::MakeStorageOperations(PageList &to_submit) {
  {
    std::lock_guard lock(mutex_);
    while (!pages_.is_empty()) {
      auto page = pages_.pop_front();
      auto num_pages_or = write_buffer_->ReserveWriteOperation(*page);
      if (num_pages_or.is_error()) {
        if (num_pages_or.status_value() == ZX_ERR_UNAVAILABLE) {
          // No available buffers. Need to submit pending StorageOperations to free buffers.
          pages_.push_front(std::move(page));
          break;
        }
        // If |page| has an invalid addr, just drop it.
        to_submit.push_back(std::move(page));
      } else {
        to_submit.push_back(std::move(page));
        if (num_pages_or.value() >= kDefaultBlocksPerSegment) {
          // Merged enough StorageOperations. Submit it.
          break;
        }
      }
    }
  }
  return write_buffer_->TakeWriteOperations();
}

fpromise::promise<> Writer::GetTaskForWriteIO(sync_completion_t *completion) {
  return fpromise::make_promise([this, completion]() mutable {
    while (true) {
      PageList pages;
      auto operations = MakeStorageOperations(pages);
      if (operations.IsEmpty()) {
        break;
      }
      zx_status_t io_status = bcache_mapper_->RunRequests(operations.TakeOperations());
      if (auto ret = operations.Completion(
              io_status,
              [pages = std::move(pages)](const StorageOperations &operation,
                                         zx_status_t io_status) mutable {
                NotifyWriteback notifier;
                while (!pages.is_empty()) {
                  auto page = pages.pop_front();
                  // The instance of a Vnode is alive when it has any writeback pages.
                  if (io_status != ZX_OK) {
                    LockedPage locked_page(page);
                    // It is safe to get |page| locked since waiters do not acquire the lock.
                    if (locked_page->GetVnode().IsMeta() || io_status == ZX_ERR_UNAVAILABLE ||
                        io_status == ZX_ERR_PEER_CLOSED) {
                      // When it fails to write metadata or the block device is not available,
                      // set kCpErrorFlag to enter read-only mode.
                      locked_page->fs()->GetSuperblockInfo().SetCpFlags(CpFlag::kCpErrorFlag);
                    } else {
                      // When IO errors occur with node and data Pages, just set a dirty flag
                      // to retry it with another LBA.
                      locked_page.SetDirty();
                    }
                  }
                  if (page->GetVnode().IsNode()) {
                    fbl::RefPtr<NodePage>::Downcast(page)->SetFsyncMark(false);
                  }
                  page->ClearColdData();
                  notifier.ReserveNotify(std::move(page));
                }
              });
          ret != ZX_OK) {
        FX_LOGS(WARNING) << "failed to write blocks. " << zx_status_get_string(ret);
      }
    }
    if (completion) {
      sync_completion_signal(completion);
    }
    return fpromise::ok();
  });
}

void Writer::ScheduleTask(fpromise::promise<> task) {
  executor_.schedule_task(sequencer_.wrap(std::move(task)));
}

void Writer::ScheduleWriteback(fpromise::promise<> task) {
  writeback_executor_.schedule_task(std::move(task));
}

void Writer::ScheduleWriteBlocks(sync_completion_t *completion, PageList pages, bool flush) {
  if (!pages.is_empty()) {
    std::lock_guard lock(mutex_);
    pages_.splice(pages_.end(), pages);
  }
  if (flush || completion) {
    auto task = GetTaskForWriteIO(completion);
    ScheduleTask(std::move(task));
  }
}

void NotifyWriteback::ReserveNotify(fbl::RefPtr<Page> page) {
  VnodeF2fs &vnode = page->GetVnode();
  if (!waiters_.is_empty() && waiters_.front().GetVnode().GetKey() == vnode.GetKey()) {
    waiters_.push_back(std::move(page));
    Notify();
  } else {
    Notify(1);
    waiters_.push_back(std::move(page));
  }
}

void NotifyWriteback::Notify(size_t interval) {
  if (waiters_.size() < interval || waiters_.is_empty()) {
    return;
  }
  auto &page = waiters_.front();
  page.GetFileCache().NotifyWriteback(std::move(waiters_));
}

}  // namespace f2fs
