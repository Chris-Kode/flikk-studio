//! Undo/redo. One entry per user command, batch or accepted change set
//! (PRD AG5: an accepted set is one undo step).

use crate::bus::Author;
use crate::ids::ChangeSetId;
use crate::ops::Op;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub label: String,
    pub author: Author,
    pub ops: Vec<Op>,
    pub change_set: Option<ChangeSetId>,
    /// Bus version after this entry was applied.
    pub version: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct History {
    pub undo: Vec<HistoryEntry>,
    pub redo: Vec<HistoryEntry>,
}

impl History {
    pub fn push(&mut self, entry: HistoryEntry) {
        self.undo.push(entry);
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
