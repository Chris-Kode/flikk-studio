//! The user's skills and styles, for the agent: reading them, and creating,
//! changing or deleting one. Saves run directly when the user allowed it;
//! otherwise, and for every delete, [`request`] turns the call into what the
//! approval card shows and the editor applies.

use super::{Done, ToolCtx};
use crate::args::{Args, ToolError};
use flikk_core::bus::Permission;
use flikk_core::skills::{self, Draft, Entry, Kind};
use serde_json::{Value, json};

/// Every tool this module serves.
pub const TOOLS: [&str; 6] = [
    "list_skills",
    "list_styles",
    "save_skill",
    "save_style",
    "delete_skill",
    "delete_style",
];

/// Whether `tool` waits for the user under `permission`: saves unless the
/// user allowed them, deletes always, reads never.
pub fn asks(tool: &str, permission: Permission) -> bool {
    tool.starts_with("delete_")
        || (tool.starts_with("save_") && permission == Permission::NeedsApproval)
}

/// Runs a call the user allowed.
pub fn serve(ctx: &ToolCtx, tool: &str, args: &Args) -> Result<Done, ToolError> {
    match tool {
        "list_skills" => Ok(list(Kind::Skill, None)),
        "list_styles" => Ok(list(Kind::Style, ctx.settings.style.as_deref())),
        "save_skill" | "save_style" => {
            let change = Change::read(tool, args)?;
            let saved = skills::save(change.kind, change.previous_id(), &change.draft)
                .map_err(ToolError)?;
            let mut done = Done::text(
                change.summary(),
                format!("Saved {}.", label(change.kind, &saved.name)),
            );
            done.recorded = Some(Box::new(change.json()));
            Ok(done)
        }
        _ => Err(ToolError(format!("`{tool}` waits for the user"))),
    }
}

/// The approval card's line and the arguments the editor applies, for a
/// call that waits for the user.
pub fn request(tool: &str, args: &Args) -> Result<(String, Value), ToolError> {
    if tool.starts_with("delete_") {
        let kind = kind_of(tool);
        let entry = find(kind, args.str("name")?)?;
        let summary = format!("Delete {}", label(kind, &entry.name));
        let json = json!({
            "kind": kind.noun(),
            "id": entry.id,
            "name": entry.name,
            "instructions": entry.instructions,
        });
        return Ok((summary, json));
    }
    let change = Change::read(tool, args)?;
    Ok((change.summary(), change.json()))
}

/// A save as asked: what it writes, and the entry it writes over.
struct Change {
    kind: Kind,
    draft: Draft,
    existing: Option<Entry>,
}

impl Change {
    fn read(tool: &str, args: &Args) -> Result<Self, ToolError> {
        let kind = kind_of(tool);
        let name = args.str("name")?.trim().trim_start_matches('/').to_owned();
        let existing = find(kind, &name).ok();
        let draft = Draft {
            name,
            description: match kind {
                Kind::Skill => args.opt_str("description")?.unwrap_or_default().to_owned(),
                Kind::Style => String::new(),
            },
            instructions: args.str("instructions")?.trim().to_owned(),
            // A change keeps who made it; a new one is the agent's.
            by_agent: existing.as_ref().is_none_or(|e| e.by_agent),
        };
        if draft.instructions.is_empty() {
            return Err(ToolError("`instructions` is empty".into()));
        }
        Ok(Self {
            kind,
            draft,
            existing,
        })
    }

    fn previous_id(&self) -> Option<&str> {
        self.existing.as_ref().map(|e| e.id.as_str())
    }

    fn summary(&self) -> String {
        let verb = if self.existing.is_some() {
            "Update"
        } else {
            "New"
        };
        format!("{verb} {}", label(self.kind, &self.draft.name))
    }

    /// What the approval card and the thread show, and the editor applies.
    fn json(&self) -> Value {
        json!({
            "kind": self.kind.noun(),
            "id": self.previous_id(),
            "name": self.draft.name,
            "description": self.draft.description,
            "instructions": self.draft.instructions,
            "by_agent": self.draft.by_agent,
            "created": self.existing.is_none(),
            "before": self.existing.as_ref().map(|e| e.instructions.clone()),
        })
    }
}

fn kind_of(tool: &str) -> Kind {
    if tool.ends_with("_style") || tool == "list_styles" {
        Kind::Style
    } else {
        Kind::Skill
    }
}

/// `skill /name` or `style Name`.
fn label(kind: Kind, name: &str) -> String {
    match kind {
        Kind::Skill => format!("skill /{name}"),
        Kind::Style => format!("style {name}"),
    }
}

/// The saved entry called `name`, ignoring case and a skill's slash.
fn find(kind: Kind, name: &str) -> Result<Entry, ToolError> {
    let name = name.trim().trim_start_matches('/');
    skills::list(kind)
        .into_iter()
        .find(|e| e.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| ToolError(format!("there is no {} named {name}", kind.noun())))
}

fn list(kind: Kind, in_chat: Option<&str>) -> Done {
    let entries = skills::list(kind);
    let items: Vec<Value> = entries
        .iter()
        .map(|e| match kind {
            Kind::Skill => json!({
                "name": e.name,
                "description": e.description,
                "instructions": e.instructions,
            }),
            Kind::Style => json!({
                "name": e.name,
                "instructions": e.instructions,
                "in_chat": in_chat == Some(e.id.as_str()),
            }),
        })
        .collect();
    let plural = match kind {
        Kind::Skill => "skills",
        Kind::Style => "styles",
    };
    Done::json(format!("{} {plural}", items.len()), &json!(items))
}
