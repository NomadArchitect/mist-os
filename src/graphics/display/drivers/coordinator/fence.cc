// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/fence.h"

#include <lib/async/cpp/wait.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/fit/thread_checker.h>
#include <lib/trace/event.h>
#include <lib/zx/event.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <thread>
#include <utility>

#include <fbl/alloc_checker.h>
#include <fbl/auto_lock.h>
#include <fbl/intrusive_single_list.h>
#include <fbl/ref_ptr.h>

#include "src/graphics/display/lib/api-types/cpp/event-id.h"

namespace display_coordinator {

bool Fence::CreateRef() {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent() == fence_creation_dispatcher_);

  fbl::AllocChecker ac;
  cur_ref_ = fbl::AdoptRef(
      new (&ac) FenceReference(fbl::RefPtr<Fence>(this), fence_creation_dispatcher_->borrow()));
  if (ac.check()) {
    ref_count_++;
  }

  return ac.check();
}

void Fence::ClearRef() {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent() == fence_creation_dispatcher_);
  cur_ref_ = nullptr;
}

fbl::RefPtr<FenceReference> Fence::GetReference() { return cur_ref_; }

void Fence::Signal() const { event_.signal(0, ZX_EVENT_SIGNALED); }

bool Fence::OnRefDead() { return --ref_count_ == 0; }

zx_status_t Fence::OnRefArmed(fbl::RefPtr<FenceReference>&& ref) {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent() == fence_creation_dispatcher_);

  if (armed_refs_.is_empty()) {
    ready_wait_.set_object(event_.get());
    ready_wait_.set_trigger(ZX_EVENT_SIGNALED);

    zx_status_t status = ready_wait_.Begin(&event_dispatcher_);
    if (status != ZX_OK) {
      return status;
    }
  }

  armed_refs_.push_back(std::move(ref));
  return ZX_OK;
}

void Fence::OnRefDisarmed(FenceReference* ref) {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent() == fence_creation_dispatcher_);

  armed_refs_.erase(*ref);
  if (armed_refs_.is_empty()) {
    ready_wait_.Cancel();
  }
}

void Fence::OnReady(async_dispatcher_t* dispatcher, async::WaitBase* self, zx_status_t status,
                    const zx_packet_signal_t* signal) {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent() == fence_creation_dispatcher_);
  ZX_DEBUG_ASSERT_MSG(status == ZX_OK, "Fence::OnReady failed: %s", zx_status_get_string(status));
  ZX_DEBUG_ASSERT(signal->observed & ZX_EVENT_SIGNALED);
  TRACE_DURATION("gfx", "Display::Fence::OnReady");
  TRACE_FLOW_END("gfx", "event_signal", koid_);

  event_.signal(ZX_EVENT_SIGNALED, 0);

  fbl::RefPtr<FenceReference> ref = armed_refs_.pop_front();
  cb_->OnFenceFired(ref.get());

  if (!armed_refs_.is_empty()) {
    ready_wait_.Begin(&event_dispatcher_);
  }
}

Fence::Fence(FenceCallback* cb, async_dispatcher_t* event_dispatcher, display::EventId fence_id,
             zx::event event)
    : cb_(cb),
      event_dispatcher_(*event_dispatcher),
      fence_creation_dispatcher_(fdf::Dispatcher::GetCurrent()),
      event_(std::move(event)) {
  ZX_DEBUG_ASSERT(event_dispatcher != nullptr);
  ZX_DEBUG_ASSERT(fence_creation_dispatcher_->get() != nullptr);

  id = fence_id;
  ZX_DEBUG_ASSERT(event_.is_valid());
  zx_info_handle_basic_t info;
  zx_status_t status = event_.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  ZX_DEBUG_ASSERT(status == ZX_OK);
  koid_ = info.koid;
}

Fence::~Fence() {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent() == fence_creation_dispatcher_);
  ZX_DEBUG_ASSERT(armed_refs_.is_empty());
  ZX_DEBUG_ASSERT(ref_count_ == 0);
}

zx_status_t FenceReference::StartReadyWait() {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent() == fence_creation_dispatcher_);
  return fence_->OnRefArmed(fbl::RefPtr<FenceReference>(this));
}

void FenceReference::ResetReadyWait() {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent() == fence_creation_dispatcher_);
  fence_->OnRefDisarmed(this);
}

void FenceReference::Signal() const { fence_->Signal(); }

FenceReference::FenceReference(fbl::RefPtr<Fence> fence,
                               fdf::UnownedDispatcher fence_creation_dispatcher)
    : fence_(std::move(fence)), fence_creation_dispatcher_(std::move(fence_creation_dispatcher)) {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent() == fence_creation_dispatcher_);
}

FenceReference::~FenceReference() {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent() == fence_creation_dispatcher_);
  fence_->cb_->OnRefForFenceDead(fence_.get());
}

FenceCollection::FenceCollection(async_dispatcher_t* dispatcher,
                                 fit::function<void(FenceReference*)> on_fence_fired)
    : dispatcher_(dispatcher), on_fence_fired_(std::move(on_fence_fired)) {
  ZX_DEBUG_ASSERT(dispatcher != nullptr);
  ZX_DEBUG_ASSERT(on_fence_fired_);
}

void FenceCollection::Clear() {
  // Use a temporary list to prevent double locking when resetting.
  fbl::SinglyLinkedList<fbl::RefPtr<Fence>> fences;
  {
    fbl::AutoLock lock(&mtx_);
    while (!fences_.is_empty()) {
      fences.push_front(fences_.erase(fences_.begin()));
    }
  }
  while (!fences.is_empty()) {
    fences.pop_front()->ClearRef();
  }
}

zx_status_t FenceCollection::ImportEvent(zx::event event, display::EventId id) {
  fbl::AutoLock lock(&mtx_);
  Fence::Map::iterator fence = fences_.find(id);
  if (fence.IsValid()) {
    FDF_LOG(ERROR, "Illegal to import an event with existing ID#%ld", id.value());
    return ZX_ERR_ALREADY_EXISTS;
  }

  fbl::AllocChecker ac;
  auto new_fence = fbl::AdoptRef(new (&ac) Fence(this, dispatcher_, id, std::move(event)));
  if (ac.check() && new_fence->CreateRef()) {
    fences_.insert_or_find(std::move(new_fence));
  } else {
    FDF_LOG(ERROR, "Failed to allocate fence ref for event#%ld", id.value());
    return ZX_ERR_NO_MEMORY;
  }
  return ZX_OK;
}

void FenceCollection::ReleaseEvent(display::EventId id) {
  // Hold a ref to prevent double locking if this destroys the fence.
  auto fence_ref = GetFence(id);
  if (fence_ref) {
    fbl::AutoLock lock(&mtx_);
    fences_.find(id)->ClearRef();
  }
}

fbl::RefPtr<FenceReference> FenceCollection::GetFence(display::EventId id) {
  if (id == display::kInvalidEventId) {
    return nullptr;
  }
  fbl::AutoLock l(&mtx_);
  auto fence = fences_.find(id);
  return fence.IsValid() ? fence->GetReference() : nullptr;
}

void FenceCollection::OnFenceFired(FenceReference* fence) { on_fence_fired_(fence); }

void FenceCollection::OnRefForFenceDead(Fence* fence) {
  fbl::AutoLock lock(&mtx_);
  if (fence->OnRefDead()) {
    fences_.erase(fence->id);
  }
}

}  // namespace display_coordinator
