// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/clock.h>

#include <functional>
#include <memory>
#include <utility>

#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-env.h"
#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-frame.h"
#include "src/connectivity/wlan/drivers/testing/lib/sim-fake-ap/sim-fake-ap.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/cfg80211.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim_device.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim_utils.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"
#include "src/connectivity/wlan/lib/common/cpp/include/wlan/common/macaddr.h"

namespace wlan::brcmfmac {

using simulation::InformationElement;
using simulation::SimBeaconFrame;

class PassiveScanTest;

struct ApInfo {
  explicit ApInfo(simulation::Environment* env, const common::MacAddr& bssid,
                  const fuchsia_wlan_ieee80211::Ssid& ssid, const wlan_common::WlanChannel& channel)
      : ap_(env, bssid, ssid, channel) {}

  simulation::FakeAp ap_;
  size_t beacons_seen_count_ = 0;
};

class PassiveScanTestInterface : public SimInterface {
 public:
  // TODO(https://fxbug.dev/https://fxbug.dev/42164585): Align the way active_scan_test and
  // passive_scan_test verify scan results.

  // Add a functor that can be run on each scan result by the VerifyScanResult method.
  // This allows scan results to be inspected (e.g. with EXPECT_EQ) as they come in, rather than
  // storing scan results for analysis after the sim env run has completed.
  void AddVerifierFunction(
      std::function<void(const wlan_fullmac_wire::WlanFullmacImplIfcOnScanResultRequest*)>);

  // Remove any verifier functions from the object.
  void ClearVerifierFunction();

  // Run the verifier method (if one was added) on the given scan result.
  void VerifyScanResult(wlan_fullmac_wire::WlanFullmacImplIfcOnScanResultRequest* result);

  void OnScanResult(OnScanResultRequestView request,
                    OnScanResultCompleter::Sync& completer) override;

  PassiveScanTest* test_ = nullptr;

 private:
  std::function<void(const wlan_fullmac_wire::WlanFullmacImplIfcOnScanResultRequest*)> verifier_fn_;
};

class PassiveScanTest : public SimTest {
 public:
  // Set our beacon interval to 80% of the passive scan dwell time
  static constexpr zx::duration kBeaconInterval =
      zx::msec((SimInterface::kDefaultPassiveScanDwellTimeMs / 5) * 4);

  void SetUp() override;

  // Create a new AP with the specified parameters, and tell it to start beaconing.
  void StartFakeAp(const common::MacAddr& bssid, const fuchsia_wlan_ieee80211::Ssid& ssid,
                   const wlan_common::WlanChannel& channel,
                   zx::duration beacon_interval = kBeaconInterval);

  // Start a fake AP with a beacon mutator that will be applied to each beacon before it is sent.
  // The fake AP will begin beaconing immediately.
  void StartFakeApWithErrInjBeacon(
      const common::MacAddr& bssid, const fuchsia_wlan_ieee80211::Ssid& ssid,
      const wlan_common::WlanChannel& channel,
      std::function<SimBeaconFrame(const SimBeaconFrame&)> beacon_mutator,
      zx::duration beacon_interval = kBeaconInterval);

  // All simulated APs
  std::list<std::unique_ptr<ApInfo>> aps_;

 protected:
  // This is the interface we will use for our single client interface
  PassiveScanTestInterface client_ifc_;
};

void PassiveScanTest::SetUp() {
  ASSERT_EQ(SimTest::Init(), ZX_OK);
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc_), ZX_OK);
  client_ifc_.test_ = this;
  client_ifc_.ClearVerifierFunction();
}

void PassiveScanTest::StartFakeAp(const common::MacAddr& bssid,
                                  const fuchsia_wlan_ieee80211::Ssid& ssid,
                                  const wlan_common::WlanChannel& channel,
                                  zx::duration beacon_interval) {
  auto ap_info = std::make_unique<ApInfo>(env_.get(), bssid, ssid, channel);
  ap_info->ap_.EnableBeacon(beacon_interval);
  aps_.push_back(std::move(ap_info));
}

