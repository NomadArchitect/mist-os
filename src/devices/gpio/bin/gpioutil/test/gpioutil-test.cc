// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "gpioutil.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/mock-function/mock-function.h>
#include <lib/zx/clock.h>
#include <zircon/types.h>

#include <zxtest/zxtest.h>

namespace {

using fuchsia_hardware_gpio::Gpio;

class FakeGpio : public fidl::WireServer<Gpio>,
                 public fidl::WireServer<fuchsia_hardware_pin::Debug> {
 public:
  explicit FakeGpio(async_dispatcher_t* dispatcher, uint32_t pin = 0,
                    std::string_view name = "NO_NAME")
      : dispatcher_(dispatcher), pin_(pin), name_(name) {}

  void GetProperties(GetPropertiesCompleter::Sync& completer) override {
    mock_get_pin_.Call();
    mock_get_name_.Call();
    fidl::Arena arena;
    auto properties = fuchsia_hardware_pin::wire::DebugGetPropertiesResponse::Builder(arena)
                          .pin(pin_)
                          .name(fidl::StringView::FromExternal(name_))
                          .Build();
    completer.Reply(properties);
  }

  void ConnectPin(fuchsia_hardware_pin::wire::DebugConnectPinRequest* request,
                  ConnectPinCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void ConnectGpio(fuchsia_hardware_pin::wire::DebugConnectGpioRequest* request,
                   ConnectGpioCompleter::Sync& completer) override {
    fidl::BindServer(dispatcher_, std::move(request->server), this);
    completer.ReplySuccess();
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_pin::Debug> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL("Unknown method ordinal 0x%016lx", metadata.method_ordinal);
  }

  void GetPin(GetPinCompleter::Sync& completer) override {
    mock_get_pin_.Call();
    completer.ReplySuccess(pin_);
  }
  void GetName(GetNameCompleter::Sync& completer) override {
    mock_get_name_.Call();
    completer.ReplySuccess(::fidl::StringView::FromExternal(name_));
  }
  void ConfigIn(ConfigInRequestView request, ConfigInCompleter::Sync& completer) override {
    if (request->flags != fuchsia_hardware_gpio::wire::GpioFlags::kNoPull) {
      completer.ReplyError(ZX_ERR_INVALID_ARGS);
      return;
    }
    mock_config_in_.Call();
    completer.ReplySuccess();
  }
  void ConfigOut(ConfigOutRequestView request, ConfigOutCompleter::Sync& completer) override {
    if (request->initial_value != 3) {
      completer.ReplyError(ZX_ERR_INVALID_ARGS);
      return;
    }
    mock_config_out_.Call();
    completer.ReplySuccess();
  }
  void Read(ReadCompleter::Sync& completer) override {
    mock_read_.Call();
    completer.ReplySuccess(true);
  }
  void Write(WriteRequestView request, WriteCompleter::Sync& completer) override {
    if (request->value != 7) {
      completer.ReplyError(ZX_ERR_INVALID_ARGS);
      return;
    }
    mock_write_.Call();
    completer.ReplySuccess();
  }
  void SetDriveStrength(SetDriveStrengthRequestView request,
                        SetDriveStrengthCompleter::Sync& completer) override {
    if (request->ds_ua != 2000) {
      completer.ReplyError(ZX_ERR_INVALID_ARGS);
      return;
    }
    mock_set_drive_strength_.Call();
    completer.ReplySuccess(2000);
  }
  void GetDriveStrength(GetDriveStrengthCompleter::Sync& completer) override {
    mock_get_drive_strength_.Call();
    completer.ReplySuccess(2000);
  }
  void GetInterrupt(GetInterruptRequestView request,
                    GetInterruptCompleter::Sync& completer) override {
    if (client_got_interrupt_) {
      return completer.ReplyError(ZX_ERR_ALREADY_BOUND);
    }

    zx::interrupt interrupt;
    zx_status_t status = zx::interrupt::create(zx::resource{}, 0, ZX_INTERRUPT_VIRTUAL, &interrupt);
    if (status != ZX_OK) {
      return completer.ReplyError(status);
    }

    // Trigger the interrupt before returning it so that the client's call to zx_interrupt_wait
    // completes immediately.
    if ((status = interrupt.trigger(0, zx::clock::get_monotonic())); status != ZX_OK) {
      return completer.ReplyError(status);
    }

    client_got_interrupt_ = true;
    mock_get_interrupt_.Call();
    completer.ReplySuccess(std::move(interrupt));
  }
  void ConfigureInterrupt(fuchsia_hardware_gpio::wire::GpioConfigureInterruptRequest* request,
                          ConfigureInterruptCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void ReleaseInterrupt(ReleaseInterruptCompleter::Sync& completer) override {
    if (!client_got_interrupt_) {
      return completer.ReplyError(ZX_ERR_NOT_FOUND);
    }

    mock_release_interrupt_.Call();
    completer.ReplySuccess();
  }
  void SetAltFunction(SetAltFunctionRequestView request,
                      SetAltFunctionCompleter::Sync& completer) override {
    mock_set_alt_function_.Call(request->function);
    completer.ReplySuccess();
  }
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_gpio::Gpio> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL("Unknown method ordinal 0x%016lx", metadata.method_ordinal);
  }

  mock_function::MockFunction<zx_status_t>& MockGetPin() { return mock_get_pin_; }
  mock_function::MockFunction<zx_status_t>& MockGetName() { return mock_get_name_; }
  mock_function::MockFunction<zx_status_t>& MockConfigIn() { return mock_config_in_; }
  mock_function::MockFunction<zx_status_t>& MockConfigOut() { return mock_config_out_; }
  mock_function::MockFunction<zx_status_t>& MockRead() { return mock_read_; }
  mock_function::MockFunction<zx_status_t>& MockWrite() { return mock_write_; }
  mock_function::MockFunction<zx_status_t>& MockSetDriveStrength() {
    return mock_set_drive_strength_;
  }
  mock_function::MockFunction<zx_status_t>& MockGetInterrupt() { return mock_get_interrupt_; }
  mock_function::MockFunction<zx_status_t>& MockReleaseInterrupt() {
    return mock_release_interrupt_;
  }
  mock_function::MockFunction<zx_status_t, uint64_t>& MockSetAltFunction() {
    return mock_set_alt_function_;
  }

 private:
  async_dispatcher_t* const dispatcher_;
  const uint32_t pin_;
  const std::string_view name_;
  mock_function::MockFunction<zx_status_t> mock_get_pin_;
  mock_function::MockFunction<zx_status_t> mock_get_name_;
  mock_function::MockFunction<zx_status_t> mock_config_in_;
  mock_function::MockFunction<zx_status_t> mock_config_out_;
  mock_function::MockFunction<zx_status_t> mock_read_;
  mock_function::MockFunction<zx_status_t> mock_write_;
  mock_function::MockFunction<zx_status_t> mock_set_drive_strength_;
  mock_function::MockFunction<zx_status_t> mock_get_drive_strength_;
  mock_function::MockFunction<zx_status_t> mock_get_interrupt_;
  mock_function::MockFunction<zx_status_t> mock_release_interrupt_;
  mock_function::MockFunction<zx_status_t, uint64_t> mock_set_alt_function_;
  bool client_got_interrupt_ = false;
};

class GpioUtilTest : public zxtest::Test {
 public:
  void SetUp() override {
    loop_ = std::make_unique<async::Loop>(&kAsyncLoopConfigAttachToCurrentThread);
    gpio_ = std::make_unique<FakeGpio>(loop_->dispatcher());

    zx::result server = fidl::CreateEndpoints(&client_);
    ASSERT_OK(server.status_value());
    fidl::BindServer(loop_->dispatcher(), std::move(server.value()), gpio_.get());

    ASSERT_OK(loop_->StartThread("gpioutil-test-loop"));
  }

  void TearDown() override {
    gpio_->MockConfigIn().VerifyAndClear();
    gpio_->MockConfigOut().VerifyAndClear();
    gpio_->MockRead().VerifyAndClear();
    gpio_->MockWrite().VerifyAndClear();
    gpio_->MockSetDriveStrength().VerifyAndClear();

    loop_->Shutdown();
  }

 protected:
  std::unique_ptr<async::Loop> loop_;
  fidl::ClientEnd<fuchsia_hardware_pin::Debug> client_;
  std::unique_ptr<FakeGpio> gpio_;
};

TEST_F(GpioUtilTest, GetNameTest) {
  int argc = 3;
  const char* argv[] = {"gpioutil", "n", "some_path"};

  GpioFunc func;
  uint8_t write_value, out_value;
  uint64_t ds_ua;
  fuchsia_hardware_gpio::wire::GpioFlags in_flag;
  uint32_t interrupt_flags;
  uint64_t alt_function;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, &write_value, &in_flag, &out_value,
                      &ds_ua, &interrupt_flags, &alt_function),
            0);
  EXPECT_EQ(func, 6);
  EXPECT_EQ(write_value, 0);
  EXPECT_EQ(in_flag, fuchsia_hardware_gpio::wire::GpioFlags::kNoPull);
  EXPECT_EQ(out_value, 0);
  EXPECT_EQ(ds_ua, 0);
  EXPECT_EQ(interrupt_flags, 0);
  EXPECT_EQ(alt_function, 0);

