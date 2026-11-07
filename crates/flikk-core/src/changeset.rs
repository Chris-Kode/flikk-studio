//! Pending agent work (PRD AG4–AG6).

use crate::bus::Author;
use crate::command::{Command, Warning};
use crate::ids::*;
use crate::model::Project;
use crate::ops::Op;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    Pending,
    Accepted,
    Rejected,
    /// The user edited something this item touches after it was proposed;
    /// it was not applied (PRD AG6).
    Conflicted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChangeItem {
    pub id: ChangeItemId,
    pub command: Command,
    /// One line, shown on the timeline and in the review list.
    pub reason: String,
    /// Planned against the change set's draft at proposal time.
    pub ops: Vec<Op>,
    pub warnings: Vec<Warning>,
    pub state: ItemState,
    /// Bus version of the draft it was planned against; the user's edits
    /// after it conflict.
    #[serde(default)]
    pub base_version: u64,
    /// Bus version of the undo entry that applied it, while it is in the
    /// history: undoing that entry makes the item pending again, redoing it
    /// accepted.
    #[serde(default)]
    pub accepted_at: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSetState {
    /// The agent may still add items.
    Open,
    /// Complete, waiting for review.
    Pending,
    /// Every item accepted, rejected or conflicted. Undoing an accepted
    /// item's entry makes the set pending again.
    Resolved,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangeSet {
    pub id: ChangeSetId,
    pub title: String,
    pub author: Author,
    /// The chat the agent proposed it in; `None` for recipes and external
    /// clients.
    #[serde(default)]
    pub chat: Option<String>,
    pub state: ChangeSetState,
    pub items: Vec<ChangeItem>,
    /// Bus version the draft was last built from the project at.
    pub base_version: u64,
    /// Committed project plus this set's pending items: what the agent sees
    /// while it keeps working on the set.
    #[serde(skip)]
    pub draft: Option<Project>,
    /// When it was resolved, counted by the bus, for keeping the latest.
    #[serde(skip)]
    pub(crate) resolved_at: u64,
}

impl ChangeSet {
    pub fn item(&self, id: ChangeItemId) -> Option<&ChangeItem> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn pending_items(&self) -> impl Iterator<Item = &ChangeItem> {
        self.items.iter().filter(|i| i.state == ItemState::Pending)
    }

    /// All pending ops, for drawing on the timeline.
    pub fn pending_ops(&self) -> impl Iterator<Item = (&ChangeItem, &Op)> {
        self.pending_items()
            .flat_map(|i| i.ops.iter().map(move |op| (i, op)))
    }
}
