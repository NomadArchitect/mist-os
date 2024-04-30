// Copyright 2024 Mist Tecnologia LTDA. All rights reserved.
// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/mistos/starnix/kernel/vfs/buffers/io_buffers.h"

#include <array>

#include <ktl/span.h>
#include <zxtest/zxtest.h>

namespace starnix {

TEST(IoBuffers, test_vec_input_buffer) {
  auto input_buffer = VecInputBuffer::New(ktl::span<uint8_t>{(uint8_t*)"helloworld", 10});
  ASSERT_TRUE(input_buffer
                  .peek_each([](const ktl::span<uint8_t>& data) -> fit::result<Errno, size_t> {
                    return fit::ok(data.size() + 1);
                  })
                  .is_error());

  input_buffer = VecInputBuffer::New(ktl::span<uint8_t>{(uint8_t*)"helloworld", 10});
  ASSERT_EQ(0, input_buffer.bytes_read());
  ASSERT_EQ(10, input_buffer.available());
  ASSERT_EQ(10, input_buffer.drain());
  ASSERT_EQ(10, input_buffer.bytes_read());
  ASSERT_EQ(0, input_buffer.available());

  input_buffer = VecInputBuffer::New(ktl::span<uint8_t>{(uint8_t*)"helloworld", 10});
  ASSERT_EQ(0, input_buffer.bytes_read());
  ASSERT_EQ(10, input_buffer.available());

  const char* ptr = "helloworld";
  ASSERT_EQ(std::vector<uint8_t>(ptr, ptr + 10),
            input_buffer.read_all().value_or(std::vector<uint8_t>()), "read_all");
  ASSERT_EQ(10, input_buffer.bytes_read());
  ASSERT_EQ(0, input_buffer.available());

  input_buffer = VecInputBuffer::New(ktl::span<uint8_t>{(uint8_t*)"helloworld", 10});
  uint8_t buffer[5];
  ASSERT_EQ(5, input_buffer.read_exact(ktl::span<uint8_t>{buffer, 5}).value_or(0), "read");
  ASSERT_EQ(5, input_buffer.bytes_read());
  ASSERT_EQ(5, input_buffer.available());
  ASSERT_BYTES_EQ("hello", buffer, 5);
  ASSERT_EQ(5, input_buffer.read_exact(ktl::span<uint8_t>{buffer, 5}).value_or(0), "read");
  ASSERT_EQ(10, input_buffer.bytes_read());
  ASSERT_EQ(0, input_buffer.available());
  ASSERT_BYTES_EQ("world", buffer, 5);

  //  TODO (Herrera): Test read_object
  // input_buffer = VecInputBuffer::New(ktl::span<uint8_t>{(uint8_t*)"hello", 5});
  // ASSERT_EQ(0, input_buffer.bytes_read());
  // std::array<uint8_t, 3> buffer2 = input_buffer.read_object<std::array<uint8_t, 3>>().value();
  // ASSERT_EQ(3, input_buffer.bytes_read());
}

TEST(IoBuffers, test_vec_output_buffer) {
  auto output_buffer = VecOutputBuffer::New(10);

  ASSERT_TRUE(output_buffer
                  .write_each([](ktl::span<uint8_t> data) -> fit::result<Errno, size_t> {
                    return fit::ok(data.size() + 1);
                  })
                  .is_error());
  ASSERT_EQ(0, output_buffer.bytes_written());
  ASSERT_EQ(10, output_buffer.available());
  ASSERT_EQ(5, output_buffer.write_all(ktl::span<uint8_t>{(uint8_t*)"hello", 5}).value_or(0),
            "write");
  ASSERT_EQ(5, output_buffer.bytes_written());
  ASSERT_EQ(5, output_buffer.available());

  ASSERT_BYTES_EQ("hello", output_buffer.data(), 5);

  ASSERT_EQ(5, output_buffer.write_all(ktl::span<uint8_t>{(uint8_t*)"world", 5}).value_or(0),
            "write");
  ASSERT_EQ(10, output_buffer.bytes_written());
  ASSERT_EQ(0, output_buffer.available());
  ASSERT_BYTES_EQ("helloworld", output_buffer.data(), 10);
  ASSERT_TRUE(output_buffer.write_all(ktl::span<uint8_t>{(uint8_t*)"foo", 3}).is_error());
}

TEST(IoBuffers, test_vec_write_buffer) {
  ktl::span<uint8_t> data{(uint8_t*)"helloworld", 10};
  auto input_buffer = VecInputBuffer::New(data);
  auto output_buffer = VecOutputBuffer::New(20);
  ASSERT_EQ(10, output_buffer.write_buffer(&input_buffer).value(), "write_buffer");
  ASSERT_EQ(0, memcmp(data.data(), output_buffer.data(), data.size()));
}

}  // namespace starnix
