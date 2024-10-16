// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-clk.h"

#include <lib/async-loop/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/mmio-ptr/fake.h>
#include <lib/mmio/mmio-buffer.h>

#include <soc/aml-a311d/a311d-hw.h>
#include <soc/aml-meson/aml-clk-common.h>
#include <soc/aml-meson/axg-clk.h>
#include <soc/aml-meson/g12a-clk.h>
#include <soc/aml-meson/sm1-clk.h>
#include <soc/aml-s905d2/s905d2-hw.h>
#include <soc/aml-s905d3/s905d3-hw.h>
#include <soc/aml-s912/s912-hw.h>

#include "aml-axg-blocks.h"
#include "aml-g12a-blocks.h"
#include "aml-g12b-blocks.h"
#include "aml-sm1-blocks.h"
#include "src/devices/lib/mmio/test-helper.h"
#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/lib/testing/predicates/status.h"

namespace amlogic_clock::test {

class IncomingNamespace {
 public:
  void Init(fidl::ServerEnd<fuchsia_io::Directory> outgoing_server, fdf::MmioBuffer hiu_buffer,
            fdf::MmioBuffer dos_buffer, uint32_t pdev_did) {
    fdf_fake_platform_device::FakePDev::Config pdev_config{
        .device_info = std::make_optional<fdf::PDev::DeviceInfo>({.did = pdev_did})};
    pdev_config.mmios[AmlClock::kHiuMmio] = std::move(hiu_buffer);
    pdev_config.mmios[AmlClock::kDosbusMmio] = std::move(dos_buffer);
    ASSERT_OK(pdev_.SetConfig(std::move(pdev_config)));
    ASSERT_OK(outgoing_.AddService<fuchsia_hardware_platform_device::Service>(
        pdev_.GetInstanceHandler()));
    ASSERT_OK(outgoing_.Serve(std::move(outgoing_server)));
  }

 private:
  fdf_fake_platform_device::FakePDev pdev_;
  component::OutgoingDirectory outgoing_{async_get_default_dispatcher()};
};

class AmlClockTest : public gtest::TestLoopFixture {
 protected:
  static constexpr uint64_t kilohertz(uint64_t hz) { return hz * 1000; }
  static constexpr uint64_t megahertz(uint64_t hz) { return kilohertz(hz) * 1000; }
  static constexpr uint64_t gigahertz(uint64_t hz) { return megahertz(hz) * 1000; }

  static constexpr uint32_t kCpuClkSupportedFrequencies[] = {
      100'000'000,   250'000'000,   500'000'000,   667'000'000,   1'000'000'000, 1'200'000'000,
      1'398'000'000, 1'512'000'000, 1'608'000'000, 1'704'000'000, 1'896'000'000,
  };

  static bool MmioMemcmp(fdf::MmioBuffer& actual, std::unique_ptr<uint8_t[]>& expected) {
    for (size_t i = 0; i < actual.get_size(); i++) {
      if (actual.Read8(i) != expected[i]) {
        return true;
      }
    }
    return false;
  }

  static std::tuple<fdf::MmioView, fdf::MmioBuffer> MakeDosbusMmio() {
    auto dos_buffer = fdf_testing::CreateMmioBuffer(S912_DOS_LENGTH);
    return std::make_tuple(dos_buffer.View(0), std::move(dos_buffer));
  }

  void InitDriver(fdf::MmioBuffer hiu_buffer, uint32_t pdev_did) {
    auto [_, dos_buffer] = MakeDosbusMmio();
    InitDriver(std::move(hiu_buffer), std::move(dos_buffer), pdev_did);
  }

  void InitDriver(fdf::MmioBuffer hiu_buffer, fdf::MmioBuffer dos_buffer, uint32_t pdev_did) {
    ASSERT_OK(incoming_loop_.StartThread("incoming-namespace"));
    zx::result endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
    ASSERT_OK(endpoints);
    incoming_namespace_.SyncCall(&IncomingNamespace::Init, std::move(endpoints->server),
                                 std::move(hiu_buffer), std::move(dos_buffer), pdev_did);
    parent_->AddFidlService(fuchsia_hardware_platform_device::Service::Name,
                            std::move(endpoints->client));
    ASSERT_OK(AmlClock::Bind(nullptr, parent_.get()));
    auto aml_clock_device = parent_->GetLatestChild();
    ASSERT_NE(aml_clock_device, nullptr);
    ASSERT_OK(ddk::ClockImplProtocolClient::CreateFromDevice(aml_clock_device, &clock_impl_));
    ASSERT_TRUE(clock_impl_.is_valid());
  }

