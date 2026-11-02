//! The project: media pool, one timeline, markers, and the project's own
//! FX programs.
//!
//! Plain data with serde, so the store can persist it and the agent can read
//! it as JSON. Mutation goes through [`crate::ops::Op`] only.

use crate::audiofx::AudioFx;
use crate::fx::{FxAsset, FxInstance, Transition};
use crate::ids::*;
use crate::mask::Mask;
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
    /// Shader and HTML programs written for this project (by the user or
    /// the agent). Built-ins are not stored here; see [`crate::fx::builtins`].
    #[serde(default)]
    pub fx: Vec<FxAsset>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: ProjectId::new(),
            name: name.into(),
            media: Vec::new(),
            bins: Vec::new(),
            timeline: Timeline::default(),
            fx: Vec::new(),
        }
    }

    pub fn media(&self, id: MediaId) -> Option<&MediaItem> {
        self.media.iter().find(|m| m.id == id)
    }

    /// Footage and photos on video tracks: the clips that fit or fill the
    /// frame ([`Fit`]) as a whole. Overlays and generators keep their place.
    pub fn framed_clips(&self) -> impl Iterator<Item = &Clip> {
        self.timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Video)
            .flat_map(|t| &t.clips)
            .filter(|c| {
                c.media()
                    .and_then(|m| self.media(m))
                    .is_some_and(|m| m.kind != MediaKind::Audio)
            })
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
    /// How a provider made it, when it was generated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<Box<crate::cloud::GenerationInfo>>,
    /// How HDR pictures are mapped to SDR (see [`crate::hdr`]).
    #[serde(default, skip_serializing_if = "crate::hdr::ToneMap::is_default")]
    pub tone_map: crate::hdr::ToneMap,
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
    /// Set for PQ and HLG streams.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdr: Option<Box<crate::hdr::Hdr>>,
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
    /// Applied to every media picture after its own grade.
    #[serde(default)]
    pub grade: Grade,
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
            grade: Grade::default(),
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

impl SequenceSettings {
    /// The smaller of width and height: what "1080p" names in any shape.
    pub fn short_side(&self) -> u32 {
        self.width.min(self.height)
    }

    /// The built-in format this size has, if any.
    pub fn format(&self) -> Option<Format> {
        Format::of(self.width, self.height)
    }

    /// This frame's shape with `short` pixels on the short side, both even.
    pub fn scaled(&self, short: u32) -> (u32, u32) {
        if let Some(f) = self.format() {
            return f.size(short);
        }
        let k = short as f64 / self.short_side().max(1) as f64;
        let even = |v: u32| ((v as f64 * k / 2.0).round() as u32).max(1) * 2;
        (even(self.width), even(self.height))
    }
}

/// A frame shape for the sequence, named for where it plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// Width : height, e.g. `(9, 16)`.
    pub aspect: (u32, u32),
    /// Platforms that play this shape.
    pub uses: &'static str,
}

impl Format {
    pub const ALL: [Format; 6] = [
        Format {
            aspect: (16, 9),
            uses: "YouTube, TV, desktop",
        },
        Format {
            aspect: (9, 16),
            uses: "TikTok, Reels, Shorts, Stories",
        },
        Format {
            aspect: (1, 1),
            uses: "Instagram, Facebook, LinkedIn square posts",
        },
        Format {
            aspect: (4, 5),
            uses: "Instagram and Facebook feed",
        },
        Format {
            aspect: (21, 9),
            uses: "Cinema widescreen",
        },
        Format {
            aspect: (4, 3),
            uses: "Classic TV, presentations",
        },
    ];

