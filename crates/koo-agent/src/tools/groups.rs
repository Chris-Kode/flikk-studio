//! Tool groups, so a turn lists only the tools it is likely to need.
//!
//! Every turn lists the [`CORE`] tools plus the groups picked for it
//! ([`pick`]: the workspace, the selection and words in the message) and
//! those the chat already used. `load_tools` hands the agent another group's
//! schemas mid-turn and `run_tool` calls them, so the list stays the same
//! for the whole turn (the backends cache the prompt behind it). External
//! clients still see every tool.
//!
//! Groups follow the permission categories, except that a few read tools
//! sit with the edits they serve (`get_scopes` with color, `preview_fx`
//! with effects). Permissions still go by category.

use super::catalog::{ToolSpec, catalog};
use super::{Done, visible};
use koo_core::agent::{AgentSettings, EditorContext, Workspace};
use koo_core::bus::{AgentRole, CommandBus};
use koo_core::command::ToolCategory;
use koo_core::model::{ClipContent, Project, TrackKind};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::Write;

pub const LOAD_TOOLS: &str = "load_tools";
pub const RUN_TOOL: &str = "run_tool";

/// Listed on every turn: reading, looking, listening and cutting.
pub const CORE: [&str; 20] = [
    "get_timeline",
    "get_selection",
    "list_media",
    "get_clip",
    "get_markers",
    "search_transcript",
    "get_transcript",
    "get_speakers",
    "find_silences",
    "find_shot_changes",
    "get_audio",
    "view_range",
    "view_frame",
    "blade",
    "ripple_delete",
    "cut_words",
    "trim",
    "move_clip",
    "ask_user",
    LOAD_TOOLS,
];

/// The groups `load_tools` takes, in the order it lists them.
pub const GROUPS: [ToolCategory; 9] = [
    ToolCategory::EditTimeline,
    ToolCategory::Transform,
    ToolCategory::Captions,
    ToolCategory::Color,
    ToolCategory::Audio,
    ToolCategory::Effects,
    ToolCategory::Overlays,
    ToolCategory::ProjectMemory,
    ToolCategory::Deliver,
];

pub fn key(group: ToolCategory) -> &'static str {
    match group {
        ToolCategory::EditTimeline => "edit",
        ToolCategory::Transform => "transform",
        ToolCategory::Captions => "captions",
        ToolCategory::Color => "color",
        ToolCategory::Audio => "audio",
        ToolCategory::Effects => "effects",
        ToolCategory::Overlays => "overlays",
        ToolCategory::ProjectMemory => "memory",
        ToolCategory::Deliver => "deliver",
        ToolCategory::ReadProject => "read_project",
        ToolCategory::ReadAnalysis => "analysis",
        ToolCategory::View => "view",
        ToolCategory::Media => "media",
    }
}

fn from_key(text: &str) -> Option<ToolCategory> {
    GROUPS.into_iter().find(|g| key(*g) == text)
}

/// The group a tool is loaded with; `None` for tools without a category.
pub fn group_of(spec: &ToolSpec) -> Option<ToolCategory> {
    Some(match spec.name {
        "list_fx" | "get_fx" | "preview_fx" => ToolCategory::Effects,
        "get_layout" => ToolCategory::Transform,
        "get_scopes" => ToolCategory::Color,
        "view_audio" | "measure_loudness" | "get_music" | "search_sounds" => ToolCategory::Audio,
        _ => spec.category?,
    })
}

/// Whether a turn with `groups` loaded lists `spec`.
pub fn listed(spec: &ToolSpec, groups: &BTreeSet<ToolCategory>) -> bool {
    CORE.contains(&spec.name) || group_of(spec).is_some_and(|g| groups.contains(&g))
}

