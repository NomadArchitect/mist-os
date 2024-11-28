// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/assert.h>
#include <zircon/errors.h>

#include <wlan/common/band.h>
#include <wlan/common/channel.h>

#include "fidl/fuchsia.wlan.common/cpp/wire_types.h"
#include "fidl/fuchsia.wlan.ieee80211/cpp/wire_types.h"

namespace wlan {
namespace common {

namespace wlan_ieee80211 = ::fuchsia::wlan::ieee80211;

wlan_band_t GetWlanBand(const fuchsia_wlan_common::wire::WlanChannel& channel) {
  return Is2Ghz(channel) ? WLAN_BAND_TWO_GHZ : WLAN_BAND_FIVE_GHZ;
}

std::string WlanBandStr(wlan_band_t band) {
  switch (band) {
    case WLAN_BAND_TWO_GHZ:
      return "2 GHz";
    case WLAN_BAND_FIVE_GHZ:
      return "5 GHz";
  }
  return "INVALID";
}

std::string WlanBandStr(const wlan_channel_t& channel) { return WlanBandStr(GetWlanBand(channel)); }

zx_status_t ToFidl(wlan_ieee80211::WlanBand* out_fidl_band, wlan_band_t banjo_band) {
  switch (banjo_band) {
    case WLAN_BAND_TWO_GHZ:
      *out_fidl_band = wlan_ieee80211::WlanBand::TWO_GHZ;
      break;
    case WLAN_BAND_FIVE_GHZ:
      *out_fidl_band = wlan_ieee80211::WlanBand::FIVE_GHZ;
      break;
    default:
      return ZX_ERR_INVALID_ARGS;
  }
  return ZX_OK;
}

wlan_band_t FromFidl(wlan_ieee80211::WlanBand fidl_band) {
  return static_cast<wlan_band_t>(fidl_band);
}

}  // namespace common
}  // namespace wlan
