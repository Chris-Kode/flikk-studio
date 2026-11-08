//! Which track and clip a command acts on, and whether a lock keeps it.

use crate::command::TrackScope;
use crate::error::{CoreError, Result};
use crate::ids::*;
use crate::model::*;

pub(super) fn find_clip(tl: &Timeline, id: ClipId) -> Result<(&Track, &Clip)> {
    tl.clip(id).ok_or(CoreError::ClipNotFound(id))
}

pub(super) fn scoped_tracks<'a>(
    tl: &'a Timeline,
    scope: &'a TrackScope,
) -> impl Iterator<Item = &'a Track> {
    tl.tracks
        .iter()
        .filter(move |t| !t.locked && scope.as_ref().is_none_or(|ids| ids.contains(&t.id)))
}

pub(super) fn editable_track(tl: &Timeline, id: TrackId) -> Result<&Track> {
    let track = tl.track(id).ok_or(CoreError::TrackNotFound(id))?;
    if track.locked {
        return Err(locked(track));
    }
    Ok(track)
}

/// [`find_clip`], refusing a clip on a locked track.
pub(super) fn editable_clip(tl: &Timeline, id: ClipId) -> Result<(&Track, &Clip)> {
    let (track, clip) = find_clip(tl, id)?;
    if track.locked {
        return Err(locked(track));
    }
    Ok((track, clip))
}

pub(super) fn locked(track: &Track) -> CoreError {
    CoreError::Invalid(format!("track {} is locked", track.name))
}

pub(super) fn clip_index(track: &Track, id: ClipId) -> usize {
    track.clips.iter().position(|c| c.id == id).unwrap_or(0)
}
