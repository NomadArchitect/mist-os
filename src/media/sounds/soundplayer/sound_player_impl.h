// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_SOUNDS_SOUNDPLAYER_SOUND_PLAYER_IMPL_H_
#define SRC_MEDIA_SOUNDS_SOUNDPLAYER_SOUND_PLAYER_IMPL_H_

#include <fuchsia/media/cpp/fidl.h>
#include <fuchsia/media/sounds/cpp/fidl.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>

#include <unordered_map>

#include "src/media/sounds/soundplayer/sound.h"

namespace soundplayer {

class SoundPlayerImpl : public fuchsia::media::sounds::Player {
 public:
  SoundPlayerImpl(fuchsia::media::AudioPtr audio_service,
                  fidl::InterfaceRequest<fuchsia::media::sounds::Player> request);

  ~SoundPlayerImpl() override;

  // fuchsia::media::sounds::Player implementation.
  void AddSoundFromFile(uint32_t id, fidl::InterfaceHandle<class fuchsia::io::File> file,
                        AddSoundFromFileCallback callback) override;

  void AddSoundBuffer(uint32_t id, fuchsia::mem::Buffer buffer,
                      fuchsia::media::AudioStreamType stream_type) override;

  void RemoveSound(uint32_t id) override;

  void PlaySound(uint32_t id, fuchsia::media::AudioRenderUsage usage,
                 PlaySoundCallback callback) override;

  void PlaySound2(uint32_t id, fuchsia::media::AudioRenderUsage2 usage,
                  PlaySound2Callback callback) override;

  void StopPlayingSound(uint32_t id) override;

  void handle_unknown_method(uint64_t ordinal, bool method_has_response) override;

 private:
  class Renderer {
   public:
    using PlaySoundCallback = fit::function<void(fuchsia::media::sounds::Player_PlaySound_Result)>;

    Renderer(fuchsia::media::AudioRendererPtr audio_renderer,
             fuchsia::media::AudioRenderUsage2 usage);

    ~Renderer();

    // Plays the sound, returning ZX_OK and calling the callback when playback is complete.
    // If a failure occurs, an error status is returned, and the callback is not called.
    zx_status_t PlaySound(uint32_t id, std::shared_ptr<Sound> sound,
                          PlaySoundCallback completion_callback);
    zx_status_t PlaySound2(uint32_t id, std::shared_ptr<Sound> sound,
                           PlaySound2Callback completion_callback);

    // Stops playing the sound, if one is playing, and calls the completion callback.
    void StopPlayingSound();

   private:
    fuchsia::media::AudioRendererPtr audio_renderer_;
    PlaySoundCallback play_sound_callback_;
    PlaySound2Callback play_sound2_callback_;
    std::shared_ptr<Sound> locked_sound_;
  };

  void DeleteThis();

  void WhenAudioServiceIsWarm(fit::closure callback);

  static fpromise::result<std::shared_ptr<Sound>, zx_status_t> SoundFromFile(
      fidl::InterfaceHandle<fuchsia::io::File> file);

  fidl::Binding<fuchsia::media::sounds::Player> binding_;
  fuchsia::media::AudioPtr audio_service_;
  std::unordered_map<uint32_t, std::shared_ptr<Sound>> sounds_by_id_;
  std::unordered_map<Renderer*, std::unique_ptr<Renderer>> renderers_;
  std::unordered_map<uint32_t, Renderer*> renderers_by_sound_id_;

  // Used only in |WhenAudioServiceIsWarm|.
  fuchsia::media::AudioRendererPtr audio_renderer_;

 public:
  // Disallow copy, assign and move.
  SoundPlayerImpl(const SoundPlayerImpl&) = delete;
  SoundPlayerImpl(SoundPlayerImpl&&) = delete;
  SoundPlayerImpl& operator=(const SoundPlayerImpl&) = delete;
  SoundPlayerImpl& operator=(SoundPlayerImpl&&) = delete;
};

}  // namespace soundplayer

#endif  // SRC_MEDIA_SOUNDS_SOUNDPLAYER_SOUND_PLAYER_IMPL_H_
