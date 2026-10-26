//! Pending agent change sets drawn over the timeline (PRD AG4), from their
//! ops, at committed-timeline positions:
//! - inserted clips: green ghosts;
//! - removed material (whole clips, trimmed or rippled-out spans): a red band
//!   where it plays today, so ripple deletes read as red gaps;
//! - real modifications (grade, gain, transform, keyframes, text): a yellow
//!   outline;
//! - clips that only slide along (a ripple's knock-on shift): nothing;
//! - a blade that removes nothing: a thin cut line.
//!
//! Each item is planned against its set's draft, which already holds the
//! set's earlier items, so draft times are mapped back to committed time:
//! media through source time, other clips through their committed twin or
//! the set's earlier ripple deletes.

use koo_core::changeset::{ChangeItem, ChangeSet};
use koo_core::command::{Command, Warning};
use koo_core::model::{Clip, ClipContent, Timeline};
use koo_core::ops::Op;
use koo_core::{ChangeItemId, ChangeSetId, Time, TimeRange, TrackId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Added,
    Removed,
    Modified,
    /// A split with nothing removed: drawn as a line at `range.start`.
    Cut,
}

#[derive(Clone, Debug)]
pub struct OverlayItem {
    pub set: ChangeSetId,
    pub item: ChangeItemId,
    pub mark: Mark,
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
        // Ripple-delete ranges of the set's earlier items, in draft time.
        let mut cuts: Vec<TimeRange> = Vec::new();
        for item in set.pending_items() {
            let marks = item_marks(timeline, item, &cuts);
            let warnings: Vec<String> = item.warnings.iter().map(warning_line).collect();
            for (mark, track, range) in marks {
                if track.is_some_and(|t| timeline.track(t).is_none()) {
                    continue;
                }
                let overlaps = mark == Mark::Added
                    && track
                        .and_then(|t| timeline.track(t))
                        .is_some_and(|t| t.clips.iter().any(|c| c.range().overlaps(&range)));
                out.push(OverlayItem {
                    set: set.id,
                    item: item.id,
                    mark,
                    track,
                    range,
                    overlaps,
                    reason: item.reason.clone(),
                    warnings: warnings.clone(),
                });
            }
            if let Command::RippleDelete { range, .. } = &item.command {
                cuts.push(*range);
            }
        }
    }
    out
}

type Marked = (Mark, Option<TrackId>, TimeRange);

fn item_marks(timeline: &Timeline, item: &ChangeItem, cuts: &[TimeRange]) -> Vec<Marked> {
    let to_committed = |t: Time| {
        cuts.iter()
            .rev()
            .fold(t, |t, c| if t >= c.start { t + c.duration() } else { t })
    };
    let map_range = |r: TimeRange| {
        let start = to_committed(r.start);
        TimeRange::new(start, start + r.duration())
    };
    let ripple = match &item.command {
        Command::RippleDelete { range, .. } => Some(*range),
        _ => None,
    };
    // Clips inserted by this item that are pieces of a clip it also changed
    // (the right half of a split) are not new material.
    let pieces: Vec<(&TrackId, &Clip)> = item
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::InsertClip { track, clip } => Some((track, clip)),
            _ => None,
        })
        .filter(|(track, clip)| {
            item.ops.iter().any(|op| match op {
                Op::UpdateClip {
                    track: t, before, ..
                } => t == *track && is_piece(clip, before),
                _ => false,
            })
        })
        .collect();

    let mut out = Vec::new();
    for op in &item.ops {
        match op {
            Op::InsertClip { track, clip } => {
                if !pieces.iter().any(|(_, p)| p.id == clip.id) {
                    out.push((Mark::Added, Some(*track), map_range(clip.range())));
                }
            }
            Op::RemoveClip { track, clip } => {
                let range = committed_span(timeline, *track, clip, clip.range(), &map_range);
                out.push((Mark::Removed, Some(*track), range));
            }
            Op::UpdateClip {
                track,
                before,
                after,
            } => {
                let place = || match timeline.clip(after.id) {
                    Some((_, c)) => c.range(),
                    None => map_range(after.range()),
                };
                if !same_look(before, after) {
                    out.push((Mark::Modified, Some(*track), place()));
                    continue;
                }
                if is_shift(before, after) {
                    continue;
                }
                let kept: Vec<&Clip> = std::iter::once(after)
                    .chain(
                        pieces
                            .iter()
                            .filter(|(t, p)| *t == track && is_piece(p, before))
                            .map(|(_, p)| *p),
                    )
                    .collect();
                let removed = removed_spans(before, &kept, ripple);
                if removed.is_empty() {
                    // A split: mark where the first piece ends.
                    let end = TimeRange::new(after.end(), after.end());
                    let at = committed_span(timeline, *track, before, end, &map_range);
                    out.push((Mark::Cut, Some(*track), at));
                }
                for span in removed {
                    let range = committed_span(timeline, *track, before, span, &map_range);
                    out.push((Mark::Removed, Some(*track), range));
                }
            }
            Op::InsertMarker { marker } => out.push((Mark::Added, None, map_range(marker.range))),
            Op::RemoveMarker { marker } => {
                let range = timeline
                    .marker(marker.id)
                    .map(|m| m.range)
                    .unwrap_or_else(|| map_range(marker.range));
                out.push((Mark::Removed, None, range));
            }
            Op::UpdateMarker { before, after } => {
                let moved_only = before.kind == after.kind
                    && before.note == after.note
                    && before.range.duration() == after.range.duration();
                if !moved_only {
                    let range = timeline
                        .marker(after.id)
                        .map(|m| m.range)
                        .unwrap_or_else(|| map_range(after.range));
                    out.push((Mark::Modified, None, range));
                }
            }
            _ => {}
        }
    }
    out
}

