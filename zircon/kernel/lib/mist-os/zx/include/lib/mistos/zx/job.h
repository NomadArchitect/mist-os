// Copyright 2024 Mist Tecnologia LTDA. All rights reserved.
// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_MIST_OS_ZX_INCLUDE_LIB_MISTOS_ZX_JOB_H_
#define ZIRCON_KERNEL_LIB_MIST_OS_ZX_INCLUDE_LIB_MISTOS_ZX_JOB_H_

#include <lib/mistos/util/process.h>
#include <lib/mistos/zx/process.h>
#include <lib/mistos/zx/task.h>
#include <zircon/errors.h>

#include <object/job_dispatcher.h>

namespace zx {

class process;

class job final : public task<job> {
 public:
  static constexpr zx_obj_type_t TYPE = ZX_OBJ_TYPE_JOB;

  constexpr job() = default;

  explicit job(fbl::RefPtr<JobDispatcher> value) : task(value) {}

  job(job&& other) : task(other.release()) {}

  job& operator=(job&& other) {
    reset(other.release());
    return *this;
  }

  static zx_status_t create(const zx::job& parent, uint32_t options, job* result);

  // Provide strongly-typed overloads, in addition to get_child(handle*).
  using task<job>::get_child;
  zx_status_t get_child(uint64_t koid, zx_rights_t rights, job* result) const {
    // Allow for |result| and |this| aliasing the same container.
    job h;
    // zx_status_t status = zx_object_get_child(value_, koid, rights, h.reset_and_get_address());
    result->reset(h.release());
    return ZX_OK;
  }
  zx_status_t get_child(uint64_t koid, zx_rights_t rights, process* result) const;

  zx_status_t set_policy(uint32_t options, uint32_t topic, const void* policy,
                         uint32_t count) const {
    // return zx_job_set_policy(get(), options, topic, policy, count);
    return ZX_ERR_NOT_SUPPORTED;
  }

  zx_status_t set_critical(uint32_t options, const zx::process& process) const {
    // return zx_job_set_critical(get(), options, process.get());
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Ideally this would be called zx::job::default(), but default is a
  // C++ keyword and cannot be used as a function name.
  static inline unowned<job> default_job() { return unowned<job>(zx_job_default()); }
};

using unowned_job = unowned<job>;

}  // namespace zx

#endif  // ZIRCON_KERNEL_LIB_MIST_OS_ZX_INCLUDE_LIB_MISTOS_ZX_JOB_H_
