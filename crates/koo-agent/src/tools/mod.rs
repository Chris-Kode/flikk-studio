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
mod edit;
mod fx;
mod layout;
mod preview;
mod read;
mod view;

pub use catalog::{ToolSpec, catalog};

use crate::args::{Args, ToolError};
use crate::bridge::ToolResult;
use koo_core::agent::{AgentEvent, AgentSettings, EditorContext};
use koo_core::bus::{AgentRole, Author, CommandBus, Outcome as BusOutcome, Permission};
use koo_core::command::{Command, ToolCategory, Warning};
use koo_core::memory::{Recipe, SourceFact, StyleRule};
use koo_core::model::Project;
use koo_core::services::Services;
use serde_json::Value;
use std::path::PathBuf;

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
}

impl ToolCtx<'_> {
    pub fn project(&self) -> &Project {
        self.bus.view_for(&self.author)
    }

    pub fn role(&self) -> AgentRole {
        self.role
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

/// What a job hands back to the UI thread.
pub enum JobDone {
    Done(Done),
    /// Commands to propose on the UI thread (e.g. `normalize` after measuring).
    Propose {
        commands: Vec<Command>,
        reason: String,
        excerpts: Vec<String>,
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
/// role's own kinds of tools.
pub fn list(bus: &CommandBus, role: AgentRole) -> Vec<Value> {
    catalog()
        .iter()
        .filter(|t| t.category.is_none_or(|c| visible(bus, role, c, t.name)))
        .map(ToolSpec::to_mcp)
        .collect()
}

/// Builders write FX programs too, but don't put them on the timeline.
const BUILDER_TOOLS: [&str; 2] = ["create_fx", "update_fx"];

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

fn visible(bus: &CommandBus, role: AgentRole, category: ToolCategory, tool: &str) -> bool {
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
    match tool {
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

        "match_grade" => preview::match_grade(ctx, &args),
        "create_fx" | "update_fx" => fx::author(ctx, tool, &args),
        _ if fx::TOOLS.contains(&tool) => fx::serve(ctx, tool, &args),
        _ => edit::serve(ctx, tool, &args),
    }
}

/// Change sets of external clients are titled so the user knows where they
/// came from.
pub const EXTERNAL_PREFIX: &str = "External agent: ";

/// Propose `commands` in order as one tool call's worth of change items.
pub fn propose(ctx: &mut ToolCtx, commands: Vec<Command>, reason: &str) -> Done {
    let opens_set =
        !ctx.bus.change_sets().iter().any(|s| {
            s.author == ctx.author && s.state == koo_core::changeset::ChangeSetState::Open
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
        match ctx.bus.execute(command, ctx.author.clone(), reason) {
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