void PassiveScanTest::StartFakeApWithErrInjBeacon(
    const common::MacAddr& bssid, const fuchsia_wlan_ieee80211::Ssid& ssid,
    const wlan_common::WlanChannel& channel,
    std::function<SimBeaconFrame(const SimBeaconFrame&)> beacon_mutator,
    zx::duration beacon_interval) {
  auto ap_info = std::make_unique<ApInfo>(env_.get(), bssid, ssid, channel);
  ap_info->ap_.AddErrInjBeacon(beacon_mutator);
  ap_info->ap_.EnableBeacon(beacon_interval);
  aps_.push_back(std::move(ap_info));
}

void PassiveScanTestInterface::AddVerifierFunction(
    std::function<void(const wlan_fullmac_wire::WlanFullmacImplIfcOnScanResultRequest*)>
        verifier_fn) {
  verifier_fn_ = std::move(verifier_fn);
}

void PassiveScanTestInterface::ClearVerifierFunction() { verifier_fn_ = nullptr; }

void PassiveScanTestInterface::VerifyScanResult(
    wlan_fullmac_wire::WlanFullmacImplIfcOnScanResultRequest* result) {
  if (verifier_fn_ != nullptr) {
    verifier_fn_(result);
  }
}

// Verify that each incoming scan result is as expected, using VerifyScanResult.
void PassiveScanTestInterface::OnScanResult(OnScanResultRequestView request,
                                            OnScanResultCompleter::Sync& completer) {
  SimInterface::OnScanResult(request, completer);
  VerifyScanResult(request);
}

constexpr wlan_common::WlanChannel kDefaultChannel = {
    .primary = 9, .cbw = wlan_common::ChannelBandwidth::kCbw40, .secondary80 = 0};
const common::MacAddr kDefaultBssid({0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc});

TEST_F(PassiveScanTest, BasicFunctionality) {
  const int64_t test_start_timestamp_nanos = zx::clock::get_monotonic().get();
  constexpr zx::duration kScanStartTime = zx::sec(1);
  constexpr zx::duration kDefaultTestDuration = zx::sec(100);
  constexpr uint64_t kScanId = 0x1248;

  // Start up a single AP
  StartFakeAp(kDefaultBssid, kDefaultSsid, kDefaultChannel);

  // // Request a future scan
  env_->ScheduleNotification(std::bind(&PassiveScanTestInterface::StartScan, &client_ifc_, kScanId,
                                       false, std::optional<const std::vector<uint8_t>>{}),
                             kScanStartTime);

  // The lambda arg will be run on each result, inside PassiveScanTestInterface::VerifyScanResults.
  client_ifc_.AddVerifierFunction(
      [&test_start_timestamp_nanos](
          const wlan_fullmac_wire::WlanFullmacImplIfcOnScanResultRequest* result) {
        // Verify timestamp is after test start
        ASSERT_GT(result->timestamp_nanos(), test_start_timestamp_nanos);

        // Verify BSSID.
        ASSERT_EQ(sizeof(result->bss().bssid.data()), sizeof(common::MacAddr::byte));
        const common::MacAddr result_bssid(result->bss().bssid.data());
        EXPECT_EQ(result_bssid.Cmp(kDefaultBssid), 0);

        // Verify SSID.
        auto ssid = brcmf_find_ssid_in_ies(result->bss().ies.data(), result->bss().ies.count());
        EXPECT_EQ(ssid, kDefaultSsid);

        // Verify channel
        EXPECT_EQ(result->bss().channel.primary, kDefaultChannel.primary);
        EXPECT_EQ(result->bss().channel.cbw, kDefaultChannel.cbw);
        EXPECT_EQ(result->bss().channel.secondary80, kDefaultChannel.secondary80);

        // Verify has RSSI value
        ASSERT_LT(result->bss().rssi_dbm, 0);
        ASSERT_GE(result->bss().snr_db, sim_utils::SnrDbFromSignalStrength(
                                            result->bss().rssi_dbm, simulation::kNoiseLevel));
      });

  env_->Run(kDefaultTestDuration);
}