/// What `load_tools` says each group holds.
pub fn index(tools: &[ToolSpec]) -> String {
    GROUPS
        .iter()
        .map(|g| {
            let names: Vec<_> = tools
                .iter()
                .filter(|t| !CORE.contains(&t.name) && group_of(t) == Some(*g))
                .map(|t| t.name)
                .collect();
            format!("{}: {}", key(*g), names.join(", "))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Words that suggest a group. Matched at the start of a word, so `normaliz`
/// covers "normalize" and "normalise"; a trailing space asks for the whole
/// word.
const KEYWORDS: [(ToolCategory, &[&str]); 9] = [
    (
        ToolCategory::EditTimeline,
        &[
            "insert",
            "b roll",
            "broll",
            "marker",
            "speed",
            "slow",
            "fast",
            "duplicate",
            "copy",
            "rename",
            "label",
            "disable",
            "enable",
            "reorder",
            "swap",
            "add clip",
        ],
    ),
    (
        ToolCategory::Transform,
        &[
            "zoom",
            "scale",
            "position",
            "crop",
            "rotat",
            "mask",
            "place",
            "center",
            "centre",
            "corner",
            "picture in picture",
            "pip ",
            "keyframe",
            "animat",
            "punch in",
            "reframe",
            "vertical",
            "resize",
            "flip",
            "perspective",
            "pin ",
            "bigger",
            "smaller",
            "safe area",
            "layout",
        ],
    ),
    (
        ToolCategory::Captions,
        &[
            "caption",
            "subtitle",
            "title",
            "srt ",
            "lower third",
            "on screen text",
        ],
    ),
    (
        ToolCategory::Color,
        &[
            "grade",
            "grading",
            "color",
            "colour",
            "lut ",
            "luts ",
            "contrast",
            "exposure",
            "saturat",
            "white balance",
            "curve",
            "skin tone",
            "scope",
            "brighter",
            "darker",
        ],
    ),
    (
        ToolCategory::Audio,
        &[
            "audio",
            "music",
            "song",
            "sound",
            "sfx",
            "loud",
            "volume",
            "gain",
            "duck",
            "fade",
            "noise",
            "normaliz",
            "normalis",
            "lufs",
            "mix",
            "reverb",
            "eq ",
            "compress",
            "hum ",
            "playlist",
            "beat",
            "spectrogram",
            "waveform",
        ],
    ),
    (
        ToolCategory::Effects,
        &[
            "effect",
            "transition",
            "shader",
            "glsl",
            "blur",
            "glow",
            "generator",
            "lower third",
            "motion graphic",
            "fx ",
            "dissolve",
            "wipe",
            "crossfade",
            "html",
            "gsap",
            "background",
        ],
    ),
    (
        ToolCategory::Overlays,
        &[
            "overlay",
            "logo",
            "watermark",
            "sticker",
            "emoji",
            "image",
            "png ",
            "gif ",
        ],
    ),
    (
        ToolCategory::ProjectMemory,
        &[
            "rule",
            "recipe",
            "remember",
            "from now on",
            "always",
            "fact",
            "style guide",
            "preference",
        ],
    ),
    (
        ToolCategory::Deliver,
        &[
            "export",
            "render",
            "deliver",
            "otio",
            "still",
            "thumbnail",
            "publish",
        ],
    ),
];

/// Lowercase words separated by single spaces, with a space at each end.
fn words(text: &str) -> String {
    let mut out = String::from(" ");
    for word in text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        out.push_str(word);
        out.push(' ');
    }
    out
}

/// Groups this turn likely needs, from what the user is looking at and
/// what they wrote (the recipe's prompt for a `/recipe`).
pub fn pick(
    text: &str,
    project: &Project,
    ctx: &EditorContext,
    role: AgentRole,
) -> BTreeSet<ToolCategory> {
    let mut out = BTreeSet::new();
    match ctx.workspace {
        Workspace::Edit => {}
        Workspace::Captions => {
            out.insert(ToolCategory::Captions);
        }
        Workspace::Color => {
            out.insert(ToolCategory::Color);
        }
        Workspace::Audio => {
            out.insert(ToolCategory::Audio);
        }
        Workspace::Deliver => {
            out.insert(ToolCategory::Deliver);
        }
    }
    for (track, clip) in ctx
        .selection
        .clips
        .iter()
        .filter_map(|id| project.timeline.clip(*id))
    {
        match track.kind {
            TrackKind::Audio => out.insert(ToolCategory::Audio),
            TrackKind::Title | TrackKind::Caption => out.insert(ToolCategory::Captions),
            TrackKind::Overlay => out.insert(ToolCategory::Overlays),
            TrackKind::Video => false,
        };
        if matches!(clip.content, ClipContent::Generator { .. }) {
            out.insert(ToolCategory::Effects);
        }
    }
    let text = words(text);
    for (group, keywords) in KEYWORDS {
        if keywords.iter().any(|k| text.contains(&format!(" {k}"))) {
            out.insert(group);
        }
    }
    if role == AgentRole::Builder {
        out.extend([ToolCategory::ProjectMemory, ToolCategory::Effects]);
    }
    out
}

/// `run_tool {name, arguments}` → the tool it calls, so the activity log
/// and the permission check see the real one.
pub fn unwrap_run(tool: String, args: Value) -> (String, Value) {
    if tool != RUN_TOOL {
        return (tool, args);
    }
    match args.get("name").and_then(Value::as_str) {
        Some(name) => (
            name.to_owned(),
            args.get("arguments").cloned().unwrap_or(json!({})),
        ),
        None => (tool, args),
    }
}

/// `load_tools`: add groups to the turn and hand back their schemas.
pub fn load(
    bus: &CommandBus,
    role: AgentRole,
    groups: &mut BTreeSet<ToolCategory>,
    args: &Value,
) -> Done {
    let asked: Vec<&str> = match args.get("groups") {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        Some(Value::String(one)) => vec![one.as_str()],
        _ => vec![],
    };
    let mut wanted = Vec::new();
    for text in asked {
        match from_key(text.trim()) {
            Some(g) => wanted.push(g),
            None => {
                let keys: Vec<_> = GROUPS.iter().map(|g| key(*g)).collect();
                return Done::error(format!(
                    "No group `{text}`. The groups are: {}.",
                    keys.join(", ")
                ));
            }
        }
    }
    if wanted.is_empty() {
        return Done::error("Name at least one group in `groups`.");
    }
    let tools = catalog();
    let mut schemas = Vec::new();
    let mut notes = Vec::new();
    for g in wanted {
        if groups.contains(&g) {
            notes.push(format!(
                "`{}` is already listed; call its tools directly.",
                key(g)
            ));
            continue;
        }
        let found: Vec<_> = tools
            .iter()
            .filter(|t| !CORE.contains(&t.name) && group_of(t) == Some(g))
            .filter(|t| t.category.is_none_or(|c| visible(bus, role, c, t.name)))
            .collect();
        if found.is_empty() {
            notes.push(format!(
                "`{}` has no tools you may use (the user turned it off, or it is not for this role).",
                key(g)
            ));
            continue;
        }
        groups.insert(g);
        schemas.extend(found.iter().map(|t| t.to_mcp()));
    }
    let loaded: Vec<_> = schemas
        .iter()
        .filter_map(|s| s.get("name").and_then(Value::as_str))
        .collect();
    let mut text = notes.join("\n");
    if !schemas.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&format!(
            "Loaded {}. Call them through `run_tool` with `name` and `arguments` for the rest of this message; from the next message on they are listed directly.\n\n{}",
            loaded.join(", "),
            serde_json::to_string(&schemas).unwrap_or_default()
        ));
    }
    let summary = format!("loaded {}", loaded.join(", "));
    Done::text(summary, text)
}

