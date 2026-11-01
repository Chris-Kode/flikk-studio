//! Work an online provider does instead of this machine: transcription (in
//! place of the local speech models), music, sound effects and voiceovers.
//! What can be done ([`Capability`]), who the user picked for each ([`CloudSettings`]),
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
    Voiceover,
}

impl Capability {
    pub const ALL: [Capability; 4] = [
        Self::Transcription,
        Self::Music,
        Self::SoundEffects,
        Self::Voiceover,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Transcription => "Transcription",
            Self::Music => "Music",
            Self::SoundEffects => "Sound effects",
            Self::Voiceover => "Voiceover",
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

/// Music, a sound effect or a voiceover to make.
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
    Voiceover {
        /// What is said, with its delivery written in.
        text: String,
        reading: Reading,
    },
}

/// Who reads a voiceover, and how.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    /// The provider's id for the voice.
    pub voice: String,
    pub voice_name: String,
    /// 0..1; lower reads with a wider emotional range.
    pub stability: f32,
    /// 0..1; how closely it keeps to the original voice.
    pub similarity: f32,
}

impl Reading {
    pub const STABILITY: f32 = 0.5;
    pub const SIMILARITY: f32 = 0.75;
}

/// The most one voiceover can say, about 10 minutes of speech.
pub const SCRIPT_CHARS: usize = 10_000;

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
            Self::Voiceover { .. } => Capability::Voiceover,
        }
    }

    pub fn prompt(&self) -> &str {
        match self {
            Self::Music { prompt, .. } | Self::SoundEffect { prompt, .. } => prompt,
            Self::Voiceover { text, .. } => text,
        }
    }

    pub fn reading(&self) -> Option<&Reading> {
        match self {
            Self::Voiceover { reading, .. } => Some(reading),
            _ => None,
        }
    }

    /// The length asked for: the sections' together, else `length`.
    pub fn length(&self) -> Option<Time> {
        match self {
            Self::Music { sections, .. } if !sections.is_empty() => {
                Some(sections.iter().fold(Time::ZERO, |t, s| t + s.length))
            }
            Self::Music { length, .. } | Self::SoundEffect { length, .. } => *length,
            Self::Voiceover { .. } => None,
        }
    }

    /// One line for cards and logs: "Music, 30 s: calm lo-fi beat".
    pub fn summary(&self) -> String {
        if let Self::Voiceover { text, reading } = self {
            let start: String = text.chars().take(SUMMARY_CHARS).collect();
            let more = if start.len() < text.len() { "…" } else { "" };
            return format!("Voiceover, {}: {start}{more}", reading.voice_name);
        }
        let what = match self {
            Self::Music { .. } => "Music",
            Self::SoundEffect { .. } => "Sound effect",
            Self::Voiceover { .. } => "Voiceover",
        };
        match self.length() {
            Some(t) => format!("{what}, {:.0} s: {}", t.as_secs_f64(), self.prompt()),
            None => format!("{what}: {}", self.prompt()),
        }
    }
}

/// How much of a voiceover's script a one-line summary shows.
const SUMMARY_CHARS: usize = 80;

/// Why `text` can't be read as a voiceover; the rewrite that can, when
/// there is one.
pub fn check_script(text: &str) -> CloudResult<()> {
    let refuse = |message: String, suggestion| {
        Err(CloudError::BadPrompt {
            message,
            suggestion,
        })
    };
    let chars = text.chars().count();
    if chars > SCRIPT_CHARS {
        return refuse(
            format!("A voiceover says at most {SCRIPT_CHARS} characters; this has {chars}"),
            None,
        );
    }
    match without_breaks(text) {
        Some(fixed) => refuse("<break> tags aren't read; use pauses".into(), Some(fixed)),
        None => Ok(()),
    }
}

/// `text` with its SSML `<break/>` tags as the pauses a voice reads; `None`
/// when it has none.
pub fn without_breaks(text: &str) -> Option<String> {
    // Lower case keeps every byte where it is.
    let lower = text.to_ascii_lowercase();
    let mut out = String::new();
    let mut at = 0;
    while let Some(start) = lower[at..].find("<break").map(|i| at + i) {
        let Some(end) = lower[start..].find('>').map(|i| start + i + 1) else {
            break;
        };
        out.push_str(&text[at..start]);
        out.push_str(if break_seconds(&lower[start..end]) >= 1.0 {
            "[long pause]"
        } else {
            "[short pause]"
        });
        at = end;
    }
    (at > 0).then(|| out + &text[at..])
}

/// Seconds in a lower-case `<break time="1.5s"/>`; 0 without a time.
fn break_seconds(tag: &str) -> f64 {
    let Some(time) = tag.split("time=").nth(1) else {
        return 0.0;
    };
    let time = time.trim_start_matches(['"', '\'']);
    let digits = time
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(time.len());
    let value: f64 = time[..digits].parse().unwrap_or(0.0);
    if time[digits..].starts_with("ms") {
        value / 1000.0
    } else {
        value
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
    /// Who read it, for a voiceover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading: Option<Reading>,
}

/// A file a provider made.
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    pub path: PathBuf,
    pub info: GenerationInfo,
    /// Credits it cost, as the provider counts them.
    pub cost: Option<u64>,
}

/// A voice a provider can read with.
#[derive(Clone, Debug, PartialEq)]
pub struct Voice {
    pub id: String,
    pub name: String,
    /// "premade", "cloned"…, as the provider names them.
    pub category: String,
    /// What it is like: "calm", "american", "narration".
    pub traits: Vec<String>,
    /// Where a sample of it can be heard.
    pub sample: Option<String>,
}

pub trait GenerationService: Send + Sync {
    /// The provider doing `capability`, as the user knows it, when one is
    /// picked and has what it needs.
    fn provider_for(&self, capability: Capability) -> Option<String>;
    /// Make `request` into a new file in `dir`. Slow: minutes for music.
    fn generate(&self, request: &GenerateRequest, dir: &Path) -> CloudResult<Generated>;
    /// The voices of the provider doing voiceovers. Slow: a request.
    fn voices(&self) -> CloudResult<Vec<Voice>>;
    /// A sample of `voice` on disk, fetched the first time. Slow: a request.
    fn voice_sample(&self, voice: &Voice) -> CloudResult<PathBuf>;
}

/// Name of the bin generated `item` goes into.
pub fn generated_bin(item: &MediaItem) -> &'static str {
    match item.generated.as_deref().map(|g| g.capability) {
        Some(Capability::SoundEffects) => "Sound Effects Generated",
        Some(Capability::Voiceover) => "Voiceover Generated",
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

/// `path`, a sample of a voice, as an item to listen to: not in any pool.
pub fn sample_item(path: PathBuf, media: Option<&dyn crate::services::MediaService>) -> MediaItem {
    MediaItem {
        id: MediaId::new(),
        kind: MediaKind::Audio,
        fingerprint: None,
        info: media.and_then(|m| m.probe(&path).ok()),
        path,
        bin: None,
        tags: Vec::new(),
        proxy: None,
        generated: None,
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
