//! Pending agent change sets drawn over the timeline (PRD AG4), straight from
//! their ops: insertions as green ghosts, removals as a red tint, updates as a
//! yellow outline.

use koo_core::changeset::ChangeSet;
use koo_core::command::Warning;
use koo_core::model::Timeline;
use koo_core::ops::{DiffKind, Op};
use koo_core::{ChangeItemId, ChangeSetId, TimeRange, TrackId};

#[derive(Clone, Debug)]
pub struct OverlayItem {
    pub set: ChangeSetId,
    pub item: ChangeItemId,
    pub kind: DiffKind,
    /// `None` for marker ops, drawn on the ruler.
    pub track: Option<TrackId>,
    pub range: TimeRange,
    /// An added clip lands on top of committed clips: drawn offset and striped.
    pub overlaps: bool,
    pub reason: String,
    pub warnings: Vec<String>,
}

pub fn collect(timeline: &Timeline, sets: &[ChangeSet]) -> Vec<OverlayItem> {
    let mut out = Vec::new();
    for set in sets {
        for (item, op) in set.pending_ops() {
            let warnings: Vec<String> = item.warnings.iter().map(warning_line).collect();
            let (track, range) = match op {
                Op::InsertClip { track, clip } | Op::RemoveClip { track, clip } => {
                    (Some(*track), clip.range())
                }
                Op::UpdateClip { track, after, .. } => (Some(*track), after.range()),
                Op::InsertMarker { marker } | Op::RemoveMarker { marker } => (None, marker.range),
                Op::UpdateMarker { after, .. } => (None, after.range),
                _ => continue,
            };
            if track.is_some_and(|t| timeline.track(t).is_none()) {
                continue;
            }
            let kind = op.diff_kind();
            let overlaps = kind == DiffKind::Added
                && track
                    .and_then(|t| timeline.track(t))
                    .is_some_and(|t| t.clips.iter().any(|c| c.range().overlaps(&range)));
            out.push(OverlayItem {
                set: set.id,
                item: item.id,
                kind,
                track,
                range,
                overlaps,
                reason: item.reason.clone(),
                warnings,
            });
        }
    }
    out
}

fn warning_line(warning: &Warning) -> String {
    match warning {
        Warning::TouchesProtected { note, .. } if note.is_empty() => {
            "Touches a protected range".into()
        }
        Warning::TouchesProtected { note, .. } => format!("Touches protected range “{note}”"),
        Warning::NeedsConfirmation { message } => message.clone(),
    }
}
