// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_PHYSICAL_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_PHYSICAL_H_

#include <lib/user_copy/user_ptr.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <stdint.h>
#include <zircon/listnode.h>
#include <zircon/types.h>

#include <fbl/array.h>
#include <fbl/canary.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/macros.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <kernel/mutex.h>
#include <vm/vm.h>
#include <vm/vm_object.h>

// VMO representing a physical range of memory
class VmObjectPhysical final : public VmObject, public VmDeferredDeleter<VmObjectPhysical> {
 public:
  static zx_status_t Create(paddr_t base, uint64_t size, fbl::RefPtr<VmObjectPhysical>* vmo);

  zx_status_t CreateChildSlice(uint64_t offset, uint64_t size, bool copy_name,
                               fbl::RefPtr<VmObject>* child_vmo) override
      // This function reaches into the created child, which confuses analysis.
      TA_NO_THREAD_SAFETY_ANALYSIS;

  ChildType child_type() const override {
    return is_slice() ? ChildType::kSlice : ChildType::kNotChild;
  }
  bool is_contiguous() const override { return true; }
  bool is_slice() const { return is_slice_; }
  uint64_t parent_user_id() const override { return parent_user_id_; }

  uint64_t size_locked() const override { return size_; }

  zx_status_t Lookup(uint64_t offset, uint64_t len, LookupFunction lookup_fn) override;
  zx_status_t LookupContiguous(uint64_t offset, uint64_t len, paddr_t* out_paddr) override;
  zx_status_t LookupContiguousLocked(uint64_t offset, uint64_t len, paddr_t* out_paddr)
      TA_REQ(lock());

  zx_status_t CommitRangePinned(uint64_t offset, uint64_t len, bool write) override;
  zx_status_t PrefetchRange(uint64_t offset, uint64_t len) override;

  void Unpin(uint64_t offset, uint64_t len) override {
    // Unpin is a no-op for physical VMOs as they are always pinned.
  }

  void SetUserContentSize(fbl::RefPtr<ContentSizeManager> csm) override {
    // Physical VMOs have no operations that can be told to use the user content size, so can safely
    // just ignore this request.
  }

  // Physical VMOs are implicitly pinned.
  bool DebugIsRangePinned(uint64_t offset, uint64_t len) override { return true; }

  void Dump(uint depth, bool verbose) override;

  zx_status_t GetPage(uint64_t offset, uint pf_flags, list_node* alloc_list,
                      MultiPageRequest* page_request, vm_page_t** page, paddr_t* pa) override {
    return ZX_ERR_NOT_SUPPORTED;
  }

  uint32_t GetMappingCachePolicyLocked() const override TA_REQ(lock());
  zx_status_t SetMappingCachePolicy(const uint32_t cache_policy) override;

  void MaybeDeadTransition() {}

 private:
  // private constructor (use Create())
  VmObjectPhysical(fbl::RefPtr<VmHierarchyState> state, paddr_t base, uint64_t size, bool is_slice_,
                   uint64_t parent_user_id);

  // private destructor, only called from refptr
  ~VmObjectPhysical() override;
  friend fbl::RefPtr<VmObjectPhysical>;

  DISALLOW_COPY_ASSIGN_AND_MOVE(VmObjectPhysical);

  // members
  const uint64_t size_ = 0;
  const paddr_t base_ = 0;
  const bool is_slice_ = false;
  const uint64_t parent_user_id_;
  uint32_t mapping_cache_flags_ TA_GUARDED(lock()) = 0;

  // parent pointer (may be null)
  fbl::RefPtr<VmObjectPhysical> parent_ TA_GUARDED(lock()) = nullptr;
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_PHYSICAL_H_