    /// Short sides offered for every format, with their names.
    pub const RESOLUTIONS: [(u32, &'static str); 4] = [
        (720, "720p"),
        (1080, "1080p"),
        (1440, "1440p"),
        (2160, "4K"),
    ];

    /// `"9:16"`.
    pub fn name(self) -> String {
        format!("{}:{}", self.aspect.0, self.aspect.1)
    }

    /// The format whose shape `width` × `height` has, to within rounding to
    /// even sizes.
    pub fn of(width: u32, height: u32) -> Option<Format> {
        Self::ALL.into_iter().find(|f| {
            let (w, h) = f.size(width.min(height));
            w.abs_diff(width) <= 2 && h.abs_diff(height) <= 2
        })
    }

    /// Frame size with `short` pixels on the short side, both even.
    pub fn size(self, short: u32) -> (u32, u32) {
        let (a, b) = (self.aspect.0.max(1) as f64, self.aspect.1.max(1) as f64);
        let long = |ratio: f64| ((short as f64 * ratio / 2.0).round() as u32) * 2;
        let short = short & !1;
        if a >= b {
            (long(a / b), short)
        } else {
            (short, long(b / a))
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
    /// On video tracks: hidden.
    pub mute: bool,
    pub solo: bool,
    /// Track-level audio effects, after every clip's own (Fairlight-style
    /// track inserts), in order.
    #[serde(default)]
    pub effects: Vec<AudioFx>,
}

impl Default for TrackMix {
    fn default() -> Self {
        Self {
            volume_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            effects: Vec::new(),
        }
    }
}

fn one() -> f64 {
    1.0
}

fn yes() -> bool {
    true
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
    /// Playback speed of media clips: 2.0 plays twice as fast, so the clip
    /// covers twice as much source per timeline second. Always > 0.
    #[serde(default = "one")]
    pub speed: f64,
    /// Disabled clips stay on the timeline but are not seen or heard.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// User label shown on the timeline instead of the source name.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<ClipColor>,
    /// Video effect stack, applied in order after the grade and before the
    /// transform (see [`crate::fx`]).
    #[serde(default)]
    pub effects: Vec<FxInstance>,
    /// Transition into this clip: from the clip ending where this one starts
    /// on the same track, or from nothing (see [`crate::fx::transitions`]).
    #[serde(default)]
    pub transition_in: Option<Transition>,
    /// Transition out of this clip; at a cut it is used only when the next
    /// clip has no `transition_in`.
    #[serde(default)]
    pub transition_out: Option<Transition>,
    /// Shapes that keep or cut away parts of the picture, in order (see
    /// [`crate::mask`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<Mask>,
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
            speed: 1.0,
            enabled: true,
            name: None,
            color: None,
            effects: Vec::new(),
            transition_in: None,
            transition_out: None,
            masks: Vec::new(),
        }
    }

    pub fn end(&self) -> Time {
        self.start + self.duration
    }

    pub fn range(&self) -> TimeRange {
        TimeRange::new(self.start, self.end())
    }

    /// Timeline time → source time for media clips (PRD E8), honouring
    /// [`Clip::speed`]. Times outside the clip map onto its handles.
    pub fn to_source(&self, t: Time) -> Option<Time> {
        match &self.content {
            ClipContent::Media { source_in, .. } => {
                Some(*source_in + (t - self.start).scale(self.speed))
            }
            _ => None,
        }
    }

    /// Source time → timeline time, if that source frame is inside the clip.
    pub fn to_timeline(&self, source_t: Time) -> Option<Time> {
        match &self.content {
            ClipContent::Media { source_in, .. } => {
                let t = self.start + (source_t - *source_in).scale(1.0 / self.speed);
                self.range().contains(t).then_some(t)
            }
            _ => None,
        }
    }

    /// Label for lists and chat: the user's name, else what the clip shows.
    pub fn display_name(&self, project: &Project) -> String {
        if let Some(name) = self.name.as_ref().filter(|n| !n.trim().is_empty()) {
            return name.clone();
        }
        match &self.content {
            ClipContent::Media { media, .. } => project
                .media(*media)
                .and_then(|m| m.path.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Clip".into()),
            ClipContent::Title { text, .. } | ClipContent::Caption { text, .. } => {
                let mut s: String = text.chars().take(28).collect();
                if text.chars().count() > 28 {
                    s.push('…');
                }
                s
            }
            ClipContent::Overlay { asset, .. } => asset
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Overlay".into()),
            ClipContent::Generator { fx } => crate::fx::asset(project, &fx.fx)
                .map(|a| a.name.clone())
                .unwrap_or_else(|| "Generator".into()),
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
        /// When each word of `text` is spoken, from the clip's start; empty
        /// when unknown. Highlights spread unknown words by their length.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        words: Vec<Time>,
    },
    /// Image/asset overlay tied optionally to a speaker (PRD C5).
    Overlay {
        asset: PathBuf,
        speaker: Option<SpeakerId>,
    },
    /// Synthesized picture: a GLSL generator or an HTML motion graphic
    /// (titles, lower thirds, callouts), with alpha. See [`crate::fx`].
    Generator {
        fx: FxInstance,
    },
}

/// Clip color label, as in Resolve's clip color menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipColor {
    Orange,
    Yellow,
    Lime,
    Green,
    Teal,
    Blue,
    Navy,
    Purple,
    Pink,
    Red,
    Brown,
    Gray,
}

impl ClipColor {
    pub const ALL: [ClipColor; 12] = [
        Self::Orange,
        Self::Yellow,
        Self::Lime,
        Self::Green,
        Self::Teal,
        Self::Blue,
        Self::Navy,
        Self::Purple,
        Self::Pink,
        Self::Red,
        Self::Brown,
        Self::Gray,
    ];

    /// sRGB, 0xRRGGBB.
    pub fn rgb(self) -> u32 {
        match self {
            Self::Orange => 0xE8833A,
            Self::Yellow => 0xE6C229,
            Self::Lime => 0x9ACD32,
            Self::Green => 0x3FA34D,
            Self::Teal => 0x2A9D8F,
            Self::Blue => 0x3A86FF,
            Self::Navy => 0x264B8C,
            Self::Purple => 0x8E5BD9,
            Self::Pink => 0xE85D9E,
            Self::Red => 0xD64545,
            Self::Brown => 0x8B5E3C,
            Self::Gray => 0x8A8F98,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Orange => "Orange",
            Self::Yellow => "Yellow",
            Self::Lime => "Lime",
            Self::Green => "Green",
            Self::Teal => "Teal",
            Self::Blue => "Blue",
            Self::Navy => "Navy",
            Self::Purple => "Purple",
            Self::Pink => "Pink",
            Self::Red => "Red",
            Self::Brown => "Brown",
            Self::Gray => "Gray",
        }
    }
}

// ------------------------------------------------------ transform & keyframes

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transform {
    /// Offset from where the picture rests (centred, or where the style
    /// puts a caption), as a fraction of the canvas: (0, 0) = at rest, (0.5,
    /// 0) = its anchor moved half the frame width to the right.
    pub position: [f32; 2],
    pub scale: f32,
    /// Clockwise, about the anchor.
    pub rotation_deg: f32,
    pub opacity: f32,
    /// Per-axis scale on top of `scale`, [x, y]; 1 = none.
    pub stretch: [f32; 2],
    /// Slant in degrees, [x, y]: x leans vertical edges to the right going
    /// down, y leans horizontal edges down going right; 0 = none.
    pub skew_deg: [f32; 2],
    /// The point of the picture it is scaled, skewed and rotated about, as
    /// a fraction of its size from its centre: (0, 0) = centre, (-0.5,
    /// -0.5) = top-left corner. It stays where it rests (plus `position`),
    /// so moving it shifts a scaled, rotated or skewed picture; the
    /// viewer's anchor handle moves `position` to keep the picture still.
    pub anchor: [f32; 2],
    /// Fractions of the source picture cut away before scaling:
    /// [left, top, right, bottom], each 0..1, 0 = none.
    pub crop: [f32; 4],
    /// Mirror horizontally / vertically.
    pub flip: [bool; 2],
    /// How the clip composites over the tracks below it.
    pub blend: BlendMode,
    /// How the picture rests in the frame before the rest applies.
    pub fit: Fit,
    /// Corner pin: how far each corner of the picture moves, [top-left,
    /// top-right, bottom-right, bottom-left] as [x, y] fractions of its
    /// size, before it is stretched, scaled, skewed and turned; all zero =
    /// none. The picture is drawn in perspective onto the quad its corners
    /// make (a screen, a sign).
    pub corner_pin: [[f32; 2]; 4],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0],
            scale: 1.0,
            rotation_deg: 0.0,
            opacity: 1.0,
            stretch: [1.0, 1.0],
            skew_deg: [0.0, 0.0],
            anchor: [0.0, 0.0],
            crop: [0.0; 4],
            flip: [false; 2],
            blend: BlendMode::Normal,
            fit: Fit::Contain,
            corner_pin: [[0.0; 2]; 4],
        }
    }
}

