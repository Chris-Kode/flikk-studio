//! The command vocabulary shared by the UI, the agent's MCP tools and recipes
//! (PRD "Agent tool API"). Read and view tools are not commands: they do not
//! change the project and are served by the agent crate from a snapshot.

use crate::audiofx::AudioFx;
use crate::error::Result;
use crate::fx::{FxAsset, FxInstance, ParamValue, Transition};
use crate::ids::*;
use crate::model::*;
use crate::ops::Op;
use crate::time::{Time, TimeRange};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Permission bucket for a tool (PRD "Agent tool API" table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    ReadProject,
    ReadAnalysis,
    View,
    EditTimeline,
    Transform,
    Captions,
    Color,
    Audio,
    Overlays,
    ProjectMemory,
    Deliver,
    /// Importing or removing pool media. Never available to the agent.
    Media,
    /// Shader effects, transitions, generators (incl. HTML motion graphics)
    /// and the project's FX programs.
    Effects,
}

impl ToolCategory {
    pub const ALL: [ToolCategory; 13] = [
        Self::ReadProject,
        Self::ReadAnalysis,
        Self::View,
        Self::EditTimeline,
        Self::Transform,
        Self::Captions,
        Self::Color,
        Self::Audio,
        Self::Overlays,
        Self::Effects,
        Self::ProjectMemory,
        Self::Deliver,
        Self::Media,
    ];

