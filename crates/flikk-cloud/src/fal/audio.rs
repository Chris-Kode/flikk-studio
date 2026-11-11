//! Music and sound effects from the audio models fal hosts ([`MUSIC`],
//! [`SOUND`]): the request in each model's fields (`crate::audio`), made on
//! the queue, and the file it made fetched next to the others. The
//! provider's catalog lists the same ids, in the same order, the default
//! first.

use super::{NAME, queue};
use crate::audio::{self, Fixed, HostedAudio, Instrumental, Length, Sections, lengths, song};
use flikk_core::cloud::{AudioModel, CloudError, CloudResult, GenerateRequest, GenerationTask};
use std::path::{Path, PathBuf};

pub const MUSIC: &[HostedAudio] = &[
    HostedAudio {
        id: "elevenlabs/music/v2.5",
        spec: AudioModel {
            name: "ElevenLabs Music v2.5",
            // fal takes 10 min; the composition plan, like ElevenLabs' own
            // API, 5.
            lengths: lengths(3.0, 300.0, 1.0),
            looping: false,
        },
        prompt: "prompt",
        prompt_chars: 4_100,
        length: Length::Millis("music_length_ms"),
        instrumental: Instrumental::Field("force_instrumental"),
        sections: Sections::ElevenLabsPlan,
        lyrics: None,
        write_lyrics: None,
        looping: None,
        fixed: &[("output_format", Fixed::Str("mp3_48000_320"))],
    },
    HostedAudio {
        id: "sonilo/v1.1/text-to-music",
        spec: AudioModel {
            name: "Sonilo 1.1",
            lengths: lengths(5.0, 600.0, 1.0),
            looping: false,
        },
        prompt: "prompt",
        prompt_chars: 2_000,
        length: Length::Seconds {
            field: "duration",
            default: Some(30.0),
        },
        instrumental: Instrumental::Prompt,
        sections: Sections::Prompt,
        lyrics: None,
        write_lyrics: None,
        looping: None,
        fixed: &[("num_samples", Fixed::Int(1))],
    },
    song("google/lyria-3.5", "Lyria 3.5", 5_000),
    song("fal-ai/lyria3/pro", "Lyria 3 Pro", 5_000),
    HostedAudio {
        id: "fal-ai/stable-audio-3/medium/text-to-audio",
        spec: AudioModel {
            name: "Stable Audio 3",
            lengths: lengths(5.0, 380.0, 1.0),
            looping: false,
        },
        prompt: "prompt",
        prompt_chars: 2_000,
        length: Length::Seconds {
            field: "duration",
            default: None,
        },
        instrumental: Instrumental::Prompt,
        sections: Sections::Prompt,
        lyrics: None,
        write_lyrics: None,
        looping: None,
        fixed: &[("output_format", Fixed::Str("wav"))],
    },
];

pub const SOUND: &[HostedAudio] = &[
    HostedAudio {
        id: "elevenlabs/sound-effects/v3",
        spec: AudioModel {
            name: "ElevenLabs Sound Effects v3",
            lengths: lengths(0.5, 30.0, 0.5),
            looping: true,
        },
        prompt: "text",
        prompt_chars: 450,
        length: Length::Seconds {
            field: "duration_seconds",
            default: None,
        },
        instrumental: Instrumental::Prompt,
        sections: Sections::Prompt,
        lyrics: None,
        write_lyrics: None,
        looping: Some("loop"),
        fixed: &[("output_format", Fixed::Str("mp3_44100_192"))],
    },
    HostedAudio {
        id: "sonilo/v1.1/text-to-sound-effects",
        spec: AudioModel {
            name: "Sonilo 1.1 SFX",
            lengths: lengths(0.5, 180.0, 0.5),
            looping: false,
        },
        prompt: "prompt",
        prompt_chars: 2_000,
        length: Length::Seconds {
            field: "duration",
            default: None,
        },
        instrumental: Instrumental::Prompt,
        sections: Sections::Prompt,
        lyrics: None,
        write_lyrics: None,
        looping: None,
        fixed: &[("audio_format", Fixed::Str("wav"))],
    },
    HostedAudio {
        id: "mirelo-ai/sfx2.0/text-to-audio",
        spec: AudioModel {
            name: "Mirelo SFX 2.0",
            lengths: lengths(0.5, 600.0, 0.5),
            looping: false,
        },
        prompt: "prompt",
        prompt_chars: 2_000,
        length: Length::Seconds {
            field: "duration",
            default: None,
        },
        instrumental: Instrumental::Prompt,
        sections: Sections::Prompt,
        lyrics: None,
        write_lyrics: None,
        looping: None,
        // Two takes by default, each billed.
        fixed: &[
            ("num_samples", Fixed::Int(1)),
            ("upload_audio_format", Fixed::Str("wav")),
        ],
    },
    HostedAudio {
        id: "fal-ai/stable-audio-3/small/sfx/text-to-audio",
        spec: AudioModel {
            name: "Stable Audio 3 SFX",
            lengths: lengths(1.0, 120.0, 0.5),
            looping: false,
        },
        prompt: "prompt",
        prompt_chars: 2_000,
        length: Length::Seconds {
            field: "duration",
            default: None,
        },
        instrumental: Instrumental::Prompt,
        sections: Sections::Prompt,
        lyrics: None,
        write_lyrics: None,
        looping: None,
        fixed: &[("output_format", Fixed::Str("wav"))],
    },
];

pub fn find(id: &str) -> Option<&'static HostedAudio> {
    MUSIC.iter().chain(SOUND).find(|m| m.id == id)
}

pub fn run(
    key: &str,
    model: &HostedAudio,
    request: &GenerateRequest,
    stem: &Path,
    task: &GenerationTask,
) -> CloudResult<(PathBuf, Option<u64>)> {
    let input = audio::input(model, request)?;
    let result = queue::run(key, model.id, &input, task)?;
    let (url, content_type) = audio::made(&result)
        .ok_or_else(|| CloudError::Failed(format!("{NAME} sent no sound back")))?;
    let path = audio::fetch(url, content_type, stem, task)?;
    Ok((path, None))
}
