//! Voiceovers from the speech models fal hosts ([`SPEECH`]): the request in
//! each model's fields (`crate::speech`), made on the queue, and the file it
//! made fetched next to the others. The provider's catalog lists the same
//! ids, in the same order, the default first. fal lists no voices and
//! clones none, so each model reads with the voices its schema names.

use super::{NAME, queue};
use crate::audio::{self, Fixed};
use crate::speech::{self, HostedSpeech};
use flikk_core::cloud::{
    CloudError, CloudResult, DeliveryMarks, GenerateRequest, GenerationTask, VoiceOps,
    VoiceoverModel,
};
use std::path::{Path, PathBuf};

pub const SPEECH: &[HostedSpeech] = &[
    HostedSpeech {
        id: "elevenlabs/tts/eleven-v4",
        spec: VoiceoverModel {
            name: "ElevenLabs v4",
            script_chars: 5_000,
            marks: DeliveryMarks::AudioTags,
            tuned: true,
            voices: VoiceOps::NONE,
        },
        text: "text",
        voice: "voice",
        tuning: Some(("stability", "similarity_boost")),
        // Its PCM has no header; MP3 at 192 kbps is the best real file.
        fixed: &[("output_format", Fixed::Str("mp3_44100_192"))],
        voices: &[
            "Aria",
            "Roger",
            "Sarah",
            "Laura",
            "Charlie",
            "George",
            "Callum",
            "River",
            "Liam",
            "Charlotte",
            "Alice",
            "Matilda",
            "Will",
            "Jessica",
            "Eric",
            "Chris",
            "Brian",
            "Daniel",
            "Lily",
            "Bill",
        ],
        clone_id: None,
    },
    HostedSpeech {
        id: "fal-ai/minimax/speech-2.8-hd",
        spec: VoiceoverModel {
            name: "MiniMax Speech 2.8 HD",
            script_chars: 10_000,
            marks: DeliveryMarks::Interjections,
            tuned: false,
            voices: VoiceOps::NONE,
        },
        text: "prompt",
        voice: "voice_setting.voice_id",
        tuning: None,
        fixed: &[
            // A link to the file, not the file in hex.
            ("output_format", Fixed::Str("url")),
            ("audio_setting.format", Fixed::Str("flac")),
            ("audio_setting.sample_rate", Fixed::Int(44_100)),
        ],
        voices: &[
            "Wise_Woman",
            "Friendly_Person",
            "Inspirational_girl",
            "Deep_Voice_Man",
            "Calm_Woman",
            "Casual_Guy",
            "Lively_Girl",
            "Patient_Man",
            "Young_Knight",
            "Determined_Man",
            "Lovely_Girl",
            "Decent_Boy",
            "Imposing_Manner",
            "Elegant_Man",
            "Abbess",
            "Sweet_Girl_2",
            "Exuberant_Girl",
        ],
        clone_id: None,
    },
    HostedSpeech {
        id: "fal-ai/gemini-3.1-flash-tts",
        spec: VoiceoverModel {
            name: "Gemini 3.1 Flash TTS",
            // It takes 50,000; a voiceover is kept to the editor's limit.
            script_chars: 10_000,
            marks: DeliveryMarks::AudioTags,
            tuned: false,
            voices: VoiceOps::NONE,
        },
        text: "prompt",
        voice: "voice",
        tuning: None,
        fixed: &[("output_format", Fixed::Str("wav"))],
        voices: &[
            "Kore",
            "Achernar",
            "Achird",
            "Algenib",
            "Algieba",
            "Alnilam",
            "Aoede",
            "Autonoe",
            "Callirrhoe",
            "Charon",
            "Despina",
            "Enceladus",
            "Erinome",
            "Fenrir",
            "Gacrux",
            "Iapetus",
            "Laomedeia",
            "Leda",
            "Orus",
            "Pulcherrima",
            "Puck",
            "Rasalgethi",
            "Sadachbia",
            "Sadaltager",
            "Schedar",
            "Sulafat",
            "Umbriel",
            "Vindemiatrix",
            "Zephyr",
            "Zubenelgenubi",
        ],
        clone_id: None,
    },
];

pub fn find(id: &str) -> Option<&'static HostedSpeech> {
    SPEECH.iter().find(|m| m.id == id)
}

pub fn run(
    key: &str,
    model: &HostedSpeech,
    request: &GenerateRequest,
    stem: &Path,
    task: &GenerationTask,
) -> CloudResult<(PathBuf, Option<u64>)> {
    let input = speech::input(model, request)?;
    let result = queue::run(key, model.id, &input, task)?;
    let (url, content_type) = audio::made(&result)
        .ok_or_else(|| CloudError::Failed(format!("{NAME} sent no speech back")))?;
    let path = audio::fetch(url, content_type, stem, task)?;
    Ok((path, None))
}
