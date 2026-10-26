//! Invertible primitive edits.
//!
//! Every [`crate::command::Command`] plans into a list of `Op`s. Each op
//! carries the value it expects to replace, so:
//! - undo is `invert()` applied in reverse order;
//! - applying an agent's op after the user edited the same thing fails with
//!   [`CoreError::Conflict`] instead of silently reverting the hand edit
//!   (PRD AG6);
//! - the timeline draws a pending change set straight from its ops
//!   (insert = added, remove = removed, update = modified).

use crate::error::{CoreError, Result};
use crate::ids::*;
use crate::model::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackProps {
    pub name: String,
    pub mix: TrackMix,
    pub locked: bool,
    pub caption_style: Option<String>,
}

impl TrackProps {
    pub fn of(track: &Track) -> Self {
        Self {
            name: track.name.clone(),
            mix: track.mix.clone(),
            locked: track.locked,
            caption_style: track.caption_style.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    InsertClip {
        track: TrackId,
        clip: Clip,
    },
    RemoveClip {
        track: TrackId,
        clip: Clip,
    },
    UpdateClip {
        track: TrackId,
        before: Clip,
        after: Clip,
    },
    InsertMarker {
        marker: Marker,
    },
    RemoveMarker {
        marker: Marker,
    },
    UpdateMarker {
        before: Marker,
        after: Marker,
    },
    InsertTrack {
        index: usize,
        track: Track,
    },
    RemoveTrack {
        index: usize,
        track: Track,
    },
    UpdateTrack {
        track: TrackId,
        before: TrackProps,
        after: TrackProps,
    },
    SetDucking {
        before: Option<Ducking>,
        after: Option<Ducking>,
    },
    SetTimelineGrade {
        before: Grade,
        after: Grade,
    },
    AddMedia {
        media: MediaItem,
    },
    RemoveMedia {
        media: MediaItem,
    },
    UpdateMedia {
        before: MediaItem,
        after: MediaItem,
    },
    SetBins {
        before: Vec<Bin>,
        after: Vec<Bin>,
    },
}

/// How an op shows on the timeline while pending.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffKind {
    Added,
    Removed,
    Modified,
}

impl Op {
    pub fn diff_kind(&self) -> DiffKind {
        match self {
            Op::InsertClip { .. }
            | Op::InsertMarker { .. }
            | Op::InsertTrack { .. }
            | Op::AddMedia { .. } => DiffKind::Added,
            Op::RemoveClip { .. }
            | Op::RemoveMarker { .. }
            | Op::RemoveTrack { .. }
            | Op::RemoveMedia { .. } => DiffKind::Removed,
            _ => DiffKind::Modified,
        }
    }

    /// The clip this op touches, for drawing and conflict display.
    pub fn clip(&self) -> Option<ClipId> {
        match self {
            Op::InsertClip { clip, .. } | Op::RemoveClip { clip, .. } => Some(clip.id),
            Op::UpdateClip { after, .. } => Some(after.id),
            _ => None,
        }
    }

    pub fn invert(&self) -> Op {
        match self.clone() {
            Op::InsertClip { track, clip } => Op::RemoveClip { track, clip },
            Op::RemoveClip { track, clip } => Op::InsertClip { track, clip },
            Op::UpdateClip {
                track,
                before,
                after,
            } => Op::UpdateClip {
                track,
                before: after,
                after: before,
            },
            Op::InsertMarker { marker } => Op::RemoveMarker { marker },
            Op::RemoveMarker { marker } => Op::InsertMarker { marker },
            Op::UpdateMarker { before, after } => Op::UpdateMarker {
                before: after,
                after: before,
            },
            Op::InsertTrack { index, track } => Op::RemoveTrack { index, track },
            Op::RemoveTrack { index, track } => Op::InsertTrack { index, track },
            Op::UpdateTrack {
                track,
                before,
                after,
            } => Op::UpdateTrack {
                track,
                before: after,
                after: before,
            },
            Op::SetDucking { before, after } => Op::SetDucking {
                before: after,
                after: before,
            },
            Op::SetTimelineGrade { before, after } => Op::SetTimelineGrade {
                before: after,
                after: before,
            },
            Op::AddMedia { media } => Op::RemoveMedia { media },
            Op::RemoveMedia { media } => Op::AddMedia { media },
            Op::UpdateMedia { before, after } => Op::UpdateMedia {
                before: after,
                after: before,
            },
            Op::SetBins { before, after } => Op::SetBins {
                before: after,
                after: before,
            },
        }
    }