/// One line per turn in `agent/tool_usage.jsonl`: which tools were listed,
/// loaded and called, to tune [`CORE`] and [`KEYWORDS`] from real use.
pub struct Usage {
    pub picked: BTreeSet<ToolCategory>,
    pub start: BTreeSet<ToolCategory>,
    pub loaded: BTreeSet<ToolCategory>,
    pub listed: usize,
    pub listed_bytes: usize,
    /// Tool name and whether it went through `run_tool`.
    pub calls: Vec<(String, bool)>,
}

impl Usage {
    pub fn new(picked: BTreeSet<ToolCategory>, start: BTreeSet<ToolCategory>) -> Self {
        Self {
            picked,
            start,
            loaded: BTreeSet::new(),
            listed: 0,
            listed_bytes: 0,
            calls: Vec::new(),
        }
    }

    /// The groups the chat lists from the next turn on: those listed now,
    /// those loaded, and those of tools called through `run_tool`.
    pub fn next_groups(&self, now: &BTreeSet<ToolCategory>) -> BTreeSet<ToolCategory> {
        let tools = catalog();
        let mut out = now.clone();
        for (name, _) in &self.calls {
            if let Some(g) = tools
                .iter()
                .find(|t| t.name == name)
                .and_then(group_of)
                .filter(|g| GROUPS.contains(g))
            {
                out.insert(g);
            }
        }
        out
    }

    pub fn log(&self, settings: &AgentSettings, role: AgentRole, ctx: &EditorContext, at: i64) {
        let keys =
            |set: &BTreeSet<ToolCategory>| -> Vec<&str> { set.iter().map(|g| key(*g)).collect() };
        let line = json!({
            "at": at,
            "backend": settings.backend.label(),
            "role": match role {
                AgentRole::Editor => "editor",
                AgentRole::Builder => "builder",
            },
            "workspace": ctx.workspace.label(),
            "picked": keys(&self.picked),
            "listed_groups": keys(&self.start),
            "loaded": keys(&self.loaded),
            "listed_tools": self.listed,
            "listed_bytes": self.listed_bytes,
            "calls": self
                .calls
                .iter()
                .map(|(name, via_run)| if *via_run { format!("{name} (run_tool)") } else { name.clone() })
                .collect::<Vec<_>>(),
        });
        let path = crate::session::agent_dir().join("tool_usage.jsonl");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}
