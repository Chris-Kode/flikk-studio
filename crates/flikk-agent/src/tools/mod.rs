//! Serving tool calls against the bus, on the UI thread.
//!
//! Reads go through `bus.view_for(author)` so the agent sees its own pending
//! proposals. Edits go through `bus.execute` as `Author::Agent`, which puts
//! them in the turn's change set. Anything slow (collages, frames, loudness)
//! runs as a [`Job`] off-thread on a project snapshot.

mod analysis;
mod approval;
mod audio;
pub mod caption_styles;
pub(crate) mod catalog;
mod depth;
mod edit;
mod files;
mod fx;
pub mod generate;
pub(crate) mod groups;
pub mod image;
mod layout;
mod masks;
mod music;
mod pool;
mod preview;
mod read;
pub mod respeak;
mod skills;
mod sounds;
mod speed;
mod stabilize;
mod sync;
pub mod upscale;
pub mod video;
mod view;
pub mod voices;
pub(crate) mod web;

pub(crate) use catalog::{ToolSpec, catalog};

use crate::args::{Args, ToolError};
use crate::bridge::ToolResult;
use flikk_core::agent::{AgentEvent, AgentSettings, EditorContext};
use flikk_core::bus::{Author, CommandBus, Outcome as BusOutcome, Permission, Since};
use flikk_core::changeset::{ChangeSetState, ItemState};
use flikk_core::command::{Command, ToolCategory, Warning};
use flikk_core::memory::{CaptionStyle, SourceFact, StyleRule};
use flikk_core::model::Project;
use flikk_core::services::Services;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Stops what a turn's jobs are doing when the turn is cancelled: every
/// long wait they started through it. Clones share it.
#[derive(Clone, Default)]
pub(crate) struct Stop(Arc<Mutex<Stops>>);

#[derive(Default)]
struct Stops {
    stopped: bool,
    flags: Vec<Arc<AtomicBool>>,
}

impl Stop {
    fn stops(&self) -> std::sync::MutexGuard<'_, Stops> {
        flikk_os::sync::lock(&self.0)
    }

    /// A flag set when they are stopped, for waits that take one.
    pub(crate) fn flag(&self) -> Arc<AtomicBool> {
        let mut stops = self.stops();
        let flag = Arc::new(AtomicBool::new(stops.stopped));
        stops.flags.push(flag.clone());
        flag
    }

    /// Stop them all, and any started from now on.
    pub(crate) fn stop(&self) {
        let mut stops = self.stops();
        stops.stopped = true;
        for flag in &stops.flags {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

/// Long-term memory the host was given (the store owns it).
#[derive(Clone, Debug, Default)]
pub(crate) struct Memory {
    pub rules: Vec<StyleRule>,
    pub facts: Vec<SourceFact>,
    /// The caption styles of the selected style profile, as saved.
    pub caption_styles: Vec<CaptionStyle>,
}

/// Everything a tool may use while it runs on the UI thread.
pub(crate) struct ToolCtx<'a> {
    pub bus: &'a mut CommandBus,
    pub services: &'a Services,
    pub author: Author,
    /// An external MCP client rather than this editor's own turn.
    pub external: bool,
    pub editor: &'a EditorContext,
    pub settings: &'a AgentSettings,
    pub memory: &'a Memory,
    /// Stops the jobs the calls start, with their turn.
    pub stop: &'a Stop,
}

impl ToolCtx<'_> {
    pub(crate) fn project(&self) -> &Project {
        self.bus.view_for(&self.author)
    }

    /// Where [`Self::project`] is at: a job working on a snapshot of it
    /// hands this back with its commands ([`JobDone::Propose`]).
    pub(crate) fn version(&self) -> Since {
        self.bus.view_since(&self.author)
    }
}

/// A request that needs the user's approval (memory, deliver). The UI
/// approves or denies it through `AgentLink`; this crate does not perform it.
#[derive(Clone, Debug, PartialEq)]
pub struct ApprovalRequest {
    pub tool: String,
    pub category: ToolCategory,
    pub args: Value,
    /// One line for the approval dialog.
    pub summary: String,
    /// The user allowed this category: the UI carries it out without
    /// asking (caption styles, which only the UI can save).
    pub allowed: bool,
}

/// The result of a finished tool call plus what the activity log shows.
#[derive(Debug, Default)]
pub(crate) struct Done {
    pub result: ToolResult,
    pub summary: String,
    pub images: Vec<PathBuf>,
    pub excerpts: Vec<String>,
    pub events: Vec<AgentEvent>,
    pub approval: Option<ApprovalRequest>,
    /// What the activity log records as the call's arguments, when not the
    /// arguments as sent (a save records what it wrote).
    pub recorded: Option<Box<Value>>,
}

