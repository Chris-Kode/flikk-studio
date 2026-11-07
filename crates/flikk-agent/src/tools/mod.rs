//! Serving tool calls against the bus, on the UI thread.
//!
//! Reads go through `bus.view_for(author)` so the agent sees its own pending
//! proposals. Edits go through `bus.execute` as `Author::Agent`, which puts
//! them in the turn's change set. Anything slow (collages, frames, loudness)
//! runs as a [`Job`] off-thread on a project snapshot.

mod analysis;
mod approval;
mod audio;
pub mod catalog;
mod depth;
mod edit;
mod fx;
pub mod generate;
pub mod groups;
pub mod image;
mod layout;
mod masks;
mod music;
mod preview;
mod read;
pub mod respeak;
mod sounds;
mod speed;
mod stabilize;
mod sync;
pub mod video;
mod view;
pub mod voices;
pub mod web;

pub use catalog::{ToolSpec, catalog};

use crate::args::{Args, ToolError};
use crate::bridge::ToolResult;
use flikk_core::agent::{AgentEvent, AgentSettings, EditorContext};
use flikk_core::bus::{AgentRole, Author, CommandBus, Outcome as BusOutcome, Permission};
use flikk_core::cloud::GenerationTask;
use flikk_core::command::{Command, ToolCategory, Warning};
use flikk_core::memory::{Recipe, SourceFact, StyleRule};
use flikk_core::model::Project;
use flikk_core::services::Services;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Stops what a turn's jobs are doing when the turn is cancelled: every
/// generation and long wait they started through it. Clones share it.
#[derive(Clone, Default)]
pub struct Stop(Arc<Mutex<Stops>>);

#[derive(Default)]
struct Stops {
    stopped: bool,
    tasks: Vec<GenerationTask>,
    flags: Vec<Arc<AtomicBool>>,
}

impl Stop {
    fn stops(&self) -> std::sync::MutexGuard<'_, Stops> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A generation to follow and stop with the rest; stopped already if
    /// they were.
    pub fn task(&self) -> GenerationTask {
        let task = GenerationTask::default();
        let mut stops = self.stops();
        if stops.stopped {
            task.cancel();
        }
        stops.tasks.push(task.clone());
        task
    }

    /// A flag set when they are stopped, for waits that take one.
    pub fn flag(&self) -> Arc<AtomicBool> {
        let mut stops = self.stops();
        let flag = Arc::new(AtomicBool::new(stops.stopped));
        stops.flags.push(flag.clone());
        flag
    }

