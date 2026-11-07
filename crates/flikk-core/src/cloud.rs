//! Work an online provider does instead of this machine: transcription (in
//! place of the local speech models), music, sound effects, voiceovers,
//! video and pictures.
//! What can be done ([`Capability`]), who the user picked for each ([`CloudSettings`]),
//! and the interfaces the editor calls ([`SpeechService`],
//! [`GenerationService`]). `flikk-cloud` implements them for each provider it
//! knows; nothing here names one.

use crate::analysis::Sound;
use crate::command::Command;
use crate::ids::{BinId, MediaId};
use crate::model::{Bin, MediaItem, MediaKind, Project};
use crate::time::{Time, TimeRange};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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

// --------------------------------------------------------------- generation

/// Music, a sound effect, a voiceover or a video to make.
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
        /// What is said just before and after it, so its delivery fits
        /// there; heard by the provider, not read.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<SpeechContext>,
    },
    /// From a description alone, from a first frame (image to video), or
    /// after references (reference to video); see [`VideoMode`].
    Video {
        /// What happens, the camera, the look. With references it names
        /// them by kind and place: `@Image1`, `@Video2`, `@Audio1`.
        prompt: String,
        /// Whole seconds; `None` lets the model choose.
        #[serde(default)]
        length: Option<Time>,
        /// "16:9", "9:16"…; `None` lets the model choose (a first frame's
        /// shape, when there is one).
        #[serde(default)]
        aspect: Option<String>,
        /// "720p"; `None` takes the model's default.
        #[serde(default)]
        resolution: Option<String>,
        /// The picture it starts on.
        #[serde(default)]
        first_frame: Option<PathBuf>,
        /// The picture it ends on; only with a first frame.
        #[serde(default)]
        last_frame: Option<PathBuf>,
        /// Pictures, clips and sounds it follows (a character, a style, a
        /// motion, a beat); not with a first frame.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        references: Vec<VideoReference>,
        /// Sound made with it, where the model makes sound.
        #[serde(default = "sound_on")]
        audio: bool,
    },
    /// A picture from a description alone, or from pictures it edits or
    /// follows (image to image); see [`ImageMode`].
    Image {
        /// What it shows, its style and light, and any words in it. With
        /// references it names them by place: `@Image1`, `@Image2`.
        prompt: String,
        /// "16:9", "1:1"…; `None` lets the model choose.
        #[serde(default)]
        aspect: Option<String>,
        /// "2K"; `None` takes the model's default.
        #[serde(default)]
        resolution: Option<String>,
        /// Pictures it edits or follows, in the order the prompt counts them.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        references: Vec<PathBuf>,
        /// A transparent background, where the model makes one.
        #[serde(default)]
        transparent: bool,
    },
}

fn sound_on() -> bool {
    true
}

/// A file a video follows. The prompt names it by kind and place among
/// that kind: the second picture is `@Image2`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoReference {
    pub kind: MediaKind,
    pub path: PathBuf,
    /// How long a clip or a sound runs, when known.
    #[serde(default)]
    pub length: Option<Time>,
}

impl VideoReference {
    /// What the prompt calls the `n`th (from 1) of its kind: "@Image2".
    pub fn tag(kind: MediaKind, n: usize) -> String {
        let what = match kind {
            MediaKind::Image => "Image",
            MediaKind::Video => "Video",
            MediaKind::Audio => "Audio",
        };
        format!("@{what}{n}")
    }
}

/// How a video is made, by what its request carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoMode {
    Text,
    /// From a first frame, to a last one when there is one.
    Image,
    /// After references.
    Reference,
}

/// What one video model can make, so a request is checked before it costs
/// anything.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoModel {
    pub name: &'static str,
    /// The whole seconds it makes, shortest first.
    pub lengths: &'static [u32],
    /// Aspect ratios it takes, "16:9"…
    pub aspects: &'static [&'static str],
    /// Its resolutions in lower case ("720p", "4k"), the default first;
    /// none when it has no choice.
    pub resolutions: &'static [&'static str],
    /// A first frame sets the shape; an aspect ratio is ignored with one.
    pub frame_sets_aspect: bool,
    /// Ends on a picture of its own, after a first frame.
    pub last_frame: bool,
    /// What it follows; `None` when it takes no references.
    pub references: Option<ReferenceLimits>,
    /// Whether it makes sound with the picture.
    pub audio: VideoSound,
}

