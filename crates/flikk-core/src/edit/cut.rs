//! Blades and deletes: cutting time and clips out of tracks.

use super::diff::diff_track;
use super::ops;
use super::slice::{split_clip, trim_start};
use super::target::{editable_track, find_clip, scoped_tracks};
use crate::command::{Plan, TrackScope};
use crate::error::{CoreError, Result};
use crate::ids::*;
use crate::model::*;
use crate::ops::Op;
use crate::time::{Time, TimeRange};

pub(super) fn blade(tl: &Timeline, at: Time, scope: &TrackScope) -> Result<Plan> {
    let mut out = Vec::new();
    for track in scoped_tracks(tl, scope) {
        if let Some(clip) = track.clips.iter().find(|c| c.start < at && at < c.end()) {
            let (left, right) = split_clip(clip, at);
            out.push(Op::UpdateClip {
                track: track.id,
                before: Box::new(clip.clone()),
                after: Box::new(left),
            });
            out.push(Op::InsertClip {
                track: track.id,
                clip: Box::new(right),
            });
        }
    }
    Ok(ops(out))
}

pub(super) fn ripple_delete(
    tl: &Timeline,
    range: TimeRange,
    scope: &TrackScope,
    fade: Time,
) -> Result<Plan> {
    if range.is_empty() {
        return Err(CoreError::Invalid("empty range".into()));
    }
    let len = range.duration();
    let mut plan = Plan::default();
    for track in scoped_tracks(tl, scope) {
        let mut next = Vec::with_capacity(track.clips.len() + 1);
        for c in &track.clips {
            if c.end() <= range.start {
                next.push(c.clone());
            } else if c.start >= range.end {
                next.push(Clip {
                    start: c.start - len,
                    ..c.clone()
                });
            } else {
                // Audio either side of the join fades into it.
                let fades =
                    fade > Time::ZERO && track.kind == TrackKind::Audio && c.media().is_some();
                if c.start < range.start {
                    let (mut left, _) = split_clip(c, range.start);
                    if fades {
                        left.audio.fade_out = fade.min(left.duration);
                    }
                    next.push(Clip { id: c.id, ..left });
                }
                if c.end() > range.end {
                    let mut right = trim_start(c, range.end);
                    right.start = range.start;
                    if c.start < range.start {
                        right.id = ClipId::new();
                    }
                    if fades {
                        right.audio.fade_in = fade.min(right.duration);
                    }
                    next.push(right);
                }
            }
        }
        plan.ops.extend(diff_track(track, next));
    }
    for m in &tl.markers {
        let r = m.range;
        let after = if r.start >= range.end {
            Some(r.shift(-len))
        } else if r.start < range.start {
            Some(TimeRange::new(r.start, clamp_end(r.end, range)))
        } else if r.end > range.end {
            Some(TimeRange::new(range.start, r.end - len))
        } else {
            None
        };
        match after {
            Some(r) if r == m.range => {}
            Some(r) => plan.ops.push(Op::UpdateMarker {
                before: m.clone(),
                after: Marker {
                    range: r,
                    ..m.clone()
                },
            }),
            None => plan.ops.push(Op::RemoveMarker { marker: m.clone() }),
        }
    }
    Ok(plan)
}

/// End of a range that started before a deleted `range`, after the delete.
fn clamp_end(end: Time, range: TimeRange) -> Time {
    if end <= range.start {
        end
    } else if end <= range.end {
        range.start
    } else {
        end - range.duration()
    }
}

pub(super) fn delete_clips(tl: &Timeline, ids: &[ClipId], ripple: bool) -> Result<Plan> {
    let mut out = Vec::new();
    for track in &tl.tracks {
        if !track.clips.iter().any(|c| ids.contains(&c.id)) {
            continue;
        }
        editable_track(tl, track.id)?;
        let mut shift = Time::ZERO;
        let mut next = Vec::new();
        for c in &track.clips {
            if ids.contains(&c.id) {
                if ripple {
                    shift = shift + c.duration;
                }
            } else {
                next.push(Clip {
                    start: c.start - shift,
                    ..c.clone()
                });
            }
        }
        out.extend(diff_track(track, next));
    }
    for id in ids {
        find_clip(tl, *id)?;
    }
    Ok(ops(out))
}