// TODO(https://fxbug.dev/42170829): The correct behavior is to default to scanning all supported
// channels.
TEST_F(PassiveScanTest, EmptyChannelList) {
  constexpr zx::duration kScanStartTime = zx::sec(1);
  constexpr zx::duration kDefaultTestDuration = zx::sec(100);
  constexpr uint64_t kScanId = 0x2012;

  // Start up a single AP
  StartFakeAp(kDefaultBssid, kDefaultSsid, kDefaultChannel);

  // Request a future scan with an empty channel list
  env_->ScheduleNotification(std::bind(&PassiveScanTestInterface::StartScan, &client_ifc_, kScanId,
                                       false, std::optional<const std::vector<uint8_t>>{{}}),
                             kScanStartTime);

  // The driver should exit early and return no scan results.
  client_ifc_.AddVerifierFunction(
      [](const wlan_fullmac_wire::WlanFullmacImplIfcOnScanResultRequest* result) { FAIL(); });

  env_->Run(kDefaultTestDuration);

  auto result_code = client_ifc_.ScanResultCode(kScanId);
  ASSERT_TRUE(result_code.has_value());
  ASSERT_EQ(result_code.value(), wlan_fullmac_wire::WlanScanResult::kInvalidArgs);
}

TEST_F(PassiveScanTest, ScanWithMalformedBeaconMissingSsidInformationElement) {
  const int64_t test_start_timestamp_nanos = zx::clock::get_monotonic().get();
  constexpr zx::duration kScanStartTime = zx::sec(1);
  constexpr zx::duration kDefaultTestDuration = zx::sec(100);
  constexpr uint64_t kScanId = 0x1248;

  // Functor that will remove the SSID information element from a beacon frame.
  auto beacon_mutator = [](const SimBeaconFrame& beacon) {
    auto tmp_beacon(beacon);
    tmp_beacon.RemoveIe(InformationElement::IE_TYPE_SSID);
    return tmp_beacon;
  };

  // Start up a single AP, with beacon error injection.
  StartFakeApWithErrInjBeacon(kDefaultBssid, kDefaultSsid, kDefaultChannel, beacon_mutator);

  // Request a future scan
  env_->ScheduleNotification(std::bind(&PassiveScanTestInterface::StartScan, &client_ifc_, kScanId,
                                       false, std::optional<const std::vector<uint8_t>>{}),
                             kScanStartTime);

  client_ifc_.AddVerifierFunction(
      [&test_start_timestamp_nanos](
          const wlan_fullmac_wire::WlanFullmacImplIfcOnScanResultRequest* result) {
        // Verify timestamp is after test start
        ASSERT_GT(result->timestamp_nanos(), test_start_timestamp_nanos);

        // Verify BSSID.
        ASSERT_EQ(result->bss().bssid.size(), sizeof(common::MacAddr::byte));
        const common::MacAddr result_bssid(result->bss().bssid.data());
        EXPECT_EQ(result_bssid.Cmp(kDefaultBssid), 0);

        // Verify that SSID is empty, since there was no SSID IE.
        auto ssid = brcmf_find_ssid_in_ies(result->bss().ies.data(), result->bss().ies.count());
        EXPECT_EQ(ssid.size(), 0u);

        // Verify channel
        EXPECT_EQ(result->bss().channel.primary, kDefaultChannel.primary);
        EXPECT_EQ(result->bss().channel.cbw, kDefaultChannel.cbw);
        EXPECT_EQ(result->bss().channel.secondary80, kDefaultChannel.secondary80);

        // Verify has RSSI value
        ASSERT_LT(result->bss().rssi_dbm, 0);
      });

  env_->Run(kDefaultTestDuration);
}

