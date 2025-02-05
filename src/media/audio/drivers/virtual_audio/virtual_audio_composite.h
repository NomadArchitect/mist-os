// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_VIRTUAL_AUDIO_COMPOSITE_H_
#define SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_VIRTUAL_AUDIO_COMPOSITE_H_

#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <fidl/fuchsia.virtualaudio/cpp/wire.h>
#include <lib/ddk/platform-defs.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/zx/result.h>

#include <cstdio>

#include <ddktl/device.h>

#include "src/media/audio/drivers/virtual_audio/virtual_audio_device.h"
#include "src/media/audio/drivers/virtual_audio/virtual_audio_driver.h"

namespace virtual_audio {

class VirtualAudioComposite;
using VirtualAudioCompositeDeviceType =
    ddk::Device<VirtualAudioComposite,
                ddk::Messageable<fuchsia_hardware_audio::CompositeConnector>::Mixin>;

// One ring buffer and one DAI interconnect only are supported by this driver.
class VirtualAudioComposite final
    : public VirtualAudioCompositeDeviceType,
      public ddk::internal::base_protocol,
      public fidl::Server<fuchsia_hardware_audio::Composite>,
      public fidl::Server<fuchsia_hardware_audio_signalprocessing::SignalProcessing>,
      public fidl::Server<fuchsia_hardware_audio::RingBuffer>,
      public VirtualAudioDriver {
 public:
  static fuchsia_virtualaudio::Configuration GetDefaultConfig();

  VirtualAudioComposite(fuchsia_virtualaudio::Configuration config,
                        std::weak_ptr<VirtualAudioDevice> owner, zx_device_t* parent,
                        fit::closure on_shutdown);
  void ResetCompositeState();
  void ShutdownAsync() override;
  void DdkRelease();

  // VirtualAudioDriver overrides.
  // TODO(https://fxbug.dev/42075676): Add support for GetPositionForVA,
  // SetNotificationFrequencyFromVA and AdjustClockRateFromVA.
  using ErrorT = fuchsia_virtualaudio::Error;
  void GetFormatForVA(fit::callback<void(fit::result<ErrorT, CurrentFormat>)> callback) override;
  void GetBufferForVA(fit::callback<void(fit::result<ErrorT, CurrentBuffer>)> callback) override;

 protected:
  // FIDL LLCPP method for fuchsia.hardware.audio.CompositeConnector.
  void Connect(ConnectRequestView request, ConnectCompleter::Sync& completer) override;

  // FIDL natural C++ methods for fuchsia.hardware.audio.Composite.
  void Reset(ResetCompleter::Sync& completer) override;
  void GetProperties(fidl::Server<fuchsia_hardware_audio::Composite>::GetPropertiesCompleter::Sync&
                         completer) override;
  void GetHealthState(GetHealthStateCompleter::Sync& completer) override;
  void SignalProcessingConnect(SignalProcessingConnectRequest& request,
                               SignalProcessingConnectCompleter::Sync& completer) override;
  void GetRingBufferFormats(GetRingBufferFormatsRequest& request,
                            GetRingBufferFormatsCompleter::Sync& completer) override;
  void CreateRingBuffer(CreateRingBufferRequest& request,
                        CreateRingBufferCompleter::Sync& completer) override;
  void GetDaiFormats(GetDaiFormatsRequest& request,
                     GetDaiFormatsCompleter::Sync& completer) override;
  void SetDaiFormat(SetDaiFormatRequest& request, SetDaiFormatCompleter::Sync& completer) override;

  // FIDL natural C++ methods for fuchsia.hardware.audio.RingBuffer.
  void GetProperties(fidl::Server<fuchsia_hardware_audio::RingBuffer>::GetPropertiesCompleter::Sync&
                         completer) override;
  void GetVmo(
      GetVmoRequest& request,
      fidl::Server<fuchsia_hardware_audio::RingBuffer>::GetVmoCompleter::Sync& completer) override;
  void Start(StartCompleter::Sync& completer) override;
  void Stop(StopCompleter::Sync& completer) override;
  void WatchClockRecoveryPositionInfo(
      WatchClockRecoveryPositionInfoCompleter::Sync& completer) override;
  void WatchDelayInfo(WatchDelayInfoCompleter::Sync& completer) override;
  void SetActiveChannels(fuchsia_hardware_audio::RingBufferSetActiveChannelsRequest& request,
                         SetActiveChannelsCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_audio::RingBuffer> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // FIDL natural C++ methods for fuchsia.hardware.audio.signalprocessing.SignalProcessing.
  void GetElements(GetElementsCompleter::Sync& completer) override;
  void WatchElementState(WatchElementStateRequest& request,
                         WatchElementStateCompleter::Sync& completer) override;
  void SetElementState(SetElementStateRequest& request,
                       SetElementStateCompleter::Sync& completer) override;
  void GetTopologies(GetTopologiesCompleter::Sync& completer) override;
  void WatchTopology(WatchTopologyCompleter::Sync& completer) override;
  void SetTopology(SetTopologyRequest& request, SetTopologyCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_audio_signalprocessing::SignalProcessing>
          metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  static constexpr fuchsia_hardware_audio::TopologyId kTopologyId = 789;
  static constexpr size_t kNumberOfElements = 2;
  static constexpr fuchsia_hardware_audio::ElementId kRingBufferId = 123;
  static constexpr fuchsia_hardware_audio::ElementId kDaiId = 456;
  bool ring_buffer_is_outgoing_;

  void ResetRingBuffer();
  void OnRingBufferClosed(fidl::UnbindInfo info);
  void OnSignalProcessingClosed(fidl::UnbindInfo info);
  fuchsia_virtualaudio::RingBuffer& GetRingBuffer(uint64_t id);
  fuchsia_virtualaudio::Composite& composite_config() {
    return config_.device_specific()->composite().value();
  }

  // This should never be invalid: this VirtualAudioStream should always be destroyed before
  // its parent. This field is a weak_ptr to avoid a circular reference count.
  const std::weak_ptr<VirtualAudioDevice> parent_;
  static int instance_count_;
  char instance_name_[64];
  bool connected_ = false;

  // One ring buffer and one DAI interconnect only are supported by this driver.
  fzl::VmoMapper ring_buffer_mapper_;
  uint32_t notifications_per_ring_ = 0;
  uint32_t num_ring_buffer_frames_ = 0;
  uint32_t frame_size_ = 4;
  zx::vmo ring_buffer_vmo_;

  bool watch_delay_info_needs_reply_ = true;
  std::optional<WatchDelayInfoCompleter::Async> delay_info_completer_;
  bool watch_position_info_needs_reply_ = true;
  std::optional<WatchClockRecoveryPositionInfoCompleter::Async> position_info_completer_;

  bool watch_element_state_needs_reply_[kNumberOfElements] = {true, true};
  std::optional<WatchElementStateCompleter::Async>
      watch_element_state_completers_[kNumberOfElements];
  bool watch_topology_needs_reply_ = true;
  std::optional<WatchTopologyCompleter::Async> watch_topology_completer_;

  bool ring_buffer_vmo_fetched_ = false;
  bool ring_buffer_started_ = false;
  std::optional<fuchsia_hardware_audio::Format> ring_buffer_format_;
  uint64_t ring_buffer_active_channel_mask_;
  zx::time active_channel_set_time_;

  std::optional<fuchsia_hardware_audio::DaiFormat> dai_format_;
  fuchsia_virtualaudio::Configuration config_;
  std::optional<fidl::ServerBinding<fuchsia_hardware_audio::RingBuffer>> ring_buffer_;
  std::optional<fidl::ServerBinding<fuchsia_hardware_audio_signalprocessing::SignalProcessing>>
      signal_;
  async_dispatcher_t* dispatcher_ = fdf::Dispatcher::GetCurrent()->async_dispatcher();
};

}  // namespace virtual_audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_VIRTUAL_AUDIO_COMPOSITE_H_