impl Transform {
    /// Whether any corner is pinned away from where it rests.
    pub fn is_pinned(&self) -> bool {
        self.corner_pin.iter().flatten().any(|v| v.abs() > 1e-6)
    }
}

/// How a picture of another shape rests in the frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    /// Whole picture inside the frame, with bars on the other sides.
    #[default]
    Contain,
    /// Picture covering the frame, its overflow cut off; `position` pans it.
    Cover,
}

impl Fit {
    pub const ALL: [Fit; 2] = [Fit::Contain, Fit::Cover];

    pub fn name(self) -> &'static str {
        match self {
            Fit::Contain => "contain",
            Fit::Cover => "cover",
        }
    }

    /// Scale that rests a `picture`-sized frame on a `canvas`.
    pub fn scale(self, picture: (u32, u32), canvas: (u32, u32)) -> f64 {
        let x = canvas.0 as f64 / picture.0.max(1) as f64;
        let y = canvas.1 as f64 / picture.1.max(1) as f64;
        match self {
            Fit::Contain => x.min(y),
            Fit::Cover => x.max(y),
        }
    }
}

/// Composite mode (Resolve's "Composite Mode"). Formulas are the usual
/// separable blend modes on straight sRGB values, then mixed by alpha ×
/// opacity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
    Add,
    Screen,
    Multiply,
    Overlay,
    SoftLight,
    HardLight,
    Lighten,
    Darken,
    Difference,
}

