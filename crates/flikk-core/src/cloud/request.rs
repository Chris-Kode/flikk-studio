//! What a provider is asked to make, and what each video and image model
//! can make, so a request is checked before it costs anything.

use super::{Capability, CloudError, CloudResult};
use crate::model::MediaKind;
use crate::time::Time;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

    /// The resolution it makes for `preferred` ("1080p"): the largest of
    /// its own at or below it, else its smallest; `None` when it has no
    /// choice of resolution.
    pub fn resolution_for(&self, preferred: &str) -> Option<&'static str> {
        let most = resolution_lines(preferred)?;
        let lines = |r: &&&str| resolution_lines(r).unwrap_or(0);
        self.resolutions
            .iter()
            .filter(|r| lines(r) <= most)
            .max_by_key(lines)
            .or_else(|| self.resolutions.iter().min_by_key(lines))
            .copied()
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

/// The resolutions a user can prefer for video, smallest first; each
/// model makes the nearest it has ([`VideoModel::resolution_for`]).
pub const VIDEO_RESOLUTIONS: [&str; 5] = ["480p", "720p", "1080p", "2k", "4k"];

/// How many lines a resolution has on its short side: "720p" is 720,
/// "2k" 1440 and "4k" 2160, in any case; `None` when it isn't one.
pub fn resolution_lines(resolution: &str) -> Option<u32> {
    match resolution.trim().to_ascii_lowercase().as_str() {
        "2k" => Some(1440),
        "4k" | "uhd" => Some(2160),
        other => other.strip_suffix('p')?.parse().ok().filter(|n| *n > 0),
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
