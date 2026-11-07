//! Pending agent work (PRD AG4–AG6).

use crate::bus::Author;
use crate::command::{Command, Warning};
use crate::ids::*;
use crate::model::Project;
use crate::ops::{Op, Target, TrackProps};
use crate::subject::Fnv;
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
    /// history: undoing that entry rejects the item, redoing it accepts it
    /// again.
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
    /// item's entry rejects it; the set stays resolved.
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

/// A hash of what `ops` change as `project` has it, stable across runs and
/// builds. Kept with a saved proposal, it tells on load whether any of that
/// changed since, e.g. in an older save of the project.
pub fn targets_hash(project: &Project, ops: &[Op]) -> u64 {
    let mut h = Fnv::default();
    for target in ops.iter().flat_map(Op::targets) {
        match target_value(project, &target) {
            Some(bytes) => {
                h.write(&[1]);
                h.write(&(bytes.len() as u64).to_le_bytes());
                h.write(&bytes);
            }
            None => h.write(&[0]),
        }
    }
    h.0
}

/// `target` as `project` has it, serialized; `None` when it has none.
fn target_value(project: &Project, target: &Target) -> Option<Vec<u8>> {
    let timeline = &project.timeline;
    let track = |id: &TrackId| timeline.tracks.iter().find(|t| t.id == *id);
    let json = match target {
        // With its track, so a clip moved to another track differs.
        Target::Clip(id) => timeline.tracks.iter().find_map(|t| {
            let clip = t.clips.iter().find(|c| c.id == *id)?;
            Some(serde_json::to_vec(&(t.id, clip)))
        })?,
        Target::Track(id) => {
            let t = track(id)?;
            serde_json::to_vec(&(t.kind, TrackProps::of(t)))
        }
        Target::TrackClips(id) => {
            let clips: Vec<ClipId> = track(id)?.clips.iter().map(|c| c.id).collect();
            serde_json::to_vec(&clips)
        }
        Target::Marker(id) => serde_json::to_vec(timeline.markers.iter().find(|m| m.id == *id)?),
        Target::Media(id) => serde_json::to_vec(project.media(*id)?),
        Target::Fx(id) => serde_json::to_vec(project.fx.iter().find(|a| a.id == *id)?),
        Target::Bins => serde_json::to_vec(&project.bins),
        Target::Ducking => serde_json::to_vec(&timeline.ducking),
        Target::Grade => serde_json::to_vec(&timeline.grade),
        Target::Sequence => serde_json::to_vec(&timeline.settings),
    };
    json.ok()
}
