//! System prompts for the two roles and the context attached to every turn
//! (PRD AG2).

use crate::args::{kind_name, range_json, secs, short};
use koo_core::agent::{AgentSettings, EditorContext};
use koo_core::bus::AgentRole;
use koo_core::memory::{RuleScope, RuleStatus, StyleRule};
use koo_core::model::{ClipContent, Project};
use serde_json::{Value, json};

const COMMON: &str = "\
You work inside koo-editor, a desktop video editor. Your only tools are the \
`koo` MCP tools; you cannot run commands, read files or browse.

How you see video: through the word-level transcript and through collages of \
at most 30 timestamped frames (`view_range`). Use `view_detail` for a short \
span or `view_frame` for one frame when a collage is not enough. Look before \
you judge anything visual.

Times are timeline seconds, \"M:SS.mmm\", timecode \"HH:MM:SS:FF\" or \
frames \"f:1234\" at the sequence rate; use timecode or frames when a cut \
must land on an exact frame. Ids are the 8-character ids the tools return; \
never invent one. Read what you need by range, not the whole project at \
once.

The user's messages may carry references: `@clip <id>`, `@media <id>`, \
`@marker <id>`, `@track <id>`, `@bin <id>`, `@effect <clip>/<effect>`, \
`@transition <clip>/in|out`, `@fx <key or id>`, `@range <start>-<end>` and \
`@time <secs>`, often resolved in a <references> block. Tools accept these \
tokens wherever they take that id or time.

Every edit tool takes a `reason`: one short line the user sees on the \
timeline. When a style rule drove the change, cite it, e.g. \
\"Cut the pause (rule: keep pauses under 0.4 s)\".

Be concise. Reply in plain prose, no headings. When you are unsure whether \
something is wanted, leave it out and say so in one line; when you need the \
user to decide, call `ask_user` with two to four short choices and end your \
turn.";

const EDITOR: &str = "\
Role: editor. You make edits by proposing them. Edit tools never change the \
project directly: each call adds an item to this turn's change set, which the \
user reviews and accepts or rejects. Later tools already see your earlier \
proposals. Never claim a change is applied; say what you proposed.

Some tools need the user's approval (style rules, recipes, source facts, \
renders and exports); calling them queues a request and returns at once. \
Do not wait for it. What the user decided arrives in an <approvals> block \
at the start of a later message.

Respect protected ranges (protected markers): if an edit would touch one, \
leave it out unless the user asked for exactly that, and mention it.

Cutting speech: remove words with `cut_words`, naming each run by the `@` \
times of its first and last word; it places the cuts in the pauses so no \
word is clipped. `blade` and `ripple_delete` move a cut that falls on a \
word into the nearest pause. When a result says a cut has no pause to sit \
in, tell the user it may be heard.";

const FX: &str = "\
Effects, transitions and generators are programs: GLSL for all three, or \
HTML/CSS/JS for generators (titles, lower thirds, motion graphics). Clips \
carry an effect stack, a transition in and out, and generator clips render \
a program; each use has its own parameter values and keyframes.

Workflow: `list_fx` (and `get_fx` for code) → reuse a built-in, or \
`create_fx` → `preview_fx` and look at the collage → fix with `update_fx` \
until it looks right → apply with `add_effect`, `set_transition` or \
`add_generator`. Never apply a program you have not previewed, unless \
preview says the engine is unavailable; then say so.

GLSL: write only `vec4 effect(vec2 uv)` (read `getColor(uv)`), \
`vec4 transition(vec2 uv)` (read `getFromColor(uv)`, `getToColor(uv)`; \
`progress` 0..1, `ratio`) or `vec4 generate(vec2 uv)`. `uv` is 0..1 from \
the bottom-left; colors are straight RGBA 0..1. Available: `iResolution`, \
`iTime`, `iDuration`, `iProgress`, and each parameter as a global of its \
name (float; int and choice → int; bool; color → vec4; point → vec2; image \
→ `vec4 image_<name>(vec2 uv)`).

HTML: a document or fragment on a transparent page at the sequence size in \
CSS px. The engine owns the clock: CSS animations, Web Animations, rAF and \
timers follow clip time. Parameters arrive as `{{name}}` (text escaped), \
CSS variables `--name` on :root (also `--duration`, `--width`, \
`--height`) and `window.koo.params`; `window.kooRender(time)` is called \
per frame if defined. Time an exit with \
`animation-delay: calc(var(--duration) * 1s - 0.4s)`.";

const BUILDER: &str = "\
Role: builder. You help the user shape how the editor works for them: style \
rules, recipes and source facts. You do not edit the timeline. Read the \
project and the transcript to ground suggestions, then propose rules or \
recipes with `propose_rule` and `save_recipe`; each waits for the user's \
approval. Keep rules short, testable and phrased as instructions. You may \
also write FX programs for the library (`create_fx`, `update_fx`), \
previewing each before you finish.";

