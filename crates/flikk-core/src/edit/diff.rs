//! Ops that turn a track into its planned end state.

use crate::ids::*;
use crate::model::*;
use crate::ops::Op;
use std::collections::HashMap;

/// Ops that turn `track.clips` into `next`, ordered so no intermediate state
/// overlaps: removals, shrinks, left moves (ascending), right moves
/// (descending), growths, then insertions.
pub(crate) fn diff_track(track: &Track, mut next: Vec<Clip>) -> Vec<Op> {
    next.sort_by_key(|c| c.start);
    let old: HashMap<ClipId, &Clip> = track.clips.iter().map(|c| (c.id, c)).collect();
    let new_ids: Vec<ClipId> = next.iter().map(|c| c.id).collect();
    let id = track.id;

    let mut out: Vec<Op> = track
        .clips
        .iter()
        .filter(|c| !new_ids.contains(&c.id))
        .map(|c| Op::RemoveClip {
            track: id,
            clip: Box::new(c.clone()),
        })
        .collect();

    let (mut shrinks, mut lefts, mut rights, mut grows) = (vec![], vec![], vec![], vec![]);
    for after in next.iter().filter(|c| old.contains_key(&c.id)) {
        let before = old[&after.id];
        if before == after {
            continue;
        }
        let op = Op::UpdateClip {
            track: id,
            before: Box::new(before.clone()),
            after: Box::new(after.clone()),
        };
        let (b, a) = (before.range(), after.range());
        if a.start >= b.start && a.end <= b.end {
            shrinks.push(op);
        } else if a.start < b.start && a.end <= b.end {
            lefts.push((a.start, op));
        } else if a.start > b.start && a.end > b.end {
            rights.push((a.start, op));
        } else {
            grows.push(op);
        }
    }
    lefts.sort_by_key(|(t, _)| *t);
    rights.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    out.extend(shrinks);
    out.extend(lefts.into_iter().map(|(_, op)| op));
    out.extend(rights.into_iter().map(|(_, op)| op));
    out.extend(grows);
    out.extend(
        next.into_iter()
            .filter(|c| !old.contains_key(&c.id))
            .map(|clip| Op::InsertClip {
                track: id,
                clip: Box::new(clip),
            }),
    );
    out
}