impl Done {
    pub(crate) fn text(summary: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            result: ToolResult::text(text),
            summary: summary.into(),
            ..Default::default()
        }
    }

    pub(crate) fn json(summary: impl Into<String>, value: &Value) -> Self {
        Self {
            result: ToolResult::json(value),
            summary: summary.into(),
            ..Default::default()
        }
    }

    pub(crate) fn error(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            result: ToolResult::error(message.clone()),
            summary: message,
            ..Default::default()
        }
    }
}

/// Remakes a job's commands on the project as it is when the job is done
/// (see [`JobDone::Propose`]); `None` when what they change was edited
/// since the snapshot.
pub(crate) type Rebase = Box<dyn FnOnce(&Project) -> Option<Vec<Command>> + Send>;

/// What a job hands back to the UI thread.
pub(crate) enum JobDone {
    Done(Done),
    /// Commands to propose on the UI thread (e.g. `normalize` after measuring).
    Propose {
        commands: Vec<Command>,
        reason: String,
        excerpts: Vec<String>,
        /// [`ToolCtx::version`] when the job took its snapshot. The commands
        /// set whole values worked out on it, so they conflict with the
        /// user's edits, and the agent's other proposals, since to what they
        /// change instead of undoing them.
        since: Since,
        /// For a change small enough to make again on the project as it is
        /// now (one mask of a clip's list), keeping what changed meanwhile;
        /// proposed then as a fresh change. If it gives `None`, `commands`
        /// are proposed from `since`.
        rebase: Option<Rebase>,
    },
    /// A picture from the web to add to the pool on the UI thread, with
    /// what the call returns.
    Fetched {
        item: Box<flikk_core::model::MediaItem>,
        done: Box<Done>,
        /// The pool bin it goes to.
        bin: &'static str,
    },
}

pub(crate) type Job = Box<dyn FnOnce() -> JobDone + Send>;

pub(crate) enum Step {
    Done(Box<Done>),
    Job(Job),
}

impl From<Done> for Step {
    fn from(d: Done) -> Self {
        Step::Done(Box::new(d))
    }
}

/// Tools the author may see: categories that are not `Off`. With `groups` (the editor's own turns), only
/// the core tools and those groups, plus `load_tools` and `run_tool` for the
/// rest; without (external clients), every tool. `view_web_image` only
/// with `web`, `generate_image` only when [`image::offered`].
pub(crate) fn list(
    bus: &CommandBus,
    groups: Option<&BTreeSet<ToolCategory>>,
    settings: &AgentSettings,
    services: &Services,
) -> Vec<Value> {
    let images = image::offered(settings, services);
    catalog()
        .iter()
        .filter(|t| t.category.is_none_or(|c| visible(bus, c)))
        .filter(|t| settings.web || t.name != web::TOOL)
        .filter(|t| t.name != image::TOOL || images)
        .filter(|t| match groups {
            Some(g) => t.name == groups::RUN_TOOL || groups::listed(t, g),
            // Attached files are only for the editor's own chat.
            None => {
                t.name != groups::LOAD_TOOLS
                    && t.name != groups::RUN_TOOL
                    && !files::TOOLS.contains(&t.name)
            }
        })
        .map(ToolSpec::to_mcp)
        .collect()
}

pub(crate) fn visible(bus: &CommandBus, category: ToolCategory) -> bool {
    bus.check(&Author::Agent, category).is_ok() && category != ToolCategory::Media
}

/// Run one tool call.
pub(crate) fn serve(ctx: &mut ToolCtx, tool: &str, args: &Value) -> Step {
    match serve_inner(ctx, tool, args) {
        Ok(step) => step,
        Err(ToolError(message)) => Done::error(message).into(),
    }
}