    pub fn is_read(self) -> bool {
        matches!(self, Self::ReadProject | Self::ReadAnalysis | Self::View)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ReadProject => "Read project",
            Self::ReadAnalysis => "Read analysis",
            Self::View => "View",
            Self::EditTimeline => "Edit timeline",
            Self::Transform => "Transform",
            Self::Captions => "Captions and titles",
            Self::Color => "Color",
            Self::Audio => "Audio",
            Self::Overlays => "Overlays",
            Self::ProjectMemory => "Project and memory",
            Self::Deliver => "Deliver",
            Self::Media => "Media",
            Self::Effects => "Effects and motion graphics",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Start,
    End,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertMode {
    /// Push later clips right.
    #[default]
    Insert,
    /// Replace whatever is underneath.
    Overwrite,
}

/// `None` = every unlocked track.
pub type TrackScope = Option<Vec<TrackId>>;

/// A clip to place by [`Command::PlaceClips`]: a copy of `clip` (any
/// content, with its effects, keyframes and grade) on `track`, starting at
/// `clip.start`. The copy gets a new id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlacedClip {
    pub track: TrackId,
    pub clip: Clip,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    // ---- edit timeline (E2, E4)
    Blade {
        at: Time,
        #[serde(default)]
        tracks: TrackScope,
    },
    RippleDelete {
        range: TimeRange,
        #[serde(default)]
        tracks: TrackScope,
        /// Audio fade on both sides of the join (see [`crate::cuts::FADE`]).
        #[serde(default)]
        fade: Time,
    },
    /// Remove clips, optionally closing the gap.
    DeleteClips {
        clips: Vec<ClipId>,
        ripple: bool,
    },
    InsertClip {
        media: MediaId,
        source: TimeRange,
        track: TrackId,
        at: Time,
        #[serde(default)]
        mode: InsertMode,
    },
    MoveClip {
        clip: ClipId,
        at: Time,
        track: Option<TrackId>,
    },
    Trim {
        clip: ClipId,
        edge: Edge,
        delta: Time,
        ripple: bool,
    },
    Roll {
        left: ClipId,
        right: ClipId,
        delta: Time,
    },
    Slip {
        clip: ClipId,
        delta: Time,
    },
    Slide {
        clip: ClipId,
        delta: Time,
    },
    AddMarker {
        range: TimeRange,
        #[serde(default)]
        kind: MarkerKind,
        note: String,
    },
    EditMarker {
        marker: MarkerId,
        range: TimeRange,
        kind: MarkerKind,
        note: String,
    },
    RemoveMarker {
        marker: MarkerId,
    },
    AddTrack {
        kind: TrackKind,
        index: usize,
        name: String,
    },
    RemoveTrack {
        track: TrackId,
    },
    /// Locked tracks refuse edits until unlocked.
    SetTrackLocked {
        track: TrackId,
        locked: bool,
    },
    /// Paste or duplicate: place copies of clips. Insert pushes later clips
    /// on each target track right by the span placed there; overwrite
    /// replaces what is underneath.
    PlaceClips {
        clips: Vec<PlacedClip>,
        #[serde(default)]
        mode: InsertMode,
    },
    /// Retime a media clip; the same source plays in `duration / speed`.
    /// Without ripple the clip stops at the next clip.
    SetSpeed {
        clip: ClipId,
        speed: f64,
        #[serde(default)]
        ripple: bool,
    },
    /// Disabled clips are kept but not seen or heard.
    SetClipsEnabled {
        clips: Vec<ClipId>,
        enabled: bool,
    },
    /// `None` restores the source name.
    RenameClip {
        clip: ClipId,
        name: Option<String>,
    },
    SetClipColor {
        clips: Vec<ClipId>,
        color: Option<ClipColor>,
    },

    // ---- transform (E5)
    SetTransform {
        clip: ClipId,
        transform: Transform,
    },
    /// Timeline-time zoom: ease in to `scale` at `target`, hold, ease out.
    AddZoom {
        clip: ClipId,
        range: TimeRange,
        target: [f32; 2],
        scale: f32,
        ease: Ease,
    },
    SetKeyframe {
        clip: ClipId,
        property: Property,
        at: Time,
        value: f32,
        ease: Ease,
    },
    RemoveKeyframe {
        clip: ClipId,
        property: Property,
        at: Time,
    },

    // ---- captions and titles (C1–C4)
    /// Caption style preset for a caption track (PRD C2); `None` = default.
    SetCaptionStyle {
        track: TrackId,
        style: Option<String>,
    },
    AddCaption {
        track: TrackId,
        range: TimeRange,
        text: String,
        speaker: Option<SpeakerId>,
    },
    /// Retext or retime a caption; titles take the same edits.
    EditCaption {
        clip: ClipId,
        text: Option<String>,
        range: Option<TimeRange>,
    },
    ApplyTitle {
        track: TrackId,
        template: String,
        range: TimeRange,
        text: String,
    },

    // ---- color (K1, K3, K4)
    SetGrade {
        clips: Vec<ClipId>,
        grade: Grade,
    },
    ApplyLut {
        clips: Vec<ClipId>,
        lut: Option<PathBuf>,
    },
    CopyGrade {
        from: ClipId,
        to: Vec<ClipId>,
    },
    SetTimelineGrade {
        grade: Grade,
    },

    // ---- audio (A1, A3, A4)
    SetGain {
        clip: ClipId,
        db: f32,
    },
    AddFade {
        clip: ClipId,
        edge: Edge,
        duration: Time,
    },
    SetTrackMix {
        track: TrackId,
        mix: TrackMix,
    },
    SetDucking {
        ducking: Option<Ducking>,
    },
    /// Lay songs back to back from `at`, replacing what the track had there.
    SetPlaylist {
        track: TrackId,
        at: Time,
        songs: Vec<MediaId>,
    },

    // ---- overlays (C5)
    AddOverlay {
        track: TrackId,
        asset: PathBuf,
        range: TimeRange,
        speaker: Option<SpeakerId>,
    },

    // ---- effects, transitions, generators
    /// Add `effect` to each clip's stack at `index` (default: last, i.e.
    /// applied after the others). Each clip gets its own copy and id.
    AddEffect {
        clips: Vec<ClipId>,
        effect: FxInstance,
        #[serde(default)]
        index: Option<usize>,
    },
    /// Replace the effect with `effect.id` on `clip`: params, keyframes,
    /// enabled.
    SetEffect {
        clip: ClipId,
        effect: FxInstance,
    },
    RemoveEffect {
        clip: ClipId,
        effect: EffectId,
    },
    MoveEffect {
        clip: ClipId,
        effect: EffectId,
        index: usize,
    },
    /// Key a parameter of a clip effect, of a generator clip's program
    /// (`effect` = the generator instance's id), or of a transition on the
    /// clip (`effect` = the transition's instance id). `at` is timeline
    /// time, like [`Command::SetKeyframe`].
    SetEffectKeyframe {
        clip: ClipId,
        effect: EffectId,
        param: String,
        at: Time,
        value: ParamValue,
        #[serde(default)]
        ease: Ease,
    },
    RemoveEffectKeyframe {
        clip: ClipId,
        effect: EffectId,
        param: String,
        at: Time,
    },
    /// Transition at a clip's start (from the clip before it, or from
    /// nothing) or end; `None` removes it. See [`crate::fx::transitions`].
    SetTransition {
        clip: ClipId,
        edge: Edge,
        transition: Option<Transition>,
    },
    /// A shader or HTML generator clip (backgrounds, titles, motion
    /// graphics) on a video, overlay or title track.
    AddGenerator {
        track: TrackId,
        range: TimeRange,
        fx: FxInstance,
    },
    SetGenerator {
        clip: ClipId,
        fx: FxInstance,
    },
    /// Create or replace a project FX program, matched by id.
    SetFxAsset {
        asset: FxAsset,
    },
    /// Fails while a clip still uses it.
    RemoveFxAsset {
        asset: FxId,
    },

    /// Replace a clip's audio effect stack.
    SetAudioEffects {
        clip: ClipId,
        effects: Vec<AudioFx>,
    },

    // ---- media pool (user only)
    ImportMedia {
        media: MediaItem,
    },
    UpdateMedia {
        media: MediaItem,
    },
    RemoveMedia {
        media: MediaId,
    },
    /// Replace the bin list (create, rename, reparent).
    SetBins {
        bins: Vec<Bin>,
    },
}

impl Command {
    pub fn category(&self) -> ToolCategory {
        use Command::*;
        match self {
            Blade { .. }
            | RippleDelete { .. }
            | DeleteClips { .. }
            | InsertClip { .. }
            | MoveClip { .. }
            | Trim { .. }
            | Roll { .. }
            | Slip { .. }
            | Slide { .. }
            | AddMarker { .. }
            | EditMarker { .. }
            | RemoveMarker { .. }
            | AddTrack { .. }
            | RemoveTrack { .. }
            | SetTrackLocked { .. }
            | PlaceClips { .. }
            | SetSpeed { .. }
            | SetClipsEnabled { .. }
            | RenameClip { .. }
            | SetClipColor { .. } => ToolCategory::EditTimeline,
            SetTransform { .. } | AddZoom { .. } | SetKeyframe { .. } | RemoveKeyframe { .. } => {
                ToolCategory::Transform
            }
            SetCaptionStyle { .. } | AddCaption { .. } | EditCaption { .. } | ApplyTitle { .. } => {
                ToolCategory::Captions
            }
            SetGrade { .. } | ApplyLut { .. } | CopyGrade { .. } | SetTimelineGrade { .. } => {
                ToolCategory::Color
            }
            SetGain { .. }
            | AddFade { .. }
            | SetTrackMix { .. }
            | SetDucking { .. }
            | SetPlaylist { .. }
            | SetAudioEffects { .. } => ToolCategory::Audio,
            AddOverlay { .. } => ToolCategory::Overlays,
            AddEffect { .. }
            | SetEffect { .. }
            | RemoveEffect { .. }
            | MoveEffect { .. }
            | SetEffectKeyframe { .. }
            | RemoveEffectKeyframe { .. }
            | SetTransition { .. }
            | AddGenerator { .. }
            | SetGenerator { .. }
            | SetFxAsset { .. }
            | RemoveFxAsset { .. } => ToolCategory::Effects,
            ImportMedia { .. } | UpdateMedia { .. } | RemoveMedia { .. } | SetBins { .. } => {
                ToolCategory::Media
            }
        }
    }

    /// Tool / undo-menu name, e.g. `ripple_delete`.
    pub fn name(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.get("command").and_then(|c| c.as_str()).map(str::to_owned))
            .unwrap_or_default()
    }

    /// Turn the command into ops against `project`, without changing it.
    pub fn plan(&self, project: &Project) -> Result<Plan> {
        crate::edit::plan(self, project)
    }
}

/// What a command would do, plus anything the user should know first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub ops: Vec<Op>,
    pub warnings: Vec<Warning>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "warning", rename_all = "snake_case")]
pub enum Warning {
    /// The edit removes or alters part of a protected range (PRD F4).
    TouchesProtected { marker: MarkerId, note: String },
    /// Removing media needs a confirmation dialog (hard limit).
    NeedsConfirmation { message: String },
}
