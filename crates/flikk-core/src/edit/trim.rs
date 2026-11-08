//! Edge edits: trim, roll, slip and slide, held to the source media.

use super::bounds::{clamp, lower, min_len, room_after, room_before, upper};
use super::diff::diff_track;
use super::ops;
use super::slice::{replace_shifting, trim_start, with_duration};
use super::target::{clip_index, editable_clip, editable_track, find_clip};
use super::update::update_clip;
use crate::command::{Edge, Plan};
use crate::error::{CoreError, Result};
use crate::ids::*;
use crate::model::*;
use crate::time::Time;

pub(super) fn trim(
    project: &Project,
    id: ClipId,
    edge: Edge,
    delta: Time,
    ripple: bool,
) -> Result<Plan> {
    let tl = &project.timeline;
    let (track, clip) = editable_clip(tl, id)?;
    let ix = clip_index(track, id);
    let prev = ix.checked_sub(1).map(|i| &track.clips[i]);
    let next = track.clips.get(ix + 1);
    let min = min_len(tl);
    // Positive delta moves the edge right: a start trim shortens, an end trim
    // lengthens.
    let (lo, hi) = match edge {
        Edge::Start => {
            let mut lo = room_before(clip).map(|r| -r);
            if !ripple {
                let floor = prev.map_or(Time::ZERO, |p| p.end());
                lo = lower(lo, Some(floor - clip.start));
            }
            (lo, Some(clip.duration - min))
        }
        Edge::End => {
            let mut hi = room_after(project, clip);
            if !ripple {
                hi = upper(hi, next.map(|n| n.start - clip.end()));
            }
            (Some(min - clip.duration), hi)
        }
    };
    let delta = clamp(delta, lo, hi);
    if delta == Time::ZERO {
        return Ok(Plan::default());
    }
    let trimmed = match edge {
        Edge::Start => {
            let mut c = trim_start(clip, clip.start + delta);
            if ripple {
                c.start = clip.start;
            }
            c
        }
        Edge::End => with_duration(clip, clip.duration + delta),
    };
    let shift = match (ripple, edge) {
        (false, _) => Time::ZERO,
        (true, Edge::Start) => -delta,
        (true, Edge::End) => delta,
    };
    Ok(ops(diff_track(
        track,
        replace_shifting(track, ix, trimmed, shift),
    )))
}

pub(super) fn roll(project: &Project, left: ClipId, right: ClipId, delta: Time) -> Result<Plan> {
    let tl = &project.timeline;
    let (track, l) = find_clip(tl, left)?;
    let (right_track, r) = find_clip(tl, right)?;
    if track.id != right_track.id || l.end() != r.start {
        return Err(CoreError::Invalid(
            "a roll needs two adjacent clips on one track".into(),
        ));
    }
    editable_track(tl, track.id)?;
    let min = min_len(tl);
    let lo = lower(Some(min - l.duration), room_before(r).map(|x| -x));
    let hi = upper(Some(r.duration - min), room_after(project, l));
    let delta = clamp(delta, lo, hi);
    if delta == Time::ZERO {
        return Ok(Plan::default());
    }
    let next = track
        .clips
        .iter()
        .map(|c| {
            if c.id == l.id {
                with_duration(l, l.duration + delta)
            } else if c.id == r.id {
                trim_start(r, r.start + delta)
            } else {
                c.clone()
            }
        })
        .collect();
    Ok(ops(diff_track(track, next)))
}

pub(super) fn slip(project: &Project, id: ClipId, delta: Time) -> Result<Plan> {
    let tl = &project.timeline;
    let (_, clip) = editable_clip(tl, id)?;
    let Some(before) = room_before(clip) else {
        return Err(CoreError::Invalid("only media clips can slip".into()));
    };
    let delta = clamp(delta, Some(-before), room_after(project, clip));
    // `delta` is timeline time; the source moves by that much at the clip's speed.
    update_clip(tl, id, |c| {
        if let ClipContent::Media { source_in, .. } = &mut c.content {
            *source_in = *source_in + delta.scale(c.speed);
        }
    })
}

pub(super) fn slide(project: &Project, id: ClipId, delta: Time) -> Result<Plan> {
    let tl = &project.timeline;
    let (track, clip) = editable_clip(tl, id)?;
    let ix = clip_index(track, id);
    let min = min_len(tl);
    // Adjacent neighbours absorb the move; otherwise the gap does.
    let prev = ix
        .checked_sub(1)
        .map(|i| &track.clips[i])
        .filter(|p| p.end() == clip.start);
    let next = track.clips.get(ix + 1).filter(|n| n.start == clip.end());
    let mut lo = Some(match prev {
        Some(p) => min - p.duration,
        None => {
            let floor = ix
                .checked_sub(1)
                .map_or(Time::ZERO, |i| track.clips[i].end());
            floor - clip.start
        }
    });
    let mut hi = match next {
        Some(n) => Some(n.duration - min),
        None => track.clips.get(ix + 1).map(|n| n.start - clip.end()),
    };
    if let Some(n) = next {
        lo = lower(lo, room_before(n).map(|r| -r));
    }
    if let Some(p) = prev {
        hi = upper(hi, room_after(project, p));
    }
    let delta = clamp(delta, lo, hi);
    if delta == Time::ZERO {
        return Ok(Plan::default());
    }
    let clips = track
        .clips
        .iter()
        .map(|c| {
            if c.id == clip.id {
                Clip {
                    start: c.start + delta,
                    ..c.clone()
                }
            } else if prev.is_some_and(|p| p.id == c.id) {
                with_duration(c, c.duration + delta)
            } else if next.is_some_and(|n| n.id == c.id) {
                trim_start(c, c.start + delta)
            } else {
                c.clone()
            }
        })
        .collect();
    Ok(ops(diff_track(track, clips)))
}
