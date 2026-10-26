//! Types shared between the agent host and the UI (chat, change set review,
//! settings, activity log). The host itself lives in `koo-agent`.

use crate::bus::Permissions;
use crate::ids::*;
use crate::time::{Time, TimeRange};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// Claude Code, run as `claude -p`.
    #[default]
    Claude,
    /// Codex CLI in non-interactive exec mode.
    Codex,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Claude => "Claude",
            Backend::Codex => "Codex",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VisionSettings {
    /// At most 30 (PRD V2).
    pub frames_per_collage: u32,
    pub tile_width: u32,
}

impl Default for VisionSettings {
    fn default() -> Self {
        Self {
            frames_per_collage: 30,
            tile_width: 320,
        }
    }
}

/// Agent settings (PRD "Agent configuration"). Per user, with per-project
/// overrides for everything except `backend`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentSettings {
    pub backend: Backend,
    pub permissions: Permissions,
    pub instructions: String,
    pub enabled_recipes: Vec<RecipeId>,
    pub vision: VisionSettings,
    /// Language for chat replies and captions; `None` follows the source.
    pub language: Option<String>,
    pub style_profile: Option<RuleId>,
    /// Editor (editing, captions, color, audio) or Builder (effects and
    /// recipes in a sandbox); both run on `backend`.
    #[serde(default)]
    pub role: crate::bus::AgentRole,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Workspace {
    Media,
    #[default]
    Edit,
    Captions,
    Color,
    Audio,
    Deliver,
}

impl Workspace {
    pub const ALL: [Workspace; 6] = [
        Self::Media,
        Self::Edit,
        Self::Captions,
        Self::Color,
        Self::Audio,
        Self::Deliver,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Media => "Media",
            Self::Edit => "Edit",
            Self::Captions => "Captions",
            Self::Color => "Color",
            Self::Audio => "Audio",
            Self::Deliver => "Deliver",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Selection {
    pub clips: Vec<ClipId>,
    /// Selected transcript words, as a source range of one media item.
    pub words: Option<(MediaId, TimeRange)>,
}

/// Automatic context attached to every chat turn (PRD AG2).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EditorContext {
    pub workspace: Workspace,
    pub playhead: Time,
    pub in_out: Option<TimeRange>,
    pub selection: Selection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    User,
    Agent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
    /// Clickable answers for a clarifying question (PRD AG9).
    pub choices: Vec<String>,
    pub change_set: Option<ChangeSetId>,
    /// Unix seconds.
    pub at: i64,
}

/// Streamed from the agent host to the UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    TextDelta {
        text: String,
    },
    Question {
        text: String,
        choices: Vec<String>,
    },
    Progress {
        label: String,
        fraction: Option<f32>,
    },
    Activity {
        entry: ActivityEntry,
    },
    ChangeSetReady {
        change_set: ChangeSetId,
    },
    TurnDone,
    Error {
        message: String,
    },
}

/// One tool call, as shown in the activity log (PRD AG10, V7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub at: i64,
    pub tool: String,
    pub args: serde_json::Value,
    pub summary: String,
    /// Collages and frames the model was shown.
    pub images: Vec<PathBuf>,
    /// Transcript excerpts sent to the model.
    pub excerpts: Vec<String>,
    pub ok: bool,
}