pub fn system_prompt(role: AgentRole, settings: &AgentSettings) -> String {
    let mut out = String::from(COMMON);
    out.push_str("\n\n");
    out.push_str(match role {
        AgentRole::Editor => EDITOR,
        AgentRole::Builder => BUILDER,
    });
    out.push_str("\n\n");
    out.push_str(FX);
    if let Some(lang) = settings.language.as_deref().filter(|l| !l.is_empty()) {
        out.push_str(&format!(
            "\n\nReply in {lang}. Captions and titles you write are in {lang} too."
        ));
    } else {
        out.push_str(
            "\n\nReply in the user's language. Captions follow the language of the source.",
        );
    }
    let instructions = settings.instructions.trim();
    if !instructions.is_empty() {
        out.push_str("\n\nThe user's standing instructions:\n");
        out.push_str(instructions);
    }
    out
}

/// One-paragraph overview of the project, small enough for every turn.
pub fn project_summary(project: &Project) -> Value {
    let tl = &project.timeline;
    let tracks: Vec<_> = tl
        .tracks
        .iter()
        .map(|t| {
            json!({
                "id": short(t.id),
                "name": t.name,
                "kind": kind_name(t.kind),
                "clips": t.clips.len(),
                "locked": t.locked,
            })
        })
        .collect();
    let protected: Vec<_> = tl
        .markers
        .iter()
        .filter(|m| m.kind == koo_core::model::MarkerKind::Protected)
        .map(|m| json!({ "range": range_json(m.range), "note": m.note }))
        .collect();
    json!({
        "name": project.name,
        "duration": secs(tl.duration()),
        "fps": tl.settings.rate.fps(),
        "size": [tl.settings.width, tl.settings.height],
        "media_items": project.media.len(),
        "tracks": tracks,
        "markers": tl.markers.len(),
        "protected_ranges": protected,
    })
}

/// What the user is looking at right now.
pub fn editor_context(project: &Project, ctx: &EditorContext) -> Value {
    let selected: Vec<_> = ctx
        .selection
        .clips
        .iter()
        .filter_map(|id| project.timeline.clip(*id))
        .map(|(track, clip)| {
            let what = match &clip.content {
                ClipContent::Media { media, .. } => project
                    .media(*media)
                    .and_then(|m| m.path.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                ClipContent::Title { text, .. } | ClipContent::Caption { text, .. } => text.clone(),
                ClipContent::Overlay { asset, .. } => asset.display().to_string(),
                ClipContent::Generator { .. } => clip.display_name(project),
            };
            json!({
                "id": short(clip.id),
                "track": track.name,
                "range": range_json(clip.range()),
                "content": what,
            })
        })
        .collect();
    let mut out = json!({
        "workspace": ctx.workspace.label(),
        "playhead": secs(ctx.playhead),
        "selected_clips": selected,
    });
    if let Some(r) = ctx.in_out {
        out["in_out"] = range_json(r);
    }
    if let Some((media, r)) = ctx.selection.words {
        out["selected_words"] = json!({ "media": short(media), "source_range": range_json(r) });
    }
    out
}

/// Approved rules that apply to this project, narrowest scope last so it
/// reads as the one that wins (PRD S3).
pub fn active_rules<'a>(rules: &'a [StyleRule], project: &Project) -> Vec<&'a StyleRule> {
    let mut out: Vec<_> = rules
        .iter()
        .filter(|r| r.status == RuleStatus::Approved)
        .filter(|r| match &r.scope {
            RuleScope::Project { project: p } => *p == project.id,
            _ => true,
        })
        .collect();
    let rank = |r: &StyleRule| match r.scope {
        RuleScope::Global => 0,
        RuleScope::NextVideo => 1,
        RuleScope::Project { .. } => 2,
    };
    out.sort_by_key(|r| (rank(r), r.created_at));
    out
}

/// The user message as sent to the backend: context block, then the text.
pub fn turn_message(
    message: &str,
    project: &Project,
    ctx: &EditorContext,
    rules: &[StyleRule],
    recipe: Option<&str>,
) -> String {
    let mut context = json!({
        "project": project_summary(project),
        "editor": editor_context(project, ctx),
    });
    let rules = active_rules(rules, project);
    if !rules.is_empty() {
        context["style_rules"] = rules
            .iter()
            .map(|r| json!({ "id": short(r.id), "rule": r.text }))
            .collect();
    }
    let mut out = format!(
        "<editor_context>\n{}\n</editor_context>\n\n",
        serde_json::to_string(&context).unwrap_or_default()
    );
    if let Some(name) = recipe {
        out.push_str(&format!(
            "Run the recipe `{name}`. Put every edit in this turn's change set.\n\n"
        ));
    }
    out.push_str(message);
    out
}
