//! Work an online provider does instead of this machine: transcription (in
//! place of the local speech models), music, sound effects, voiceovers,
//! video and pictures.
//! What can be done ([`Capability`]), who the user picked for each ([`CloudSettings`]),
//! and the interfaces the editor calls ([`SpeechService`],
//! [`GenerationService`]). `flikk-cloud` implements them for each provider it
//! knows; nothing here names one.

mod request;
mod script;
mod task;
mod voice;

pub use crate::pool::{generated_bin, image_item, media_item, pool_commands, sample_item};
pub use request::{
    GenerateRequest, ImageMode, ImageModel, MusicSection, Reading, ReferenceLimits, SCRIPT_CHARS,
    SpeechContext, VideoMode, VideoModel, VideoReference, VideoSound,
};
pub use script::{check_script, spoken_words, without_breaks};
pub use task::{GenerationTask, Stage};
pub use voice::{
    DESIGN_DESCRIPTION, DESIGN_TEXT, Designed, VOICE_AGES, VOICE_GENDERS, VOICE_LANGUAGES,
    VOICE_USES, Voice, VoiceClone, VoiceDesign, VoiceOrigin, VoicePage, VoicePreview, VoiceQuery,
    voice_label,
};

use crate::analysis::Sound;
use crate::time::TimeRange;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Something a provider can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Words, their times and speakers, in place of the local speech models.
    Transcription,
    Music,
    SoundEffects,
    Voiceover,
    Video,
    /// Pictures, from a description or from pictures to edit or follow.
    Image,
}

impl Capability {
    pub const ALL: [Capability; 6] = [
        Self::Transcription,
        Self::Music,
        Self::SoundEffects,
        Self::Voiceover,
        Self::Video,
        Self::Image,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Transcription => "Transcription",
            Self::Music => "Music",
            Self::SoundEffects => "Sound effects",
            Self::Voiceover => "Voiceover",
            Self::Video => "Video",
            Self::Image => "Image",
        }
    }

    /// Whether this machine does it when no provider is picked.
    pub fn runs_locally(self) -> bool {
        matches!(self, Self::Transcription)
    }
}

/// Who does one capability, on which of their models.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderChoice {
    /// A provider id ("elevenlabs"); `None` does it on this machine, or
    /// not at all when it can't run here.
    pub provider: Option<String>,
    /// One of the provider's model ids; `None` takes its default.
    pub model: Option<String>,
}

/// The provider picked for each capability, per user.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudSettings {
    pub choices: BTreeMap<Capability, ProviderChoice>,
    /// Id of the voice that last read a voiceover.
    pub voice: Option<String>,
    /// Videos are made without sound, which costs less on some models. The
    /// media pool's form starts from it; the agent keeps to it.
    pub silent_video: bool,
}

impl CloudSettings {
    pub fn choice(&self, capability: Capability) -> ProviderChoice {
        self.choices.get(&capability).cloned().unwrap_or_default()
    }

    /// The provider doing `capability`; `None` on this machine.
    pub fn provider(&self, capability: Capability) -> Option<&str> {
        self.choices.get(&capability)?.provider.as_deref()
    }

    pub fn set(&mut self, capability: Capability, choice: ProviderChoice) {
        if choice == ProviderChoice::default() {
            self.choices.remove(&capability);
        } else {
            self.choices.insert(capability, choice);
        }
    }
}

/// Why a provider could not do what was asked.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum CloudError {
    /// No provider is picked for it, or the one picked has no key.
    #[error("{0}")]
    Unavailable(String),
    #[error("{provider} did not accept the API key")]
    InvalidKey { provider: String },
    #[error("The {provider} account is out of credits")]
    NoCredits { provider: String },
    /// The key's scope or the plan does not cover it.
    #[error("{0}")]
    NotAllowed(String),
    #[error("{provider} is busy; try again in a moment")]
    Busy { provider: String },
    /// The prompt was refused (an artist's name, lyrics it knows), with
    /// the provider's rewrite of it when it offered one.
    #[error("{message}")]
    BadPrompt {
        message: String,
        suggestion: Option<String>,
    },
    #[error("{0}")]
    Failed(String),
    /// The user stopped it before it was made.
    #[error("Stopped")]
    Cancelled,
}

impl CloudError {
    /// Fixed in Settings → Integrations, not by trying again.
    pub fn needs_settings(&self) -> bool {
        matches!(
            self,
            Self::Unavailable(_)
                | Self::InvalidKey { .. }
                | Self::NoCredits { .. }
                | Self::NotAllowed(_)
        )
    }
}

pub type CloudResult<T> = std::result::Result<T, CloudError>;

