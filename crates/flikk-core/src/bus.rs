//! The command bus: the single place where permissions, undo and change sets
//! are enforced (PRD "Architecture" principles 1–3).

use crate::changeset::{ChangeItem, ChangeSet, ChangeSetState, ItemState};
use crate::command::{Command, Plan, ToolCategory, Warning};
use crate::error::{CoreError, Result};
use crate::history::{History, HistoryEntry};
use crate::ids::*;
use crate::model::Project;
use crate::ops::{Op, Target, apply_all, invert_all};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Resolved change sets kept besides those an undo or redo entry is theirs,
/// for the review cards that still show what came of them.
const KEPT_RESOLVED: usize = 16;

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
    /// Read categories, and generating media once the user allows it.
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
            ToolCategory::ProjectMemory | ToolCategory::Deliver | ToolCategory::Generate => {
                Permission::NeedsApproval
            }
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
            // Generating spends the user's credits: asked each time unless
            // they allow it.
            (ToolCategory::Generate, Permission::Allowed) => Permission::Allowed,
            (ToolCategory::Generate, _) => Permission::NeedsApproval,
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

/// Where a view was seen, for a job that works on a snapshot of it: the
/// bus version, which the user's edits are counted in, and the proposals
/// count, which the change items are stamped with (`proposed_at`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Since {
    pub version: u64,
    pub proposals: u64,
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
    /// What the user changed, with the version they changed it at.
    user_touched: Vec<(u64, Target)>,
    /// The chat open in the editor. The agent's change sets are stamped with
    /// it, and only its sets are shown.
    chat: Option<String>,
    /// Counts resolutions, so the latest resolved sets are the ones kept.
    resolutions: u64,
    /// Counts proposed items. Versions only move on commits, so this is
    /// what orders proposals.
    proposals: u64,
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
            resolutions: 0,
            proposals: 0,
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
    /// committed project otherwise. A set with no items yet plans its first
    /// on the committed project, so that is what it sees.
    pub fn view_for(&self, author: &Author) -> &Project {
        self.open_draft(author)
            .and_then(|s| s.draft.as_ref())
            .unwrap_or(&self.project)
    }

    /// The bus version [`Self::view_for`] shows the project at: a job that
    /// works on a snapshot of it proposes with [`Self::execute_since`] and
    /// this, so edits made meanwhile conflict rather than being undone.
    pub fn view_version(&self, author: &Author) -> u64 {
        self.open_draft(author)
            .map_or(self.version, |s| s.base_version)
    }

    /// [`Self::view_version`] with the proposals [`Self::view_for`] holds,
    /// for [`Self::execute_since`].
    pub fn view_since(&self, author: &Author) -> Since {
        Since {
            version: self.view_version(author),
            proposals: self.proposals,
        }
    }

    fn open_draft(&self, author: &Author) -> Option<&ChangeSet> {
        self.open_set(author)
            .filter(|s| !s.items.is_empty() && s.draft.is_some())
    }

    /// Start `author`'s turn in the open chat: the chat's set still waiting
    /// for review opens again, so the turn sees those proposals and adds to
    /// them, and one review takes them all. Returns the set and how many
    /// items it had.
    ///
    /// An author has one open set. One of another chat goes to review
    /// first; several of this chat's (one left open, one still waiting
    /// for review) become the one set.
    pub fn reopen_change_set(&mut self, author: &Author) -> Option<(ChangeSetId, usize)> {
        let chat = self.chat.clone();
        let ours = |s: &ChangeSet| &s.author == author && s.chat.is_some() && s.chat == chat;
        let stray: Vec<_> = self
            .change_sets
            .iter()
            .filter(|s| &s.author == author && s.state == ChangeSetState::Open && !ours(s))
            .map(|s| s.id)
            .collect();
        for id in stray {
            self.close_change_set(id);
        }
        let waiting = |s: &ChangeSet| {
            ours(s) && matches!(s.state, ChangeSetState::Open | ChangeSetState::Pending)
        };
        let keep = self
            .change_sets
            .iter()
            .position(|s| waiting(s) && s.state == ChangeSetState::Open)
            .or_else(|| self.change_sets.iter().position(&waiting))?;
        let id = self.change_sets[keep].id;
        let mut merged = Vec::new();
        let mut i = 0;
        self.change_sets.retain(|s| {
            let merge = i != keep && waiting(s);
            i += 1;
            if merge {
                merged.push(s.clone());
            }
            !merge
        });
        let set = self
            .change_sets
            .iter_mut()
            .find(|s| s.id == id)
            .expect("kept above");
        for other in &merged {
            set.items.extend(other.items.iter().cloned());
        }
        set.state = ChangeSetState::Open;
        if !merged.is_empty() || set.draft.is_none() || set.base_version != self.version {
            // The project changed since: replay the pending items on it.
            rebuild(set, &self.project, &self.user_touched, self.version);
        }
        let len = set.items.len();
        // Undoing what was accepted from a merged set reaches it here.
        let gone: HashSet<_> = merged.iter().map(|s| s.id).collect();
        for entry in self.history.undo.iter_mut().chain(&mut self.history.redo) {
            if entry.change_set.is_some_and(|c| gone.contains(&c)) {
                entry.change_set = Some(id);
            }
        }
        Some((id, len))
    }

    /// Copies of the change sets still waiting for review, for saving with
    /// the project's chats: open ones as pending, without drafts or undo
    /// marks (undo history isn't saved). Items that meet an edit the user
    /// made since they were planned are conflicted, as accepting would find.
    pub fn waiting_change_sets(&self) -> Vec<ChangeSet> {
        self.change_sets
            .iter()
            .filter(|s| s.state != ChangeSetState::Resolved && !s.items.is_empty())
            .map(|s| ChangeSet {
                id: s.id,
                title: s.title.clone(),
                author: s.author.clone(),
                chat: s.chat.clone(),
                state: ChangeSetState::Pending,
                items: s
                    .items
                    .iter()
                    .map(|i| {
                        let mut item = i.clone();
                        item.accepted_at = None;
                        if item.state == ItemState::Pending
                            && touched_since(&self.user_touched, item.base_version, &item.ops)
                        {
                            item.state = ItemState::Conflicted;
                        }
                        item
                    })
                    .collect(),
                base_version: s.base_version,
                draft: None,
                resolved_at: 0,
            })
            .collect()
    }

    /// Bring back change sets saved by [`Self::waiting_change_sets`], e.g.
    /// when the project opens again. Nothing is applied: they wait for
    /// review, re-planned on the project as it is now, and items that no
    /// longer apply are conflicted. Ids stay unless one is taken already.
    /// Returns each restored set's saved id with the id it has now.
    pub fn restore_change_sets(
        &mut self,
        sets: Vec<ChangeSet>,
    ) -> HashMap<ChangeSetId, ChangeSetId> {
        let mut set_ids: HashSet<_> = self.change_sets.iter().map(|s| s.id).collect();
        let mut item_ids: HashSet<_> = self
            .change_sets
            .iter()
            .flat_map(|s| &s.items)
            .map(|i| i.id)
            .collect();
        let mut restored = HashMap::new();
        for mut set in sets {
            if set.state == ChangeSetState::Resolved || set.items.is_empty() {
                continue;
            }
            let saved = set.id;
            if !set_ids.insert(set.id) {
                set.id = ChangeSetId::new();
                set_ids.insert(set.id);
            }
            set.state = ChangeSetState::Pending;
            set.resolved_at = 0;
            for item in &mut set.items {
                if !item_ids.insert(item.id) {
                    item.id = ChangeItemId::new();
                    item_ids.insert(item.id);
                }
                item.base_version = self.version;
                item.accepted_at = None;
                // Like `base_version`: as if all were proposed now.
                item.proposed_at = self.proposals;
            }
            rebuild(&mut set, &self.project, &self.user_touched, self.version);
            restored.insert(saved, set.id);
            self.change_sets.push(set);
        }
        restored
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
            Permission::Propose => {
                let call = self.proposals;
                self.propose(command, author, reason, None, call)
            }
            _ => Err(CoreError::PermissionDenied(command.category())),
        }
    }

    /// Like [`Self::execute`] for a non-user author, for one tool call's
    /// commands worked out on the project as it was seen at `since` (a
    /// job's snapshot, see [`Self::view_since`]): the user's edits since
    /// then to what a command changes conflict with it, as if it had been
    /// proposed then. So does another of the author's proposals made since
    /// to the same things, not one of this call's. Stops at the first
    /// command that fails.
    pub fn execute_since(
        &mut self,
        commands: Vec<Command>,
        author: Author,
        reason: impl Into<String>,
        since: Since,
    ) -> Vec<Result<Outcome>> {
        let reason = reason.into();
        let call = self.proposals;
        let mut outcomes = Vec::new();
        for command in commands {
            let outcome = if author.is_user() {
                self.execute(command, author.clone(), reason.clone())
            } else {
                match self.check(&author, command.category()) {
                    Ok(Permission::Propose) => {
                        self.propose(command, author.clone(), reason.clone(), Some(since), call)
                    }
                    Ok(_) => Err(CoreError::PermissionDenied(command.category())),
                    Err(e) => Err(e),
                }
            };
            let failed = outcome.is_err();
            outcomes.push(outcome);
            if failed {
                break;
            }
        }
        outcomes
    }

    /// Several user commands as one undo step (e.g. "remove all silences").
    pub fn execute_batch(
        &mut self,
        commands: Vec<Command>,
        label: impl Into<String>,
    ) -> Result<Outcome> {
        self.apply_user(commands, label.into())
    }

    /// Like [`Self::execute_batch`], and the same commands go into the
    /// drafts of the change sets not yet reviewed, so an agent planning on
    /// one sees the change. For additions no proposal can conflict with,
    /// such as media a provider just made.
    pub fn execute_shared(
        &mut self,
        commands: Vec<Command>,
        label: impl Into<String>,
    ) -> Result<Outcome> {
        self.execute_shared_as(commands, label, Author::User)
    }

    /// [`Self::execute_shared`] as `author`: applied at once as one undo
    /// step, with `author` on it, not proposed for review.
    ///
    /// Only for results the user already approved, such as what a
    /// generation they allowed on its approval card made: that card is the
    /// permission gate, so no category is checked here and nothing waits
    /// for review. Not being the user's edit, a non-user author's change is
    /// not counted against the change sets, so the author's own proposals
    /// to the same things (the clip that places it, its track) don't
    /// conflict with it. Undoing or redoing it is the user's edit, as any
    /// undo is.
    pub fn execute_shared_as(
        &mut self,
        commands: Vec<Command>,
        label: impl Into<String>,
        author: Author,
    ) -> Result<Outcome> {
        let outcome = self.apply(commands.clone(), label.into(), author)?;
        for draft in self.change_sets.iter_mut().filter_map(|s| s.draft.as_mut()) {
            for command in &commands {
                if let Ok(plan) = command.plan(draft) {
                    let _ = apply_all(draft, &plan.ops);
                }
            }
        }
        Ok(outcome)
    }

    fn apply_user(&mut self, commands: Vec<Command>, label: String) -> Result<Outcome> {
        self.apply(commands, label, Author::User)
    }

    /// Apply `commands` at once as one undo step by `author`.
    fn apply(&mut self, commands: Vec<Command>, label: String, author: Author) -> Result<Outcome> {
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
        // Not from a change set, so undoing it marks none (`reflect`).
        self.commit(label, author, all_ops, None);
        // The edit cleared the redo entries.
        self.prune();
        Ok(Outcome::Applied { warnings })
    }

    /// Propose `command` into `author`'s open set. `call` is the proposals
    /// count before the tool call it is part of began.
    fn propose(
        &mut self,
        command: Command,
        author: Author,
        reason: String,
        since: Option<Since>,
        call: u64,
    ) -> Result<Outcome> {
        let version = self.version;
        let chat = if author.is_agent() {
            self.chat.clone()
        } else {
            None
        };
        let ix = match self
            .change_sets
            .iter()
            .position(|s| s.author == author && s.state == ChangeSetState::Open)
        {
            Some(i) => i,
            None => {
                self.change_sets.push(ChangeSet {
                    id: ChangeSetId::new(),
                    title: reason.clone(),
                    author: author.clone(),
                    chat,
                    state: ChangeSetState::Open,
                    items: Vec::new(),
                    base_version: version,
                    draft: None,
                    resolved_at: 0,
                });
                self.change_sets.len() - 1
            }
        };
        let set = &mut self.change_sets[ix];
        // A set with no items yet (e.g. its first command failed to plan)
        // must not keep planning against a snapshot older than the user's
        // latest edits.
        if set.items.is_empty() {
            set.draft = None;
            set.base_version = version;
        }
        let draft = set.draft.get_or_insert_with(|| self.project.clone());
        let Plan { ops, mut warnings } = command.plan(draft)?;
        // Worked out before another proposal to the same things, it would
        // undo that one in the draft: the earlier one stays.
        let overwrites = since.and_then(|s| self.proposed_since(&author, s, call, &ops));
        let set = &mut self.change_sets[ix];
        let state = match overwrites {
            Some(target) => {
                let what = match target {
                    Target::Clip(_) => "this clip",
                    Target::Track(_) | Target::TrackClips(_) => "this track",
                    _ => "this",
                };
                warnings = vec![Warning::NeedsConfirmation {
                    message: format!(
                        "Another change to {what} was proposed while this was worked out; ask again"
                    ),
                }];
                ItemState::Conflicted
            }
            None => {
                apply_all(set.draft.as_mut().expect("made above"), &ops)?;
                ItemState::Pending
            }
        };
        self.proposals += 1;
        let item = ChangeItem {
            id: ChangeItemId::new(),
            command,
            reason,
            ops,
            warnings: warnings.clone(),
            state,
            // Planned on the draft, but from what the project was at
            // `since`: edits after either conflict.
            base_version: since.map_or(set.base_version, |s| s.version.min(set.base_version)),
            accepted_at: None,
            proposed_at: self.proposals,
        };
        let item_id = item.id;
        set.items.push(item);
        Ok(Outcome::Proposed {
            change_set: set.id,
            item: item_id,
            warnings,
        })
    }

    /// What `ops` change that one of `author`'s pending items, proposed
    /// after `since` and before the tool call that began at `call`, changes
    /// too; `None` when nothing.
    fn proposed_since(
        &self,
        author: &Author,
        since: Since,
        call: u64,
        ops: &[Op],
    ) -> Option<Target> {
        let theirs: Vec<Target> = self
            .change_sets
            .iter()
            .filter(|s| &s.author == author && s.state != ChangeSetState::Resolved)
            .flat_map(ChangeSet::pending_items)
            .filter(|i| i.proposed_at > since.proposals && i.proposed_at <= call)
            .flat_map(|i| touches(&i.ops))
            .collect();
        ops.iter()
            .flat_map(Op::targets)
            .find(|t| theirs.contains(t))
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
            if set.pending_items().next().is_none() {
                resolve(set, &mut self.resolutions);
            } else {
                set.state = ChangeSetState::Pending;
            }
        }
        self.prune();
    }

    /// Accept the given items (all pending ones if `None`) as one undo step.
    /// Items touching anything the user edited since they were planned, or a
    /// locked track, are marked conflicted and not applied; others are
    /// re-planned if their ops no longer fit (e.g. an earlier item in the
    /// set was rejected).
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
            match replay(&mut scratch, item, user_touched) {
                Some(ops) => {
                    item.ops = ops.clone();
                    item.state = ItemState::Accepted;
                    report.accepted.push(item.id);
                    all_ops.extend(ops);
                }
                None => {
                    item.state = ItemState::Conflicted;
                    report.conflicted.push(item.id);
                }
            }
        }
        let (label, author) = (set.title.clone(), set.author.clone());
        if !all_ops.is_empty() {
            self.project = scratch;
            self.commit(label, author, all_ops, Some(id));
            let version = self.version;
            for item in self.change_sets[ix]
                .items
                .iter_mut()
                .filter(|i| report.accepted.contains(&i.id))
            {
                item.accepted_at = Some(version);
            }
        }
        self.refresh_set(ix);
        self.prune();
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
            self.prune();
        }
    }

    /// After a review: resolve the set once nothing is left to review, or
    /// rebuild its draft from what is still pending, so neither the agent nor
    /// the preview keeps items that were rejected or already applied.
    fn refresh_set(&mut self, ix: usize) {
        let set = &mut self.change_sets[ix];
        if set.state == ChangeSetState::Resolved {
            return;
        }
        if set.state != ChangeSetState::Open && set.pending_items().next().is_none() {
            resolve(set, &mut self.resolutions);
        } else {
            rebuild(set, &self.project, &self.user_touched, self.version);
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
        // Undo and redo are the user's edits, whoever made the entry.
        self.bump(true, &entry.ops);
        self.reflect(&entry, false);
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
        self.bump(true, &entry.ops);
        self.reflect(&entry, true);
        let label = entry.label.clone();
        self.history.undo.push(entry);
        Ok(Some(label))
    }

    /// Show on its change set whether `entry`, just undone or redone, is
    /// applied. Undoing what was accepted turns it down, as Reject would:
    /// no review comes back, and the agent doesn't build on it. Redone, it
    /// is accepted again.
    fn reflect(&mut self, entry: &HistoryEntry, applied: bool) {
        let Some(ix) = entry
            .change_set
            .and_then(|id| self.change_sets.iter().position(|s| s.id == id))
        else {
            return;
        };
        let set = &mut self.change_sets[ix];
        let mut any = false;
        for item in set
            .items
            .iter_mut()
            .filter(|i| i.accepted_at == Some(entry.version))
        {
            any = true;
            item.state = if applied {
                ItemState::Accepted
            } else {
                ItemState::Rejected
            };
        }
        if any {
            // An open set's draft drops what was turned down.
            self.refresh_set(ix);
        }
    }

    /// Drop resolved sets nothing brings back, past the latest
    /// [`KEPT_RESOLVED`]: a set an undo or redo entry is from stays, as
    /// undoing or redoing that entry marks its items.
    fn prune(&mut self) {
        let referenced: HashSet<ChangeSetId> = self
            .history
            .undo
            .iter()
            .chain(&self.history.redo)
            .filter_map(|e| e.change_set)
            .collect();
        let droppable =
            |s: &ChangeSet| s.state == ChangeSetState::Resolved && !referenced.contains(&s.id);
        let mut order: Vec<u64> = self
            .change_sets
            .iter()
            .filter(|s| droppable(s))
            .map(|s| s.resolved_at)
            .collect();
        if order.len() <= KEPT_RESOLVED {
            return;
        }
        order.sort_unstable();
        let oldest_kept = order[order.len() - KEPT_RESOLVED];
        self.change_sets
            .retain(|s| !droppable(s) || s.resolved_at >= oldest_kept);
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
        // Only the user's edits conflict with proposals.
        self.bump(author.is_user(), &ops);
        self.history.push(HistoryEntry {
            label,
            author,
            ops,
            change_set,
            version: self.version,
        });
    }

    fn bump(&mut self, by_user: bool, ops: &[Op]) {
        self.version += 1;
        if by_user {
            let v = self.version;
            self.user_touched
                .extend(touches(ops).into_iter().map(|t| (v, t)));
        }
    }
}