/// Whether a video model makes sound, and whether that can be turned off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoSound {
    None,
    /// Always, with no switch.
    Always,
    /// When asked; off makes it silent (and on some models, cheaper).
    Switch,
}

/// How many references a video model follows, and how long their clips and
/// sounds run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReferenceLimits {
    pub images: usize,
    pub videos: usize,
    pub sounds: usize,
    /// Of every kind together.
    pub files: usize,
    /// Seconds the clips run together, and the sounds together.
    pub seconds: f64,
}

impl VideoModel {
    /// Whether a video it makes has sound, when sound is `wanted`.
    pub fn makes_sound(&self, wanted: bool) -> bool {
        match self.audio {
            VideoSound::None => false,
            VideoSound::Always => true,
            VideoSound::Switch => wanted,
        }
    }

    pub fn shortest(&self) -> u32 {
        self.lengths.first().copied().unwrap_or(1)
    }

    pub fn longest(&self) -> u32 {
        self.lengths.last().copied().unwrap_or(1)
    }

    /// The length it makes nearest `secs`.
    pub fn nearest_length(&self, secs: f64) -> u32 {
        self.lengths
            .iter()
            .copied()
            .min_by(|a, b| {
                (*a as f64 - secs)
                    .abs()
                    .total_cmp(&(*b as f64 - secs).abs())
            })
            .unwrap_or(1)
    }

    /// "4 to 15 s", or "6, 8 or 10 s".
    fn lengths_text(&self) -> String {
        let (first, last) = (self.shortest(), self.longest());
        if self.lengths.len() as u32 == last - first + 1 {
            return format!("{first} to {last} s");
        }
        let names: Vec<String> = self.lengths.iter().map(u32::to_string).collect();
        match names.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} or {last} s", rest.join(", ")),
            _ => format!("{} s", names.join("")),
        }
    }

    /// Why it can't make `request`, before any money is spent on it.
    pub fn check(&self, request: &GenerateRequest) -> CloudResult<()> {
        let GenerateRequest::Video {
            length,
            aspect,
            resolution,
            first_frame,
            last_frame,
            references,
            ..
        } = request
        else {
            return Ok(());
        };
        let refuse = |message: String| {
            Err(CloudError::BadPrompt {
                message,
                suggestion: None,
            })
        };
        let name = self.name;
        if let Some(length) = length {
            let secs = length.as_secs_f64().round();
            if !self.lengths.iter().any(|l| *l as f64 == secs) {
                return refuse(format!("{name} makes {} of video", self.lengths_text()));
            }
        }
        if let Some(aspect) = aspect
            && !(first_frame.is_some() && self.frame_sets_aspect)
            && !self.aspects.contains(&aspect.as_str())
        {
            return refuse(format!(
                "{name} makes {} video, not {aspect}",
                self.aspects.join(", ")
            ));
        }
        if let Some(resolution) = resolution
            && !self
                .resolutions
                .iter()
                .any(|r| r.eq_ignore_ascii_case(resolution))
        {
            return refuse(if self.resolutions.is_empty() {
                format!("{name} has no choice of resolution")
            } else {
                format!(
                    "{name} makes {} video, not {resolution}",
                    self.resolutions.join(" or ")
                )
            });
        }
        if last_frame.is_some() && first_frame.is_none() {
            return refuse("A last frame needs a first frame".into());
        }
        if last_frame.is_some() && !self.last_frame {
            return refuse(format!("{name} doesn't end on a given frame"));
        }
        if references.is_empty() {
            return Ok(());
        }
        if first_frame.is_some() {
            return refuse("A video follows references or starts on a frame, not both".into());
        }
        let Some(limits) = self.references else {
            return refuse(format!("{name} doesn't follow references"));
        };
        let of = |kind: MediaKind| references.iter().filter(move |r| r.kind == kind);
        let counts = [
            (MediaKind::Image, limits.images, "pictures"),
            (MediaKind::Video, limits.videos, "clips"),
            (MediaKind::Audio, limits.sounds, "sounds"),
        ];
        for (kind, most, what) in counts {
            let n = of(kind).count();
            if n > 0 && most == 0 {
                return refuse(format!("{name} doesn't follow {what}"));
            }
            if n > most {
                return refuse(format!(
                    "{name} follows at most {most} {what}; this has {n}"
                ));
            }
            let secs: f64 = of(kind)
                .filter_map(|r| r.length)
                .map(Time::as_secs_f64)
                .sum();
            if kind != MediaKind::Image && secs > limits.seconds + 0.05 {
                return refuse(format!(
                    "{name} follows {what} of {:.0} s together at most; these run {secs:.1} s",
                    limits.seconds
                ));
            }
        }
        if references.len() > limits.files {
            return refuse(format!(
                "{name} follows at most {} files; this has {}",
                limits.files,
                references.len()
            ));
        }
        Ok(())
    }
}

