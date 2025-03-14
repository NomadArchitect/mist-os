// Copyright 2024 Mist Tecnologia LTDA. All rights reserved.
// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <lib/fit/defer.h>
#include <lib/mistos/util/testing/unittest.h>
#include <lib/mistos/zx/event.h>
#include <lib/mistos/zx/handle.h>
#include <lib/mistos/zx/object.h>
#include <lib/mistos/zx/vmar.h>
#include <lib/mistos/zx/vmo.h>
#include <lib/unittest/unittest.h>

namespace unit_testing {
namespace {

zx_status_t validate_handle(fbl::RefPtr<zx::Value> handle) {
  return (handle && handle->is_valid() && (handle->get()->dispatcher() != nullptr))
             ? ZX_OK
             : ZX_ERR_BAD_HANDLE;
}

bool handle_invalid() {
  BEGIN_TEST;

  zx::handle handle;
  // A default constructed handle is invalid.
  ASSERT_TRUE(handle.release() == nullptr);

  END_TEST;
}

bool handle_close() {
  BEGIN_TEST;
  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));
  ASSERT_OK(validate_handle(event.get()));
  {
    zx::handle handle(event.release());
  }
  // Make sure the handle was closed.
  ASSERT_EQ(validate_handle(event.get()), ZX_ERR_BAD_HANDLE);

  END_TEST;
}

bool handle_move() {
  BEGIN_TEST;

  zx::event event;
  // Check move semantics.
  ASSERT_OK(zx::event::create(0u, &event));
  zx::handle handle(std::move(event));
  ASSERT_TRUE(event.release() == nullptr);
  ASSERT_OK(validate_handle(handle.get()));

  END_TEST;
}

bool handle_replace() {
  BEGIN_TEST;

  zx::event event;
  zx::handle rep;
  ASSERT_OK(zx::event::create(0u, &event));
  {
    zx::handle handle(event.release());
    ASSERT_OK(handle.replace(ZX_RIGHT_SAME_RIGHTS, &rep));
    ASSERT_TRUE(handle.release() == nullptr);
  }
  // The original shoould be invalid and the replacement should be valid.
  ASSERT_EQ(validate_handle(event.get()), ZX_ERR_BAD_HANDLE);
  ASSERT_OK(validate_handle(rep.get()));

  END_TEST;
}

bool handle_duplicate() {
  BEGIN_TEST;

  zx::event event;
  zx::handle dup;
  ASSERT_OK(zx::event::create(0u, &event));
  zx::handle handle(event.get());
  ASSERT_OK(handle.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
  // The duplicate must be valid as well as the original.
  ASSERT_OK(validate_handle(dup.get()));
  ASSERT_OK(validate_handle(event.get()));

  END_TEST;
}

bool get_info() {
  BEGIN_TEST;
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(1, 0u, &vmo));

  // zx::vmo is just an easy object to create; this is really a test of zx::object_base.
  const zx::object_base& object = vmo;
  zx_info_handle_count_t info;
  EXPECT_OK(object.get_info(ZX_INFO_HANDLE_COUNT, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(1u, info.handle_count);

  END_TEST;
}

bool set_get_property() {
  BEGIN_TEST;

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(1, 0u, &vmo));

  // zx::vmo is just an easy object to create; this is really a test of zx::object_base.
  const char name[] = "a great maximum length vmo name";
  const zx::object_base& object = vmo;
  EXPECT_OK(object.set_property(ZX_PROP_NAME, name, sizeof(name)));

  char read_name[ZX_MAX_NAME_LEN];
  EXPECT_OK(object.get_property(ZX_PROP_NAME, read_name, sizeof(read_name)));
  EXPECT_STREQ(name, read_name);

  END_TEST;
}

bool event() {
  BEGIN_TEST;
  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));
  ASSERT_OK(validate_handle(event.get()));
  // TODO(cpu): test more.

  END_TEST;
}

