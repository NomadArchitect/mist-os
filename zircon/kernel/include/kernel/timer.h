// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008-2009 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_TIMER_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_TIMER_H_

#include <lib/kconcurrent/chainlock.h>
#include <string-file.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <fbl/canary.h>
#include <fbl/intrusive_double_list.h>
#include <kernel/deadline.h>
#include <kernel/spinlock.h>
#include <ktl/atomic.h>

// Rules for Timers:
// - Timer callbacks occur from interrupt context.
// - Timers may be programmed or canceled from interrupt or thread context.
// - Timers may be canceled or reprogrammed from within their callback.
// - Setting and canceling timers is not thread safe and cannot be done concurrently.
// - Timer::cancel() may spin waiting for a pending timer to complete on another cpu.

// Timers may be removed from an arbitrary TimerQueue, so their list
// node requires the AllowRemoveFromContainer option.
class Timer : public fbl::DoublyLinkedListable<Timer*, fbl::NodeOptions::AllowRemoveFromContainer> {
 public:
  using Callback = void (*)(Timer*, zx_time_t now, void* arg);

  // Keeps track of which timeline a Timer is operating on.
  enum class ReferenceTimeline : uint8_t {
    kMono,
    kBoot,
  };

  // Timers need a constexpr constructor, as it is valid to construct them in static storage.
  // TODO(https://fxbug.dev/328306129): The default value for the timeline parameter should be
  // removed, thus forcing users of the Timer class to explicitly declare the timeline they wish
  // to use.
  constexpr explicit Timer(ReferenceTimeline timeline = ReferenceTimeline::kMono)
      : timeline_(timeline) {}

  // We ensure that timers are not on a list or an active cpu when destroyed.
  ~Timer();

  // Timers are not moved or copied.
  Timer(const Timer&) = delete;
  Timer(Timer&&) = delete;
  Timer& operator=(const Timer&) = delete;
  Timer& operator=(Timer&&) = delete;

  // Set up a timer that executes once
  //
  // This function specifies a callback function to be run after a specified
  // deadline passes. The function will be called one time.
  //
  // deadline: specifies when the timer should be executed
  // callback: the function to call when the timer expires
  // arg: the argument to pass to the callback
  //
  // The timer function is declared as:
  //   void callback(Timer *, zx_time_t now, void *arg) { ... }
  void Set(const Deadline& deadline, Callback callback, void* arg);

  // Cancel a pending timer
  //
  // Returns true if the timer was canceled before it was
  // scheduled in a cpu and false otherwise or if the timer
  // was not scheduled at all.
  //
  bool Cancel();

  // Equivalent to Set with no slack
  // The deadline parameter should be interpreted differently depending on the timeline_ field.
  // If timeline_ is set to kMono, deadline is a zx_time_t.
  // If timeline_ is set to kBoot, deadline is a zx_boot_time_t.
  void SetOneshot(int64_t deadline, Callback callback, void* arg) {
    Set(Deadline::no_slack(deadline), callback, arg);
  }

  // Special helper routine to simultaneously try to acquire a spinlock and
  // check for timer cancel, which is needed in a few special cases. Returns
  // ZX_OK if spinlock was acquired, ZX_ERR_TIMED_OUT if timer was canceled.
  zx_status_t TrylockOrCancel(MonitoredSpinLock* lock) TA_TRY_ACQ(false, lock);
  zx_status_t TrylockOrCancel(ChainLock& lock) TA_REQ(chainlock_transaction_token)
      TA_TRY_ACQ(false, lock);

  // Private accessors for timer tests.
  zx_duration_t slack_for_test() const { return slack_; }

  // This function returns a zx_time_t if the expected_timeline is kMono and a zx_boot_time_t if
  // the expected_timeline is kBoot.
  int64_t scheduled_time_for_test(ReferenceTimeline expected_timeline) const {
    DEBUG_ASSERT(timeline_ == expected_timeline);
    return scheduled_time_;
  }

 private:
  // TimerQueues can directly manipulate the state of their enqueued Timers.
  friend class TimerQueue;

  static constexpr uint32_t kMagic = fbl::magic("timr");
  uint32_t magic_ = kMagic;

  // This field should be interpreted differently depending on the timeline_ field.
  // If timeline_ is set to kMono, this is a zx_time_t.
  // If timeline_ is set to kBoot, this is a zx_boot_time_t.
  int64_t scheduled_time_ = 0;

  // Stores the applied slack adjustment from the ideal scheduled_time.
  zx_duration_t slack_ = 0;
  Callback callback_ = nullptr;
  void* arg_ = nullptr;

