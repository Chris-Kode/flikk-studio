//! The project: media pool, one timeline, markers.
//!
//! Plain data with serde, so the store can persist it and the agent can read
//! it as JSON. Mutation goes through [`crate::ops::Op`] only.

use crate::ids::*;
use crate::time::{Rate, Time, TimeRange};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub media: Vec<MediaItem>,
    pub bins: Vec<Bin>,
    pub timeline: Timeline,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: ProjectId::new(),
            name: name.into(),
            media: Vec::new(),
            bins: Vec::new(),
            timeline: Timeline::default(),
        }
    }

    pub fn media(&self, id: MediaId) -> Option<&MediaItem> {
        self.media.iter().find(|m| m.id == id)
    }
}

// ---------------------------------------------------------------- media pool

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bin {
    pub id: BinId,
    pub name: String,
    pub parent: Option<BinId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Video,
    Audio,
    Image,
}

/// Content hash of a source file. Analysis caches, collages and source facts
/// are keyed by this, never by path (PRD F1, F2, V6).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Fingerprint(pub String);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: MediaId,
    pub path: PathBuf,
    pub kind: MediaKind,
    pub fingerprint: Option<Fingerprint>,
    pub info: Option<MediaInfo>,
    pub bin: Option<BinId>,
    pub tags: Vec<String>,
    /// Proxy file used for playback when present (PRD M4).
    pub proxy: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaInfo {
    pub duration: Time,
    pub video: Option<VideoStream>,
    pub audio: Vec<AudioStream>,
    pub container: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoStream {
    pub index: u32,
    pub width: u32,
    pub height: u32,
    pub rate: Rate,
    pub codec: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioStream {
    pub index: u32,
    pub channels: u32,
    pub sample_rate: u32,
    pub codec: String,
    /// Language tag from the container or detected by analysis.
    pub language: Option<String>,
}

// ------------------------------------------------------------------ timeline

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    pub settings: SequenceSettings,
    /// Top to bottom as drawn: overlays/titles/captions/video above, audio below.
    pub tracks: Vec<Track>,
    pub markers: Vec<Marker>,
    pub ducking: Option<Ducking>,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            settings: SequenceSettings::default(),
            tracks: vec![
                Track::new(TrackKind::Caption, "C1"),
                Track::new(TrackKind::Video, "V1"),
                Track::new(TrackKind::Audio, "A1"),
            ],
            markers: Vec::new(),
            ducking: None,
        }
    }
}

impl Timeline {
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn clip(&self, id: ClipId) -> Option<(&Track, &Clip)> {
        self.tracks
            .iter()
            .find_map(|t| t.clips.iter().find(|c| c.id == id).map(|c| (t, c)))
    }

    pub fn marker(&self, id: MarkerId) -> Option<&Marker> {
        self.markers.iter().find(|m| m.id == id)
    }

    /// End of the last clip on any track.
    pub fn duration(&self) -> Time {
        self.tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.end()))
            .max()
            .unwrap_or(Time::ZERO)
    }

    /// Protected ranges that a cut over `range` would touch (PRD F4).
    pub fn protected_overlapping(&self, range: TimeRange) -> impl Iterator<Item = &Marker> {
        self.markers
            .iter()
            .filter(move |m| m.kind == MarkerKind::Protected && m.range.overlaps(&range))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequenceSettings {
    pub width: u32,
    pub height: u32,
    pub rate: Rate,
    pub sample_rate: u32,
}

impl Default for SequenceSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            rate: Rate::FPS_60,
            sample_rate: 48_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Video,
    Audio,
    Title,
    Caption,
    Overlay,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub name: String,
    /// Sorted by `start`, never overlapping.
    pub clips: Vec<Clip>,
    pub mix: TrackMix,
    pub locked: bool,
    /// Caption tracks: preset name from the style profile (PRD C2).
    pub caption_style: Option<String>,
}

impl Track {
    pub fn new(kind: TrackKind, name: impl Into<String>) -> Self {
        Self {
            id: TrackId::new(),
            kind,
            name: name.into(),
            clips: Vec::new(),
            mix: TrackMix::default(),
            locked: false,
            caption_style: None,
        }
    }