    /// Apply to `project`, failing with [`CoreError::Conflict`] if the project
    /// no longer holds the value this op expects. On error nothing changed.
    pub fn apply(&self, project: &mut Project) -> Result<()> {
        let timeline = &mut project.timeline;
        match self {
            Op::InsertClip { track, clip } => {
                let t = track_mut(timeline, *track)?;
                if t.clips
                    .iter()
                    .any(|c| c.id == clip.id || c.range().overlaps(&clip.range()))
                {
                    return Err(CoreError::Conflict);
                }
                let at = t.clips.partition_point(|c| c.start < clip.start);
                t.clips.insert(at, clip.clone());
            }
            Op::RemoveClip { track, clip } => {
                let t = track_mut(timeline, *track)?;
                let ix = t
                    .clips
                    .iter()
                    .position(|c| c == clip)
                    .ok_or(CoreError::Conflict)?;
                t.clips.remove(ix);
            }
            Op::UpdateClip {
                track,
                before,
                after,
            } => {
                let t = track_mut(timeline, *track)?;
                let ix = t
                    .clips
                    .iter()
                    .position(|c| c == before)
                    .ok_or(CoreError::Conflict)?;
                if t.clips
                    .iter()
                    .enumerate()
                    .any(|(i, c)| i != ix && c.range().overlaps(&after.range()))
                {
                    return Err(CoreError::Conflict);
                }
                t.clips[ix] = after.clone();
                t.clips.sort_by_key(|c| c.start);
            }
            Op::InsertMarker { marker } => {
                if timeline.marker(marker.id).is_some() {
                    return Err(CoreError::Conflict);
                }
                timeline.markers.push(marker.clone());
                timeline.markers.sort_by_key(|m| m.range.start);
            }
            Op::RemoveMarker { marker } => {
                let ix = timeline
                    .markers
                    .iter()
                    .position(|m| m == marker)
                    .ok_or(CoreError::Conflict)?;
                timeline.markers.remove(ix);
            }
            Op::UpdateMarker { before, after } => {
                let m = timeline
                    .markers
                    .iter_mut()
                    .find(|m| *m == before)
                    .ok_or(CoreError::Conflict)?;
                *m = after.clone();
                timeline.markers.sort_by_key(|m| m.range.start);
            }
            Op::InsertTrack { index, track } => {
                if timeline.track(track.id).is_some() || *index > timeline.tracks.len() {
                    return Err(CoreError::Conflict);
                }
                timeline.tracks.insert(*index, track.clone());
            }
            Op::RemoveTrack { index, track } => {
                if timeline.tracks.get(*index) != Some(track) {
                    return Err(CoreError::Conflict);
                }
                timeline.tracks.remove(*index);
            }
            Op::UpdateTrack {
                track,
                before,
                after,
            } => {
                let t = track_mut(timeline, *track)?;
                if TrackProps::of(t) != *before {
                    return Err(CoreError::Conflict);
                }
                t.name = after.name.clone();
                t.mix = after.mix.clone();
                t.locked = after.locked;
                t.caption_style = after.caption_style.clone();
            }
            Op::SetDucking { before, after } => {
                if timeline.ducking != *before {
                    return Err(CoreError::Conflict);
                }
                timeline.ducking = after.clone();
            }
            Op::SetTimelineGrade { before, after } => {
                if timeline.grade != *before {
                    return Err(CoreError::Conflict);
                }
                timeline.grade = after.clone();
            }
            Op::AddMedia { media } => {
                if project.media(media.id).is_some() {
                    return Err(CoreError::Conflict);
                }
                project.media.push(media.clone());
            }
            Op::RemoveMedia { media } => {
                let ix = project
                    .media
                    .iter()
                    .position(|m| m == media)
                    .ok_or(CoreError::Conflict)?;
                project.media.remove(ix);
            }
            Op::UpdateMedia { before, after } => {
                let m = project
                    .media
                    .iter_mut()
                    .find(|m| *m == before)
                    .ok_or(CoreError::Conflict)?;
                *m = after.clone();
            }
            Op::SetBins { before, after } => {
                if project.bins != *before {
                    return Err(CoreError::Conflict);
                }
                project.bins = after.clone();
            }
        }
        Ok(())
    }
}

fn track_mut(timeline: &mut Timeline, id: TrackId) -> Result<&mut Track> {
    timeline
        .tracks
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or(CoreError::TrackNotFound(id))
}

/// Apply ops in order; on failure roll back the ones already applied, so the
/// project is either fully changed or untouched.
pub fn apply_all(project: &mut Project, ops: &[Op]) -> Result<()> {
    for (i, op) in ops.iter().enumerate() {
        if let Err(e) = op.apply(project) {
            for done in ops[..i].iter().rev() {
                done.invert()
                    .apply(project)
                    .expect("inverse of an applied op always applies");
            }
            return Err(e);
        }
    }
    Ok(())
}

/// The ops that undo `ops`.
pub fn invert_all(ops: &[Op]) -> Vec<Op> {
    ops.iter().rev().map(Op::invert).collect()
}
