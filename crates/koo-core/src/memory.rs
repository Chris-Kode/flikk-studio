//! Long-term agent knowledge: style profile, source facts, recipes
//! (PRD "Automations and memory").

use crate::ids::*;
use crate::model::{Ease, Fingerprint};
use crate::time::TimeRange;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum RuleScope {
    Global,
    NextVideo,
    Project { project: ProjectId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleStatus {
    Proposed,
    Approved,
    Rejected,
}

/// PRD S1. Conflicts resolve to the newest approved rule in the narrowest
/// scope (S3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StyleRule {
    pub id: RuleId,
    pub text: String,
    pub scope: RuleScope,
    pub status: RuleStatus,
    pub supersedes: Option<RuleId>,
    /// Unix seconds.
    pub created_at: i64,
    /// Why it was proposed: change set ids, quotes from feedback.
    pub evidence: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionPosition {
    #[default]
    Bottom,
    Middle,
    Top,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptionStyle {
    pub name: String,
    pub font: String,
    pub size: f32,
    pub uppercase: bool,
    pub outline: f32,
    pub shadow: f32,
    pub position: CaptionPosition,
    pub max_chars_per_line: u32,
    pub max_lines: u32,
}

impl Default for CaptionStyle {
    fn default() -> Self {
        Self {
            name: "Default".into(),
            font: "Inter".into(),
            size: 48.0,
            uppercase: false,
            outline: 2.0,
            shadow: 0.0,
            position: CaptionPosition::Bottom,
            max_chars_per_line: 32,
            max_lines: 2,
        }
    }
}

/// Presets that are part of the profile (PRD S4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Presets {
    pub caption_styles: Vec<CaptionStyle>,
    pub look: Option<std::path::PathBuf>,
    pub ducking_db: f32,
    pub loudness_lufs: f32,
    pub true_peak_db: f32,
    pub zoom_scale: f32,
    pub zoom_ease: Ease,
}

impl Default for Presets {
    fn default() -> Self {
        Self {
            caption_styles: vec![CaptionStyle::default()],
            look: None,
            ducking_db: -12.0,
            loudness_lufs: -14.0,
            true_peak_db: -1.0,
            zoom_scale: 1.8,
            zoom_ease: Ease::EaseInOut,
        }
    }
}

/// One profile per client or channel (PRD S6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StyleProfile {
    pub id: RuleId,
    pub name: String,
    pub rules: Vec<StyleRule>,
    pub presets: Presets,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactKind {
    SpeakerReference,
    OriginalAudioTrack,
    LiteralQuote,
    /// Read from a collage: names, scores, on-screen text (PRD F5).
    OnScreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactStatus {
    Proposed,
    Confirmed,
    /// The file behind the fingerprint was replaced (PRD F2).
    Suspended,
}

/// PRD F1: bound to the source fingerprint, never to a path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceFact {
    pub id: FactId,
    pub fingerprint: Fingerprint,
    pub kind: FactKind,
    pub text: String,
    /// Source-time range the fact is about, if any.
    pub range: Option<TimeRange>,
    pub status: FactStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecipeParam {
    pub name: String,
    pub description: String,
    pub default: Option<String>,
}

/// A saved multi-step agent task; each run produces one change set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub id: RecipeId,
    /// Slash name in chat, e.g. `rough-cut`.
    pub name: String,
    pub description: String,
    pub params: Vec<RecipeParam>,
    /// Prompt template; `{param}` placeholders are filled at run time.
    pub prompt: String,
    pub builtin: bool,
}