impl BlendMode {
    pub const ALL: [BlendMode; 10] = [
        Self::Normal,
        Self::Add,
        Self::Screen,
        Self::Multiply,
        Self::Overlay,
        Self::SoftLight,
        Self::HardLight,
        Self::Lighten,
        Self::Darken,
        Self::Difference,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Add => "Add",
            Self::Screen => "Screen",
            Self::Multiply => "Multiply",
            Self::Overlay => "Overlay",
            Self::SoftLight => "Soft light",
            Self::HardLight => "Hard light",
            Self::Lighten => "Lighten",
            Self::Darken => "Darken",
            Self::Difference => "Difference",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Property {
    PositionX,
    PositionY,
    Scale,
    /// [`Transform::stretch`].
    ScaleX,
    ScaleY,
    Rotation,
    /// [`Transform::skew_deg`].
    SkewX,
    SkewY,
    /// [`Transform::anchor`].
    AnchorX,
    AnchorY,
    Opacity,
    CropLeft,
    CropTop,
    CropRight,
    CropBottom,
    /// Clip gain in dB; keyframes override [`ClipAudio::gain_db`]
    /// (volume automation).
    Volume,
    /// [`Transform::corner_pin`], corner by corner.
    PinTopLeftX,
    PinTopLeftY,
    PinTopRightX,
    PinTopRightY,
    PinBottomRightX,
    PinBottomRightY,
    PinBottomLeftX,
    PinBottomLeftY,
}

impl Property {
    pub const ALL: [Property; 24] = [
        Self::PositionX,
        Self::PositionY,
        Self::Scale,
        Self::ScaleX,
        Self::ScaleY,
        Self::Rotation,
        Self::SkewX,
        Self::SkewY,
        Self::AnchorX,
        Self::AnchorY,
        Self::Opacity,
        Self::CropLeft,
        Self::CropTop,
        Self::CropRight,
        Self::CropBottom,
        Self::Volume,
        Self::PinTopLeftX,
        Self::PinTopLeftY,
        Self::PinTopRightX,
        Self::PinTopRightY,
        Self::PinBottomRightX,
        Self::PinBottomRightY,
        Self::PinBottomLeftX,
        Self::PinBottomLeftY,
    ];

    /// The properties stored in [`Transform`].
    pub const TRANSFORM: [Property; 23] = [
        Self::PositionX,
        Self::PositionY,
        Self::Scale,
        Self::ScaleX,
        Self::ScaleY,
        Self::Rotation,
        Self::SkewX,
        Self::SkewY,
        Self::AnchorX,
        Self::AnchorY,
        Self::Opacity,
        Self::CropLeft,
        Self::CropTop,
        Self::CropRight,
        Self::CropBottom,
        Self::PinTopLeftX,
        Self::PinTopLeftY,
        Self::PinTopRightX,
        Self::PinTopRightY,
        Self::PinBottomRightX,
        Self::PinBottomRightY,
        Self::PinBottomLeftX,
        Self::PinBottomLeftY,
    ];

    /// The properties that place the picture: everything in [`Transform`]
    /// but opacity, crop and the corner pin.
    pub const GEOMETRY: [Property; 10] = [
        Self::PositionX,
        Self::PositionY,
        Self::Scale,
        Self::ScaleX,
        Self::ScaleY,
        Self::Rotation,
        Self::SkewX,
        Self::SkewY,
        Self::AnchorX,
        Self::AnchorY,
    ];

    /// [`Transform::corner_pin`] as properties: corner by corner, x then y.
    pub const PIN: [Property; 8] = [
        Self::PinTopLeftX,
        Self::PinTopLeftY,
        Self::PinTopRightX,
        Self::PinTopRightY,
        Self::PinBottomRightX,
        Self::PinBottomRightY,
        Self::PinBottomLeftX,
        Self::PinBottomLeftY,
    ];

    /// Tool / JSON name, e.g. `crop_left`.
    pub fn name(self) -> &'static str {
        match self {
            Self::PositionX => "position_x",
            Self::PositionY => "position_y",
            Self::Scale => "scale",
            Self::Rotation => "rotation",
            Self::Opacity => "opacity",
            Self::CropLeft => "crop_left",
            Self::CropTop => "crop_top",
            Self::CropRight => "crop_right",
            Self::CropBottom => "crop_bottom",
            Self::Volume => "volume",
            Self::ScaleX => "scale_x",
            Self::ScaleY => "scale_y",
            Self::SkewX => "skew_x",
            Self::SkewY => "skew_y",
            Self::AnchorX => "anchor_x",
            Self::AnchorY => "anchor_y",
            Self::PinTopLeftX => "pin_top_left_x",
            Self::PinTopLeftY => "pin_top_left_y",
            Self::PinTopRightX => "pin_top_right_x",
            Self::PinTopRightY => "pin_top_right_y",
            Self::PinBottomRightX => "pin_bottom_right_x",
            Self::PinBottomRightY => "pin_bottom_right_y",
            Self::PinBottomLeftX => "pin_bottom_left_x",
            Self::PinBottomLeftY => "pin_bottom_left_y",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }
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

/// Primary correction (PRD K1) plus curves. Neutral values leave the image
/// untouched. Order: LUT, exposure, white balance (temperature, tint),
/// lift/gamma/gain and offset, shadows/highlights, contrast, gamma,
/// saturation and hue, then curves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
    /// Added to every channel, [r, g, b]; 0 = neutral (Resolve's Offset wheel).
    pub offset: [f32; 3],
    /// Lift or lower the darks / brights, -1..1; 0 = neutral.
    pub shadows: f32,
    pub highlights: f32,
    /// Hue rotation in degrees; 0 = neutral.
    pub hue: f32,
    pub curves: Curves,
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
            offset: [0.0; 3],
            shadows: 0.0,
            highlights: 0.0,
            hue: 0.0,
            curves: Curves::default(),
        }
    }
}

/// Custom curves: control points `[input, output]` in 0..1, sorted by
/// input, through which a monotone cubic is drawn. An empty list is the
/// identity. `master` applies to all channels after the per-channel curves.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Curves {
    pub master: Vec<[f32; 2]>,
    pub red: Vec<[f32; 2]>,
    pub green: Vec<[f32; 2]>,
    pub blue: Vec<[f32; 2]>,
}

impl Curves {
    pub fn is_identity(&self) -> bool {
        [&self.master, &self.red, &self.green, &self.blue]
            .iter()
            .all(|c| c.is_empty() || c.iter().all(|[x, y]| (x - y).abs() < 1e-4))
    }
}

// --------------------------------------------------------------------- audio

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClipAudio {
    pub gain_db: f32,
    pub fade_in: Time,
    pub fade_out: Time,
    /// Clip audio effects in order (EQ, dynamics, cleanup, space).
    #[serde(default)]
    pub effects: Vec<AudioFx>,
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