bool event_duplicate() {
  BEGIN_TEST;

  zx::event event;
  zx::event dup;
  ASSERT_OK(zx::event::create(0u, &event));
  ASSERT_OK(event.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
  // The duplicate must be valid as well as the original.
  ASSERT_OK(validate_handle(dup.get()));
  ASSERT_OK(validate_handle(event.get()));

  END_TEST;
}

bool vmar() {
  BEGIN_TEST;
  zx::vmar vmar;
  const size_t size = PAGE_SIZE;
  uintptr_t addr;
  ASSERT_OK(zx::vmar::kernel_vmar()->allocate(ZX_VM_CAN_MAP_READ, 0u, size, &vmar, &addr));
  ASSERT_OK(validate_handle(vmar.get()));
  ASSERT_OK(vmar.destroy());
  // TODO(teisenbe): test more.
  END_TEST;
}

#if 0
TEST(ZxTestCase, TimeConstruction) {
  // time construction
  ASSERT_EQ(zx::time().get(), 0);
  ASSERT_EQ(zx::time::infinite().get(), ZX_TIME_INFINITE);
  ASSERT_EQ(zx::time(-1).get(), -1);
  ASSERT_EQ(zx::time(ZX_TIME_INFINITE_PAST).get(), ZX_TIME_INFINITE_PAST);
#if __cplusplus >= 201703L
  ASSERT_EQ(zx::time(timespec{123, 456}).get(), ZX_SEC(123) + ZX_NSEC(456));
#endif
}

TEST(ZxTestCase, TimeConversions) {
#if __cplusplus >= 201703L
  const timespec ts = zx::time(timespec{123, 456}).to_timespec();
  ASSERT_EQ(ts.tv_sec, 123);
  ASSERT_EQ(ts.tv_nsec, 456);
#endif
}

TEST(ZxTestCase, DurationConstruction) {
  // duration construction
  ASSERT_EQ(zx::duration().get(), 0);
  ASSERT_EQ(zx::duration::infinite().get(), ZX_TIME_INFINITE);
  ASSERT_EQ(zx::duration(-1).get(), -1);
  ASSERT_EQ(zx::duration(ZX_TIME_INFINITE_PAST).get(), ZX_TIME_INFINITE_PAST);
#if __cplusplus >= 201703L
  ASSERT_EQ(zx::duration(timespec{123, 456}).get(), ZX_SEC(123) + ZX_NSEC(456));
#endif
}

TEST(ZxTestCase, DurationConversions) {
  // duration to/from nsec, usec, msec, etc.
  ASSERT_EQ(zx::nsec(-10).get(), ZX_NSEC(-10));
  ASSERT_EQ(zx::nsec(-10).to_nsecs(), -10);
  ASSERT_EQ(zx::nsec(10).get(), ZX_NSEC(10));
  ASSERT_EQ(zx::nsec(10).to_nsecs(), 10);
  ASSERT_EQ(zx::usec(10).get(), ZX_USEC(10));
  ASSERT_EQ(zx::usec(10).to_usecs(), 10);
  ASSERT_EQ(zx::msec(10).get(), ZX_MSEC(10));
  ASSERT_EQ(zx::msec(10).to_msecs(), 10);
  ASSERT_EQ(zx::sec(10).get(), ZX_SEC(10));
  ASSERT_EQ(zx::sec(10).to_secs(), 10);
  ASSERT_EQ(zx::min(10).get(), ZX_MIN(10));
  ASSERT_EQ(zx::min(10).to_mins(), 10);
  ASSERT_EQ(zx::hour(10).get(), ZX_HOUR(10));
  ASSERT_EQ(zx::hour(10).to_hours(), 10);

#if __cplusplus >= 201703L
  const timespec ts = zx::duration(timespec{123, 456}).to_timespec();
  ASSERT_EQ(ts.tv_sec, 123);
  ASSERT_EQ(ts.tv_nsec, 456);
#endif

  ASSERT_EQ((zx::time() + zx::usec(19)).get(), ZX_USEC(19));
  ASSERT_EQ((zx::usec(19) + zx::time()).get(), ZX_USEC(19));
  ASSERT_EQ((zx::time::infinite() - zx::time()).get(), ZX_TIME_INFINITE);
  ASSERT_EQ((zx::time::infinite() - zx::time::infinite()).get(), 0);
  ASSERT_EQ((zx::time() + zx::duration::infinite()).get(), ZX_TIME_INFINITE);

  zx::duration d(0u);
  d += zx::nsec(19);
  ASSERT_EQ(d.get(), ZX_NSEC(19));
  d -= zx::nsec(19);
  ASSERT_EQ(d.get(), ZX_NSEC(0));

  d = zx::min(1);
  d *= 19u;
  ASSERT_EQ(d.get(), ZX_MIN(19));
  d /= 19u;
  ASSERT_EQ(d.get(), ZX_MIN(1));

  ASSERT_EQ(zx::sec(19) % zx::sec(7), ZX_SEC(5));

  zx::time t(0u);
  t += zx::msec(19);
  ASSERT_EQ(t.get(), ZX_MSEC(19));
  t -= zx::msec(19);
  ASSERT_EQ(t.get(), ZX_MSEC(0));

  ASSERT_EQ((2 * zx::msec(10)).get(), ZX_MSEC(20));
  ASSERT_EQ((zx::msec(10) * 2).get(), ZX_MSEC(20));
  ASSERT_EQ((-zx::msec(10)).get(), ZX_MSEC(-10));
  ASSERT_EQ((-zx::duration::infinite()).get(), ZX_TIME_INFINITE_PAST + 1);
  ASSERT_EQ((-zx::duration::infinite_past()).get(), ZX_TIME_INFINITE);

  // Just a smoke test
  ASSERT_GE(zx::deadline_after(zx::usec(10)).get(), ZX_USEC(10));
}

TEST(ZxTestCase, TimeNanoSleep) {
  ASSERT_OK(zx::nanosleep(zx::time(ZX_TIME_INFINITE_PAST)));
  ASSERT_OK(zx::nanosleep(zx::time(-1)));
  ASSERT_OK(zx::nanosleep(zx::time(0)));
  ASSERT_OK(zx::nanosleep(zx::time(1)));
}

TEST(ZxTestCase, Ticks) {
  // Check that the default constructor initialized to 0.
  ASSERT_EQ(zx::ticks().get(), 0);

  // Sanity check the math operators.
  zx::ticks res;

  // Addition
  res = zx::ticks(5) + zx::ticks(7);
  ASSERT_EQ(res.get(), 12);
  res = zx::ticks(5);
  res += zx::ticks(7);
  ASSERT_EQ(res.get(), 12);

  // Subtraction
  res = zx::ticks(5) - zx::ticks(7);
  ASSERT_EQ(res.get(), -2);
  res = zx::ticks(5);
  res -= zx::ticks(7);
  ASSERT_EQ(res.get(), -2);

  // Multiplication
  res = zx::ticks(7) * 3;
  ASSERT_EQ(res.get(), 21);
  res = zx::ticks(7);
  res *= 3;
  ASSERT_EQ(res.get(), 21);

  // Division
  res = zx::ticks(25) / 7;
  ASSERT_EQ(res.get(), 3);
  res = zx::ticks(25);
  res /= 7;
  ASSERT_EQ(res.get(), 3);

  // Modulus
  res = zx::ticks(25) % 7;
  ASSERT_EQ(res.get(), 4);
  res = zx::ticks(25);
  res %= 7;
  ASSERT_EQ(res.get(), 4);

  // Test basic comparison, also set up for testing monotonicity.
  zx::ticks before = zx::ticks::now();
  ASSERT_GT(before.get(), 0);
  zx::ticks after = before + zx::ticks(1);

  ASSERT_LT(before.get(), after.get());
  ASSERT_TRUE(before < after);
  ASSERT_TRUE(before <= after);
  ASSERT_TRUE(before <= before);

  ASSERT_TRUE(after > before);
  ASSERT_TRUE(after >= before);
  ASSERT_TRUE(after >= after);

  ASSERT_TRUE(before == before);
  ASSERT_TRUE(before != after);

  after -= zx::ticks(1);
  ASSERT_EQ(before.get(), after.get());
  ASSERT_TRUE(before == after);

  // Make sure that zx::ticks TPS agrees with the syscall.
  ASSERT_EQ(zx::ticks::per_second().get(), zx_ticks_per_second());

#if 0
  // Compare a duration (nanoseconds) with the ticks equivalent.
  zx::ticks second = zx::ticks::per_second();
  ASSERT_EQ(fzl::TicksToNs(second).get(), zx::sec(1).get());
  ASSERT_TRUE(fzl::TicksToNs(second) == zx::sec(1));
#endif

  // Make sure that the libzx ticks operators saturate properly, instead of
  // overflowing.  Start with addition.
  constexpr zx::ticks ALMOST_MAX = zx::ticks(std::numeric_limits<zx_ticks_t>::max() - 5);
  constexpr zx::ticks ALMOST_MIN = zx::ticks(std::numeric_limits<zx_ticks_t>::min() + 5);
  constexpr zx::ticks ABSOLUTE_MIN = zx::ticks(std::numeric_limits<zx_ticks_t>::min());
  constexpr zx::ticks ZERO = zx::ticks(0);

  res = ALMOST_MAX + zx::ticks(10);
  ASSERT_EQ(res.get(), zx::ticks::infinite().get());
  res = ALMOST_MAX;
  res += zx::ticks(10);
  ASSERT_EQ(res.get(), zx::ticks::infinite().get());

  res = ALMOST_MIN + zx::ticks(-10);
  ASSERT_EQ(res.get(), zx::ticks::infinite_past().get());
  res = ALMOST_MIN;
  res += zx::ticks(-10);
  ASSERT_EQ(res.get(), zx::ticks::infinite_past().get());

  // Now, subtraction
  res = ALMOST_MIN - zx::ticks(10);
  ASSERT_EQ(res.get(), zx::ticks::infinite_past().get());
  res = ALMOST_MIN;
  res -= zx::ticks(10);
  ASSERT_EQ(res.get(), zx::ticks::infinite_past().get());

  res = ALMOST_MAX - zx::ticks(-10);
  ASSERT_EQ(res.get(), zx::ticks::infinite().get());
  res = ALMOST_MAX;
  res -= zx::ticks(-10);
  ASSERT_EQ(res.get(), zx::ticks::infinite().get());

  res = ZERO - ABSOLUTE_MIN;
  ASSERT_EQ(res.get(), zx::ticks::infinite().get());
  res = ZERO;
  res -= ABSOLUTE_MIN;
  ASSERT_EQ(res.get(), zx::ticks::infinite().get());

  // Finally, multiplication
  res = ALMOST_MAX * 2;
  ASSERT_EQ(res.get(), zx::ticks::infinite().get());
  res = ALMOST_MAX;
  res *= 2;
  ASSERT_EQ(res.get(), zx::ticks::infinite().get());

  res = ALMOST_MIN * 2;
  ASSERT_EQ(res.get(), zx::ticks::infinite_past().get());
  res = ALMOST_MIN;
  res *= 2;
  ASSERT_EQ(res.get(), zx::ticks::infinite_past().get());

  // Hopefully, we haven't moved backwards in time.
  after = zx::ticks::now();
  ASSERT_LE(before.get(), after.get());
  ASSERT_TRUE(before <= after);
}
#endif

template <typename T>
bool IsValidHandle(const T& p) {
  BEGIN_TEST;
  ASSERT_TRUE(static_cast<bool>(p), "invalid handle");
  END_TEST;
}

#if 0
TEST(ZxTestCase, ThreadSelf) {
  zx_handle_t raw = zx_thread_self();
  ASSERT_OK(validate_handle(raw));

  ASSERT_NO_FATAL_FAILURE(IsValidHandle<zx::thread>(*zx::thread::self()));
  EXPECT_OK(validate_handle(raw));

  // This does not compile:
  // const zx::thread self = zx::thread::self();
}

TEST(ZxTestCase, ThreadCreate) {
  zx::thread thread;
  const char* name = "test thread";
  ASSERT_OK(zx::thread::create(*zx::process::self(), name, sizeof(name), 0u, &thread));
  EXPECT_TRUE(thread.is_valid());
  EXPECT_OK(validate_handle(thread.get()));
}

TEST(ZxTestCase, ProcessSelf) {
  zx_handle_t raw = zx_process_self();
  ASSERT_OK(validate_handle(raw));

  ASSERT_NO_FATAL_FAILURE(IsValidHandle<zx::process>(*zx::process::self()));
  EXPECT_OK(validate_handle(raw));

  // This does not compile:
  // const zx::process self = zx::process::self();
}


TEST(ZxTestCase, VmarRootSelf) {
  zx_handle_t raw = zx_vmar_root_self();
  ASSERT_OK(validate_handle(raw));

  ASSERT_NO_FATAL_FAILURE(IsValidHandle<zx::vmar>(*zx::vmar::root_self()));
  EXPECT_OK(validate_handle(raw));

  // This does not compile:
  // const zx::vmar root_self = zx::vmar::root_self();
}

TEST(ZxTestCase, JobDefault) {
  zx_handle_t raw = zx_job_default();
  ASSERT_OK(validate_handle(raw));

  ASSERT_NO_FATAL_FAILURE(IsValidHandle<zx::job>(*zx::job::default_job()));
  EXPECT_OK(validate_handle(raw));

  // This does not compile:
  // const zx::job default_job = zx::job::default_job();
}

#if 0
bool takes_any_handle(const zx::handle& handle) { return handle.is_valid(); }

TEST(ZxTestCase, HandleConversion) {
  EXPECT_TRUE(takes_any_handle(*zx::unowned_handle(zx_thread_self())));
  ASSERT_OK(validate_handle(zx_thread_self()));
}
#endif

#endif

bool unowned() {
  BEGIN_TEST;

  // Create a handle to test with.
  zx::event handle;
  ASSERT_OK(zx::event::create(0, &handle));
  ASSERT_OK(validate_handle(handle.get()));

  // Verify that unowned<T>(zx_handle_t) doesn't close handle on teardown.
  {
    zx::unowned<zx::event> unowned(handle.get());
    EXPECT_TRUE(unowned->get() == handle.get());
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned));
  }
  ASSERT_OK(validate_handle(handle.get()));

  // Verify that unowned<T>(const T&) doesn't close handle on teardown.
  {
    zx::unowned<zx::event> unowned(handle);
    EXPECT_TRUE(unowned->get() == handle.get());
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned));
  }
  ASSERT_OK(validate_handle(handle.get()));

  // Verify that unowned<T>(const unowned<T>&) doesn't close on teardown.
  {
    zx::unowned<zx::event> unowned(handle);
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned));

    zx::unowned<zx::event> unowned2(unowned);
    EXPECT_TRUE(unowned->get() == unowned2->get());
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned2));
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned));
  }
  ASSERT_OK(validate_handle(handle.get()));

  // Verify copy-assignment from unowned<> to unowned<> doesn't close.
  {
    zx::unowned<zx::event> unowned(handle);
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned));

    zx::unowned<zx::event> unowned2;
    ASSERT_FALSE(unowned2->is_valid());

    const zx::unowned<zx::event>& assign_ref = unowned2 = unowned;
    EXPECT_TRUE(assign_ref->get() == unowned2->get());
    EXPECT_TRUE(unowned->get() == unowned2->get());
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned2));
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned));
  }
  ASSERT_OK(validate_handle(handle.get()));

  // Verify move from unowned<> to unowned<> doesn't close on teardown.
  {
    zx::unowned<zx::event> unowned(handle);
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned));

    zx::unowned<zx::event> unowned2(static_cast<zx::unowned<zx::event>&&>(unowned));
    EXPECT_TRUE(unowned2->get() == handle.get());
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned2));
    EXPECT_FALSE(unowned->is_valid());
  }
  ASSERT_OK(validate_handle(handle.get()));

  // Verify move-assignment from unowned<> to unowned<> doesn't close.
  {
    zx::unowned<zx::event> unowned(handle);
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned));

    zx::unowned<zx::event> unowned2;
    ASSERT_FALSE(unowned2->is_valid());

    const zx::unowned<zx::event>& assign_ref = unowned2 =
        static_cast<zx::unowned<zx::event>&&>(unowned);
    EXPECT_TRUE(assign_ref->get() == unowned2->get());
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned2));
    EXPECT_FALSE(unowned->is_valid());
  }
  ASSERT_OK(validate_handle(handle.get()));

  // Verify move-assignment into non-empty unowned<>  doesn't close.
  {
    zx::unowned<zx::event> unowned(handle);
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned));

    zx::unowned<zx::event> unowned2(handle);
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned2));

    unowned2 = static_cast<zx::unowned<zx::event>&&>(unowned);
    EXPECT_TRUE(unowned2->get() == handle.get());
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned2));
    EXPECT_FALSE(unowned->is_valid());
  }
  ASSERT_OK(validate_handle(handle.get()));

  // Explicitly verify dereference operator allows methods to be called.
  {
    zx::unowned<zx::event> unowned(handle);
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned));

    const zx::event& event_ref = *unowned;
    zx::event duplicate;
    EXPECT_OK(event_ref.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate));
  }
  ASSERT_OK(validate_handle(handle.get()));

  // Explicitly verify member access operator allows methods to be called.
  {
    zx::unowned<zx::event> unowned(handle);
    ASSERT_TRUE(IsValidHandle<zx::event>(*unowned));

    zx::event duplicate;
    EXPECT_OK(unowned->duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate));
  }
  ASSERT_OK(validate_handle(handle.get()));
  END_TEST;
}