  gpio_->MockGetPin().ExpectCall(ZX_OK);
  gpio_->MockGetName().ExpectCall(ZX_OK);
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, write_value, in_flag,
                       out_value, ds_ua, interrupt_flags, alt_function),
            0);
}

TEST_F(GpioUtilTest, ReadTest) {
  int argc = 3;
  const char* argv[] = {"gpioutil", "r", "some_path"};

  GpioFunc func;
  uint8_t write_value, out_value;
  uint64_t ds_ua;
  fuchsia_hardware_gpio::wire::GpioFlags in_flag;
  uint32_t interrupt_flags;
  uint64_t alt_function;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, &write_value, &in_flag, &out_value,
                      &ds_ua, &interrupt_flags, &alt_function),
            0);
  EXPECT_EQ(func, 0);
  EXPECT_EQ(write_value, 0);
  EXPECT_EQ(in_flag, fuchsia_hardware_gpio::wire::GpioFlags::kNoPull);
  EXPECT_EQ(out_value, 0);
  EXPECT_EQ(ds_ua, 0);
  EXPECT_EQ(interrupt_flags, 0);
  EXPECT_EQ(alt_function, 0);

  gpio_->MockRead().ExpectCall(ZX_OK);
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, write_value, in_flag,
                       out_value, ds_ua, interrupt_flags, alt_function),
            0);
}