/// How a picture is made, by what its request carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageMode {
    Text,
    /// From pictures it edits or follows.
    Edit,
}

/// What one image model can make, so a request is checked before it costs
/// anything.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageModel {
    pub name: &'static str,
    /// Aspect ratios it takes, "16:9"…
    pub aspects: &'static [&'static str],
    /// It can pick the shape itself (from the first picture, when editing).
    pub auto_aspect: bool,
    /// Its resolutions ("1K", "2K"), the default first; none when it has no
    /// choice.
    pub resolutions: &'static [&'static str],
    /// The most pictures it edits or follows at once; 0 when it only makes
    /// pictures from a description.
    pub references: usize,
    /// It makes pictures with a transparent background.
    pub transparent: bool,
}

impl ImageModel {
    /// Why it can't make `request`, before any money is spent on it.
    pub fn check(&self, request: &GenerateRequest) -> CloudResult<()> {
        let GenerateRequest::Image {
            aspect,
            resolution,
            references,
            transparent,
            ..
        } = request
        else {
            return Ok(());
        };
        let refuse = |message: String| {
            Err(CloudError::BadPrompt {
                message,
                suggestion: None,
            })
        };
        let name = self.name;
        if let Some(aspect) = aspect
            && !self.aspects.contains(&aspect.as_str())
        {
            return refuse(if self.aspects.is_empty() {
                format!("{name} has no choice of shape")
            } else {
                format!(
                    "{name} makes {} pictures, not {aspect}",
                    self.aspects.join(", ")
                )
            });
        }
        if let Some(resolution) = resolution
            && !self
                .resolutions
                .iter()
                .any(|r| r.eq_ignore_ascii_case(resolution))
        {
            return refuse(if self.resolutions.is_empty() {
                format!("{name} has no choice of resolution")
            } else {
                format!(
                    "{name} makes {} pictures, not {resolution}",
                    self.resolutions.join(" or ")
                )
            });
        }
        if !references.is_empty() && self.references == 0 {
            return refuse(format!("{name} only makes pictures from a description"));
        }
        if references.len() > self.references {
            return refuse(format!(
                "{name} follows at most {} pictures; this has {}",
                self.references,
                references.len()
            ));
        }
        if *transparent && !self.transparent {
            return refuse(format!("{name} doesn't make transparent pictures"));
        }
        Ok(())
    }
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

/// The speech around a voiceover that continues a recording: a word fixed
/// in the speaker's voice is read as part of what they said.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpeechContext {
    #[serde(default)]
    pub previous: String,
    #[serde(default)]
    pub next: String,
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
            Self::Video { .. } => Capability::Video,
            Self::Image { .. } => Capability::Image,
        }
    }

    pub fn prompt(&self) -> &str {
        match self {
            Self::Music { prompt, .. }
            | Self::SoundEffect { prompt, .. }
            | Self::Video { prompt, .. }
            | Self::Image { prompt, .. } => prompt,
            Self::Voiceover { text, .. } => text,
        }
    }

    /// How a video is made; `None` for anything else.
    pub fn video_mode(&self) -> Option<VideoMode> {
        match self {
            Self::Video { references, .. } if !references.is_empty() => Some(VideoMode::Reference),
            Self::Video {
                first_frame: Some(_),
                ..
            } => Some(VideoMode::Image),
            Self::Video { .. } => Some(VideoMode::Text),
            _ => None,
        }
    }

    /// How a picture is made; `None` for anything else.
    pub fn image_mode(&self) -> Option<ImageMode> {
        match self {
            Self::Image { references, .. } if !references.is_empty() => Some(ImageMode::Edit),
            Self::Image { .. } => Some(ImageMode::Text),
            _ => None,
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
            Self::Music { length, .. }
            | Self::SoundEffect { length, .. }
            | Self::Video { length, .. } => *length,
            Self::Voiceover { .. } | Self::Image { .. } => None,
        }
    }

    /// One line for cards and logs: "Music, 30 s: calm lo-fi beat".
    pub fn summary(&self) -> String {
        if let Self::Voiceover { text, reading, .. } = self {
            let start: String = text.chars().take(SUMMARY_CHARS).collect();
            let more = if start.len() < text.len() { "…" } else { "" };
            return format!("Voiceover, {}: {start}{more}", reading.voice_name);
        }
        let what = match self {
            Self::Music { .. } => "Music",
            Self::SoundEffect { .. } => "Sound effect",
            Self::Voiceover { .. } => "Voiceover",
            Self::Video { .. } => "Video",
            Self::Image { .. } => "Image",
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

/// The words a voiceover's `script` says, punctuation on the word before
/// it. Audio tags are not said, and a dash, an ellipsis or a full stop in
/// Chinese or Japanese ends a word without a space; so does each character
/// that is a word or close to one there.
pub fn spoken_words(script: &str) -> Vec<String> {
    let ends_word = |c: char| {
        matches!(c, '—' | '–' | '…' | '、' | '。' | '，' | '！' | '？')
            || matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
    };
    let mut words: Vec<String> = Vec::new();
    let text = without_tags(script);
    for part in text
        .split_whitespace()
        .flat_map(|token| token.split_inclusive(ends_word))
    {
        let said = part.chars().any(char::is_alphanumeric);
        match words.last_mut() {
            Some(word) if !said => word.push_str(part),
            _ if said => words.push(part.to_owned()),
            _ => {}
        }
    }
    words
}

/// `script` with a space for each `[audio tag]`.
fn without_tags(script: &str) -> String {
    let mut out = String::new();
    let mut rest = script;
    while let Some((said, tag)) = rest.split_once('[') {
        out.push_str(said);
        out.push(' ');
        rest = tag.split_once(']').map_or(tag, |(_, after)| after);
    }
    out + rest
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

/// A voice a provider can read with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Voice {
    pub id: String,
    pub name: String,
    /// "premade", "cloned", "generated"…, as the provider names them.
    pub category: String,
    /// What it is like: "calm", "american", "narration".
    pub traits: Vec<String>,
    /// Where a sample of it can be heard.
    pub sample: Option<String>,
    /// What whoever made it says of it.
    pub description: String,
    /// Shared in the provider's voice library by someone else: their public
    /// id. Such a voice is added to the account before it reads.
    pub owner: Option<String>,
    /// What a character read with it costs against the usual, when it
    /// costs more (a shared voice whose owner set a rate).
    pub rate: Option<f32>,
    /// Days of notice its owner gives before withdrawing a shared voice.
    pub notice_days: Option<u32>,
    /// ISO 639-1 language it speaks best, when the provider says.
    pub language: Option<String>,
}

impl Voice {
    /// Made by the account's owner: designed or cloned, not one of the
    /// provider's own or added from the library.
    pub fn mine(&self) -> bool {
        self.owner.is_none() && matches!(self.category.as_str(), "generated" | "cloned")
    }

    /// From the provider's shared library, not on the account yet.
    pub fn shared(&self) -> bool {
        self.owner.is_some()
    }

    /// Name, traits and description in one lower-case line, to match
    /// words against.
    pub fn about(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name,
            self.category,
            self.traits.join(" "),
            self.description
        )
        .to_lowercase()
    }
}

/// What to look for in a provider's shared voice library. Empty fields
/// don't filter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceQuery {
    /// Words in the name, description or labels.
    pub text: String,
    /// One of [`VOICE_GENDERS`].
    pub gender: Option<String>,
    /// One of [`VOICE_AGES`].
    pub age: Option<String>,
    /// ISO 639-1.
    pub language: Option<String>,
    /// One of [`VOICE_USES`].
    pub use_case: Option<String>,
}