    /// The clip under timeline time `t`, if any.
    pub fn clip_at(&self, t: Time) -> Option<&Clip> {
        self.clips.iter().find(|c| c.range().contains(t))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackMix {
    pub volume_db: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
}

impl Default for TrackMix {
    fn default() -> Self {
        Self {
            volume_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    /// Timeline position of the first frame.
    pub start: Time,
    pub duration: Time,
    pub content: ClipContent,
    pub transform: Transform,
    pub keyframes: Vec<Keyframe>,
    pub grade: Grade,
    pub audio: ClipAudio,
}

impl Clip {
    pub fn new(start: Time, duration: Time, content: ClipContent) -> Self {
        Self {
            id: ClipId::new(),
            start,
            duration,
            content,
            transform: Transform::default(),
            keyframes: Vec::new(),
            grade: Grade::default(),
            audio: ClipAudio::default(),
        }
    }

    pub fn end(&self) -> Time {
        self.start + self.duration
    }

    pub fn range(&self) -> TimeRange {
        TimeRange::new(self.start, self.end())
    }

    /// Timeline time → source time for media clips (PRD E8).
    pub fn to_source(&self, t: Time) -> Option<Time> {
        match &self.content {
            ClipContent::Media { source_in, .. } => Some(*source_in + (t - self.start)),
            _ => None,
        }
    }

    /// Source time → timeline time, if that source frame is inside the clip.
    pub fn to_timeline(&self, source_t: Time) -> Option<Time> {
        match &self.content {
            ClipContent::Media { source_in, .. } => {
                let t = self.start + (source_t - *source_in);
                self.range().contains(t).then_some(t)
            }
            _ => None,
        }
    }

    pub fn media(&self) -> Option<MediaId> {
        match &self.content {
            ClipContent::Media { media, .. } => Some(*media),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClipContent {
    /// A range of a pooled source. `source_in` is the source time at `start`.
    Media {
        media: MediaId,
        source_in: Time,
        /// Audio stream used on audio tracks: the container index
        /// (`AudioStream::index`). Media services fall back to the n-th
        /// audio stream when no stream has that index.
        audio_stream: Option<u32>,
    },
    Title {
        template: String,
        text: String,
    },
    Caption {
        text: String,
        speaker: Option<SpeakerId>,
    },
    /// Image/asset overlay tied optionally to a speaker (PRD C5).
    Overlay {
        asset: PathBuf,
        speaker: Option<SpeakerId>,
    },
}

// ------------------------------------------------------ transform & keyframes

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    /// Normalized offset, (0, 0) = centered.
    pub position: [f32; 2],
    pub scale: f32,
    pub rotation_deg: f32,
    pub opacity: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0],
            scale: 1.0,
            rotation_deg: 0.0,
            opacity: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Property {
    PositionX,
    PositionY,
    Scale,
    Rotation,
    Opacity,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    Linear,
    #[default]
    EaseInOut,
    EaseIn,
    EaseOut,
    Hold,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub property: Property,
    /// Relative to the clip's start.
    pub at: Time,
    pub value: f32,
    /// Easing into the next keyframe.
    pub ease: Ease,
}

// --------------------------------------------------------------------- color

/// Primary correction (PRD K1). Neutral values leave the image untouched.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Grade {
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub gamma: f32,
    pub temperature: f32,
    pub tint: f32,
    pub lift: [f32; 3],
    pub gamma_rgb: [f32; 3],
    pub gain: [f32; 3],
    pub lut: Option<PathBuf>,
}

impl Default for Grade {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            gamma: 1.0,
            temperature: 0.0,
            tint: 0.0,
            lift: [0.0; 3],
            gamma_rgb: [1.0; 3],
            gain: [1.0; 3],
            lut: None,
        }
    }
}

// --------------------------------------------------------------------- audio

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClipAudio {
    pub gain_db: f32,
    pub fade_in: Time,
    pub fade_out: Time,
}

/// Music under dialogue (PRD A3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ducking {
    pub music_track: TrackId,
    pub reduction_db: f32,
    pub attack: Time,
    pub release: Time,
}

// ------------------------------------------------------------------- markers

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerKind {
    #[default]
    Note,
    /// No cut may remove this range without a warning (PRD F4).
    Protected,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub id: MarkerId,
    /// Zero-length for point markers.
    pub range: TimeRange,
    pub kind: MarkerKind,
    pub note: String,
}