TEST_F(GpioUtilTest, WriteTest) {
  int argc = 4;
  const char* argv[] = {"gpioutil", "w", "some_path", "7"};

  GpioFunc func;
  uint8_t write_value, out_value;
  uint64_t ds_ua;
  fuchsia_hardware_gpio::wire::GpioFlags in_flag;
  uint32_t interrupt_flags;
  uint64_t alt_function;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, &write_value, &in_flag, &out_value,
                      &ds_ua, &interrupt_flags, &alt_function),
            0);
  EXPECT_EQ(func, 1);
  EXPECT_EQ(write_value, 7);
  EXPECT_EQ(in_flag, fuchsia_hardware_gpio::wire::GpioFlags::kNoPull);
  EXPECT_EQ(out_value, 0);
  EXPECT_EQ(ds_ua, 0);
  EXPECT_EQ(interrupt_flags, 0);
  EXPECT_EQ(alt_function, 0);

  gpio_->MockWrite().ExpectCall(ZX_OK);
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, write_value, in_flag,
                       out_value, ds_ua, interrupt_flags, alt_function),
            0);
}

TEST_F(GpioUtilTest, ConfigInTest) {
  int argc = 4;
  const char* argv[] = {"gpioutil", "i", "some_path", "none"};

  GpioFunc func;
  uint8_t write_value, out_value;
  uint64_t ds_ua;
  fuchsia_hardware_gpio::wire::GpioFlags in_flag;
  uint32_t interrupt_flags;
  uint64_t alt_function;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, &write_value, &in_flag, &out_value,
                      &ds_ua, &interrupt_flags, &alt_function),
            0);
  EXPECT_EQ(func, 2);
  EXPECT_EQ(write_value, 0);
  EXPECT_EQ(in_flag, fuchsia_hardware_gpio::wire::GpioFlags::kNoPull);
  EXPECT_EQ(out_value, 0);
  EXPECT_EQ(ds_ua, 0);
  EXPECT_EQ(interrupt_flags, 0);
  EXPECT_EQ(alt_function, 0);

  gpio_->MockConfigIn().ExpectCall(ZX_OK);
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, write_value, in_flag,
                       out_value, ds_ua, interrupt_flags, alt_function),
            0);
}