fn serve_inner(ctx: &mut ToolCtx, tool: &str, args: &Value) -> Result<Step, ToolError> {
    let Some(spec) = catalog().into_iter().find(|t| t.name == tool) else {
        return Err(ToolError(format!("unknown tool `{tool}`")));
    };
    let empty = serde_json::Map::new();
    let rate = ctx.project().timeline.settings.rate;
    let args = if args.is_null() {
        Args(&empty, rate)
    } else {
        Args::of(args, rate)?
    };
    let Some(category) = spec.category else {
        if tool != "ask_user" {
            // The host serves these for its own turns.
            return Err(ToolError(if ctx.external {
                "Every tool is already in your list; call it directly.".into()
            } else {
                format!("`{tool}` needs a tool name")
            }));
        }
        if ctx.external {
            return Err(ToolError(
                "ask_user is for the editor's own chat; ask in your own conversation instead."
                    .into(),
            ));
        }
        return approval::ask_user(&args).map(Into::into);
    };
    let permission = ctx.bus.check(&ctx.author, category).map_err(|_| {
        ToolError(format!(
            "The user turned off \"{}\" for the agent.",
            category.label()
        ))
    })?;
    let ask = match category {
        ToolCategory::Skills => skills::asks(tool, permission),
        _ => permission == Permission::NeedsApproval,
    };
    // Caption styles are saved by the UI, so even allowed ones go there
    // as a request it carries out at once.
    if ask || category == ToolCategory::CaptionStyles {
        return approval::request(ctx, tool, category, &args, !ask).map(Into::into);
    }
    match internal(tool, &args)? {
        "get_timeline" => read::get_timeline(ctx, &args),
        "get_selection" => read::get_selection(ctx),
        "list_media" => read::list_media(ctx, &args),
        "get_clip" => read::get_clip(ctx, &args),
        "get_markers" => read::get_markers(ctx, &args),
        "list_fx" => read::list_fx(ctx, &args),
        "get_fx" => read::get_fx(ctx, &args),
        "list_caption_styles" => caption_styles::list(ctx).map(Into::into),

        "search_transcript" => analysis::search_transcript(ctx, &args),
        "get_transcript" => analysis::get_transcript(ctx, &args),
        "get_speakers" => analysis::get_speakers(ctx, &args),
        "find_silences" => analysis::find_silences(ctx, &args),
        "find_shot_changes" => analysis::find_shot_changes(ctx, &args),
        "get_audio" => audio::get_audio(ctx, &args),
        "get_music" => audio::get_music(ctx, &args),
        "search_sounds" => audio::search_sounds(ctx, &args),

        "view_range" => view::view_range(ctx, &args, false),
        "view_detail" => view::view_range(ctx, &args, true),
        "view_frame" => view::view_frame(ctx, &args),
        "view_media" => pool::view_media(ctx, &args),
        "get_layout" => layout::get_layout(ctx, &args),
        "view_audio" => audio::view_audio(ctx, &args),
        "get_scopes" => preview::get_scopes(ctx, &args),
        "preview_fx" => preview::preview_fx(ctx, &args),
        video::PREVIEW => video::preview(ctx, &args),
        "measure_loudness" => view::measure_loudness(ctx, &args),
        web::TOOL => web::serve(ctx, &args),

        "match_grade" => preview::match_grade(ctx, &args),
        "create_fx" | "update_fx" => fx::author(ctx, tool, &args),
        "use_library_fx" => fx::use_library(ctx, &args),
        "find_sounds" => sounds::find(ctx, &args),
        "use_library_sound" => sounds::use_sound(ctx, &args),
        "find_voices" => voices::find_voices(ctx, &args),
        "stabilize" => stabilize::stabilize(ctx, &args),
        "get_shake" => stabilize::get_shake(ctx, &args),
        "fit_music" => music::fit_music(ctx, &args),
        "sync_audio" => sync::serve(ctx, &args),
        tool if files::TOOLS.contains(&tool) => files::serve(ctx, tool, &args),
        tool if skills::TOOLS.contains(&tool) => skills::serve(ctx, tool, &args).map(Into::into),
        tool if fx::TOOLS.contains(&tool) => fx::serve(ctx, tool, &args),
        tool if depth::TOOLS.contains(&tool) => depth::serve(ctx, tool, &args),
        tool => edit::serve(ctx, tool, &args),
    }
}

/// The handler behind a call: listed tools fold their siblings in behind
/// flags (`set_mask` with `remove` is `remove_mask`), so fewer schemas are
/// sent while the handlers stay one per operation.
fn internal<'a>(tool: &'a str, args: &Args) -> Result<&'a str, ToolError> {
    let flag = |key| args.opt_bool(key).map(|b| b == Some(true));
    Ok(match tool {
        "view_range" if flag("detail")? => "view_detail",
        "set_keyframe" if flag("remove")? => "remove_keyframe",
        "set_mask" if flag("remove")? => "remove_mask",
        "set_effect_keyframe" if flag("remove")? => "remove_effect_keyframe",
        "set_transition" if flag("remove")? => "remove_transition",
        "set_primary" if !args.has("clips") => "set_timeline_grade",
        "set_effect" if flag("remove")? => "remove_effect",
        "set_effect" if args.has("index") => {
            if ["params", "reset_params", "enabled"]
                .iter()
                .any(|k| args.has(k))
            {
                return Err(ToolError(
                    "Move an effect (`index`) in a call of its own, without other changes.".into(),
                ));
            }
            "move_effect"
        }
        _ => tool,
    })
}

