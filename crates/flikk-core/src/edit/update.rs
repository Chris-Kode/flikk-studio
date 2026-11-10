//! Plans that change one clip, track or range in place.

use super::ops;
use super::target::find_clip;
use crate::command::Plan;
use crate::error::{CoreError, Result};
use crate::ids::*;
use crate::model::*;
use crate::ops::{Op, TrackProps};
use crate::time::TimeRange;

pub(super) fn update_clip(tl: &Timeline, id: ClipId, f: impl FnOnce(&mut Clip)) -> Result<Plan> {
    let (track, before) = find_clip(tl, id)?;
    let mut after = before.clone();
    f(&mut after);
    Ok(ops(if after == *before {
        vec![]
    } else {
        vec![Op::UpdateClip {
            track: track.id,
            before: Box::new(before.clone()),
            after: Box::new(after),
        }]
    }))
}

pub(super) fn update_clips(tl: &Timeline, ids: &[ClipId], f: impl Fn(&mut Clip)) -> Result<Plan> {
    let mut out = Vec::new();
    for id in ids {
        out.extend(update_clip(tl, *id, &f)?.ops);
    }
    Ok(ops(out))
}

/// One [`Op::UpdateTrack`] that changes the header properties of `id` as
/// `f` does, even when that leaves them as they were.
pub(super) fn update_track(
    tl: &Timeline,
    id: TrackId,
    f: impl FnOnce(&mut TrackProps),
) -> Result<Plan> {
    let t = tl.track(id).ok_or(CoreError::TrackNotFound(id))?;
    let before = TrackProps::of(t);
    let mut after = before.clone();
    f(&mut after);
    Ok(ops(vec![Op::UpdateTrack {
        track: id,
        before,
        after,
    }]))
}

pub(super) fn insert_new(
    tl: &Timeline,
    track: TrackId,
    range: TimeRange,
    content: ClipContent,
) -> Result<Plan> {
    let t = tl.track(track).ok_or(CoreError::TrackNotFound(track))?;
    let clip = Clip::new(range.start, range.duration(), content);
    if range.is_empty() || t.clips.iter().any(|c| c.range().overlaps(&range)) {
        return Err(CoreError::Invalid(
            "the range is empty or already occupied".into(),
        ));
    }
    Ok(ops(vec![Op::InsertClip {
        track,
        clip: Box::new(clip),
    }]))
}

/// Every clip of `from` on an unlocked picture track showing `to` instead
/// (see [`crate::command::Command::SwapMedia`]).
pub(super) fn swap_media(project: &Project, from: MediaId, to: MediaId) -> Result<Plan> {
    let source = project.media(from).ok_or(CoreError::MediaNotFound(from))?;
    let version = project.media(to).ok_or(CoreError::MediaNotFound(to))?;
    if source.kind != version.kind {
        return Err(CoreError::Invalid(
            "a clip can only show media of the same kind".into(),
        ));
    }
    let mut out = Vec::new();
    let pictures = project
        .timeline
        .tracks
        .iter()
        .filter(|t| !t.locked && t.kind != TrackKind::Audio);
    for track in pictures {
        for clip in &track.clips {
            let mut after = clip.clone();
            let ClipContent::Media { media, .. } = &mut after.content else {
                continue;
            };
            if *media != from {
                continue;
            }
            *media = to;
            out.push(Op::UpdateClip {
                track: track.id,
                before: Box::new(clip.clone()),
                after: Box::new(after),
            });
        }
    }
    Ok(ops(out))
}