/// Everything but position and length is unchanged.
fn same_look(a: &Clip, b: &Clip) -> bool {
    a.transform == b.transform
        && a.keyframes == b.keyframes
        && a.grade == b.grade
        && a.audio == b.audio
        && match (&a.content, &b.content) {
            (
                ClipContent::Media {
                    media: m1,
                    audio_stream: s1,
                    ..
                },
                ClipContent::Media {
                    media: m2,
                    audio_stream: s2,
                    ..
                },
            ) => m1 == m2 && s1 == s2,
            (x, y) => x == y,
        }
}

/// Only `start` moved.
fn is_shift(a: &Clip, b: &Clip) -> bool {
    a.duration == b.duration && source_in(a) == source_in(b)
}

fn source_in(c: &Clip) -> Option<Time> {
    match &c.content {
        ClipContent::Media { source_in, .. } => Some(*source_in),
        _ => None,
    }
}

/// `piece` is a new clip cut from `whole`: same material, shorter, and for
/// media inside the same source range.
fn is_piece(piece: &Clip, whole: &Clip) -> bool {
    piece.id != whole.id
        && piece.duration < whole.duration
        && same_look(piece, whole)
        && match (piece.source_range(), whole.source_range()) {
            (Some(p), Some(w)) => p.start >= w.start && p.end <= w.end,
            _ => true,
        }
}

/// Spans of `before` that no kept clip plays any more, in `before`'s own
/// draft timeline time. Media is compared in source time, so shifted pieces
/// still line up; other clips use the ripple range when there is one.
fn removed_spans(before: &Clip, kept: &[&Clip], ripple: Option<TimeRange>) -> Vec<TimeRange> {
    if let Some(whole) = before.source_range() {
        let kept: Vec<TimeRange> = kept.iter().filter_map(|c| c.source_range()).collect();
        let offset = before.start - whole.start;
        return subtract(whole, &kept)
            .into_iter()
            .map(|r| r.shift(offset))
            .collect();
    }
    if let Some(r) = ripple {
        return before.range().intersect(&r).into_iter().collect();
    }
    let kept: Vec<TimeRange> = kept.iter().map(|c| c.range()).collect();
    subtract(before.range(), &kept)
}

fn subtract(whole: TimeRange, kept: &[TimeRange]) -> Vec<TimeRange> {
    let mut out = vec![whole];
    for k in kept {
        out = out
            .into_iter()
            .flat_map(|r| {
                if k.end <= r.start || k.start >= r.end {
                    return vec![r];
                }
                [
                    TimeRange::new(r.start, k.start),
                    TimeRange::new(k.end, r.end),
                ]
                .into_iter()
                .filter(|p| p.start < p.end)
                .collect()
            })
            .collect();
    }
    out
}

/// Committed timeline position of `span`, a part of `clip` in draft time.
/// Media goes through source time and the committed clip playing it (the
/// same clip when it still exists); other clips through their committed twin;
/// failing both, through the set's earlier ripple deletes.
fn committed_span(
    timeline: &Timeline,
    track: TrackId,
    clip: &Clip,
    span: TimeRange,
    map_range: &dyn Fn(TimeRange) -> TimeRange,
) -> TimeRange {
    let source = clip.to_source(span.start);
    if let (Some(media), Some(src)) = (clip.media(), source) {
        let candidates = timeline.track(track).into_iter().flat_map(|t| &t.clips);
        let mut best: Option<&Clip> = None;
        for c in candidates.filter(|c| c.media() == Some(media)) {
            let Some(used) = c.source_range() else {
                continue;
            };
            if !used.contains(src) {
                continue;
            }
            if c.id == clip.id || best.is_none() {
                best = Some(c);
            }
        }
        if let Some(c) = best
            && let Some(start) = c.to_timeline(src)
        {
            return TimeRange::new(start, start + span.duration());
        }
    }
    if let Some((_, twin)) = timeline.clip(clip.id) {
        let offset = twin.start - clip.start;
        return span.shift(offset);
    }
    map_range(span)
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