bool unowned2() {
  BEGIN_TEST;

  zx::event handle;
  ASSERT_OK(zx::event::create(0, &handle));
  ASSERT_OK(validate_handle(handle.get()));
  {
    const zx::unowned_event event{handle};
  }
  EXPECT_TRUE(handle.is_valid());

  END_TEST;
}

bool vmo_content_size() {
  BEGIN_TEST;

  zx::vmo vmo;
  constexpr uint32_t options = 0;
  constexpr uint64_t initial_size = 8 * 1024;
  ASSERT_OK(zx::vmo::create(initial_size, options, &vmo));

  uint64_t retrieved_size = 0;
  ASSERT_OK(vmo.get_prop_content_size(&retrieved_size));
  EXPECT_EQ(retrieved_size, initial_size);
  retrieved_size = 0;

  constexpr uint64_t new_size = 500;
  EXPECT_OK(vmo.set_prop_content_size(new_size));

  ASSERT_OK(vmo.get_prop_content_size(&retrieved_size));
  EXPECT_EQ(retrieved_size, new_size);
  retrieved_size = 0;

  END_TEST;
}

#if 0
TEST(ZxTestCase, DebugLog) {
  zx::resource res{ZX_HANDLE_INVALID};
  zx::debuglog log;
  ASSERT_OK(zx::debuglog::create(res, 0, &log));
  EXPECT_OK(log.write(0, "Hello!", sizeof("Hello!")));
}
#endif

}  // namespace
}  // namespace unit_testing

UNITTEST_START_TESTCASE(mistos_zx_test)
UNITTEST("handle invalid", unit_testing::handle_invalid)
UNITTEST("handle close", unit_testing::handle_close)
UNITTEST("handle move", unit_testing::handle_move)
UNITTEST("handle replace", unit_testing::handle_replace)
UNITTEST("handle duplicate", unit_testing::handle_duplicate)
UNITTEST("get info", unit_testing::get_info)
UNITTEST("set get property", unit_testing::set_get_property)
UNITTEST("event", unit_testing::event)
UNITTEST("event duplicate", unit_testing::event_duplicate)
UNITTEST("vmar", unit_testing::vmar)
UNITTEST("unowned", unit_testing::unowned)
UNITTEST("unowned2", unit_testing::unowned2)
UNITTEST("vmo content size", unit_testing::vmo_content_size)
UNITTEST_END_TESTCASE(mistos_zx_test, "mistos_zx_test", "mistos zx test")