// ------------------------------------------------------------ transcription

/// One item's audio, sent to be transcribed.
pub struct SpeechRequest<'a> {
    /// One stream, mono.
    pub audio: &'a Path,
    /// ISO 639-1; `None` detects it.
    pub language: Option<&'a str>,
    /// Tell the voices apart.
    pub speakers: bool,
}

/// What a transcription provider heard. Times are source times.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Heard {
    /// As the provider gives it ("en", or "eng").
    pub language: String,
    pub words: Vec<HeardWord>,
    /// Laughter, applause, music…, as the provider names them.
    pub events: Vec<Sound>,
    /// Who and what transcribed it: "elevenlabs/scribe_v2".
    pub engine: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HeardWord {
    pub range: TimeRange,
    pub text: String,
    /// 0..1.
    pub confidence: f32,
    /// The provider's number for the voice, in its own order.
    pub speaker: Option<usize>,
}

pub trait SpeechService: Send + Sync {
    /// The provider, as the user knows it: "ElevenLabs".
    fn name(&self) -> String;
    /// Upload `request.audio` and wait for the words; `progress` gets 0..1.
    fn transcribe(
        &self,
        request: &SpeechRequest,
        progress: &(dyn Fn(f32) + Sync),
    ) -> CloudResult<Heard>;
}

/// How a generated media item was made; kept with the item in the project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenerationInfo {
    pub capability: Capability,
    /// Provider id: "elevenlabs".
    pub provider: String,
    pub model: String,
    pub prompt: String,
    /// Who read it, for a voiceover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading: Option<Reading>,
}

impl GenerationInfo {
    /// What a voiceover reads, with its delivery written in.
    pub fn script(&self) -> Option<&str> {
        (self.capability == Capability::Voiceover).then_some(self.prompt.as_str())
    }
}

/// A file a provider made.
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    pub path: PathBuf,
    pub info: GenerationInfo,
    /// Credits it cost, as the provider counts them.
    pub cost: Option<u64>,
}

pub trait GenerationService: Send + Sync {
    /// The provider doing `capability`, as the user knows it, when one is
    /// picked and has what it needs.
    fn provider_for(&self, capability: Capability) -> Option<String>;
    /// Provider id ("elevenlabs") doing voiceovers, when one is picked.
    fn voice_provider(&self) -> Option<String>;
    /// What the model picked for video can make, when one is.
    fn video_model(&self) -> Option<VideoModel>;
    /// What the model picked for pictures can make, when one is.
    fn image_model(&self) -> Option<ImageModel>;
    /// Whether videos are made with sound, as the user set it.
    fn video_sound(&self) -> bool;
    /// Make `request` into a new file in `dir`, reporting to `task` and
    /// stopping when it is cancelled. Slow: minutes for music and video,
    /// up to a couple for a picture.
    fn generate(
        &self,
        request: &GenerateRequest,
        dir: &Path,
        task: &GenerationTask,
    ) -> CloudResult<Generated>;
    /// The voices of the provider doing voiceovers. Slow: a request.
    fn voices(&self) -> CloudResult<Vec<Voice>>;
    /// A sample of `voice` on disk, fetched the first time. Slow: a request.
    fn voice_sample(&self, voice: &Voice) -> CloudResult<PathBuf>;
    /// Page `page` (from 0) of the provider's shared voices that fit
    /// `query`. Slow: a request.
    fn search_voices(&self, query: &VoiceQuery, page: usize) -> CloudResult<VoicePage>;
    /// Shared voices that sound like the speech in `audio`. Slow: an
    /// upload.
    fn similar_voices(&self, audio: &Path) -> CloudResult<Vec<Voice>>;
    /// Add shared `voice` to the account as `name`; the voice as it is
    /// there. Takes one of the plan's voice slots.
    fn add_voice(&self, voice: &Voice, name: &str) -> CloudResult<Voice>;
    /// Voices made to fit `design`, to hear before one is kept. Spends
    /// credits.
    fn design_voice(&self, design: &VoiceDesign) -> CloudResult<Designed>;
    /// Keep `preview` of `design` on the account as `name`.
    fn save_voice(
        &self,
        design: &VoiceDesign,
        preview: &VoicePreview,
        name: &str,
    ) -> CloudResult<Voice>;
    /// Clone a voice from recordings. Takes a voice slot.
    fn clone_voice(&self, clone: &VoiceClone) -> CloudResult<Voice>;
    fn rename_voice(&self, voice: &str, name: &str) -> CloudResult<()>;
    /// Remove a voice from the account; what it read stays.
    fn delete_voice(&self, voice: &str) -> CloudResult<()>;
}