    /// Stop them all, and any started from now on.
    pub fn stop(&self) {
        let mut stops = self.stops();
        stops.stopped = true;
        for task in &stops.tasks {
            task.cancel();
        }
        for flag in &stops.flags {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

/// Long-term memory the host was given (the store owns it).
#[derive(Clone, Debug, Default)]
pub struct Memory {
    pub rules: Vec<StyleRule>,
    pub facts: Vec<SourceFact>,
    pub recipes: Vec<Recipe>,
}

/// Everything a tool may use while it runs on the UI thread.
pub struct ToolCtx<'a> {
    pub bus: &'a mut CommandBus,
    pub services: &'a Services,
    pub author: Author,
    /// The role the calls act as (external clients act as the agent's
    /// current role under their own author).
    pub role: AgentRole,
    /// An external MCP client rather than this editor's own turn.
    pub external: bool,
    pub editor: &'a EditorContext,
    pub settings: &'a AgentSettings,
    pub memory: &'a Memory,
    /// Stops the jobs the calls start, with their turn.
    pub stop: &'a Stop,
}

impl ToolCtx<'_> {
    pub fn project(&self) -> &Project {
        self.bus.view_for(&self.author)
    }

    pub fn role(&self) -> AgentRole {
        self.role
    }

    /// The bus version [`Self::project`] is at: a job working on a snapshot
    /// of it hands this back with its commands ([`JobDone::Propose`]).
    pub fn version(&self) -> u64 {
        self.bus.view_version(&self.author)
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
}

/// The result of a finished tool call plus what the activity log shows.
#[derive(Debug, Default)]
pub struct Done {
    pub result: ToolResult,
    pub summary: String,
    pub images: Vec<PathBuf>,
    pub excerpts: Vec<String>,
    pub events: Vec<AgentEvent>,
    pub approval: Option<ApprovalRequest>,
}

impl Done {
    pub fn text(summary: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            result: ToolResult::text(text),
            summary: summary.into(),
            ..Default::default()
        }
    }

    pub fn json(summary: impl Into<String>, value: &Value) -> Self {
        Self {
            result: ToolResult::json(value),
            summary: summary.into(),
            ..Default::default()
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
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
pub type Rebase = Box<dyn FnOnce(&Project) -> Option<Vec<Command>> + Send>;

/// What a job hands back to the UI thread.
pub enum JobDone {
    Done(Done),
    /// Commands to propose on the UI thread (e.g. `normalize` after measuring).
    Propose {
        commands: Vec<Command>,
        reason: String,
        excerpts: Vec<String>,
        /// [`ToolCtx::version`] when the job took its snapshot. The commands
        /// set whole values worked out on it, so they conflict with the
        /// user's edits since to what they change instead of undoing them.
        since: u64,
        /// For a change small enough to make again on the project as it is
        /// now (one mask of a clip's list), keeping what changed meanwhile;
        /// proposed then as a fresh change. If it gives `None`, `commands`
        /// are proposed from `since`.
        rebase: Option<Rebase>,
    },
    /// A file a provider made, to add to the pool and place on the UI
    /// thread.
    Generated {
        item: Box<flikk_core::model::MediaItem>,
        placement: Option<generate::Placement>,
        summary: String,
    },
    /// A picture from the web to add to the pool on the UI thread, with
    /// what the call returns.
    Fetched {
        item: Box<flikk_core::model::MediaItem>,
        done: Box<Done>,
    },
    /// Words said again for a fix, to add to the pool and cut in on the UI
    /// thread.
    Respoken {
        item: Box<flikk_core::model::MediaItem>,
        fix: Box<flikk_core::respeak::Fix>,
        summary: String,
    },
}

pub type Job = Box<dyn FnOnce() -> JobDone + Send>;

pub enum Step {
    Done(Box<Done>),
    Job(Job),
}

impl From<Done> for Step {
    fn from(d: Done) -> Self {
        Step::Done(Box::new(d))
    }
}

/// Tools the author may see: categories that are not `Off`, and only the
/// role's own kinds of tools. With `groups` (the editor's own turns), only
/// the core tools and those groups, plus `load_tools` and `run_tool` for the
/// rest; without (external clients), every tool. `view_web_image` only
/// with `web`, `generate_image` only when [`image::offered`].
pub fn list(
    bus: &CommandBus,
    role: AgentRole,
    groups: Option<&BTreeSet<ToolCategory>>,
    settings: &AgentSettings,
    services: &Services,
) -> Vec<Value> {
    let images = image::offered(settings, services);
    catalog()
        .iter()
        .filter(|t| t.category.is_none_or(|c| visible(bus, role, c, t.name)))
        .filter(|t| settings.web || t.name != web::TOOL)
        .filter(|t| t.name != image::TOOL || images)
        .filter(|t| match groups {
            Some(g) => t.name == groups::RUN_TOOL || groups::listed(t, g),
            None => t.name != groups::LOAD_TOOLS && t.name != groups::RUN_TOOL,
        })
        .map(ToolSpec::to_mcp)
        .collect()
}

/// Builders write FX programs too, but don't put them on the timeline.
const BUILDER_TOOLS: [&str; 3] = ["create_fx", "update_fx", "use_library_fx"];

fn role_allows(role: AgentRole, category: ToolCategory, tool: &str) -> bool {
    match role {
        AgentRole::Editor => true,
        AgentRole::Builder => {
            category.is_read()
                || category == ToolCategory::ProjectMemory
                || BUILDER_TOOLS.contains(&tool)
        }
    }
}

pub(crate) fn visible(
    bus: &CommandBus,
    role: AgentRole,
    category: ToolCategory,
    tool: &str,
) -> bool {
    role_allows(role, category, tool)
        && bus.check(&Author::Agent { role }, category).is_ok()
        && category != ToolCategory::Media
}

/// Run one tool call.
pub fn serve(ctx: &mut ToolCtx, tool: &str, args: &Value) -> Step {
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
    if !role_allows(ctx.role(), category, tool) {
        return Err(ToolError(format!(
            "`{tool}` is not available to the {} role",
            match ctx.role() {
                AgentRole::Editor => "editor",
                AgentRole::Builder => "builder",
            }
        )));
    }
    let permission = ctx.bus.check(&ctx.author, category).map_err(|_| {
        ToolError(format!(
            "The user turned off \"{}\" for the agent.",
            category.label()
        ))
    })?;
    if permission == Permission::NeedsApproval {
        return approval::request(ctx, tool, category, &args).map(Into::into);
    }
    match internal(tool, &args)? {
        "get_timeline" => read::get_timeline(ctx, &args),
        "get_selection" => read::get_selection(ctx),
        "list_media" => read::list_media(ctx, &args),
        "get_clip" => read::get_clip(ctx, &args),
        "get_markers" => read::get_markers(ctx, &args),
        "list_fx" => read::list_fx(ctx, &args),
        "get_fx" => read::get_fx(ctx, &args),

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
        "get_layout" => layout::get_layout(ctx, &args),
        "view_audio" => audio::view_audio(ctx, &args),
        "get_scopes" => preview::get_scopes(ctx, &args),
        "preview_fx" => preview::preview_fx(ctx, &args),
        "measure_loudness" => view::measure_loudness(ctx, &args),
        web::TOOL => web::serve(ctx, &args),

        "match_grade" => preview::match_grade(ctx, &args),
        "create_fx" | "update_fx" => fx::author(ctx, tool, &args),
        "use_library_fx" => fx::use_library(ctx, &args),
        "find_sounds" => sounds::find(ctx, &args),
        "use_library_sound" => sounds::use_sound(ctx, &args),
        "generate_audio" => generate::serve(ctx, &args),
        "generate_video" => video::serve(ctx, &args),
        image::TOOL => image::serve(ctx, &args),
        "find_voices" => voices::find_voices(ctx, &args),
        "add_voice" => voices::serve_add(ctx, &args),
        "design_voice" => voices::serve_design(ctx, &args),
        "stabilize" => stabilize::stabilize(ctx, &args),
        "get_shake" => stabilize::get_shake(ctx, &args),
        "fit_music" => music::fit_music(ctx, &args),
        "replace_words" => respeak::serve(ctx, &args),
        "sync_audio" => sync::serve(ctx, &args),
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
pub const EXTERNAL_PREFIX: &str = "External agent: ";

/// Propose `commands` in order as one tool call's worth of change items.
pub fn propose(ctx: &mut ToolCtx, commands: Vec<Command>, reason: &str) -> Done {
    propose_from(ctx, commands, reason, None)
}

/// Propose what a job worked out on a snapshot taken at version `since`
/// ([`JobDone::Propose`]): remade on the project as it is now if `rebase`
/// can, else as planned then, so the edits made since to what it changes
/// conflict with it rather than being overwritten.
pub fn propose_job(
    ctx: &mut ToolCtx,
    commands: Vec<Command>,
    reason: &str,
    since: u64,
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
    since: Option<u64>,
) -> Done {
    let opens_set =
        !ctx.bus.change_sets().iter().any(|s| {
            s.author == ctx.author && s.state == flikk_core::changeset::ChangeSetState::Open
        });
    let reason = if ctx.external && opens_set {
        format!("{EXTERNAL_PREFIX}{reason}")
    } else {
        reason.to_owned()
    };
    let reason = reason.as_str();
    let total = commands.len();
    let mut proposed = 0;
    let mut warnings = Vec::new();
    let mut failure = None;
    let mut last_name = String::new();
    for command in commands {
        last_name = command.name();
        let author = ctx.author.clone();
        let outcome = match since {
            Some(v) => ctx.bus.execute_since(command, author, reason, v),
            None => ctx.bus.execute(command, author, reason),
        };
        match outcome {
            Ok(BusOutcome::Proposed { warnings: w, .. })
            | Ok(BusOutcome::Applied { warnings: w }) => {
                proposed += 1;
                warnings.extend(w);
            }
            Err(e) => {
                failure = Some(e.to_string());
                break;
            }
        }
    }
    let mut text = match (proposed, &failure) {
        (0, Some(e)) => format!("Nothing proposed: {e}"),
        (n, Some(e)) => format!(
            "Proposed {n} of {total} changes; then failed: {e}. The first {n} stay in the change set."
        ),
        (1, None) => "Proposed 1 change in this turn's change set.".to_owned(),
        (n, None) => format!("Proposed {n} changes in this turn's change set."),
    };
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
        result: if proposed == 0 && failure.is_some() {
            ToolResult::error(text)
        } else {
            ToolResult::text(text)
        },
        summary,
        ..Default::default()
    }
}
