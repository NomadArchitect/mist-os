// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/job_policy.h"

#include <assert.h>
#include <lib/counters.h>
#include <zircon/errors.h>
#include <zircon/syscalls/policy.h>

#include <fbl/algorithm.h>
#include <fbl/bits.h>
#include <kernel/deadline.h>
#include <ktl/iterator.h>

#include <ktl/enforce.h>

namespace {

// It is critical that this array contain all "new object" policies because it's used to implement
// ZX_NEW_ANY.
constexpr uint32_t kNewObjectPolicies[]{
    ZX_POL_NEW_VMO,     ZX_POL_NEW_CHANNEL, ZX_POL_NEW_EVENT, ZX_POL_NEW_EVENTPAIR,
    ZX_POL_NEW_PORT,    ZX_POL_NEW_SOCKET,  ZX_POL_NEW_FIFO,  ZX_POL_NEW_TIMER,
    ZX_POL_NEW_PROCESS, ZX_POL_NEW_PROFILE, ZX_POL_NEW_PAGER, ZX_POL_NEW_IOB,
};
static_assert(
    ktl::size(kNewObjectPolicies) + 5 == ZX_POL_MAX,
    "please update JobPolicy::AddPartial, JobPolicy::QueryBasicPolicy, kNewObjectPolicies,"
    "and the add_basic_policy_deny_any_new() test");

bool PolicyOverrideIsValid(uint32_t override) {
  switch (override) {
    case ZX_POL_OVERRIDE_DENY:
    case ZX_POL_OVERRIDE_ALLOW:
      return true;
    default:
      return false;
  }
}

zx_status_t AddPartial(uint32_t mode, uint32_t condition, uint32_t action, uint32_t override,
                       JobPolicyCollection& bits) {
  if (action >= ZX_POL_ACTION_MAX) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  if (!PolicyOverrideIsValid(override)) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (condition >= ZX_POL_MAX || condition == ZX_POL_NEW_ANY) {
    return ZX_ERR_INVALID_ARGS;
  }

  bool override_bit = override == ZX_POL_OVERRIDE_ALLOW;
  auto condition_bits = bits[condition];
  if (condition_bits.override()) {
    condition_bits.set_action(action);
    condition_bits.set_override(override_bit);
    return ZX_OK;
  }

  if (condition_bits.action() == action && !override_bit) {
    return ZX_OK;
  }

  return (mode == ZX_JOB_POL_ABSOLUTE) ? ZX_ERR_ALREADY_EXISTS : ZX_OK;
}

}  // namespace

JobPolicy::JobPolicy(const JobPolicy& parent)
    : collection_(parent.collection_), slack_(parent.slack_) {}
JobPolicy::JobPolicy(JobPolicyCollection collection, const TimerSlack& slack)
    : collection_(collection), slack_(slack) {}

// static
JobPolicy JobPolicy::CreateRootPolicy() {
  static_assert((ZX_POL_ACTION_ALLOW == 0u) && (ZX_POL_OVERRIDE_ALLOW == 0u));
  return JobPolicy({}, TimerSlack::none());
}

zx_status_t JobPolicy::AddBasicPolicy(uint32_t mode, const zx_policy_basic_v2_t* policy_input,
                                      size_t policy_count) {
  // Don't allow overlong policies.
  if (policy_count > ZX_POL_MAX) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  zx_status_t status = ZX_OK;
  JobPolicyCollection updated_collection = collection_;
  bool has_new_any = false;
  uint32_t new_any_override = 0;

  for (size_t ix = 0; ix != policy_count; ++ix) {
    const auto& in = policy_input[ix];

    if (in.condition == ZX_POL_NEW_ANY) {
      for (auto cond : kNewObjectPolicies) {
        if (status = AddPartial(mode, cond, in.action, ZX_POL_OVERRIDE_ALLOW, updated_collection);
            status != ZX_OK) {
          return status;
        }
      }
      has_new_any = true;
      new_any_override = in.flags;
    } else if (status = AddPartial(mode, in.condition, in.action, in.flags, updated_collection);
               status != ZX_OK) {
      return status;
    }
  }

  if (has_new_any) {
    if (!PolicyOverrideIsValid(new_any_override)) {
      return ZX_ERR_INVALID_ARGS;
    }
    bool override_bit = new_any_override == ZX_POL_OVERRIDE_ALLOW;
    for (auto cond : kNewObjectPolicies) {
      updated_collection[cond].set_override(override_bit);
    }
  }

  collection_ = updated_collection;
  return ZX_OK;
}

uint32_t JobPolicy::QueryBasicPolicy(uint32_t condition) const {
  if (condition >= ZX_POL_MAX || condition == ZX_POL_NEW_ANY) [[unlikely]] {
    return ZX_POL_ACTION_DENY;
  }
  // The following const_cast allows us to reuse the JobPolicyCollection without having to resort to
  // template-ing over const.
  return const_cast<JobPolicy*>(this)->collection_[condition].action();
}

uint32_t JobPolicy::QueryBasicPolicyOverride(uint32_t condition) const {
  if (condition >= ZX_POL_MAX || condition == ZX_POL_NEW_ANY) [[unlikely]] {
    return ZX_POL_OVERRIDE_DENY;
  }
  // The following const_cast allows us to reuse the JobPolicyCollection without having to resort to
  // template-ing over const.
  return const_cast<JobPolicy*>(this)->collection_[condition].override() ? ZX_POL_OVERRIDE_ALLOW
                                                                         : ZX_POL_OVERRIDE_DENY;
}

void JobPolicy::SetTimerSlack(TimerSlack slack) { slack_ = slack; }

TimerSlack JobPolicy::GetTimerSlack() const { return slack_; }