impl VoiceQuery {
    /// Whether `voice` (one of the account's) fits it; the shared library
    /// filters on the provider's side.
    pub fn fits(&self, voice: &Voice) -> bool {
        let about = voice.about();
        let words = self.text.split_whitespace().map(str::to_lowercase);
        let labels = [&self.gender, &self.age, &self.use_case];
        words.into_iter().all(|w| about.contains(&w))
            && labels
                .into_iter()
                .flatten()
                .all(|l| about.contains(&l.replace('_', " ")))
            && self
                .language
                .as_ref()
                .is_none_or(|l| voice.language.as_ref().is_none_or(|v| v == l))
    }
}

/// Values the shared library's filters take, as the provider spells them.
pub const VOICE_GENDERS: [&str; 3] = ["female", "male", "neutral"];
pub const VOICE_AGES: [&str; 3] = ["young", "middle_aged", "old"];
pub const VOICE_USES: [&str; 7] = [
    "narrative_story",
    "conversational",
    "characters_animation",
    "social_media",
    "entertainment_tv",
    "advertisement",
    "informative_educational",
];
pub const VOICE_LANGUAGES: [(&str, &str); 16] = [
    ("en", "English"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("sv", "Swedish"),
    ("tr", "Turkish"),
    ("ru", "Russian"),
    ("ar", "Arabic"),
    ("hi", "Hindi"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh", "Chinese"),
];

/// A filter value as the user reads it: "middle_aged" → "Middle aged".
pub fn voice_label(value: &str) -> String {
    let text = value.replace('_', " ");
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// One page of the shared library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoicePage {
    pub voices: Vec<Voice>,
    /// Whether a next page has more.
    pub more: bool,
}

/// A voice to make from a description.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoiceDesign {
    /// Who it is and how they sound: age, gender, accent, tone, pace.
    pub description: String,
    /// What the samples say; `None` lets the provider write it.
    #[serde(default)]
    pub text: Option<String>,
}

/// How long a voice description and a sample text run, in characters.
pub const DESIGN_DESCRIPTION: (usize, usize) = (20, 1000);
pub const DESIGN_TEXT: (usize, usize) = (100, 1000);

impl VoiceDesign {
    /// Why the provider would refuse it, before any credits are spent.
    pub fn check(&self) -> CloudResult<()> {
        let refuse = |message: String| {
            Err(CloudError::BadPrompt {
                message,
                suggestion: None,
            })
        };
        let (min, max) = DESIGN_DESCRIPTION;
        let n = self.description.trim().chars().count();
        if n < min || n > max {
            return refuse(format!(
                "A voice description runs {min} to {max} characters; this has {n}"
            ));
        }
        if let Some(text) = &self.text {
            let (min, max) = DESIGN_TEXT;
            let n = text.trim().chars().count();
            if n < min || n > max {
                return refuse(format!(
                    "A sample text runs {min} to {max} characters; this has {n}"
                ));
            }
        }
        Ok(())
    }
}

/// One of the voices a design offers, heard before it is kept.
#[derive(Clone, Debug, PartialEq)]
pub struct VoicePreview {
    /// The provider's id for it until it is saved.
    pub id: String,
    /// Its sample, under `paths::cache()`.
    pub audio: PathBuf,
}

/// What a design came back with.
#[derive(Clone, Debug, PartialEq)]
pub struct Designed {
    pub previews: Vec<VoicePreview>,
    /// What the samples say.
    pub text: String,
}

/// A voice to clone from recordings of it.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceClone {
    pub name: String,
    pub description: String,
    /// Clean speech, one voice; a minute or more together.
    pub files: Vec<PathBuf>,
    /// Have the provider take out background noise first.
    pub denoise: bool,
}

