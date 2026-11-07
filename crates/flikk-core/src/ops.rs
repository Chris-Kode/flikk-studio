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
    // Clips are boxed: they are large, and history holds many ops.
    InsertClip {
        track: TrackId,
        clip: Box<Clip>,
    },
    RemoveClip {
        track: TrackId,
        clip: Box<Clip>,
    },
    UpdateClip {
        track: TrackId,
        before: Box<Clip>,
        after: Box<Clip>,
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
    SetSequence {
        before: SequenceSettings,
        after: SequenceSettings,
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
    /// The project's FX programs ([`Project::fx`]).
    SetFxAssets {
        before: Vec<crate::fx::FxAsset>,
        after: Vec<crate::fx::FxAsset>,
    },
}

/// Something an op changes, for telling when a proposal meets an edit the
/// user made after it was planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Clip(ClipId),
    /// A track's own properties, or the track itself.
    Track(TrackId),
    /// Which clips a track holds.
    TrackClips(TrackId),
    Marker(MarkerId),
    Media(MediaId),
    Fx(crate::fx::FxRef),
    Bins,
    Ducking,
    Grade,
    Sequence,
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

    /// The track whose clips this op adds, removes or changes.
    pub fn clip_track(&self) -> Option<TrackId> {
        match self {
            Op::InsertClip { track, .. }
            | Op::RemoveClip { track, .. }
            | Op::UpdateClip { track, .. } => Some(*track),
            _ => None,
        }
    }

    /// What this op changes. A removed track takes its clips with it.
    pub fn targets(&self) -> Vec<Target> {
        match self {
            Op::InsertClip { clip, .. } | Op::RemoveClip { clip, .. } => {
                vec![Target::Clip(clip.id)]
            }
            Op::UpdateClip { after, .. } => vec![Target::Clip(after.id)],
            Op::InsertMarker { marker } | Op::RemoveMarker { marker } => {
                vec![Target::Marker(marker.id)]
            }
            Op::UpdateMarker { after, .. } => vec![Target::Marker(after.id)],
            Op::InsertTrack { track, .. } => {
                vec![Target::Track(track.id), Target::TrackClips(track.id)]
            }
            Op::RemoveTrack { track, .. } => {
                [Target::Track(track.id), Target::TrackClips(track.id)]
                    .into_iter()
                    .chain(track.clips.iter().map(|c| Target::Clip(c.id)))
                    .collect()
            }
            Op::UpdateTrack { track, .. } => vec![Target::Track(*track)],
            Op::SetDucking { .. } => vec![Target::Ducking],
            Op::SetTimelineGrade { .. } => vec![Target::Grade],
            Op::SetSequence { .. } => vec![Target::Sequence],
            Op::AddMedia { media } | Op::RemoveMedia { media } => vec![Target::Media(media.id)],
            Op::UpdateMedia { after, .. } => vec![Target::Media(after.id)],
            // Bins are set as a whole list, so any change meets any other.
            Op::SetBins { .. } => vec![Target::Bins],
            Op::SetFxAssets { before, after } => {
                let find = |list: &[crate::fx::FxAsset], id: &crate::fx::FxRef| {
                    list.iter().find(|a| a.id == *id).cloned()
                };
                let mut ids: Vec<_> = before.iter().chain(after).map(|a| a.id.clone()).collect();
                ids.sort();
                ids.dedup();
                ids.into_iter()
                    .filter(|id| find(before, id) != find(after, id))
                    .map(Target::Fx)
                    .collect()
            }
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
            Op::SetSequence { before, after } => Op::SetSequence {
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
            Op::SetFxAssets { before, after } => Op::SetFxAssets {
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
                valid_clip(clip)?;
                let t = track_mut(timeline, *track)?;
                if t.clips
                    .iter()
                    .any(|c| c.id == clip.id || c.range().overlaps(&clip.range()))
                {
                    return Err(CoreError::Conflict);
                }
                let at = t.clips.partition_point(|c| c.start < clip.start);
                t.clips.insert(at, (**clip).clone());
            }
            Op::RemoveClip { track, clip } => {
                let t = track_mut(timeline, *track)?;
                let ix = t
                    .clips
                    .iter()
                    .position(|c| *c == **clip)
                    .ok_or(CoreError::Conflict)?;
                t.clips.remove(ix);
            }
            Op::UpdateClip {
                track,
                before,
                after,
            } => {
                valid_clip(after)?;
                let t = track_mut(timeline, *track)?;
                let ix = t
                    .clips
                    .iter()
                    .position(|c| *c == **before)
                    .ok_or(CoreError::Conflict)?;
                if t.clips
                    .iter()
                    .enumerate()
                    .any(|(i, c)| i != ix && c.range().overlaps(&after.range()))
                {
                    return Err(CoreError::Conflict);
                }
                t.clips[ix] = (**after).clone();
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
            Op::SetSequence { before, after } => {
                if timeline.settings != *before {
                    return Err(CoreError::Conflict);
                }
                timeline.settings = after.clone();
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
            Op::SetFxAssets { before, after } => {
                if project.fx != *before {
                    return Err(CoreError::Conflict);
                }
                project.fx = after.clone();
            }
        }
        Ok(())
    }
}

/// A clip the timeline can hold: one with a length and a speed. A clip of
/// no length would pass every overlap check.
fn valid_clip(clip: &Clip) -> Result<()> {
    if clip.duration <= crate::time::Time::ZERO {
        return Err(CoreError::Invalid("a clip needs a duration".into()));
    }
    if !(clip.speed.is_finite() && clip.speed > 0.0) {
        return Err(CoreError::Invalid("a clip needs a speed above zero".into()));
    }
    Ok(())
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
///
/// The inverse of an applied op applies to any project that keeps the
/// timeline's invariants ([`Project::normalize`]). Should one not, the
/// rollback goes on with the rest and the error is returned, the project
/// as far as it got: a broken invariant must not take the app down.
pub fn apply_all(project: &mut Project, ops: &[Op]) -> Result<()> {
    for (i, op) in ops.iter().enumerate() {
        if let Err(e) = op.apply(project) {
            for done in ops[..i].iter().rev() {
                if let Err(undo) = done.invert().apply(project) {
                    eprintln!(
                        "flikk-core: rollback of {:?} failed: {undo}",
                        done.targets()
                    );
                }
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