// This test case verifies that the driver returns SHOULD_WAIT as the scan result code when firmware
// is busy.
TEST_F(PassiveScanTest, ScanWhenFirmwareBusy) {
  constexpr zx::duration kScanStartTime = zx::sec(1);
  constexpr zx::duration kDefaultTestDuration = zx::sec(100);
  constexpr uint64_t kScanId = 0x1248;

  // Start up a single AP, with beacon error injection.
  StartFakeAp(kDefaultBssid, kDefaultSsid, kDefaultChannel);

  // Set up our injector
  WithSimDevice([](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjIovar("escan", ZX_OK, BCME_BUSY);
  });

  // Request a future scan
  env_->ScheduleNotification(std::bind(&PassiveScanTestInterface::StartScan, &client_ifc_, kScanId,
                                       false, std::optional<const std::vector<uint8_t>>{}),
                             kScanStartTime);

  env_->Run(kDefaultTestDuration);

  EXPECT_EQ(client_ifc_.ScanResultList(kScanId)->size(), 0U);
  ASSERT_NE(client_ifc_.ScanResultCode(kScanId), std::nullopt);
  EXPECT_EQ(client_ifc_.ScanResultCode(kScanId).value(),
            wlan_fullmac_wire::WlanScanResult::kShouldWait);
}

TEST_F(PassiveScanTest, ScanWhileAssocInProgress) {
  // Scan request for driver should come before connection succeeds.
  constexpr zx::duration kScanStartTime = zx::msec(3);
  constexpr zx::duration kAssocStartTime = zx::msec(1);
  constexpr zx::duration kDefaultTestDuration = zx::sec(100);
  constexpr uint64_t kScanId = 0x1248;

  // Start up an AP for association.
  StartFakeAp(kDefaultBssid, kDefaultSsid, kDefaultChannel);

  client_ifc_.AssociateWith(aps_.front()->ap_, kAssocStartTime);
  // Request a future scan
  env_->ScheduleNotification(std::bind(&PassiveScanTestInterface::StartScan, &client_ifc_, kScanId,
                                       false, std::optional<const std::vector<uint8_t>>{}),
                             kScanStartTime);

  env_->Run(kDefaultTestDuration);

  EXPECT_EQ(client_ifc_.ScanResultList(kScanId)->size(), 0U);
  ASSERT_NE(client_ifc_.ScanResultCode(kScanId), std::nullopt);
  EXPECT_EQ(client_ifc_.ScanResultCode(kScanId).value(),
            wlan_fullmac_wire::WlanScanResult::kShouldWait);
}

TEST_F(PassiveScanTest, ScanAbortedInFirmware) {
  // Assoc request for driver should come while scanning.
  constexpr zx::duration kScanStartTime = zx::msec(1);
  constexpr zx::duration kAssocStartTime = zx::msec(10);
  constexpr zx::duration kDefaultTestDuration = zx::sec(100);
  constexpr uint64_t kScanId = 0x1248;

  // Start up an AP for association.
  StartFakeAp(kDefaultBssid, kDefaultSsid, kDefaultChannel);

  // Request a future scan
  env_->ScheduleNotification(std::bind(&PassiveScanTestInterface::StartScan, &client_ifc_, kScanId,
                                       false, std::optional<const std::vector<uint8_t>>{}),
                             kScanStartTime);

  // Request an association right after the scan
  client_ifc_.AssociateWith(aps_.front()->ap_, kAssocStartTime);

  env_->Run(kDefaultTestDuration);

  EXPECT_EQ(client_ifc_.ScanResultList(kScanId)->size(), 0U);
  ASSERT_NE(client_ifc_.ScanResultCode(kScanId), std::nullopt);
  EXPECT_EQ(client_ifc_.ScanResultCode(kScanId).value(),
            wlan_fullmac_wire::WlanScanResult::kCanceledByDriverOrFirmware);
}
}  // namespace wlan::brcmfmac
