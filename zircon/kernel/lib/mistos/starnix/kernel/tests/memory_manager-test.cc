// Copyright 2024 Mist Tecnologia LTDA. All rights reserved.
// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/result.h>
#include <lib/mistos/starnix/kernel/mm/memory_manager.h>
#include <lib/mistos/starnix/kernel/task/kernel.h>
#include <lib/mistos/starnix/kernel/task/process_group.h>
#include <lib/mistos/starnix/kernel/task/task.h>
#include <lib/mistos/starnix/kernel/task/thread_group.h>
#include <lib/mistos/starnix/testing/testing.h>
#include <lib/mistos/starnix_uapi/user_address.h>
#include <lib/mistos/util/range-map.h>
#include <zircon/syscalls.h>

#include <tuple>
#include <utility>

#include <fbl/alloc_checker.h>
#include <fbl/string.h>
#include <fbl/vector.h>
#include <lockdep/guard.h>
#include <zxtest/zxtest.h>

using namespace starnix_uapi;
using namespace starnix::testing;

namespace starnix {

TEST(MemoryManager, test_brk) {
  auto [kernel, current_task] = create_kernel_and_task();

  auto mm = current_task->mm();

  auto get_range = [&mm](const UserAddress& addr) -> util::Range<UserAddress> {
    auto state = mm->state.Read();
    if (auto opt = state->mappings.get(addr); opt) {
      return opt->first;
    }
    EXPECT_TRUE(true, "failed to find mapping");
    return util::Range<UserAddress>({0, 0});
  };

  // Initialize the program break.
  auto base_addr = mm->set_brk(*current_task, 0);
  ASSERT_FALSE(base_addr.is_error(), "failed to set initial program break");
  ASSERT_TRUE(base_addr.value() > 0);

  // Check that the initial program break actually maps some memory.
  auto range0 = get_range(base_addr.value());
  ASSERT_EQ(range0.start, base_addr.value());
  ASSERT_EQ(range0.end, base_addr.value() + static_cast<uint64_t>(PAGE_SIZE));

  // Grow the program break by a tiny amount that does not actually result in a change.
  auto addr1 = mm->set_brk(*current_task, base_addr.value() + 1ul);
  ASSERT_FALSE(addr1.is_error(), "failed to grow brk");
  ASSERT_EQ(addr1.value(), base_addr.value() + 1ul);
  auto range1 = get_range(base_addr.value());
  ASSERT_EQ(range1.start, range0.start);
  ASSERT_EQ(range1.end, range0.end);

  // Grow the program break by a non-trival amount and observe the larger mapping.
  auto addr2 = mm->set_brk(*current_task, base_addr.value() + 24893ul);
  ASSERT_FALSE(addr2.is_error(), "failed to grow brk error %u", addr2.error_value().error_code());
  ASSERT_EQ(addr2.value(), base_addr.value() + 24893ul);
  auto range2 = get_range(base_addr.value());
  ASSERT_EQ(range2.start, base_addr.value());
  ASSERT_EQ(range2.end, addr2->round_up(PAGE_SIZE).value());

  // Shrink the program break and observe the smaller mapping.
  auto addr3 = mm->set_brk(*current_task, base_addr.value() + 14832ul);
  ASSERT_FALSE(addr3.is_error(), "failed to shrink brk");
  ASSERT_EQ(addr3.value(), base_addr.value() + 14832ul);
  auto range3 = get_range(base_addr.value());
  ASSERT_EQ(range3.start, base_addr.value());
  ASSERT_EQ(range3.end, addr3->round_up(PAGE_SIZE).value());

  // Shrink the program break close to zero and observe the smaller mapping.
  auto addr4 = mm->set_brk(*current_task, base_addr.value() + 3ul);
  ASSERT_FALSE(addr4.is_error(), "failed to drastically shrink brk");
  ASSERT_EQ(addr4.value(), base_addr.value() + 3ul);
  auto range4 = get_range(base_addr.value());
  ASSERT_EQ(range4.start, base_addr.value());
  ASSERT_EQ(range4.end, addr4->round_up(PAGE_SIZE).value());

  // Shrink the program break close to zero and observe that the mapping is not entirely
  auto addr5 = mm->set_brk(*current_task, base_addr.value());
  ASSERT_FALSE(addr5.is_error(), "failed to drastically shrink brk to zero");
  ASSERT_EQ(addr5.value(), base_addr.value());
  auto range5 = get_range(base_addr.value());
  ASSERT_EQ(range5.start, base_addr.value());
  ASSERT_EQ(range5.end, addr5.value() + static_cast<uint64_t>(PAGE_SIZE));
}

TEST(MemoryManager, test_mm_exec) {
  auto [kernel, current_task] = create_kernel_and_task();

  auto mm = current_task->mm();

  auto has = [&mm](UserAddress addr) -> bool {
    auto state = mm->state.Read();
    return state->mappings.get(addr).has_value();
  };

  auto brk_addr = mm->set_brk(*current_task, 0);
  EXPECT_TRUE(brk_addr.is_ok(), "failed to set initial program break");
  ASSERT_GT(brk_addr.value(), 0);
  ASSERT_TRUE(has(brk_addr.value()));

  auto mapped_addr = map_memory(*current_task, 0, PAGE_SIZE);
  ASSERT_GT(mapped_addr, 0);
  ASSERT_TRUE(has(mapped_addr));

  /*let node = current_task.lookup_path_from_root("/".into()).unwrap();*/
  auto exec_result = mm->exec(/*node*/);
  EXPECT_TRUE(exec_result.is_ok(), "failed to exec memory manager");

  ASSERT_FALSE(has(brk_addr.value()));
  ASSERT_FALSE(has(mapped_addr));

  auto brk_addr2 = map_memory(*current_task, brk_addr.value(), PAGE_SIZE);
  ASSERT_EQ(brk_addr.value(), brk_addr2);

  auto mapped_addr2 = map_memory(*current_task, mapped_addr, PAGE_SIZE);
  ASSERT_EQ(mapped_addr, mapped_addr2);
}

TEST(MemoryManager, test_get_contiguous_mappings_at) {
  auto [kernel, current_task] = create_kernel_and_task();
  auto mm = current_task->mm();

  // Create four one-page mappings with a hole between the third one and the fourth one.
  size_t page_size = PAGE_SIZE;
  size_t addr_a = mm->base_addr.ptr() + 10 * page_size;
  size_t addr_b = mm->base_addr.ptr() + 11 * page_size;
  size_t addr_c = mm->base_addr.ptr() + 12 * page_size;
  size_t addr_d = mm->base_addr.ptr() + 14 * page_size;

  ASSERT_EQ(map_memory(*current_task, addr_a, PAGE_SIZE).ptr(), addr_a);
  ASSERT_EQ(map_memory(*current_task, addr_b, PAGE_SIZE).ptr(), addr_b);
  ASSERT_EQ(map_memory(*current_task, addr_c, PAGE_SIZE).ptr(), addr_c);
  ASSERT_EQ(map_memory(*current_task, addr_d, PAGE_SIZE).ptr(), addr_d);

  {
    auto mm_state = mm->state.Read();

    // Verify that requesting an unmapped address returns an empty iterator.
    ASSERT_TRUE(mm_state->get_contiguous_mappings_at(addr_a - 100, 50)->is_empty());
    ASSERT_TRUE(mm_state->get_contiguous_mappings_at(addr_a - 100, 200)->is_empty());

    // Verify that requesting zero bytes returns an empty iterator.
    ASSERT_TRUE(mm_state->get_contiguous_mappings_at(addr_a, 0)->is_empty());

    // Verify errors
    ASSERT_EQ(errno(EFAULT),
              mm_state->get_contiguous_mappings_at(UserAddress(100), SIZE_MAX).error_value());

    ASSERT_EQ(errno(EFAULT),
              mm_state->get_contiguous_mappings_at(mm_state->max_address() + 1ul, 0).error_value());
  }

#if STARNIX_ANON_ALLOCS
  {
  }
#else
  {
    ASSERT_EQ(mm->get_mapping_count(), 4);

    auto mm_state = mm->state.Read();

    auto [map_a, map_b, map_c,
          map_d] = [&mm_state]() -> std::tuple<fbl::RefPtr<Mapping>, fbl::RefPtr<Mapping>,
                                               fbl::RefPtr<Mapping>, fbl::RefPtr<Mapping>> {
      auto map = mm_state->mappings.iter();
      auto it = map.begin();
      return std::make_tuple((*it).second, (*++it).second, (*++it).second, (*++it).second);
    }();

    fbl::AllocChecker ac;
    fbl::Vector<ktl::pair<fbl::RefPtr<Mapping>, size_t>> expected;

    // Verify result when requesting a whole mapping or portions of it.
    expected.push_back({map_a, page_size}, &ac);
    ASSERT(ac.check());

    ASSERT_EQ(expected[0], mm_state->get_contiguous_mappings_at(addr_a, page_size).value()[0]);

    expected.reset();
    expected.push_back({map_a, page_size / 2}, &ac);
    ASSERT(ac.check());
    ASSERT_EQ(expected[0], mm_state->get_contiguous_mappings_at(addr_a, page_size / 2).value()[0]);

    ASSERT_EQ(
        expected[0],
        mm_state->get_contiguous_mappings_at(addr_a + page_size / 2, page_size / 2).value()[0]);

    expected.reset();
    expected.push_back({map_a, page_size / 8}, &ac);
    ASSERT(ac.check());
    ASSERT_EQ(
        expected[0],
        mm_state->get_contiguous_mappings_at(addr_a + page_size / 4, page_size / 8).value()[0]);

    // Verify result when requesting a range spanning more than one mapping.
    expected.reset();
    expected.push_back({map_a, page_size / 2}, &ac);
    ASSERT(ac.check());
    expected.push_back({map_b, page_size / 2}, &ac);
    ASSERT(ac.check());

    ASSERT_EQ(expected[0],
              mm_state->get_contiguous_mappings_at(addr_a + page_size / 2, page_size).value()[0]);
    ASSERT_EQ(expected[1],
              mm_state->get_contiguous_mappings_at(addr_a + page_size / 2, page_size).value()[1]);

    expected.reset();
    expected.push_back({map_a, page_size / 2}, &ac);
    ASSERT(ac.check());
    expected.push_back({map_b, page_size}, &ac);
    ASSERT(ac.check());

    ASSERT_EQ(
        expected[0],
        mm_state->get_contiguous_mappings_at(addr_a + page_size / 2, page_size * 3 / 2).value()[0]);
    ASSERT_EQ(
        expected[1],
        mm_state->get_contiguous_mappings_at(addr_a + page_size / 2, page_size * 3 / 2).value()[1]);

    expected.reset();
    expected.push_back({map_a, page_size}, &ac);
    ASSERT(ac.check());
    expected.push_back({map_b, page_size / 2}, &ac);
    ASSERT(ac.check());

    ASSERT_EQ(expected[0],
              mm_state->get_contiguous_mappings_at(addr_a, page_size * 3 / 2).value()[0]);
    ASSERT_EQ(expected[1],
              mm_state->get_contiguous_mappings_at(addr_a, page_size * 3 / 2).value()[1]);

    expected.reset();
    expected.push_back({map_a, page_size / 2}, &ac);
    ASSERT(ac.check());
    expected.push_back({map_b, page_size}, &ac);
    ASSERT(ac.check());
    expected.push_back({map_c, page_size / 2}, &ac);
    ASSERT(ac.check());

    ASSERT_EQ(
        expected[0],
        mm_state->get_contiguous_mappings_at(addr_a + page_size / 2, page_size * 2).value()[0]);
    ASSERT_EQ(
        expected[1],
        mm_state->get_contiguous_mappings_at(addr_a + page_size / 2, page_size * 2).value()[1]);
    ASSERT_EQ(
        expected[2],
        mm_state->get_contiguous_mappings_at(addr_a + page_size / 2, page_size * 2).value()[2]);

    expected.reset();
    expected.push_back({map_b, page_size / 2}, &ac);
    ASSERT(ac.check());
    expected.push_back({map_c, page_size}, &ac);
    ASSERT(ac.check());

    ASSERT_EQ(
        expected[0],
        mm_state->get_contiguous_mappings_at(addr_b + page_size / 2, page_size * 3 / 2).value()[0]);
    ASSERT_EQ(
        expected[1],
        mm_state->get_contiguous_mappings_at(addr_b + page_size / 2, page_size * 3 / 2).value()[1]);

    // Verify that results stop if there is a hole.
    expected.reset();
    expected.push_back({map_a, page_size / 2}, &ac);
    ASSERT(ac.check());
    expected.push_back({map_b, page_size}, &ac);
    ASSERT(ac.check());
    expected.push_back({map_c, page_size}, &ac);
    ASSERT(ac.check());

    ASSERT_EQ(
        expected[0],
        mm_state->get_contiguous_mappings_at(addr_a + page_size / 2, page_size * 10).value()[0]);
    ASSERT_EQ(
        expected[1],
        mm_state->get_contiguous_mappings_at(addr_a + page_size / 2, page_size * 10).value()[1]);
    ASSERT_EQ(
        expected[2],
        mm_state->get_contiguous_mappings_at(addr_a + page_size / 2, page_size * 10).value()[2]);

    // Verify that results stop at the last mapped page.
    expected.reset();
    expected.push_back({map_d, page_size}, &ac);
    ASSERT(ac.check());

    ASSERT_EQ(expected[0], mm_state->get_contiguous_mappings_at(addr_d, page_size * 10).value()[0]);
  }
#endif
}

TEST(MemoryManager, test_read_write_crossing_mappings) {
  auto [kernel, current_task] = create_kernel_and_task();
  auto mm = current_task->mm();
  auto ma = *current_task;

  // Map two contiguous pages at fixed addresses, but backed by distinct mappings.
  size_t page_size = PAGE_SIZE;
  auto addr = mm->base_addr + 10 * page_size;
  ASSERT_EQ(addr, map_memory(*current_task, addr, page_size));
  ASSERT_EQ(addr + page_size, map_memory(*current_task, addr + page_size, page_size));
#if STARNIX_ANON_ALLOCS
  ASSERT_EQ(1, mm->get_mapping_count());
#else
  ASSERT_EQ(2, mm->get_mapping_count());
#endif

  // Write a pattern crossing our two mappings.
  auto test_addr = addr + page_size / 2;
  fbl::AllocChecker ac;
  fbl::Vector<uint8_t> data;
  data.reserve(page_size, &ac);
  ASSERT(ac.check());

  std::generate(data.begin(), data.end(),
                [i = 0]() mutable { return static_cast<uint8_t>(i++ % 256); });

  ASSERT_TRUE(ma.write_memory(test_addr, {data.begin(), data.end()}).is_ok(),
              "failed to write test data");

  auto read_result = ma.read_memory_to_vec(test_addr, data.size());
  ASSERT_FALSE(read_result.is_error(), "failed to read test data");
  ASSERT_BYTES_EQ(data.data(), read_result.value().data(), data.size());
}

TEST(MemoryManager, test_read_write_errors) {
  auto [kernel, current_task] = create_kernel_and_task();
  auto ma = *current_task;

  size_t page_size = PAGE_SIZE;
  auto addr = map_memory(*current_task, UserAddress(), page_size);
  fbl::Vector<uint8_t> buf;
  fbl::AllocChecker ac;
  buf.resize(page_size, &ac);
  ASSERT(ac.check());

  // Verify that accessing data that is only partially mapped is an error.
  auto partial_addr_before = addr - page_size / 2;
  ASSERT_EQ(errno(EFAULT),
            ma.write_memory(partial_addr_before, {buf.data(), buf.size()}).error_value());
  ASSERT_EQ(errno(EFAULT), ma.read_memory_to_vec(partial_addr_before, buf.size()).error_value());
  auto partial_addr_after = addr + page_size / 2;
  ASSERT_EQ(errno(EFAULT),
            ma.write_memory(partial_addr_after, {buf.data(), buf.size()}).error_value());
  ASSERT_EQ(errno(EFAULT), ma.read_memory_to_vec(partial_addr_after, buf.size()).error_value());

  // Verify that accessing unmapped memory is an error.
  auto unmapped_addr = addr + 10 * page_size;
  ASSERT_EQ(errno(EFAULT), ma.write_memory(unmapped_addr, {buf.data(), buf.size()}).error_value());
  ASSERT_EQ(errno(EFAULT), ma.read_memory_to_vec(unmapped_addr, buf.size()).error_value());

  // However, accessing zero bytes in unmapped memory is not an error.
  ASSERT_FALSE(ma.write_memory(unmapped_addr, {(uint8_t*)nullptr, 0}).is_error(),
               "failed to write no data");
  ASSERT_FALSE(ma.read_memory_to_vec(unmapped_addr, 0).is_error(), "failed to read no data");
}

TEST(MemoryManager, test_read_c_string_to_vec_large) {
  auto [kernel, current_task] = create_kernel_and_task();
  auto mm = current_task->mm();
  auto ma = *current_task;

  size_t page_size = PAGE_SIZE;
  auto max_size = 4 * page_size;
  auto addr = mm->base_addr + 10 * page_size;

  ASSERT_EQ(addr, map_memory(*current_task, addr, max_size));

  fbl::AllocChecker ac;
  fbl::Vector<uint8_t> random_data;
  random_data.resize(max_size, &ac);
  ASSERT(ac.check());
  zx_cprng_draw(random_data.data(), max_size);

  // Remove all NUL bytes.
  for (size_t i = 0; i < random_data.size(); i++) {
    if (random_data[i] == 0) {
      random_data[i] = 1;
    }
  }
  random_data[max_size - 1] = 0;

  auto write_result = ma.write_memory(addr, {random_data.data(), random_data.size()});
  ASSERT_TRUE(write_result.is_ok(), "failed to write test string, error %d",
              write_result.error_value().error_code());

  // We should read the same value minus the last byte (NUL char).
  auto read_result = ma.read_c_string_to_vec(addr, max_size);
  ASSERT_TRUE(read_result.is_ok(), "failed to read c string, error %d",
              read_result.error_value().error_code());

  ASSERT_EQ(fbl::String((char*)random_data.data(), max_size - 1), read_result.value());
}

TEST(MemoryManager, test_read_c_string_to_vec) {
  auto [kernel, current_task] = create_kernel_and_task();
  auto mm = current_task->mm();
  auto ma = *current_task;

  size_t page_size = PAGE_SIZE;
  auto max_size = 2 * page_size;
  auto addr = mm->base_addr + 10 * page_size;

  // Map a page at a fixed address and write an unterminated string at the end of it.
  ASSERT_EQ(addr, map_memory(*current_task, addr, page_size));

  ktl::span<uint8_t> test_str((uint8_t*)"foo!", 4);
  auto test_addr = addr + page_size - test_str.size();
  ASSERT_TRUE(ma.write_memory(test_addr, test_str).is_ok(), "failed to write test string");

  // Expect error if the string is not terminated.
  ASSERT_EQ(errno(ENAMETOOLONG), ma.read_c_string_to_vec(test_addr, max_size).error_value());

  // Expect success if the string is terminated.
  ASSERT_TRUE(ma.write_memory(addr + (page_size - 1), {(uint8_t*)"\0", 1}).is_ok(),
              "failed to write test string");

  auto string_of_error = ma.read_c_string_to_vec(test_addr, max_size);
  ASSERT_TRUE(string_of_error.is_ok(), "error %d", string_of_error.error_value().error_code());
  ASSERT_EQ(fbl::String("foo"), string_of_error.value());

  // Expect success if the string spans over two mappings.
  ASSERT_EQ(addr + page_size, map_memory(*current_task, addr + page_size, page_size));
  // TODO: Adjacent private anonymous mappings are collapsed. To test this case this test needs to
  // provide a backing for the second mapping.
  // assert_eq!(mm.get_mapping_count(), 2);
  ASSERT_TRUE(ma.write_memory(addr + (page_size - 1), {(uint8_t*)"bar\0", 4}).is_ok(),
              "failed to write extra chars");

  string_of_error = ma.read_c_string_to_vec(test_addr, max_size);
  ASSERT_TRUE(string_of_error.is_ok(), "error %d", string_of_error.error_value().error_code());
  ASSERT_EQ(fbl::String("foobar"), string_of_error.value());

  // Expect error if the string exceeds max limit
  ASSERT_EQ(errno(ENAMETOOLONG), ma.read_c_string_to_vec(test_addr, 2).error_value());

  // Expect error if the address is invalid.
  ASSERT_EQ(errno(EFAULT), ma.read_c_string_to_vec(UserCString(), max_size).error_value());
}

TEST(MemoryManager, test_unmap_returned_mappings) {
  auto [kernel, current_task] = create_kernel_and_task();
  auto mm = current_task->mm();
  auto addr = map_memory(*current_task, 0, PAGE_SIZE * 2);

  fbl::Vector<fbl::RefPtr<Mapping>> released_mappings;
  auto unmap_result = mm->state.Write()->unmap(addr, PAGE_SIZE, released_mappings);
  ASSERT_TRUE(unmap_result.is_ok());
  ASSERT_EQ(released_mappings.size(), 1);
}

TEST(MemoryManager, test_unmap_returns_multiple_mappings) {
  auto [kernel, current_task] = create_kernel_and_task();
  auto mm = current_task->mm();
  auto addr = map_memory(*current_task, 0, PAGE_SIZE);
  map_memory(*current_task, addr.ptr() + 2 * PAGE_SIZE, PAGE_SIZE);

  fbl::Vector<fbl::RefPtr<Mapping>> released_mappings;
  auto unmap_result = mm->state.Write()->unmap(addr, PAGE_SIZE * 3, released_mappings);
  ASSERT_TRUE(unmap_result.is_ok());
  ASSERT_EQ(released_mappings.size(), 2);
}

}  // namespace starnix