/// What `ops` change, as conflicts count it: their targets, and which
/// clips the tracks of the clips they change hold.
fn touches(ops: &[Op]) -> Vec<Target> {
    ops.iter()
        .flat_map(|op| {
            op.targets()
                .into_iter()
                .chain(op.clip_track().map(Target::TrackClips))
        })
        .collect()
}

/// Nothing in `set` is left to review.
fn resolve(set: &mut ChangeSet, resolutions: &mut u64) {
    *resolutions += 1;
    set.state = ChangeSetState::Resolved;
    set.resolved_at = *resolutions;
    set.draft = None;
}

/// Whether `ops` change anything the user changed after version `since`.
fn touched_since(touched: &[(u64, Target)], since: u64, ops: &[Op]) -> bool {
    ops.iter()
        .flat_map(Op::targets)
        .any(|t| touched.iter().any(|(v, u)| *v > since && *u == t))
}

/// Apply `item` to `project`, re-planning it if its ops no longer fit (e.g.
/// an earlier item was rejected). The ops applied, or `None` when the item
/// can't apply or would change something the user edited since it was
/// planned, or a locked track; `project` is then unchanged.
fn replay(project: &mut Project, item: &ChangeItem, touched: &[(u64, Target)]) -> Option<Vec<Op>> {
    if touched_since(touched, item.base_version, &item.ops)
        || crate::edit::locked_track(project, &item.ops).is_some()
    {
        return None;
    }
    if apply_all(project, &item.ops).is_ok() {
        return Some(item.ops.clone());
    }
    let plan = item.command.plan(project).ok()?;
    if touched_since(touched, item.base_version, &plan.ops) {
        return None;
    }
    apply_all(project, &plan.ops).ok()?;
    Some(plan.ops)
}

/// Rebuild `set`'s draft as `project` plus its pending items. Items that
/// no longer apply are marked conflicted; the others keep the ops they now
/// apply with, so the timeline draws what accepting would do.
fn rebuild(set: &mut ChangeSet, project: &Project, touched: &[(u64, Target)], version: u64) {
    let mut draft = project.clone();
    for item in set
        .items
        .iter_mut()
        .filter(|i| i.state == ItemState::Pending)
    {
        match replay(&mut draft, item, touched) {
            Some(ops) => item.ops = ops,
            None => item.state = ItemState::Conflicted,
        }
    }
    set.draft = Some(draft);
    set.base_version = version;
}
