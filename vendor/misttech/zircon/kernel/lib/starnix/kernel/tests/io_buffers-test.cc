// Copyright 2024 Mist Tecnologia LTDA. All rights reserved.
// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/mistos/starnix/kernel/mm/memory_accessor.h>
#include <lib/mistos/starnix/kernel/task/kernel.h>
#include <lib/mistos/starnix/kernel/task/task.h>
#include <lib/mistos/starnix/kernel/vfs/buffers/io_buffers.h>
#include <lib/mistos/starnix/testing/testing.h>
#include <lib/mistos/util/default_construct.h>
#include <lib/unittest/unittest.h>
#include <zircon/assert.h>

#include <fbl/alloc_checker.h>
#include <fbl/vector.h>
#include <ktl/span.h>

namespace unit_testing {

using starnix::TaskMemoryAccessor;
using starnix::VecInputBuffer;
using starnix::VecOutputBuffer;

bool test_data_input_buffer() {
  BEGIN_TEST;

  auto [kernel, current_task] = starnix::testing::create_kernel_task_and_unlocked();

  size_t page_size = PAGE_SIZE;
  auto addr = starnix::testing::map_memory(*current_task, mtl::DefaultConstruct<UserAddress>(),
                                           64ul * page_size);

  fbl::Vector<uint8_t> data;
  for (int i = 0; i < 1024; ++i) {
    fbl::AllocChecker ac;
    data.push_back(static_cast<uint8_t>(i % 256), &ac);
    ZX_DEBUG_ASSERT(ac.check());
  }

  auto& mm = *current_task;
  auto result = mm.write_memory(addr, data);
  ASSERT_TRUE(result.is_ok(), "failed to write test data");

  // Test incorrect callback.
  {
    util::SmallVector<UserBuffer, 1> input_iovec;
    input_iovec.push_back(UserBuffer{.address_ = addr, .length_ = 25});
    input_iovec.push_back(UserBuffer{.address_ = addr + 64ul, .length_ = 12});

    auto input_buffer = starnix::UserBuffersInputBuffer<TaskMemoryAccessor>::unified_new(
        mm, ktl::move(input_iovec));
    ASSERT_TRUE(input_buffer.is_ok(), "UserBuffersInputBuffer");
    ASSERT_TRUE(
        input_buffer->peek_each([](auto _data) { return fit::ok(_data.size() + 1); }).is_error());
  }

  // Test drain
  {
    util::SmallVector<UserBuffer, 1> input_iovec;
    input_iovec.push_back(UserBuffer{.address_ = addr, .length_ = 25});
    input_iovec.push_back(UserBuffer{.address_ = addr + 64ul, .length_ = 12});

    auto input_buffer = starnix::UserBuffersInputBuffer<TaskMemoryAccessor>::unified_new(
        mm, ktl::move(input_iovec));
    ASSERT_TRUE(input_buffer.is_ok(), "UserBuffersInputBuffer");
    ASSERT_EQ(37u, input_buffer->available());
    ASSERT_EQ(0u, input_buffer->bytes_read());
    ASSERT_EQ(37u, input_buffer->drain());
    ASSERT_EQ(0u, input_buffer->available());
    ASSERT_EQ(37u, input_buffer->bytes_read());
  }

  // Test read_all
  {
    util::SmallVector<UserBuffer, 1> input_iovec;
    input_iovec.push_back(UserBuffer{.address_ = addr, .length_ = 25});
    input_iovec.push_back(UserBuffer{.address_ = addr + 64ul, .length_ = 12});

    auto input_buffer = starnix::UserBuffersInputBuffer<TaskMemoryAccessor>::unified_new(
        mm, ktl::move(input_iovec));
    ASSERT_TRUE(input_buffer.is_ok(), "UserBuffersInputBuffer");
    ASSERT_EQ(37u, input_buffer->available());
    ASSERT_EQ(0u, input_buffer->bytes_read());

    auto buffer = input_buffer->read_all();
    ASSERT_TRUE(buffer.is_ok(), "read_all");

    ASSERT_EQ(0u, input_buffer->available());
    ASSERT_EQ(37u, input_buffer->bytes_read());

    ASSERT_EQ(37u, buffer->size());

    ASSERT_BYTES_EQ(data.data(), buffer->data(), 25);
    ASSERT_BYTES_EQ(data.data() + 64, buffer->data() + 25, 12);
  }

  // Test read
  {
    util::SmallVector<UserBuffer, 1> input_iovec;
    input_iovec.push_back(UserBuffer{.address_ = addr, .length_ = 25});
    input_iovec.push_back(UserBuffer{.address_ = addr + 64ul, .length_ = 12});

    auto input_buffer = starnix::UserBuffersInputBuffer<TaskMemoryAccessor>::unified_new(
        mm, ktl::move(input_iovec));
    ASSERT_TRUE(input_buffer.is_ok(), "UserBuffersInputBuffer");

    ktl::array<uint8_t, 50> buffer{0};
    ASSERT_EQ(37u, input_buffer->available());
    ASSERT_EQ(0u, input_buffer->bytes_read());

    ktl::span bytes_to_read1{buffer.data(), 20};
    auto read_result = input_buffer->read_exact(bytes_to_read1);
    ASSERT_TRUE(read_result.is_ok(), "read");
    ASSERT_EQ(20u, *read_result);
    ASSERT_EQ(17u, input_buffer->available());
    ASSERT_EQ(20u, input_buffer->bytes_read());
    ktl::span bytes_to_read2{buffer.data() + 20, 17};
    read_result = input_buffer->read_exact(bytes_to_read2);
    ASSERT_TRUE(read_result.is_ok(), "read");
    ASSERT_EQ(17u, *read_result);
    ktl::span bytes_to_read3{buffer.data() + 37, buffer.size() - 37};
    read_result = input_buffer->read_exact(bytes_to_read3);
    ASSERT_TRUE(read_result.is_error());
    ASSERT_EQ(0u, input_buffer->available());
    ASSERT_EQ(37u, input_buffer->bytes_read());
    ASSERT_BYTES_EQ(data.data(), buffer.data(), 25);
    ASSERT_BYTES_EQ(data.data() + 64, buffer.data() + 25, 12);
  }

  END_TEST;
}

bool test_data_output_buffer() {
  BEGIN_TEST;

  auto [kernel, current_task] = starnix::testing::create_kernel_task_and_unlocked();

  size_t page_size = PAGE_SIZE;
  auto addr = starnix::testing::map_memory(*current_task, mtl::DefaultConstruct<UserAddress>(),
                                           64ul * page_size);

  auto& mm = *current_task;
  fbl::Vector<uint8_t> data;
  for (int i = 0; i < 1024; ++i) {
    fbl::AllocChecker ac;
    data.push_back(static_cast<uint8_t>(i % 256), &ac);
    ZX_DEBUG_ASSERT(ac.check());
  }

  // Test incorrect callback.
  {
    util::SmallVector<UserBuffer, 1> output_iovec;
    output_iovec.push_back(UserBuffer{.address_ = addr, .length_ = 25});
    output_iovec.push_back(UserBuffer{.address_ = addr + 64ul, .length_ = 12});

    auto output_buffer = starnix::UserBuffersOutputBuffer<TaskMemoryAccessor>::unified_new(
        mm, ktl::move(output_iovec));
    ASSERT_TRUE(output_buffer.is_ok(), "UserBuffersOutputBuffer");
    ASSERT_TRUE(
        output_buffer->write_each([](auto data) { return fit::ok(data.size() + 1); }).is_error());
  }

  // Test write
  {
    util::SmallVector<UserBuffer, 1> output_iovec;
    output_iovec.push_back(UserBuffer{.address_ = addr, .length_ = 25});
    output_iovec.push_back(UserBuffer{.address_ = addr + 64ul, .length_ = 12});

    auto output_buffer = starnix::UserBuffersOutputBuffer<TaskMemoryAccessor>::unified_new(
        mm, ktl::move(output_iovec));
    ASSERT_TRUE(output_buffer.is_ok(), "UserBuffersOutputBuffer");
    ASSERT_EQ(37u, output_buffer->available());
    ASSERT_EQ(0u, output_buffer->bytes_written());
    ktl::span bytes_to_write1{data.data(), 20};
    auto write_result = output_buffer->write_all(bytes_to_write1);
    ASSERT_TRUE(write_result.is_ok(), "write");
    ASSERT_EQ(20u, write_result.value());
    ASSERT_EQ(17u, output_buffer->available());
    ASSERT_EQ(20u, output_buffer->bytes_written());
    ktl::span bytes_to_write2 = ktl::span{data.data() + 20, data.data() + 37};
    write_result = output_buffer->write_all(bytes_to_write2);
    ASSERT_TRUE(write_result.is_ok(), "write");
    ASSERT_EQ(17u, write_result.value());
    ASSERT_EQ(0u, output_buffer->available());
    ASSERT_EQ(37u, output_buffer->bytes_written());
    ktl::span bytes_to_write3 = ktl::span{data.data() + 37, data.data() + 50};
    write_result = output_buffer->write_all(bytes_to_write3);
    ASSERT_TRUE(write_result.is_error());

    auto buffer = current_task->read_memory_to_array<128>(addr);
    ASSERT_BYTES_EQ(data.data(), buffer->data(), 25);
    ASSERT_BYTES_EQ(data.data() + 25, buffer->data() + 64, 12);
  }

  END_TEST;
}

bool test_vec_input_buffer() {
  BEGIN_TEST;

  auto input_buffer1 = VecInputBuffer::New(
      ktl::span<const uint8_t>{reinterpret_cast<const uint8_t*>("helloworld"), 10});
  ASSERT_TRUE(input_buffer1
                  .peek_each([](const ktl::span<uint8_t>& data) -> fit::result<Errno, size_t> {
                    return fit::ok(data.size() + 1);
                  })
                  .is_error());

  auto input_buffer2 = VecInputBuffer::New(
      ktl::span<const uint8_t>{reinterpret_cast<const uint8_t*>("helloworld"), 10});
  ASSERT_EQ(0u, input_buffer2.bytes_read());
  ASSERT_EQ(10u, input_buffer2.available());
  ASSERT_EQ(10u, input_buffer2.drain());
  ASSERT_EQ(10u, input_buffer2.bytes_read());
  ASSERT_EQ(0u, input_buffer2.available());

  auto input_buffer3 = VecInputBuffer::New(
      ktl::span<const uint8_t>{reinterpret_cast<const uint8_t*>("helloworld"), 10});
  ASSERT_EQ(0u, input_buffer3.bytes_read());
  ASSERT_EQ(10u, input_buffer3.available());

  const char* ptr = "helloworld";
  fbl::AllocChecker ac;
  fbl::Vector<uint8_t> vec;
  vec.resize(10, &ac);
  ASSERT(ac.check());
  memcpy(vec.data(), ptr, 10);

  ASSERT_BYTES_EQ(vec.data(), input_buffer3.read_all().value_or(fbl::Vector<uint8_t>()).data(),
                  vec.size(), "read_all");

  ASSERT_EQ(10u, input_buffer3.bytes_read());
  ASSERT_EQ(0u, input_buffer3.available());

  auto input_buffer4 = VecInputBuffer::New(
      ktl::span<const uint8_t>{reinterpret_cast<const uint8_t*>("helloworld"), 10});
  uint8_t buffer[5];
  ktl::span<uint8_t> span{buffer, 5};
  ASSERT_EQ(5u, input_buffer4.read_exact(span).value_or(0), "read");
  ASSERT_EQ(5u, input_buffer4.bytes_read());
  ASSERT_EQ(5u, input_buffer4.available());
  ASSERT_BYTES_EQ((const uint8_t*)"hello", buffer, 5);
  ASSERT_EQ(5u, input_buffer4.read_exact(span).value_or(0), "read");
  ASSERT_EQ(10u, input_buffer4.bytes_read());
  ASSERT_EQ(0u, input_buffer4.available());
  ASSERT_BYTES_EQ((const uint8_t*)"world", buffer, 5);

  //  TODO (Herrera): Test read_object
  // input_buffer = VecInputBuffer::New(ktl::span<uint8_t>{(uint8_t*)"hello", 5});
  // ASSERT_EQ(0, input_buffer.bytes_read());
  // std::array<uint8_t, 3> buffer2 = input_buffer.read_object<std::array<uint8_t, 3>>().value();
  // ASSERT_EQ(3, input_buffer.bytes_read());

  END_TEST;
}

bool test_vec_output_buffer() {
  BEGIN_TEST;

  auto output_buffer = VecOutputBuffer::New(10);

  ASSERT_TRUE(output_buffer
                  .write_each([](ktl::span<uint8_t> data) -> fit::result<Errno, size_t> {
                    return fit::ok(data.size() + 1);
                  })
                  .is_error());
  ASSERT_EQ(0u, output_buffer.bytes_written());
  ASSERT_EQ(10u, output_buffer.available());
  ASSERT_EQ(5u, output_buffer.write_all(ktl::span<uint8_t>{(uint8_t*)"hello", 5}).value_or(0),
            "write");
  ASSERT_EQ(5u, output_buffer.bytes_written());
  ASSERT_EQ(5u, output_buffer.available());

  ASSERT_BYTES_EQ(reinterpret_cast<const uint8_t*>("hello"), output_buffer.data().data(),
                  output_buffer.data().size());

  ASSERT_EQ(5u, output_buffer.write_all(ktl::span<uint8_t>{(uint8_t*)"world", 5}).value_or(0),
            "write");
  ASSERT_EQ(10u, output_buffer.bytes_written());
  ASSERT_EQ(0u, output_buffer.available());
  ASSERT_BYTES_EQ(reinterpret_cast<const uint8_t*>("helloworld"), output_buffer.data().data(),
                  output_buffer.data().size());
  ASSERT_TRUE(output_buffer.write_all(ktl::span<uint8_t>{(uint8_t*)"foo", 3}).is_error());

  END_TEST;
}

bool test_vec_write_buffer() {
  BEGIN_TEST;

  auto input_buffer = VecInputBuffer::New({reinterpret_cast<const uint8_t*>("helloworld"), 10});
  auto output_buffer = VecOutputBuffer::New(20);
  ASSERT_EQ(10u, output_buffer.write_buffer(input_buffer).value(), "write_buffer");
  ASSERT_BYTES_EQ(reinterpret_cast<const uint8_t*>("helloworld"), output_buffer.data().data(),
                  output_buffer.data().size());

  END_TEST;
}

}  // namespace unit_testing

UNITTEST_START_TESTCASE(starnix_io_buffers)
UNITTEST("test data input buffer", unit_testing::test_data_input_buffer)
UNITTEST("test data output buffer", unit_testing::test_data_output_buffer)
UNITTEST("test vec input buffer", unit_testing::test_vec_input_buffer)
UNITTEST("test vec output buffer", unit_testing::test_vec_output_buffer)
UNITTEST("test vec write buffer", unit_testing::test_vec_write_buffer)
UNITTEST_END_TESTCASE(starnix_io_buffers, "starnix_io_buffers", "Tests IoBuffers")
