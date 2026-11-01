//! Work an online provider does instead of this machine: transcription (in
//! place of the local speech models), music and sound effects. What can be
//! done ([`Capability`]), who the user picked for each ([`CloudSettings`]),
//! and the interfaces the editor calls ([`SpeechService`],
//! [`GenerationService`]). `koo-cloud` implements them for each provider it
//! knows; nothing here names one.

use crate::analysis::Sound;
use crate::command::Command;
use crate::ids::{BinId, MediaId};
use crate::model::{Bin, MediaItem, MediaKind, Project};
use crate::time::{Time, TimeRange};
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
}

impl Capability {
    pub const ALL: [Capability; 3] = [Self::Transcription, Self::Music, Self::SoundEffects];

    pub fn label(self) -> &'static str {
        match self {
            Self::Transcription => "Transcription",
            Self::Music => "Music",
            Self::SoundEffects => "Sound effects",
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

// --------------------------------------------------------------- generation

/// Music or a sound effect to make.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GenerateRequest {
    Music {
        prompt: String,
        /// `None` lets the model choose; ignored with `sections`.
        #[serde(default)]
        length: Option<Time>,
        #[serde(default)]
        instrumental: bool,
        /// Parts with lengths of their own, so the music changes on cue.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sections: Vec<MusicSection>,
    },
    SoundEffect {
        prompt: String,
        #[serde(default)]
        length: Option<Time>,
        /// Made to repeat without a seam.
        #[serde(default)]
        looping: bool,
    },
}

/// One part of a piece of music.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MusicSection {
    pub name: String,
    pub length: Time,
    /// "soft piano", "building drums".
    #[serde(default)]
    pub styles: Vec<String>,
    /// Styles to keep out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub avoid: Vec<String>,
    /// Sung lines; none for an instrumental part.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lyrics: Vec<String>,
}

impl GenerateRequest {
    pub fn capability(&self) -> Capability {
        match self {
            Self::Music { .. } => Capability::Music,
            Self::SoundEffect { .. } => Capability::SoundEffects,
        }
    }

    pub fn prompt(&self) -> &str {
        match self {
            Self::Music { prompt, .. } | Self::SoundEffect { prompt, .. } => prompt,
        }
    }

    /// The length asked for: the sections' together, else `length`.
    pub fn length(&self) -> Option<Time> {
        match self {
            Self::Music { sections, .. } if !sections.is_empty() => {
                Some(sections.iter().fold(Time::ZERO, |t, s| t + s.length))
            }
            Self::Music { length, .. } | Self::SoundEffect { length, .. } => *length,
        }
    }

    /// One line for cards and logs: "Music, 30 s: calm lo-fi beat".
    pub fn summary(&self) -> String {
        let what = match self {
            Self::Music { .. } => "Music",
            Self::SoundEffect { .. } => "Sound effect",
        };
        match self.length() {
            Some(t) => format!("{what}, {:.0} s: {}", t.as_secs_f64(), self.prompt()),
            None => format!("{what}: {}", self.prompt()),
        }
    }
}

/// How a generated media item was made; kept with the item in the project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenerationInfo {
    pub capability: Capability,
    /// Provider id: "elevenlabs".
    pub provider: String,
    pub model: String,
    pub prompt: String,
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
    /// Make `request` into a new file in `dir`. Slow: minutes for music.
    fn generate(&self, request: &GenerateRequest, dir: &Path) -> CloudResult<Generated>;
}

/// Name of the bin generated `item` goes into.
pub fn generated_bin(item: &MediaItem) -> &'static str {
    match item.generated.as_deref().map(|g| g.capability) {
        Some(Capability::SoundEffects) => "Sound Effects Generated",
        _ => "Music Generated",
    }
}

/// The media item for `generated`, probed and fingerprinted by `media`
/// when there is one; not in a bin yet ([`pool_commands`] puts it in one).
pub fn media_item(
    generated: &Generated,
    media: Option<&dyn crate::services::MediaService>,
) -> MediaItem {
    MediaItem {
        id: MediaId::new(),
        path: generated.path.clone(),
        kind: MediaKind::Audio,
        fingerprint: media.and_then(|m| m.fingerprint(&generated.path).ok()),
        info: media.and_then(|m| m.probe(&generated.path).ok()),
        bin: None,
        tags: Vec::new(),
        proxy: None,
        generated: Some(Box::new(generated.info.clone())),
    }
}

/// The commands that add `item` to `project`'s pool, in the bin named
/// `name`, made first if the project has none.
pub fn pool_commands(project: &Project, mut item: MediaItem, name: &str) -> Vec<Command> {
    let mut commands = Vec::new();
    let bin = match project
        .bins
        .iter()
        .find(|b| b.parent.is_none() && b.name == name)
    {
        Some(bin) => bin.id,
        None => {
            let bin = Bin {
                id: BinId::new(),
                name: name.into(),
                parent: None,
            };
            let id = bin.id;
            let mut bins = project.bins.clone();
            bins.push(bin);
            commands.push(Command::SetBins { bins });
            id
        }
    };
    item.bin = Some(bin);
    commands.push(Command::ImportMedia { media: item });
    commands
}