/// How a voice in the user's library came to be.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VoiceOrigin {
    /// Designed from a description, which can design it again.
    Designed {
        description: String,
        #[serde(default)]
        text: Option<String>,
    },
    /// Cloned from recordings, named here.
    Cloned { sources: Vec<String> },
    /// Added from the provider's shared library.
    Shared {
        owner: String,
        /// Its name there.
        name: String,
    },
    /// On the account already (one of the provider's, or made elsewhere).
    Account,
}

/// Where a generation is, as its provider tells it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Stage {
    /// Sent, or made in one request that can't be followed or stopped.
    #[default]
    Starting,
    /// Waiting its turn; how many are ahead of it, when the provider says.
    Queued(Option<u32>),
    /// Being made.
    Running,
    /// The file coming down, 0..1.
    Downloading(f32),
}

impl Stage {
    /// Whether it can still be stopped: a provider follows it, and it isn't
    /// made yet (what is made is paid for, and kept). See also
    /// [`GenerationTask::can_stop`], for a provider that can't stop one.
    pub fn can_stop(self) -> bool {
        matches!(self, Self::Queued(_) | Self::Running)
    }
}

/// One generation while it runs: where it is, and whether the user stopped
/// it. Clones share it: the thread making it reports, the UI reads and
/// stops.
#[derive(Clone, Debug, Default)]
pub struct GenerationTask(Arc<TaskState>);

