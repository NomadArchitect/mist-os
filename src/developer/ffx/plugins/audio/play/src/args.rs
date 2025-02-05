// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use {fidl_fuchsia_audio_controller as fac, fidl_fuchsia_media as fmedia};

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "play",
    description = "Reads a WAV file from stdin and sends the audio data to audio_core's \
    AudioRenderer API.",
    example = "$ ffx audio gen sine --duration 1s --frequency 440 --amplitude 0.5 --format 48000,int16,2ch | ffx audio play \n\
            $ ffx audio play --file ~/path/to/sine.wav"
)]
pub struct PlayCommand {
    #[argh(
        option,
        description = "purpose of the audio being played. Accepted values: ACCESSIBILITY (and \
        A11Y), BACKGROUND, COMMUNICATION, INTERRUPTION, MEDIA, SYSTEM-AGENT, ULTRASOUND. \
        Default: MEDIA.",
        from_str_fn(str_to_usage),
        default = "AudioRenderUsageExtended::Media(fmedia::AudioRenderUsage2::Media)"
    )]
    pub usage: AudioRenderUsageExtended,

    #[argh(
        option,
        description = "buffer size (bytes) to allocate on device VMO. \
        Used to send audio data from ffx tool to AudioRenderer. \
        Defaults to size to hold 1 second of audio data. "
    )]
    pub buffer_size: Option<u32>,

    #[argh(
        option,
        description = "how many packets to use when sending data to an AudioRenderer. \
        Defaults to 4 packets."
    )]
    pub packet_count: Option<u32>,

    #[argh(
        option,
        description = "gain (decibels) for the renderer. Default: 0 dB",
        default = "0.0f32"
    )]
    pub gain: f32,

    #[argh(option, description = "mute the renderer. Default: false", default = "false")]
    pub mute: bool,

    #[argh(
        option,
        description = "explicitly set the renderer's reference clock. By default, \
        SetReferenceClock is not called, which leads to a flexible clock. \
        Options include: 'flexible', 'monotonic', and 'custom,<rate adjustment>,<offset>' where \
        rate adjustment and offset are integers. To set offset without rate adjustment, pass 0 \
        in place of rate adjustment.",
        from_str_fn(str_to_clock),
        default = "fac::ClockType::Flexible(fac::Flexible)"
    )]
    pub clock: fac::ClockType,

    #[argh(
        option,
        description = "file in WAV format containing audio signal. If not specified,\
        ffx command will read from stdin."
    )]
    pub file: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum AudioRenderUsageExtended {
    Accessibility(fmedia::AudioRenderUsage2),
    Background(fmedia::AudioRenderUsage2),
    Communication(fmedia::AudioRenderUsage2),
    Media(fmedia::AudioRenderUsage2),
    Interruption(fmedia::AudioRenderUsage2),
    SystemAgent(fmedia::AudioRenderUsage2),
    Ultrasound,
}

fn str_to_usage(src: &str) -> Result<AudioRenderUsageExtended, String> {
    match src.to_uppercase().as_str() {
        "ACCESSIBILITY" | "A11Y" => {
            Ok(AudioRenderUsageExtended::Background(fmedia::AudioRenderUsage2::Accessibility))
        }
        "BACKGROUND" => {
            Ok(AudioRenderUsageExtended::Background(fmedia::AudioRenderUsage2::Background))
        }
        "COMMUNICATION" => {
            Ok(AudioRenderUsageExtended::Communication(fmedia::AudioRenderUsage2::Communication))
        }
        "INTERRUPTION" => {
            Ok(AudioRenderUsageExtended::Interruption(fmedia::AudioRenderUsage2::Interruption))
        }
        "MEDIA" => Ok(AudioRenderUsageExtended::Media(fmedia::AudioRenderUsage2::Media)),
        "SYSTEM-AGENT" => {
            Ok(AudioRenderUsageExtended::SystemAgent(fmedia::AudioRenderUsage2::SystemAgent))
        }
        "ULTRASOUND" => Ok(AudioRenderUsageExtended::Ultrasound),
        _ => Err(String::from("Couldn't parse usage.")),
    }
}

fn str_to_clock(src: &str) -> Result<fac::ClockType, String> {
    fuchsia_audio::str_to_clock(src)
}
