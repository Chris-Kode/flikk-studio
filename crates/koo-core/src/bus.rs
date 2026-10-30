//! The command bus: the single place where permissions, undo and change sets
//! are enforced (PRD "Architecture" principles 1–3).

use crate::changeset::{ChangeItem, ChangeSet, ChangeSetState, ItemState};
use crate::command::{Command, Plan, ToolCategory, Warning};
use crate::error::{CoreError, Result};
use crate::history::{History, HistoryEntry};
use crate::ids::*;
use crate::model::Project;
use crate::ops::{Op, apply_all, invert_all};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    #[default]
    Editor,
    Builder,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "author", rename_all = "snake_case")]
pub enum Author {
    User,
    Agent { role: AgentRole },
    Recipe { name: String },
}

impl Author {
    pub fn is_user(&self) -> bool {
        matches!(self, Author::User)
    }

    pub fn is_agent(&self) -> bool {
        matches!(self, Author::Agent { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Off,
    /// Changes land in a change set for review.
    Propose,
    /// Read categories only.
    Allowed,
    /// Each call waits for a dialog (memory, deliver).
    NeedsApproval,
}

/// Per-category permissions for non-user authors. Edit categories can only
/// be `Off` or `Propose`: auto-apply is not offered (PRD "Agent configuration").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Permissions(pub BTreeMap<ToolCategory, Permission>);

impl Default for Permissions {
    fn default() -> Self {
        Self(
            ToolCategory::ALL
                .iter()
                .map(|c| (*c, Self::default_for(*c)))
                .collect(),
        )
    }
}

impl Permissions {
    pub fn default_for(category: ToolCategory) -> Permission {
        match category {
            c if c.is_read() => Permission::Allowed,
            ToolCategory::ProjectMemory | ToolCategory::Deliver => Permission::NeedsApproval,
            ToolCategory::Media => Permission::Off,
            _ => Permission::Propose,
        }
    }

    /// The effective permission, after the hard limits no setting can lift.
    pub fn get(&self, category: ToolCategory) -> Permission {
        let set = self
            .0
            .get(&category)
            .copied()
            .unwrap_or(Self::default_for(category));
        match (category, set) {
            (ToolCategory::Media, _) => Permission::Off,
            (_, Permission::Off) => Permission::Off,
            (c, _) if c.is_read() => Permission::Allowed,
            (ToolCategory::ProjectMemory | ToolCategory::Deliver, _) => Permission::NeedsApproval,
            _ => Permission::Propose,
        }
    }

    pub fn set(&mut self, category: ToolCategory, permission: Permission) {
        self.0.insert(category, permission);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    Applied {
        warnings: Vec<Warning>,
    },
    Proposed {
        change_set: ChangeSetId,
        item: ChangeItemId,
        warnings: Vec<Warning>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AcceptReport {
    pub accepted: Vec<ChangeItemId>,
    pub conflicted: Vec<ChangeItemId>,
}

pub struct CommandBus {
    project: Project,
    history: History,
    permissions: Permissions,
    change_sets: Vec<ChangeSet>,
    /// Bumped by every applied change.
    version: u64,
    /// Clips the user touched, with the version they touched them at.
    user_touched: Vec<(u64, ClipId)>,
    /// The chat open in the editor. The agent's change sets are stamped with
    /// it, and only its sets are shown.
    chat: Option<String>,
}

impl CommandBus {
    pub fn new(project: Project) -> Self {
        Self {
            project,
            history: History::default(),
            permissions: Permissions::default(),
            change_sets: Vec::new(),
            version: 0,
            user_touched: Vec::new(),
            chat: None,
        }
    }

    /// Resume a saved project with its undo history. Versions continue from
    /// the history so change-set conflict tracking stays monotonic.
    pub fn with_history(project: Project, history: History) -> Self {
        let version = history
            .undo
            .iter()
            .chain(&history.redo)
            .map(|e| e.version)
            .max()
            .unwrap_or(0);
        Self {
            history,
            version,
            ..Self::new(project)
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// The project's name follows its file (Save As), so renaming is not an
    /// edit: no undo step, no version bump.
    pub fn rename_project(&mut self, name: &str) {
        self.project.name = name.to_owned();
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn permissions(&self) -> &Permissions {
        &self.permissions
    }

    pub fn set_permissions(&mut self, permissions: Permissions) {
        self.permissions = permissions;
    }

    pub fn change_sets(&self) -> &[ChangeSet] {
        &self.change_sets
    }

    pub fn change_set(&self, id: ChangeSetId) -> Option<&ChangeSet> {
        self.change_sets.iter().find(|c| c.id == id)
    }

    /// The change sets of the open chat, plus those of no chat (recipes,
    /// external clients).
    pub fn chat_change_sets(&self) -> impl Iterator<Item = &ChangeSet> {
        self.change_sets
            .iter()
            .filter(|s| s.chat.is_none() || s.chat == self.chat)
    }

    /// Switch to chat `chat`: the editor shows its pending proposals, and
    /// the agent's next turn adds to them, rather than another chat's.
    pub fn set_chat(&mut self, chat: &str) {
        self.chat = Some(chat.to_owned());
    }

    /// What `author` sees: the open change set's draft for agents, the
    /// committed project otherwise.
    pub fn view_for(&self, author: &Author) -> &Project {
        self.open_set(author)
            .and_then(|s| s.draft.as_ref())
            .unwrap_or(&self.project)
    }

    /// Start `author`'s turn in the open chat: the chat's set still waiting
    /// for review opens again, so the turn sees those proposals and adds to
    /// them, and one review takes them all. Returns the set and how many
    /// items it had.
    pub fn reopen_change_set(&mut self, author: &Author) -> Option<(ChangeSetId, usize)> {
        let set = self.change_sets.iter_mut().find(|s| {
            &s.author == author
                && s.state == ChangeSetState::Pending
                && s.chat.is_some()
                && s.chat == self.chat
        })?;
        set.state = ChangeSetState::Open;
        if set.draft.is_none() || set.base_version != self.version {
            // The project changed since: replay the pending items on it,
            // re-planning those whose ops no longer fit and leaving out
            // those that can't be.
            let mut draft = self.project.clone();
            for item in set.pending_items() {
                if apply_all(&mut draft, &item.ops).is_err()
                    && let Ok(plan) = item.command.plan(&draft)
                {
                    let _ = apply_all(&mut draft, &plan.ops);
                }
            }
            set.draft = Some(draft);
            set.base_version = self.version;
        }
        Some((set.id, set.items.len()))
    }

    /// Permission check for anything the bus does not execute itself
    /// (read and view tools, deliver, memory).
    pub fn check(&self, author: &Author, category: ToolCategory) -> Result<Permission> {
        if author.is_user() {
            return Ok(Permission::Allowed);
        }
        match self.permissions.get(category) {
            Permission::Off => Err(CoreError::PermissionDenied(category)),
            p => Ok(p),
        }
    }

    /// Run one command. Users apply directly; agents and recipes propose into
    /// their open change set (opening one titled `reason` if needed).
    pub fn execute(
        &mut self,
        command: Command,
        author: Author,
        reason: impl Into<String>,
    ) -> Result<Outcome> {
        let reason = reason.into();
        if author.is_user() {
            let label = command.name();
            return self.apply_user(vec![command], label);
        }
        match self.check(&author, command.category())? {
            Permission::Propose => self.propose(command, author, reason),
            _ => Err(CoreError::PermissionDenied(command.category())),
        }
    }

    /// Several user commands as one undo step (e.g. "remove all silences").
    pub fn execute_batch(
        &mut self,
        commands: Vec<Command>,
        label: impl Into<String>,
    ) -> Result<Outcome> {
        self.apply_user(commands, label.into())
    }

    fn apply_user(&mut self, commands: Vec<Command>, label: String) -> Result<Outcome> {
        let mut scratch = self.project.clone();
        let mut all_ops = Vec::new();
        let mut warnings = Vec::new();
        for command in &commands {
            let Plan { ops, warnings: w } = command.plan(&scratch)?;
            apply_all(&mut scratch, &ops)?;
            all_ops.extend(ops);
            warnings.extend(w);
        }
        self.project = scratch;
        self.commit(label, Author::User, all_ops, None);
        Ok(Outcome::Applied { warnings })
    }

    fn propose(&mut self, command: Command, author: Author, reason: String) -> Result<Outcome> {
        let version = self.version;
        let project = &self.project;
        let chat = if author.is_agent() {
            self.chat.clone()
        } else {
            None
        };
        let set = match self
            .change_sets
            .iter_mut()
            .position(|s| s.author == author && s.state == ChangeSetState::Open)
        {
            Some(i) => &mut self.change_sets[i],
            None => {
                self.change_sets.push(ChangeSet {
                    id: ChangeSetId::new(),
                    title: reason.clone(),
                    author,
                    chat,
                    state: ChangeSetState::Open,
                    items: Vec::new(),
                    base_version: version,
                    draft: None,
                });
                self.change_sets.last_mut().unwrap()
            }
        };
        // A set with no items yet (e.g. its first command failed to plan)
        // must not keep planning against a snapshot older than the user's
        // latest edits.
        if set.items.is_empty() {
            set.draft = None;
            set.base_version = version;
        }
        let draft = set.draft.get_or_insert_with(|| project.clone());
        let Plan { ops, warnings } = command.plan(draft)?;
        apply_all(draft, &ops)?;
        let item = ChangeItem {
            id: ChangeItemId::new(),
            command,
            reason,
            ops,
            warnings: warnings.clone(),
            state: ItemState::Pending,
            base_version: set.base_version,
        };
        let item_id = item.id;
        set.items.push(item);
        Ok(Outcome::Proposed {
            change_set: set.id,
            item: item_id,
            warnings,
        })
    }

    fn open_set(&self, author: &Author) -> Option<&ChangeSet> {
        self.change_sets
            .iter()
            .find(|s| &s.author == author && s.state == ChangeSetState::Open)
    }

    /// The agent finished its turn: the set is ready for review.
    pub fn close_change_set(&mut self, id: ChangeSetId) {
        if let Some(set) = self
            .change_sets
            .iter_mut()
            .find(|s| s.id == id && s.state == ChangeSetState::Open)
        {
            set.state = if set.items.is_empty() {
                ChangeSetState::Resolved
            } else {
                ChangeSetState::Pending
            };
        }
    }

    /// Accept the given items (all pending ones if `None`) as one undo step.
    /// Items touching clips the user edited since they were planned are marked
    /// conflicted and not applied; others are re-planned if their ops no
    /// longer fit (e.g. an earlier item in the set was rejected).
    pub fn accept(
        &mut self,
        id: ChangeSetId,
        items: Option<&[ChangeItemId]>,
    ) -> Result<AcceptReport> {
        let ix = self
            .change_sets
            .iter()
            .position(|s| s.id == id)
            .ok_or(CoreError::Invalid("unknown change set".into()))?;
        let user_touched = &self.user_touched;
        let mut scratch = self.project.clone();
        let mut all_ops = Vec::new();
        let mut report = AcceptReport::default();
        let set = &mut self.change_sets[ix];
        for item in set
            .items
            .iter_mut()
            .filter(|i| i.state == ItemState::Pending)
        {
            if items.is_some_and(|ids| !ids.contains(&item.id)) {
                continue;
            }
            if item.ops.iter().filter_map(Op::clip).any(|c| {
                user_touched
                    .iter()
                    .any(|(v, t)| *v > item.base_version && *t == c)
            }) {
                item.state = ItemState::Conflicted;
                report.conflicted.push(item.id);
                continue;
            }
            let ops = match apply_all(&mut scratch, &item.ops) {
                Ok(()) => item.ops.clone(),
                Err(_) => match item
                    .command
                    .plan(&scratch)
                    .and_then(|p| apply_all(&mut scratch, &p.ops).map(|_| p.ops))
                {
                    Ok(ops) => ops,
                    Err(_) => {
                        item.state = ItemState::Conflicted;
                        report.conflicted.push(item.id);
                        continue;
                    }
                },
            };
            item.state = ItemState::Accepted;
            report.accepted.push(item.id);
            all_ops.extend(ops);
        }
        let (label, author) = (set.title.clone(), set.author.clone());
        self.refresh_set(ix);
        if !all_ops.is_empty() {
            self.project = scratch;
            self.commit(label, author, all_ops, Some(id));
        }
        Ok(report)
    }

    /// Reject the given items (all pending ones if `None`).
    pub fn reject(&mut self, id: ChangeSetId, items: Option<&[ChangeItemId]>) {
        if let Some(ix) = self.change_sets.iter().position(|s| s.id == id) {
            for item in self.change_sets[ix]
                .items
                .iter_mut()
                .filter(|i| i.state == ItemState::Pending)
            {
                if items.is_none_or(|ids| ids.contains(&item.id)) {
                    item.state = ItemState::Rejected;
                }
            }
            self.refresh_set(ix);
        }
    }

    fn refresh_set(&mut self, ix: usize) {
        let set = &mut self.change_sets[ix];
        if set.state != ChangeSetState::Open && set.pending_items().next().is_none() {
            set.state = ChangeSetState::Resolved;
            set.draft = None;
        }
    }

    pub fn undo(&mut self) -> Result<Option<String>> {
        let Some(entry) = self.history.undo.pop() else {
            return Ok(None);
        };
        if let Err(e) = apply_all(&mut self.project, &invert_all(&entry.ops)) {
            self.history.undo.push(entry);
            return Err(e);
        }
        self.bump(&entry.author, &entry.ops);
        let label = entry.label.clone();
        self.history.redo.push(entry);
        Ok(Some(label))
    }

    pub fn redo(&mut self) -> Result<Option<String>> {
        let Some(entry) = self.history.redo.pop() else {
            return Ok(None);
        };
        if let Err(e) = apply_all(&mut self.project, &entry.ops) {
            self.history.redo.push(entry);
            return Err(e);
        }
        self.bump(&entry.author, &entry.ops);
        let label = entry.label.clone();
        self.history.undo.push(entry);
        Ok(Some(label))
    }

    fn commit(
        &mut self,
        label: String,
        author: Author,
        ops: Vec<Op>,
        change_set: Option<ChangeSetId>,
    ) {
        if ops.is_empty() {
            return;
        }
        self.bump(&author, &ops);
        self.history.push(HistoryEntry {
            label,
            author,
            ops,
            change_set,
            version: self.version,
        });
    }

    fn bump(&mut self, author: &Author, ops: &[Op]) {
        self.version += 1;
        if author.is_user() {
            let v = self.version;
            self.user_touched
                .extend(ops.iter().filter_map(Op::clip).map(|c| (v, c)));
        }
    }
}
