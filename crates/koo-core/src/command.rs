//! The command vocabulary shared by the UI, the agent's MCP tools and recipes
//! (PRD "Agent tool API"). Read and view tools are not commands: they do not
//! change the project and are served by the agent crate from a snapshot.

use crate::error::Result;
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
}

impl ToolCategory {
    pub const ALL: [ToolCategory; 12] = [
        Self::ReadProject,
        Self::ReadAnalysis,
        Self::View,
        Self::EditTimeline,
        Self::Transform,
        Self::Captions,
        Self::Color,
        Self::Audio,
        Self::Overlays,
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
            | SetTrackLocked { .. } => ToolCategory::EditTimeline,
            SetTransform { .. } | AddZoom { .. } | SetKeyframe { .. } | RemoveKeyframe { .. } => {
                ToolCategory::Transform
            }
            SetCaptionStyle { .. } | AddCaption { .. } | EditCaption { .. } | ApplyTitle { .. } => {
                ToolCategory::Captions
            }
            SetGrade { .. } | ApplyLut { .. } | CopyGrade { .. } => ToolCategory::Color,
            SetGain { .. }
            | AddFade { .. }
            | SetTrackMix { .. }
            | SetDucking { .. }
            | SetPlaylist { .. } => ToolCategory::Audio,
            AddOverlay { .. } => ToolCategory::Overlays,
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