  // INVALID_CPU, if inactive.
  ktl::atomic<cpu_num_t> active_cpu_{INVALID_CPU};

  // true if cancel is pending
  ktl::atomic<bool> cancel_{false};

  // The timeline this timer is set on.
  const ReferenceTimeline timeline_;
};

// Preemption Timers
//
// Each CPU has a dedicated preemption timer that's managed using specialized
// functions (prefixed with timer_preempt_).
//
// Preemption timers are different from general timers. Preemption timers:
//
// - are reset frequently by the scheduler so performance is important
// - should not be migrated off their CPU when the CPU is shutdown
//
// Note: A preemption timer may fire even after it has been canceled.
class TimerQueue {
 public:
  // Set/reset/cancel the preemption timer.
  //
  // When the preemption timer fires, Scheduler::TimerTick is called. Set the
  // deadline to ZX_TIME_INFINITE to cancel the preemption timer.
  // Scheduler::TimerTick may be called spuriously after cancellation.
  void PreemptReset(zx_time_t deadline);

  // Returns true if the preemption deadline is set and will definitely fire in
  // the future. A false value does not definitively mean the preempt timer will
  // not fire, as a spurious expiration is allowed.
  bool PreemptArmed() const { return preempt_timer_deadline_ != ZX_TIME_INFINITE; }

  // Internal routines used when bringing cpus online/offline

  // Moves |source|'s timers (except its preemption timer) to this TimerQueue.
  void TransitionOffCpu(TimerQueue& source);

  // Prints the contents of all timer queues into |buf| of length |len| and null
  // terminates |buf|.
  static void PrintTimerQueues(char* buf, size_t len);

  // This is called periodically by timer_tick(), which itself is invoked
  // periodically by some hardware timer.
  void Tick(cpu_num_t cpu);

 private:
  // Timers can directly call Insert and Cancel.
  friend class Timer;

  // Add |timer| to this TimerQueue, possibly coalescing deadlines as well.
  void Insert(Timer* timer, zx_time_t earliest_deadline, zx_time_t latest_deadline);

  // A helper function for Insert that inserts the given timer into the given timer list.
  static void InsertIntoTimerList(fbl::DoublyLinkedList<Timer*>& timer_list, Timer* timer,
                                  zx_time_t earliest_deadline, zx_time_t latest_deadline);

  // A helper function for TransitionOffCpu that moves all timers from the src_list to the
  // dst_list. Returns the Timer at the head of the dst_list if it changed, otherwise returns
  // nullopt.
  static ktl::optional<Timer*> TransitionTimerList(fbl::DoublyLinkedList<Timer*>& src_list,
                                                   fbl::DoublyLinkedList<Timer*>& dst_list);

  // A helper function for PrintTimerQueues that prints all of the timers in the given timer_list
  // into the given buffer. Also takes in the current time, which is either a zx_time_t or a
  // zx_boot_time_t depending on the timeline the timer_list is operating on.
  template <typename TimestampType>
  static void PrintTimerList(TimestampType now, fbl::DoublyLinkedList<Timer*>& timer_list,
                             StringFile& buffer);

  // The UpdatePlatformTimer* methods are used to update the platform's oneshot timer to the
  // minimum of the existing deadline (stored in next_timer_deadline_) and the given new_deadline.
  // The two separate variations of this method are provided for convenience, so that callers can
  // provide either a montonic or a boot timestamp depending on the context they're operating in.
  //
  // These can only be called when interrupts are disabled.
  void UpdatePlatformTimerMono(zx_time_t new_deadline);
  void UpdatePlatformTimerBoot(zx_boot_time_t new_deadline);

  // This is called by Tick(), and processes all timers with scheduled times less than now.
  // Once it's done, the scheduled time of the timer at the front of the queue is returned.
  template <typename TimestampType>
  static TimestampType TickInternal(TimestampType now, cpu_num_t cpu,
                                    fbl::DoublyLinkedList<Timer*>& timer_list);

  // Timers on the monotonic timeline are placed in this list.
  fbl::DoublyLinkedList<Timer*> monotonic_timer_list_;

  // Timers on the boot timeline are placed in this list.
  fbl::DoublyLinkedList<Timer*> boot_timer_list_;

  // This TimerQueue's preemption deadline. ZX_TIME_INFINITE means not set.
  zx_time_t preempt_timer_deadline_ = ZX_TIME_INFINITE;

  // This TimerQueue's deadline for its platform timer or ZX_TIME_INFINITE if not set.
  // The deadline is stored in raw platform ticks, as that is the unit used by the
  // platform_set_oneshot_timer API.
  zx_ticks_t next_timer_deadline_ = ZX_TIME_INFINITE;
};

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_TIMER_H_