TEST_F(GpioUtilTest, ConfigOutTest) {
  int argc = 4;
  const char* argv[] = {"gpioutil", "o", "some_path", "3"};

  GpioFunc func;
  uint8_t write_value, out_value;
  uint64_t ds_ua;
  fuchsia_hardware_gpio::wire::GpioFlags in_flag;
  uint32_t interrupt_flags;
  uint64_t alt_function;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, &write_value, &in_flag, &out_value,
                      &ds_ua, &interrupt_flags, &alt_function),
            0);
  EXPECT_EQ(func, 3);
  EXPECT_EQ(write_value, 0);
  EXPECT_EQ(in_flag, fuchsia_hardware_gpio::wire::GpioFlags::kNoPull);
  EXPECT_EQ(out_value, 3);
  EXPECT_EQ(ds_ua, 0);
  EXPECT_EQ(interrupt_flags, 0);
  EXPECT_EQ(alt_function, 0);

  gpio_->MockConfigOut().ExpectCall(ZX_OK);
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, write_value, in_flag,
                       out_value, ds_ua, interrupt_flags, alt_function),
            0);
}

TEST_F(GpioUtilTest, SetDriveStrengthTest) {
  int argc = 4;
  const char* argv[] = {"gpioutil", "d", "some_path", "2000"};

  GpioFunc func;
  uint8_t write_value, out_value;
  uint64_t ds_ua;
  fuchsia_hardware_gpio::wire::GpioFlags in_flag;
  uint32_t interrupt_flags;
  uint64_t alt_function;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, &write_value, &in_flag, &out_value,
                      &ds_ua, &interrupt_flags, &alt_function),
            0);
  EXPECT_EQ(func, 4);
  EXPECT_EQ(write_value, 0);
  EXPECT_EQ(in_flag, fuchsia_hardware_gpio::wire::GpioFlags::kNoPull);
  EXPECT_EQ(out_value, 0);
  EXPECT_EQ(ds_ua, 2000);
  EXPECT_EQ(interrupt_flags, 0);
  EXPECT_EQ(alt_function, 0);

  gpio_->MockSetDriveStrength().ExpectCall(ZX_OK);
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, write_value, in_flag,
                       out_value, ds_ua, interrupt_flags, alt_function),
            0);
}

TEST_F(GpioUtilTest, InterruptTest) {
  int argc = 4;
  const char* argv[] = {"gpioutil", "q", "some_path", "level-low"};

  GpioFunc func;
  uint8_t write_value, out_value;
  uint64_t ds_ua;
  fuchsia_hardware_gpio::wire::GpioFlags in_flag;
  uint32_t interrupt_flags;
  uint64_t alt_function;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, &write_value, &in_flag, &out_value,
                      &ds_ua, &interrupt_flags, &alt_function),
            0);
  EXPECT_EQ(func, Interrupt);
  EXPECT_EQ(write_value, 0);
  EXPECT_EQ(in_flag, fuchsia_hardware_gpio::wire::GpioFlags::kNoPull);
  EXPECT_EQ(out_value, 0);
  EXPECT_EQ(ds_ua, 0);
  EXPECT_EQ(interrupt_flags, ZX_INTERRUPT_MODE_LEVEL_LOW);
  EXPECT_EQ(alt_function, 0);

  gpio_->MockGetInterrupt().ExpectCall(ZX_OK);
  gpio_->MockReleaseInterrupt().ExpectCall(ZX_OK);
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, write_value, in_flag,
                       out_value, ds_ua, interrupt_flags, alt_function),
            0);
}

TEST_F(GpioUtilTest, AltFunctionTest) {
  int argc = 4;
  const char* argv[] = {"gpioutil", "f", "some_path", "6"};

  GpioFunc func;
  uint8_t write_value, out_value;
  uint64_t ds_ua;
  fuchsia_hardware_gpio::wire::GpioFlags in_flag;
  uint32_t interrupt_flags;
  uint64_t alt_function;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, &write_value, &in_flag, &out_value,
                      &ds_ua, &interrupt_flags, &alt_function),
            0);
  EXPECT_EQ(func, AltFunction);
  EXPECT_EQ(write_value, 0);
  EXPECT_EQ(in_flag, fuchsia_hardware_gpio::wire::GpioFlags::kNoPull);
  EXPECT_EQ(out_value, 0);
  EXPECT_EQ(ds_ua, 0);
  EXPECT_EQ(interrupt_flags, 0);
  EXPECT_EQ(alt_function, 6);

  gpio_->MockSetAltFunction().ExpectCall(ZX_OK, 6);
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, write_value, in_flag,
                       out_value, ds_ua, interrupt_flags, alt_function),
            0);
}

}  // namespace
