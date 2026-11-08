//! The open project's change sets still waiting for review, saved beside
//! its chats so their review cards come back when it opens again.

use super::notify;
use flikk_core::bus::CommandBus;
use flikk_core::changeset::{self, ChangeSet, ItemState};
use flikk_core::ids::ProjectId;
use flikk_core::model::Project;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Format of the proposals file; one with a newer version is not read.
const PROPOSALS_VERSION: u32 = 1;

/// A project's change sets still waiting for review, beside its chats
/// under `paths::projects()`.
fn proposals_path(project: ProjectId) -> PathBuf {
    flikk_core::paths::projects().join(format!("{}.proposals.json", project.0.simple()))
}

#[derive(Serialize, Deserialize)]
struct ProposalsFile<T> {
    version: u32,
    sets: Vec<T>,
}

#[derive(Serialize, Deserialize)]
struct SavedSet {
    set: ChangeSet,
    /// [`changeset::targets_hash`] of each item, on the committed project
    /// when saved.
    targets: Vec<u64>,
}

/// Keeps the open project's proposals file in step with its change sets
/// still waiting for review, so their review cards come back when it opens
/// again. Undo history isn't saved, so neither is what was accepted.
#[derive(Default)]
pub struct Proposals {
    /// A quick summary of the bus as last looked at.
    seen: Option<u64>,
    /// What the file was last brought to: the hash of what was written,
    /// [`NO_PROPOSALS`] once removed.
    written: Option<u64>,
}

const NO_PROPOSALS: u64 = 0;

impl Proposals {
    /// Write the file if the bus's change sets changed since the last
    /// call; remove it when none wait for review. A demo's are not kept.
    pub fn sync(&mut self, bus: &CommandBus) {
        if crate::demo::enabled() {
            return;
        }
        let mut h = flikk_core::subject::Fnv::default();
        h.write(bus.project().id.0.as_bytes());
        h.write(&bus.version().to_le_bytes());
        for set in bus.change_sets() {
            h.write(set.id.0.as_bytes());
            h.write(&[set.state as u8]);
            h.write(&set.base_version.to_le_bytes());
            for item in &set.items {
                h.write(item.id.0.as_bytes());
                h.write(&[item.state as u8]);
            }
        }
        if self.seen.replace(h.0) == Some(h.0) {
            return;
        }
        let project = bus.project();
        let path = proposals_path(project.id);
        let sets = bus.waiting_change_sets();
        if sets.is_empty() {
            if self.written != Some(NO_PROPOSALS) {
                let _ = std::fs::remove_file(&path);
                self.written = Some(NO_PROPOSALS);
            }
            return;
        }
        let file = ProposalsFile {
            version: PROPOSALS_VERSION,
            sets: sets
                .into_iter()
                .map(|set| SavedSet {
                    targets: set
                        .items
                        .iter()
                        .map(|i| changeset::targets_hash(project, &i.ops))
                        .collect(),
                    set,
                })
                .collect(),
        };
        let bytes = match serde_json::to_vec(&file) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("flikk: can't save proposals: {e}");
                return;
            }
        };
        let mut h = flikk_core::subject::Fnv::default();
        h.write(&bytes);
        if self.written == Some(h.0) {
            return;
        }
        match flikk_os::fs::write_durable(&path, &bytes) {
            Ok(()) => self.written = Some(h.0),
            Err(e) => notify(format!("Couldn't save the agent's proposals: {e}")),
        }
    }
}

/// The proposals saved for `project`. Items that changed something the
/// project now holds differently (it was edited elsewhere, or reopened
/// from an older save) are conflicted. Unreadable sets are skipped.
pub(super) fn load_proposals(project: &Project) -> Vec<ChangeSet> {
    let path = proposals_path(project.id);
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    let file = match serde_json::from_slice::<ProposalsFile<serde_json::Value>>(&bytes) {
        Ok(file) if file.version <= PROPOSALS_VERSION => file,
        Ok(file) => {
            eprintln!(
                "flikk: {} is from a newer version ({}); not read",
                path.display(),
                file.version
            );
            return Vec::new();
        }
        Err(e) => {
            eprintln!("flikk: can't read {}: {e}", path.display());
            return Vec::new();
        }
    };
    file.sets
        .into_iter()
        .filter_map(|value| match serde_json::from_value::<SavedSet>(value) {
            Ok(saved) => Some(saved),
            Err(e) => {
                eprintln!("flikk: skipped a proposal in {}: {e}", path.display());
                None
            }
        })
        .map(|SavedSet { mut set, targets }| {
            for (ix, item) in set.items.iter_mut().enumerate() {
                if item.state == ItemState::Pending
                    && targets.get(ix) != Some(&changeset::targets_hash(project, &item.ops))
                {
                    item.state = ItemState::Conflicted;
                }
            }
            set
        })
        .collect()
}
