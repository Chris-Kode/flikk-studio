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

    /// Models offered in the chat, as (id passed to the CLI, label). Claude
    /// takes aliases that follow the latest version of each model.
    pub fn models(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Backend::Claude => &[
                ("fable", "Fable"),
                ("opus", "Opus"),
                ("sonnet", "Sonnet"),
                ("haiku", "Haiku"),
            ],
            Backend::Codex => &[
                ("gpt-5.5", "GPT-5.5"),
                ("gpt-5.4", "GPT-5.4"),
                ("gpt-5.4-mini", "GPT-5.4 mini"),
            ],
        }
    }

    /// Thinking levels, as (value passed to the CLI, label), lowest first:
    /// Claude's `--effort`, Codex's `model_reasoning_effort`.
    pub fn efforts(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Backend::Claude => &[
                ("low", "Low"),
                ("medium", "Medium"),
                ("high", "High"),
                ("xhigh", "Extra high"),
                ("max", "Max"),
            ],
            Backend::Codex => &[
                ("minimal", "Minimal"),
                ("low", "Low"),
                ("medium", "Medium"),
                ("high", "High"),
                ("xhigh", "Extra high"),
            ],
        }
    }
}

/// Model and thinking level for one backend. `None` leaves the CLI's own
/// default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelChoice {
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// A [`ModelChoice`] per backend, so switching backends keeps each pick.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelChoices {
    pub claude: ModelChoice,
    pub codex: ModelChoice,
}

impl ModelChoices {
    pub fn get(&self, backend: Backend) -> &ModelChoice {
        match backend {
            Backend::Claude => &self.claude,
            Backend::Codex => &self.codex,
        }
    }

    pub fn get_mut(&mut self, backend: Backend) -> &mut ModelChoice {
        match backend {
            Backend::Claude => &mut self.claude,
            Backend::Codex => &mut self.codex,
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
/// overrides for everything except `backend` and `models`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentSettings {
    pub backend: Backend,
    /// Model and thinking level, picked in the chat composer.
    #[serde(default)]
    pub models: ModelChoices,
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
    #[default]
    Edit,
    Captions,
    Color,
    Audio,
    Deliver,
}

impl Workspace {
    pub const ALL: [Workspace; 5] = [
        Self::Edit,
        Self::Captions,
        Self::Color,
        Self::Audio,
        Self::Deliver,
    ];

    pub fn label(self) -> &'static str {
        match self {
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
    /// Tool calls made in a row, shown stacked in the thread.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ActivityEntry>,
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

/// One tool call, as shown in the chat thread (PRD AG10, V7).
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