  void TestPlls(const uint32_t pdev_did) {
    auto ignored = std::make_unique<uint8_t[]>(S905D2_HIU_LENGTH);
    auto buffer = fdf_testing::CreateMmioBuffer(S905D2_HIU_LENGTH);

    InitDriver(std::move(buffer), pdev_did);

    constexpr uint16_t kPllStart = 0;
    constexpr uint16_t kPllEnd = HIU_PLL_COUNT;

    for (uint16_t i = kPllStart; i < kPllEnd; i++) {
      const uint32_t clkid = aml_clk_common::AmlClkId(i, aml_clk_common::aml_clk_type::kMesonPll);

      zx_status_t st;
      uint64_t best_supported_rate;
      constexpr uint64_t kMaxRateHz = gigahertz(1);
      st = clock_impl().QuerySupportedRate(clkid, kMaxRateHz, &best_supported_rate);
      EXPECT_OK(st);

      EXPECT_LE(best_supported_rate, kMaxRateHz);

      st = clock_impl().SetRate(clkid, best_supported_rate);
      EXPECT_OK(st);
    }
  }

  ddk::ClockImplProtocolClient& clock_impl() { return clock_impl_; }

 private:
  std::shared_ptr<MockDevice> parent_ = MockDevice::FakeRootParent();
  async::Loop incoming_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<IncomingNamespace> incoming_namespace_{
      incoming_loop_.dispatcher(), std::in_place};
  ddk::ClockImplProtocolClient clock_impl_;
};

namespace {

TEST_F(AmlClockTest, AxgEnableDisableAll) {
  auto expected = std::make_unique<uint8_t[]>(S912_HIU_LENGTH);
  auto buffer = fdf_testing::CreateMmioBuffer(S912_HIU_LENGTH);
  auto actual = buffer.View(0);

  InitDriver(std::move(buffer), PDEV_DID_AMLOGIC_AXG_CLK);

  // Initialization sets a bunch of registers that we don't care about, so we
  // can reset the array to a clean slate.
  memset(expected.get(), 0, S912_HIU_LENGTH);

  EXPECT_EQ(MmioMemcmp(actual, expected), 0);
  constexpr uint16_t kClkStart = 0;
  constexpr uint16_t kClkEnd = static_cast<uint16_t>(axg_clk::CLK_AXG_COUNT);

  for (uint16_t i = kClkStart; i < kClkEnd; ++i) {
    if (axg_clk_gates[i].register_set != kMesonRegisterSetHiu)
      continue;
    const uint32_t reg = axg_clk_gates[i].reg;
    const uint32_t bit = (1u << axg_clk_gates[i].bit);
    uint32_t* ptr = reinterpret_cast<uint32_t*>(&expected[reg]);
    (*ptr) |= bit;

    const uint32_t clk_i = aml_clk_common::AmlClkId(i, aml_clk_common::aml_clk_type::kMesonGate);
    zx_status_t st = clock_impl().Enable(clk_i);
    EXPECT_OK(st);
  }

  EXPECT_EQ(MmioMemcmp(actual, expected), 0);

  for (uint16_t i = kClkStart; i < kClkEnd; ++i) {
    if (axg_clk_gates[i].register_set != kMesonRegisterSetHiu)
      continue;
    const uint32_t reg = axg_clk_gates[i].reg;
    const uint32_t bit = (1u << axg_clk_gates[i].bit);
    uint32_t* ptr = reinterpret_cast<uint32_t*>(&expected[reg]);
    (*ptr) &= ~(bit);

    const uint32_t clk_i = aml_clk_common::AmlClkId(i, aml_clk_common::aml_clk_type::kMesonGate);
    zx_status_t st = clock_impl().Disable(clk_i);
    EXPECT_OK(st);
  }

  EXPECT_EQ(MmioMemcmp(actual, expected), 0);
}

TEST_F(AmlClockTest, G12aEnableDisableAll) {
  auto expected = std::make_unique<uint8_t[]>(S912_HIU_LENGTH);
  auto buffer = fdf_testing::CreateMmioBuffer(S912_HIU_LENGTH);
  auto actual = buffer.View(0);

  InitDriver(std::move(buffer), PDEV_DID_AMLOGIC_G12A_CLK);

  // Initialization sets a bunch of registers that we don't care about, so we
  // can reset the array to a clean slate.
  memset(expected.get(), 0, S905D2_HIU_LENGTH);

  EXPECT_EQ(MmioMemcmp(actual, expected), 0);

  constexpr uint16_t kClkStart = 0;
  constexpr uint16_t kClkEnd = static_cast<uint16_t>(g12a_clk::CLK_G12A_COUNT);

  for (uint16_t i = kClkStart; i < kClkEnd; ++i) {
    if (g12a_clk_gates[i].register_set != kMesonRegisterSetHiu)
      continue;
    const uint32_t reg = g12a_clk_gates[i].reg;
    const uint32_t bit = (1u << g12a_clk_gates[i].bit);
    uint32_t* ptr = reinterpret_cast<uint32_t*>(&expected[reg]);
    (*ptr) |= bit;

    const uint32_t clk_i = aml_clk_common::AmlClkId(i, aml_clk_common::aml_clk_type::kMesonGate);
    zx_status_t st = clock_impl().Enable(clk_i);
    EXPECT_OK(st);
  }

  EXPECT_EQ(MmioMemcmp(actual, expected), 0);

  for (uint16_t i = kClkStart; i < kClkEnd; ++i) {
    if (g12a_clk_gates[i].register_set != kMesonRegisterSetHiu)
      continue;
    const uint32_t reg = g12a_clk_gates[i].reg;
    const uint32_t bit = (1u << g12a_clk_gates[i].bit);
    uint32_t* ptr = reinterpret_cast<uint32_t*>(&expected[reg]);
    (*ptr) &= ~(bit);

    const uint32_t clk_i = aml_clk_common::AmlClkId(i, aml_clk_common::aml_clk_type::kMesonGate);
    zx_status_t st = clock_impl().Disable(clk_i);
    EXPECT_OK(st);
  }

  EXPECT_EQ(MmioMemcmp(actual, expected), 0);
}

TEST_F(AmlClockTest, Sm1EnableDisableAll) {
  auto expected = std::make_unique<uint8_t[]>(S905D3_HIU_LENGTH);
  auto buffer = fdf_testing::CreateMmioBuffer(S905D3_HIU_LENGTH);
  auto actual = buffer.View(0);

  InitDriver(std::move(buffer), PDEV_DID_AMLOGIC_SM1_CLK);

  // Initialization sets a bunch of registers that we don't care about, so we
  // can reset the array to a clean slate.
  memset(expected.get(), 0, S905D3_HIU_LENGTH);

  EXPECT_EQ(MmioMemcmp(actual, expected), 0);

  constexpr uint16_t kClkStart = 0;
  constexpr uint16_t kClkEnd = static_cast<uint16_t>(sm1_clk::CLK_SM1_GATE_COUNT);

  for (uint16_t i = kClkStart; i < kClkEnd; ++i) {
    if (sm1_clk_gates[i].register_set != kMesonRegisterSetHiu)
      continue;
    const uint32_t reg = sm1_clk_gates[i].reg;
    const uint32_t bit = (1u << sm1_clk_gates[i].bit);
    uint32_t* ptr = reinterpret_cast<uint32_t*>(&expected[reg]);
    (*ptr) |= bit;

    const uint32_t clk_i = aml_clk_common::AmlClkId(i, aml_clk_common::aml_clk_type::kMesonGate);
    zx_status_t st = clock_impl().Enable(clk_i);
    EXPECT_OK(st);
  }

  EXPECT_EQ(MmioMemcmp(actual, expected), 0);

  for (uint16_t i = kClkStart; i < kClkEnd; ++i) {
    if (sm1_clk_gates[i].register_set != kMesonRegisterSetHiu)
      continue;
    const uint32_t reg = sm1_clk_gates[i].reg;
    const uint32_t bit = (1u << sm1_clk_gates[i].bit);
    uint32_t* ptr = reinterpret_cast<uint32_t*>(&expected[reg]);
    (*ptr) &= ~(bit);

    const uint32_t clk_i = aml_clk_common::AmlClkId(i, aml_clk_common::aml_clk_type::kMesonGate);
    zx_status_t st = clock_impl().Disable(clk_i);
    EXPECT_OK(st);
  }

  EXPECT_EQ(MmioMemcmp(actual, expected), 0);
}

TEST_F(AmlClockTest, G12aEnableDos) {
  auto buffer = fdf_testing::CreateMmioBuffer(S905D2_HIU_LENGTH);
  auto actual = buffer.View(0);

  auto [dos_data, dos_buffer] = MakeDosbusMmio();
  InitDriver(std::move(buffer), std::move(dos_buffer), PDEV_DID_AMLOGIC_G12A_CLK);

  zx_status_t st = clock_impl().Enable(g12a_clk::CLK_DOS_GCLK_VDEC);
  EXPECT_OK(st);

  EXPECT_EQ(0x3ffu, dos_data.Read32(0x3f01 * sizeof(uint32_t)));
}

TEST_F(AmlClockTest, G12bEnableAudio) {
  auto buffer = fdf_testing::CreateMmioBuffer(A311D_HIU_LENGTH);
  auto actual = buffer.View(0);

  InitDriver(std::move(buffer), PDEV_DID_AMLOGIC_G12B_CLK);

  zx_status_t st = clock_impl().Enable(g12b_clk::G12B_CLK_AUDIO);
  EXPECT_OK(st);

  constexpr uint32_t kHhiGclkMpeg1Offset = 0x51;
  EXPECT_EQ(0x1u, actual.Read32(kHhiGclkMpeg1Offset * sizeof(uint32_t)));
}

TEST_F(AmlClockTest, G12bEnableEmmcC) {
  auto buffer = fdf_testing::CreateMmioBuffer(A311D_HIU_LENGTH);
  auto actual = buffer.View(0);

  InitDriver(std::move(buffer), PDEV_DID_AMLOGIC_G12B_CLK);

  zx_status_t st = clock_impl().Enable(g12b_clk::G12B_CLK_EMMC_C);
  EXPECT_OK(st);

  constexpr uint32_t kHhiGclkMpeg0Offset = 0x50;
  EXPECT_EQ(0x1u << 26, actual.Read32(kHhiGclkMpeg0Offset * sizeof(uint32_t)));
}

TEST_F(AmlClockTest, G12aSetRate) { TestPlls(PDEV_DID_AMLOGIC_G12A_CLK); }

TEST_F(AmlClockTest, G12bSetRate) { TestPlls(PDEV_DID_AMLOGIC_G12B_CLK); }

TEST_F(AmlClockTest, Sm1MuxRo) {
  auto buffer = fdf_testing::CreateMmioBuffer(S905D3_HIU_LENGTH);
  auto regs = buffer.View(0);

  InitDriver(std::move(buffer), PDEV_DID_AMLOGIC_SM1_CLK);

  // Ensure that SetInput fails for RO muxes.
  zx_status_t st = clock_impl().SetInput(sm1_clk::CLK_MPEG_CLK_SEL, 0);
  EXPECT_NE(st, ZX_OK);

  // Make sure we can read the number of parents.
  uint32_t out_num_inputs = 0;
  st = clock_impl().GetNumInputs(sm1_clk::CLK_MPEG_CLK_SEL, &out_num_inputs);
  EXPECT_OK(st);
  EXPECT_GT(out_num_inputs, 0u);

  // Make sure that we can read the current parent of the mux.
  uint32_t out_input = UINT32_MAX;
  st = clock_impl().GetInput(sm1_clk::CLK_MPEG_CLK_SEL, &out_input);
  EXPECT_OK(st);
  EXPECT_NE(out_input, UINT32_MAX);

  // Also ensure that we didn't whack any registers for a read-only mux.
  for (size_t i = 0; i < S905D3_HIU_LENGTH; i++) {
    EXPECT_EQ(0, regs.Read8(i));
  }
}

TEST_F(AmlClockTest, Sm1Mux) {
  constexpr uint32_t kTestMux = sm1_clk::CLK_CTS_VIPNANOQ_AXI_CLK_MUX;

  constexpr uint16_t kTestMuxIdx = aml_clk_common::AmlClkIndex(kTestMux);

  const meson_clk_mux_t& test_mux = sm1_muxes[kTestMuxIdx];

  auto buffer = fdf_testing::CreateMmioBuffer(S905D3_HIU_LENGTH);
  auto regs = buffer.View(0);

  InitDriver(std::move(buffer), PDEV_DID_AMLOGIC_SM1_CLK);

  const uint32_t newParentIdx = test_mux.n_inputs - 1;
  zx_status_t st = clock_impl().SetInput(kTestMux, newParentIdx);
  EXPECT_OK(st);

  const uint32_t actual_regval = regs.Read32((test_mux.reg >> 2) * sizeof(uint32_t));
  const uint32_t expected_regval = (newParentIdx & test_mux.mask) << test_mux.shift;
  EXPECT_EQ(expected_regval, actual_regval);

  // Make sure we can read the number of parents.
  uint32_t out_num_inputs = 0;
  st = clock_impl().GetNumInputs(kTestMux, &out_num_inputs);
  EXPECT_OK(st);
  EXPECT_EQ(out_num_inputs, test_mux.n_inputs);

  // Make sure that we can read the current parent of the mux.
  uint32_t out_input = UINT32_MAX;
  st = clock_impl().GetInput(kTestMux, &out_input);
  EXPECT_OK(st);
  EXPECT_EQ(out_input, newParentIdx);
}

TEST_F(AmlClockTest, TestCpuClkSetRate) {
  constexpr uint32_t kTestCpuClk = g12a_clk::CLK_SYS_CPU_CLK;

  auto regs = std::make_unique<uint8_t[]>(S905D2_HIU_LENGTH);
  auto buffer = fdf_testing::CreateMmioBuffer(S905D2_HIU_LENGTH);

  InitDriver(std::move(buffer), PDEV_DID_AMLOGIC_G12A_CLK);

  zx_status_t st;
  for (unsigned int kCpuClkSupportedFrequency : kCpuClkSupportedFrequencies) {
    st = clock_impl().SetRate(kTestCpuClk, kCpuClkSupportedFrequency);
    EXPECT_OK(st);

    st = clock_impl().SetRate(kTestCpuClk, kCpuClkSupportedFrequency + 1);
    EXPECT_NE(st, ZX_OK);
  }
}

TEST_F(AmlClockTest, TestCpuClkQuerySupportedRates) {
  constexpr uint32_t kTestCpuClk = g12a_clk::CLK_SYS_CPU_CLK;
  constexpr uint32_t kJustOver1GHz = gigahertz(1) + 1;

  auto regs = std::make_unique<uint8_t[]>(S905D2_HIU_LENGTH);
  auto buffer = fdf_testing::CreateMmioBuffer(S905D2_HIU_LENGTH);

  InitDriver(std::move(buffer), PDEV_DID_AMLOGIC_G12A_CLK);

  uint64_t rate;
  zx_status_t st = clock_impl().QuerySupportedRate(kTestCpuClk, kJustOver1GHz, &rate);
  EXPECT_OK(st);
  EXPECT_EQ(rate, gigahertz(1));
}

TEST_F(AmlClockTest, TestCpuClkGetRate) {
  constexpr uint32_t kTestCpuClk = g12a_clk::CLK_SYS_CPU_CLK;
  constexpr uint32_t kOneGHz = gigahertz(1);

  auto regs = std::make_unique<uint8_t[]>(S905D2_HIU_LENGTH);
  auto buffer = fdf_testing::CreateMmioBuffer(S905D2_HIU_LENGTH);

  InitDriver(std::move(buffer), PDEV_DID_AMLOGIC_G12A_CLK);
  zx_status_t st;

  st = clock_impl().SetRate(kTestCpuClk, kOneGHz);
  EXPECT_OK(st);

  uint64_t rate;
  st = clock_impl().GetRate(kTestCpuClk, &rate);
  EXPECT_OK(st);
  EXPECT_EQ(rate, kOneGHz);
}

TEST_F(AmlClockTest, TestCpuClkG12b) {
  constexpr uint32_t kTestCpuBigClk = g12b_clk::CLK_SYS_CPU_BIG_CLK;
  constexpr uint32_t kTestCpuLittleClk = g12b_clk::CLK_SYS_CPU_LITTLE_CLK;
  constexpr uint32_t kBigClockTestFreq = gigahertz(1);
  constexpr uint32_t kLittleClockTestFreq = megahertz(1800);

  auto regs = std::make_unique<uint8_t[]>(S905D2_HIU_LENGTH);
  auto buffer = fdf_testing::CreateMmioBuffer(S905D2_HIU_LENGTH);

  InitDriver(std::move(buffer), PDEV_DID_AMLOGIC_G12B_CLK);

  zx_status_t st;

  st = clock_impl().SetRate(kTestCpuBigClk, kBigClockTestFreq);
  EXPECT_OK(st);

  uint64_t rate;
  st = clock_impl().GetRate(kTestCpuBigClk, &rate);
  EXPECT_OK(st);
  EXPECT_EQ(rate, kBigClockTestFreq);

  st = clock_impl().SetRate(kTestCpuLittleClk, kLittleClockTestFreq);
  EXPECT_OK(st);

  st = clock_impl().GetRate(kTestCpuLittleClk, &rate);
  EXPECT_OK(st);
  EXPECT_EQ(rate, kLittleClockTestFreq);
}

TEST_F(AmlClockTest, DisableRefZero) {
  // Attempts to disable a clock that has never been enabled.
  // Confirm that this is fatal.
  auto expected = std::make_unique<uint8_t[]>(S905D2_HIU_LENGTH);
  auto buffer = fdf_testing::CreateMmioBuffer(S905D2_HIU_LENGTH);
  auto actual = buffer.View(0);

  InitDriver(std::move(buffer), PDEV_DID_AMLOGIC_G12A_CLK);

  // Initialization sets a bunch of registers that we don't care about, so we
  // can reset the array to a clean slate.
  memset(expected.get(), 0, S905D2_HIU_LENGTH);

  EXPECT_EQ(MmioMemcmp(actual, expected), 0);

  constexpr uint16_t kTestClock = 0;
  const uint32_t clk_id =
      aml_clk_common::AmlClkId(kTestClock, aml_clk_common::aml_clk_type::kMesonGate);

  auto assert_death_regex =
      "ASSERT FAILED at \\(.*\\): \\(.*\\)Cannot disable already disabled clock. clkid = " +
      std::to_string(kTestClock);
  ASSERT_DEATH(clock_impl().Disable(clk_id), assert_death_regex);
}

TEST_F(AmlClockTest, EnableDisableRefCount) {
  zx_status_t st;

  auto buffer = fdf_testing::CreateMmioBuffer(S905D2_HIU_LENGTH);
  auto actual = buffer.View(0);

  InitDriver(std::move(buffer), PDEV_DID_AMLOGIC_G12A_CLK);

  // Initialization sets a bunch of registers that we don't care about, so we
  // can reset the array to a clean slate.
  constexpr uint16_t kTestClock = 0;
  constexpr uint32_t kTestClockId =
      aml_clk_common::AmlClkId(kTestClock, aml_clk_common::aml_clk_type::kMesonGate);
  constexpr uint32_t reg = g12a_clk_gates[kTestClock].reg;
  constexpr uint32_t bit = (1u << g12a_clk_gates[kTestClock].bit);

  // Enable the test clock and verify that the register was written.
  st = clock_impl().Enable(kTestClockId);
  EXPECT_OK(st);
  EXPECT_EQ(actual.Read32(reg), bit);

  // Zero out the register and enable the clock again. Since the driver
  // should already think the clock is enabled, this should be a no-op.
  actual.Write32(0, reg);
  st = clock_impl().Enable(kTestClockId);
  EXPECT_OK(st);
  EXPECT_EQ(actual.Read32(reg), 0u);  // Make sure the driver didn't touch the register.

  // This time we fill the registers with Fs and try disabling the clock. Since
  // it's been enabled twice, the first disable should only drop a ref rather than
  // actually disabling clock hardware.
  actual.Write32(0xffffffff, reg);
  st = clock_impl().Disable(kTestClockId);
  EXPECT_OK(st);
  EXPECT_EQ(actual.Read32(reg), 0xffffffff);  // Make sure the driver didn't touch the register.

  // The second call to disable should actually disable the clock hardware.
  actual.Write32(0xffffffff, reg);
  st = clock_impl().Disable(kTestClockId);
  EXPECT_OK(st);
  EXPECT_EQ(actual.Read32(reg),
            (0xffffffff & (~bit)));  // Make sure the driver actually disabled the hardware.
}

}  // anonymous namespace

}  // namespace amlogic_clock::test