#[derive(Debug, Default)]
struct TaskState {
    stage: Mutex<Stage>,
    cancelled: AtomicBool,
    unstoppable: AtomicBool,
}

impl GenerationTask {
    pub fn stage(&self) -> Stage {
        *self.0.stage.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn report(&self, stage: Stage) {
        *self.0.stage.lock().unwrap_or_else(|e| e.into_inner()) = stage;
    }

    /// Said by a provider that follows it but has no way to stop it: it
    /// runs to the end, and is paid for.
    pub fn unstoppable(&self) {
        self.0.unstoppable.store(true, Ordering::Relaxed);
    }

    /// Whether stopping it now would stop it.
    pub fn can_stop(&self) -> bool {
        !self.0.unstoppable.load(Ordering::Relaxed) && self.stage().can_stop()
    }

    /// Ask the provider to stop it; what is already made is kept.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Relaxed)
    }

    /// [`CloudError::Cancelled`] once stopped, for a provider between
    /// steps.
    pub fn check(&self) -> CloudResult<()> {
        if self.cancelled() {
            Err(CloudError::Cancelled)
        } else {
            Ok(())
        }
    }
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

/// Name of the bin generated `item` goes into.
pub fn generated_bin(item: &MediaItem) -> &'static str {
    if item.kind == MediaKind::Image {
        return "Images Generated";
    }
    match item.generated.as_deref().map(|g| g.capability) {
        Some(Capability::SoundEffects) => "Sound Effects Generated",
        Some(Capability::Voiceover) => "Voiceover Generated",
        Some(Capability::Video) => "Video Generated",
        _ => "Music Generated",
    }
}

/// The media item for `generated`, probed and fingerprinted by `media`
/// when there is one; not in a bin yet ([`pool_commands`] puts it in one).
pub fn media_item(
    generated: &Generated,
    media: Option<&dyn crate::services::MediaService>,
) -> MediaItem {
    let kind = match generated.info.capability {
        Capability::Video => MediaKind::Video,
        Capability::Image => MediaKind::Image,
        _ => MediaKind::Audio,
    };
    MediaItem {
        id: MediaId::new(),
        path: generated.path.clone(),
        kind,
        fingerprint: media.and_then(|m| m.fingerprint(&generated.path).ok()),
        info: media.and_then(|m| m.probe(&generated.path).ok()),
        bin: None,
        tags: Vec::new(),
        proxy: None,
        generated: Some(Box::new(generated.info.clone())),
        name: None,
        tone_map: Default::default(),
    }
}

/// `path`, a picture an agent backend made with its own tool, as an item.
/// Not in a bin yet; with no provider, it has no [`GenerationInfo`].
pub fn image_item(path: PathBuf, media: Option<&dyn crate::services::MediaService>) -> MediaItem {
    MediaItem {
        id: MediaId::new(),
        kind: MediaKind::Image,
        fingerprint: media.and_then(|m| m.fingerprint(&path).ok()),
        info: media.and_then(|m| m.probe(&path).ok()),
        path,
        bin: None,
        tags: Vec::new(),
        proxy: None,
        generated: None,
        name: None,
        tone_map: Default::default(),
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
        name: None,
        tone_map: Default::default(),
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
