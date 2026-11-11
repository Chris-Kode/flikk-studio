//! Music and sound effects from Pika's own audio models and the other
//! vendors' Pika hosts ([`MUSIC`], [`SOUND`]): the request in each model's
//! fields (`crate::audio`), made as a media job, and the file it made
//! fetched next to the others. The provider's catalog lists the same ids,
//! in the same order, the default first. Each id is the operation's path
//! after `/v1/media/`.

use super::{NAME, jobs};
use crate::audio::{self, Fixed, HostedAudio, Instrumental, Length, Sections, lengths, song};
use flikk_core::cloud::{AudioModel, CloudError, CloudResult, GenerateRequest, GenerationTask};
use std::path::{Path, PathBuf};

pub const MUSIC: &[HostedAudio] = &[
    HostedAudio {
        id: "pika/pika-audio/pika-music",
        spec: AudioModel {
            name: "Pika Music",
            lengths: lengths(10.0, 360.0, 1.0),
            looping: false,
        },
        prompt: "prompt",
        prompt_chars: 4_000,
        length: Length::Seconds {
            field: "duration",
            default: None,
        },
        instrumental: Instrumental::Prompt,
        sections: Sections::Prompt,
        lyrics: Some("lyrics"),
        write_lyrics: None,
        looping: None,
        fixed: &[],
    },
    HostedAudio {
        id: "sonilo/sonilo-v1.1-music/text-to-music",
        spec: AudioModel {
            name: "Sonilo 1.1",
            lengths: lengths(5.0, 360.0, 1.0),
            looping: false,
        },
        prompt: "prompt",
        prompt_chars: 2_000,
        length: Length::Seconds {
            field: "duration",
            default: Some(30.0),
        },
        instrumental: Instrumental::Prompt,
        sections: Sections::SoniloSegments,
        lyrics: None,
        write_lyrics: None,
        looping: None,
        fixed: &[],
    },
    HostedAudio {
        id: "minimax/minimax-music-3.0/text-to-audio",
        spec: AudioModel {
            name: "MiniMax Music 3.0",
            lengths: None,
            looping: false,
        },
        prompt: "prompt",
        prompt_chars: 2_000,
        length: Length::Chosen,
        instrumental: Instrumental::Field("is_instrumental"),
        sections: Sections::Prompt,
        lyrics: Some("lyrics"),
        write_lyrics: Some("lyrics_optimizer"),
        looping: None,
        fixed: &[],
    },
    song("google/lyria-3.5/text-to-audio", "Lyria 3.5", 4_096),
    song("google/lyria-3-pro/text-to-audio", "Lyria 3 Pro", 4_096),
];

pub const SOUND: &[HostedAudio] = &[
    HostedAudio {
        id: "pika/pika-audio/pika-sfx",
        spec: AudioModel {
            name: "Pika SFX",
            lengths: lengths(1.0, 20.0, 0.5),
            looping: false,
        },
        prompt: "prompt",
        prompt_chars: 4_000,
        length: Length::Seconds {
            field: "duration",
            default: None,
        },
        instrumental: Instrumental::Prompt,
        sections: Sections::Prompt,
        lyrics: None,
        write_lyrics: None,
        looping: None,
        fixed: &[],
    },
    HostedAudio {
        id: "sonilo/sonilo-v1.1-sfx/sound-effects",
        spec: AudioModel {
            name: "Sonilo 1.1 SFX",
            lengths: lengths(1.0, 180.0, 1.0),
            looping: false,
        },
        prompt: "prompt",
        prompt_chars: 2_000,
        length: Length::Seconds {
            field: "duration",
            default: Some(5.0),
        },
        instrumental: Instrumental::Prompt,
        sections: Sections::Prompt,
        lyrics: None,
        write_lyrics: None,
        looping: None,
        fixed: &[("audio_format", Fixed::Str("wav"))],
    },
    HostedAudio {
        id: "elevenlabs/eleven-text-to-sound-v2/sound-effects",
        spec: AudioModel {
            name: "ElevenLabs Sound Effects v2",
            lengths: lengths(0.5, 30.0, 0.5),
            looping: true,
        },
        prompt: "text",
        prompt_chars: 450,
        length: Length::Seconds {
            field: "duration_seconds",
            default: Some(5.0),
        },
        instrumental: Instrumental::Prompt,
        sections: Sections::Prompt,
        lyrics: None,
        write_lyrics: None,
        looping: Some("loop"),
        fixed: &[],
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
    let job = jobs::run(key, model.id, &input, task)?;
    let (url, content_type) = audio::made(&job["output"])
        .ok_or_else(|| CloudError::Failed(format!("{NAME} sent no sound back")))?;
    let path = audio::fetch(url, content_type, stem, task)?;
    Ok((path, None))
}