bool JobPolicy::operator==(const JobPolicy& rhs) const {
  if (this == &rhs) {
    return true;
  }

  return collection_ == rhs.collection_ && slack_ == rhs.slack_;
}

bool JobPolicy::operator!=(const JobPolicy& rhs) const { return !operator==(rhs); }

// Evaluates to the name of the kcounter for the given action and condition.
//
// E.g. COUNTER(deny, new_channel) -> policy_action_deny_new_channel_count
#define COUNTER(action, condition) policy_##action##_##condition##_count

// Defines a kcounter for the given action and condition.
#define DEFINE_COUNTER(action, condition) \
  KCOUNTER(COUNTER(action, condition), "policy." #action "." #condition)

// Evaluates to the name of an array of pointers to Counter objects.
//
// See DEFINE_COUNTER_ARRAY for details.
#define COUNTER_ARRAY(action) counters_##action

// Defines kcounters for the given action and creates an array named |COUNTER_ARRAY(action)|.
//
// The array has length ZX_POL_MAX and contains pointers to the Counter objects. The array should be
// indexed by condition. Note, some array elements may be null.
//
// Example:
//
//     DEFINE_COUNTER_ARRAY(deny);
//     kcounter_add(*COUNTER_ARRAY(deny)[ZX_POL_NEW_CHANNEL], 1);
//
#define DEFINE_COUNTER_ARRAY(action)                                            \
  DEFINE_COUNTER(action, bad_handle)                                            \
  DEFINE_COUNTER(action, wrong_object)                                          \
  DEFINE_COUNTER(action, vmar_wx)                                               \
  DEFINE_COUNTER(action, new_vmo)                                               \
  DEFINE_COUNTER(action, new_channel)                                           \
  DEFINE_COUNTER(action, new_event)                                             \
  DEFINE_COUNTER(action, new_eventpair)                                         \
  DEFINE_COUNTER(action, new_port)                                              \
  DEFINE_COUNTER(action, new_socket)                                            \
  DEFINE_COUNTER(action, new_fifo)                                              \
  DEFINE_COUNTER(action, new_timer)                                             \
  DEFINE_COUNTER(action, new_process)                                           \
  DEFINE_COUNTER(action, new_profile)                                           \
  DEFINE_COUNTER(action, new_pager)                                             \
  DEFINE_COUNTER(action, ambient_mark_vmo_exec)                                 \
  DEFINE_COUNTER(action, new_iob)                                               \
  static constexpr const Counter* const COUNTER_ARRAY(action)[] = {             \
      [ZX_POL_BAD_HANDLE] = &COUNTER(action, bad_handle),                       \
      [ZX_POL_WRONG_OBJECT] = &COUNTER(action, wrong_object),                   \
      [ZX_POL_VMAR_WX] = &COUNTER(action, vmar_wx),                             \
      [ZX_POL_NEW_ANY] = nullptr, /* ZX_POL_NEW_ANY is a pseudo condition */    \
      [ZX_POL_NEW_VMO] = &COUNTER(action, new_vmo),                             \
      [ZX_POL_NEW_CHANNEL] = &COUNTER(action, new_channel),                     \
      [ZX_POL_NEW_EVENT] = &COUNTER(action, new_event),                         \
      [ZX_POL_NEW_EVENTPAIR] = &COUNTER(action, new_eventpair),                 \
      [ZX_POL_NEW_PORT] = &COUNTER(action, new_port),                           \
      [ZX_POL_NEW_SOCKET] = &COUNTER(action, new_socket),                       \
      [ZX_POL_NEW_FIFO] = &COUNTER(action, new_fifo),                           \
      [ZX_POL_NEW_TIMER] = &COUNTER(action, new_timer),                         \
      [ZX_POL_NEW_PROCESS] = &COUNTER(action, new_process),                     \
      [ZX_POL_NEW_PROFILE] = &COUNTER(action, new_profile),                     \
      [ZX_POL_NEW_PAGER] = &COUNTER(action, new_pager),                         \
      [ZX_POL_AMBIENT_MARK_VMO_EXEC] = &COUNTER(action, ambient_mark_vmo_exec), \
      [ZX_POL_NEW_IOB] = &COUNTER(action, new_iob),                             \
  };                                                                            \
  static_assert(ktl::size(COUNTER_ARRAY(action)) == ZX_POL_MAX);

#if defined(__clang__)
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wc99-designator"
#endif
// Counts policy violations resulting in ZX_POL_ACTION_DENY or ZX_POL_ACTION_DENY_EXCEPTION.
DEFINE_COUNTER_ARRAY(deny)
// Counts policy violations resulting in ZX_POL_ACTION_KILL.
DEFINE_COUNTER_ARRAY(kill)
#if defined(__clang__)
#pragma GCC diagnostic pop
#endif
static_assert(ZX_POL_ACTION_MAX == 5, "add another instantiation of DEFINE_COUNTER_ARRAY");

void JobPolicy::IncrementCounter(uint32_t action, uint32_t condition) {
  DEBUG_ASSERT(action < ZX_POL_ACTION_MAX);
  DEBUG_ASSERT(condition < ZX_POL_MAX);

  const Counter* counter = nullptr;
  switch (action) {
    case ZX_POL_ACTION_DENY:
    case ZX_POL_ACTION_DENY_EXCEPTION:
      counter = COUNTER_ARRAY(deny)[condition];
      break;
    case ZX_POL_ACTION_KILL:
      counter = COUNTER_ARRAY(kill)[condition];
      break;
  };
  if (!counter) {
    return;
  }
  kcounter_add(*counter, 1);
}

#undef COUNTER
#undef DEFINE_COUNTER
#undef COUNTER_ARRAY
#undef DEFINE_COUNTER_ARRAY