/// Change sets of external clients are titled so the user knows where they
/// came from.
pub(crate) const EXTERNAL_PREFIX: &str = "External agent: ";

/// Propose `commands` in order as one tool call's worth of change items.
pub(crate) fn propose(ctx: &mut ToolCtx, commands: Vec<Command>, reason: &str) -> Done {
    propose_from(ctx, commands, reason, None)
}

/// Propose what a job worked out on a snapshot taken at `since`
/// ([`JobDone::Propose`]): remade on the project as it is now if `rebase`
/// can, else as planned then, so the edits and proposals made since to
/// what it changes conflict with it rather than being overwritten.
pub(crate) fn propose_job(
    ctx: &mut ToolCtx,
    commands: Vec<Command>,
    reason: &str,
    since: Since,
    rebase: Option<Rebase>,
) -> Done {
    match rebase.and_then(|r| r(ctx.project())) {
        Some(commands) => propose_from(ctx, commands, reason, None),
        None => propose_from(ctx, commands, reason, Some(since)),
    }
}

fn propose_from(
    ctx: &mut ToolCtx,
    commands: Vec<Command>,
    reason: &str,
    since: Option<Since>,
) -> Done {
    let opens_set = !ctx
        .bus
        .change_sets()
        .iter()
        .any(|s| s.author == ctx.author && s.state == ChangeSetState::Open);
    let reason = if ctx.external && opens_set {
        format!("{EXTERNAL_PREFIX}{reason}")
    } else {
        reason.to_owned()
    };
    let reason = reason.as_str();
    let total = commands.len();
    let names: Vec<String> = commands.iter().map(Command::name).collect();
    let author = ctx.author.clone();
    let outcomes = match since {
        Some(v) => ctx.bus.execute_since(commands, author, reason, v),
        None => {
            let mut outcomes = Vec::new();
            for command in commands {
                let outcome = ctx.bus.execute(command, author.clone(), reason);
                let failed = outcome.is_err();
                outcomes.push(outcome);
                if failed {
                    break;
                }
            }
            outcomes
        }
    };
    let last_name = names
        .get(outcomes.len().saturating_sub(1))
        .cloned()
        .unwrap_or_default();
    let mut proposed = 0;
    let mut overwritten = 0;
    let mut warnings = Vec::new();
    let mut failure = None;
    for outcome in outcomes {
        match outcome {
            Ok(BusOutcome::Proposed {
                change_set,
                item,
                warnings: w,
            }) => {
                let conflicted = ctx
                    .bus
                    .change_set(change_set)
                    .and_then(|s| s.item(item))
                    .is_some_and(|i| i.state == ItemState::Conflicted);
                if conflicted {
                    overwritten += 1;
                } else {
                    proposed += 1;
                    warnings.extend(w);
                }
            }
            Ok(BusOutcome::Applied { warnings: w }) => {
                proposed += 1;
                warnings.extend(w);
            }
            Err(e) => failure = Some(e.to_string()),
        }
    }
    let mut text = match (proposed, &failure) {
        (0, None) if overwritten > 0 => "Nothing proposed.".to_owned(),
        (0, Some(e)) => format!("Nothing proposed: {e}"),
        (n, Some(e)) => format!(
            "Proposed {n} of {total} changes; then failed: {e}. The first {n} stay in the change set."
        ),
        (1, None) => "Proposed 1 change in this turn's change set.".to_owned(),
        (n, None) => format!("Proposed {n} changes in this turn's change set."),
    };
    if overwritten > 0 {
        text.push_str(&format!(
            "\n{overwritten} not proposed: another change to the same clip was proposed while this was worked out, and it would undo that one. Look again and ask again if still wanted."
        ));
    }
    for w in &warnings {
        match w {
            Warning::TouchesProtected { note, marker } => text.push_str(&format!(
                "\nWarning: touches protected range {marker} ({note}). Remove it unless the user asked for it."
            )),
            Warning::NeedsConfirmation { message } => {
                text.push_str(&format!("\nNeeds confirmation: {message}"))
            }
        }
    }
    let summary = if total == 1 {
        format!("{last_name}: {reason}")
    } else {
        format!("{proposed} × {last_name}: {reason}")
    };
    Done {
        result: if proposed == 0 && (failure.is_some() || overwritten > 0) {
            ToolResult::error(text)
        } else {
            ToolResult::text(text)
        },
        summary,
        ..Default::default()
    }
}
