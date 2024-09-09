// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_EXAMPLES_SIMPLE_ADR_SIMPLE_ADR_H_
#define SRC_MEDIA_AUDIO_EXAMPLES_SIMPLE_ADR_SIMPLE_ADR_H_

#include <fidl/fuchsia.audio.device/cpp/fidl.h>
#include <fidl/fuchsia.audio/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/fidl/cpp/client.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/fit/function.h>
#include <lib/fzl/vmo-mapper.h>

#include <iostream>
#include <string_view>

namespace examples {

class MediaApp;

template <typename ProtocolT>
class FidlHandler : public fidl::AsyncEventHandler<ProtocolT> {
 public:
  FidlHandler(MediaApp* parent, std::string_view name) : parent_(parent), name_(name) {}
  void on_fidl_error(fidl::UnbindInfo error) final;

 private:
  MediaApp* parent_;
  std::string_view name_;
};
class ControlFidlHandler : public FidlHandler<fuchsia_audio_device::Control> {
 public:
  ControlFidlHandler(MediaApp* parent, std::string_view name) : FidlHandler(parent, name) {}
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_audio_device::Control> metadata) override {
    std::cout << "ControlFidlHandler: unknown event (Control) ordinal " << metadata.event_ordinal;
  }
};

class ObserverFidlHandler : public FidlHandler<fuchsia_audio_device::Observer> {
 public:
  ObserverFidlHandler(MediaApp* parent, std::string_view name) : FidlHandler(parent, name) {}
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_audio_device::Observer> metadata) override {
    std::cout << "ObserverFidlHandler: unknown event (Observer) ordinal " << metadata.event_ordinal;
  }
};

class MediaApp {
  // Display device metadata received from AudioDeviceRegistry, for each device
  static inline constexpr bool kLogDeviceInfo = false;

  // Automatically connect to a StreamConfig ring buffer and play a sinusoid?
  static inline constexpr bool kAutoplaySinusoid = true;

  // TODO(b/306455236): Use a format / rate supported by the detected device.
  static inline constexpr fuchsia_audio::SampleType kSampleFormat =
      fuchsia_audio::SampleType::kInt16;
  static inline constexpr uint16_t kBytesPerSample = 2;
  static inline constexpr float kToneAmplitude = 0.125f;

  static inline constexpr uint32_t kFrameRate = 48000;
  static inline constexpr float kApproxToneFrequency = 240.0f;
  static inline constexpr float kApproxFramesPerCycle = kFrameRate / kApproxToneFrequency;

 public:
  MediaApp(async::Loop& loop, fit::closure quit_callback);

  void Run();
  void Shutdown();

 private:
  void ConnectToRegistry();

  void WaitForFirstAudioDevice();
  void ConnectToControlCreator();
  bool ConnectToControl();

  void ObserveStreamOutput();
  void ConnectToRingBuffer();

  bool MapRingBufferVmo();
  void WriteAudioToVmo();
  void StartRingBuffer();

  void ChangeGainByDbAfter(float change_db, zx::duration wait_duration, int32_t iterations);
  void StopRingBuffer();

  async::Loop& loop_;
  fit::closure quit_callback_;

  static std::optional<fidl::Client<fuchsia_audio_device::Registry>> registry_client_;
  fidl::Client<fuchsia_audio_device::Observer> observer_client_;
  static std::optional<fidl::SyncClient<fuchsia_audio_device::ControlCreator>>
      control_creator_client_;
  fidl::Client<fuchsia_audio_device::Control> control_client_;
  fidl::Client<fuchsia_audio_device::RingBuffer> ring_buffer_client_;

  fuchsia_audio_device::TokenId device_token_id_;
  fuchsia_audio::RingBuffer ring_buffer_;
  uint64_t ring_buffer_size_;  // From fuchsia.mem.Buffer/size and kBytesPerFrame
  fzl::VmoMapper ring_buffer_mapper_;
  float max_gain_db_;
  float min_gain_db_;
  int16_t* rb_start_;
  size_t channels_per_frame_ = 0;

  FidlHandler<fuchsia_audio_device::Registry> reg_handler_{this, "Registry"};
  FidlHandler<fuchsia_audio_device::RingBuffer> rb_handler_{this, "RingBuffer"};
  ControlFidlHandler ctl_handler_{this, "Control"};
  ObserverFidlHandler obs_handler_{this, "Observer"};
};

}  // namespace examples

#endif  // SRC_MEDIA_AUDIO_EXAMPLES_SIMPLE_ADR_SIMPLE_ADR_H_
