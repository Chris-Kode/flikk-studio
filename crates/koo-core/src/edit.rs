//! Planners: [`Command`] → [`Plan`] against a read-only project.
//!
//! Planners never mutate. They compute the desired end state of each touched
//! track and let [`diff_track`] emit ops in an order that never overlaps
//! clips mid-way. The bus validates every plan by applying it to a copy.

use crate::command::{Command, Edge, Plan, TrackScope, Warning};
use crate::error::{CoreError, Result};
use crate::ids::*;
use crate::model::*;
use crate::ops::{Op, TrackProps};
use crate::time::{Time, TimeRange};
use std::collections::HashMap;

pub fn plan(command: &Command, project: &Project) -> Result<Plan> {
    let tl = &project.timeline;
    match command {
        Command::Blade { at, tracks } => blade(tl, *at, tracks),
        Command::RippleDelete { range, tracks } => ripple_delete(tl, *range, tracks),
        Command::DeleteClips { clips, ripple } => delete_clips(tl, clips, *ripple),
        Command::MoveClip { clip, at, track } => move_clip(tl, *clip, *at, *track),

        Command::AddMarker { range, kind, note } => Ok(ops(vec![Op::InsertMarker {
            marker: Marker {
                id: MarkerId::new(),
                range: *range,
                kind: *kind,
                note: note.clone(),
            },
        }])),
        Command::EditMarker {
            marker,
            range,
            kind,
            note,
        } => {
            let before = tl
                .marker(*marker)
                .ok_or(CoreError::MarkerNotFound(*marker))?
                .clone();
            let after = Marker {
                id: before.id,
                range: *range,
                kind: *kind,
                note: note.clone(),
            };
            Ok(ops(vec![Op::UpdateMarker { before, after }]))
        }
        Command::RemoveMarker { marker } => {
            let marker = tl
                .marker(*marker)
                .ok_or(CoreError::MarkerNotFound(*marker))?
                .clone();
            Ok(ops(vec![Op::RemoveMarker { marker }]))
        }
        Command::AddTrack { kind, index, name } => {
            let index = (*index).min(tl.tracks.len());
            Ok(ops(vec![Op::InsertTrack {
                index,
                track: Track::new(*kind, name.clone()),
            }]))
        }
        Command::RemoveTrack { track } => {
            let index = tl
                .tracks
                .iter()
                .position(|t| t.id == *track)
                .ok_or(CoreError::TrackNotFound(*track))?;
            Ok(ops(vec![Op::RemoveTrack {
                index,
                track: tl.tracks[index].clone(),
            }]))
        }

        Command::SetTransform { clip, transform } => {
            update_clip(tl, *clip, |c| c.transform = transform.clone())
        }

        Command::AddCaption {
            track,
            range,
            text,
            speaker,
        } => insert_new(
            tl,
            *track,
            *range,
            ClipContent::Caption {
                text: text.clone(),
                speaker: *speaker,
            },
        ),
        Command::EditCaption { clip, text, range } => update_clip(tl, *clip, |c| {
            if let (Some(new), ClipContent::Caption { text: old, .. }) = (text, &mut c.content) {
                *old = new.clone();
            }
            if let Some(r) = range {
                c.start = r.start;
                c.duration = r.duration();
            }
        }),
        Command::ApplyTitle {
            track,
            template,
            range,
            text,
        } => insert_new(
            tl,
            *track,
            *range,
            ClipContent::Title {
                template: template.clone(),
                text: text.clone(),
            },
        ),
        Command::AddOverlay {
            track,
            asset,
            range,
            speaker,
        } => insert_new(
            tl,
            *track,
            *range,
            ClipContent::Overlay {
                asset: asset.clone(),
                speaker: *speaker,
            },
        ),

        Command::SetGrade { clips, grade } => update_clips(tl, clips, |c| c.grade = grade.clone()),
        Command::ApplyLut { clips, lut } => update_clips(tl, clips, |c| c.grade.lut = lut.clone()),
        Command::CopyGrade { from, to } => {
            let grade = find_clip(tl, *from)?.1.grade.clone();
            update_clips(tl, to, |c| c.grade = grade.clone())
        }

        Command::SetGain { clip, db } => update_clip(tl, *clip, |c| c.audio.gain_db = *db),
        Command::AddFade {
            clip,
            edge,
            duration,
        } => update_clip(tl, *clip, |c| {
            let d = (*duration).min(c.duration);
            match edge {
                Edge::Start => c.audio.fade_in = d,
                Edge::End => c.audio.fade_out = d,
            }
        }),
        Command::SetTrackMix { track, mix } => {
            let t = tl.track(*track).ok_or(CoreError::TrackNotFound(*track))?;
            let before = TrackProps::of(t);
            let after = TrackProps {
                mix: mix.clone(),
                ..before.clone()
            };
            Ok(ops(vec![Op::UpdateTrack {
                track: *track,
                before,
                after,
            }]))
        }
        Command::SetDucking { ducking } => Ok(ops(vec![Op::SetDucking {
            before: tl.ducking.clone(),
            after: ducking.clone(),
        }])),

        Command::ImportMedia { media } => Ok(ops(vec![Op::AddMedia {
            media: media.clone(),
        }])),
        Command::UpdateMedia { media } => {
            let before = project
                .media(media.id)
                .ok_or(CoreError::MediaNotFound(media.id))?
                .clone();
            Ok(ops(vec![Op::UpdateMedia {
                before,
                after: media.clone(),
            }]))
        }
        Command::RemoveMedia { media } => {
            let item = project
                .media(*media)
                .ok_or(CoreError::MediaNotFound(*media))?
                .clone();
            if tl
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .any(|c| c.media() == Some(*media))
            {
                return Err(CoreError::Invalid("media is used on the timeline".into()));
            }
            Ok(Plan {
                warnings: vec![Warning::NeedsConfirmation {
                    message: format!("Remove {} from the project?", item.path.display()),
                }],
                ops: vec![Op::RemoveMedia { media: item }],
            })
        }

        Command::InsertClip { .. } => Err(CoreError::NotImplemented("insert_clip")),
        Command::Trim { .. } => Err(CoreError::NotImplemented("trim")),
        Command::Roll { .. } => Err(CoreError::NotImplemented("roll")),
        Command::Slip { .. } => Err(CoreError::NotImplemented("slip")),
        Command::Slide { .. } => Err(CoreError::NotImplemented("slide")),
        Command::AddZoom { .. } => Err(CoreError::NotImplemented("add_zoom")),
        Command::SetKeyframe { .. } => Err(CoreError::NotImplemented("set_keyframe")),
        Command::RemoveKeyframe { .. } => Err(CoreError::NotImplemented("remove_keyframe")),
        Command::SetPlaylist { .. } => Err(CoreError::NotImplemented("set_playlist")),
    }
}

// ------------------------------------------------------------------ planners

fn blade(tl: &Timeline, at: Time, scope: &TrackScope) -> Result<Plan> {
    let mut out = Vec::new();
    for track in scoped_tracks(tl, scope) {
        if let Some(clip) = track.clips.iter().find(|c| c.start < at && at < c.end()) {
            let (left, right) = split_clip(clip, at);
            out.push(Op::UpdateClip {
                track: track.id,
                before: clip.clone(),
                after: left,
            });
            out.push(Op::InsertClip {
                track: track.id,
                clip: right,
            });
        }
    }
    Ok(ops(out))
}

fn ripple_delete(tl: &Timeline, range: TimeRange, scope: &TrackScope) -> Result<Plan> {
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
                if c.start < range.start {
                    let (left, _) = split_clip(c, range.start);
                    next.push(Clip { id: c.id, ..left });
                }
                if c.end() > range.end {
                    let mut right = trim_start(c, range.end);
                    right.start = range.start;
                    if c.start < range.start {
                        right.id = ClipId::new();
                    }
                    next.push(right);
                }
            }
        }
        plan.ops.extend(diff_track(track, next));
    }
    for m in &tl.markers {
        if m.kind == MarkerKind::Protected && m.range.overlaps(&range) {
            plan.warnings.push(Warning::TouchesProtected {
                marker: m.id,
                note: m.note.clone(),
            });
        }
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

fn delete_clips(tl: &Timeline, ids: &[ClipId], ripple: bool) -> Result<Plan> {
    let mut out = Vec::new();
    for track in &tl.tracks {
        if !track.clips.iter().any(|c| ids.contains(&c.id)) {
            continue;
        }
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

fn move_clip(tl: &Timeline, id: ClipId, at: Time, to: Option<TrackId>) -> Result<Plan> {
    let (from, clip) = find_clip(tl, id)?;
    let to = to.unwrap_or(from.id);
    let target = tl.track(to).ok_or(CoreError::TrackNotFound(to))?;
    let moved = Clip {
        start: at,
        ..clip.clone()
    };
    if at < Time::ZERO
        || target
            .clips
            .iter()
            .any(|c| c.id != id && c.range().overlaps(&moved.range()))
    {
        return Err(CoreError::Invalid(
            "the clip would overlap another clip".into(),
        ));
    }
    Ok(ops(if to == from.id {
        vec![Op::UpdateClip {
            track: to,
            before: clip.clone(),
            after: moved,
        }]
    } else {
        vec![
            Op::RemoveClip {
                track: from.id,
                clip: clip.clone(),
            },
            Op::InsertClip {
                track: to,
                clip: moved,
            },
        ]
    }))
}

fn insert_new(
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
    Ok(ops(vec![Op::InsertClip { track, clip }]))
}

fn update_clip(tl: &Timeline, id: ClipId, f: impl FnOnce(&mut Clip)) -> Result<Plan> {
    let (track, before) = find_clip(tl, id)?;
    let mut after = before.clone();
    f(&mut after);
    Ok(ops(if after == *before {
        vec![]
    } else {
        vec![Op::UpdateClip {
            track: track.id,
            before: before.clone(),
            after,
        }]
    }))
}

fn update_clips(tl: &Timeline, ids: &[ClipId], f: impl Fn(&mut Clip)) -> Result<Plan> {
    let mut out = Vec::new();
    for id in ids {
        out.extend(update_clip(tl, *id, &f)?.ops);
    }
    Ok(ops(out))
}

// ------------------------------------------------------------------- helpers

fn ops(ops: Vec<Op>) -> Plan {
    Plan {
        ops,
        warnings: Vec::new(),
    }
}

pub fn find_clip(tl: &Timeline, id: ClipId) -> Result<(&Track, &Clip)> {
    tl.clip(id).ok_or(CoreError::ClipNotFound(id))
}

pub fn scoped_tracks<'a>(
    tl: &'a Timeline,
    scope: &'a TrackScope,
) -> impl Iterator<Item = &'a Track> {
    tl.tracks
        .iter()
        .filter(move |t| !t.locked && scope.as_ref().is_none_or(|ids| ids.contains(&t.id)))
}

/// Split at timeline time `at` (strictly inside the clip). The left half keeps
/// the id; the right half is new. Fades and keyframes go with their side.
pub fn split_clip(clip: &Clip, at: Time) -> (Clip, Clip) {
    let mut left = clip.clone();
    left.duration = at - clip.start;
    left.audio.fade_out = Time::ZERO;
    left.keyframes.retain(|k| k.at <= left.duration);
    let mut right = trim_start(clip, at);
    right.id = ClipId::new();
    right.audio.fade_in = Time::ZERO;
    (left, right)
}

/// The same clip with its head trimmed so it starts at timeline time `at`.
pub fn trim_start(clip: &Clip, at: Time) -> Clip {
    let offset = at - clip.start;
    let mut c = clip.clone();
    c.start = at;
    c.duration = clip.duration - offset;
    if let ClipContent::Media { source_in, .. } = &mut c.content {
        *source_in = *source_in + offset;
    }
    c.keyframes = clip
        .keyframes
        .iter()
        .filter(|k| k.at >= offset)
        .map(|k| Keyframe {
            at: k.at - offset,
            ..k.clone()
        })
        .collect();
    c
}

/// Ops that turn `track.clips` into `next`, ordered so no intermediate state
/// overlaps: removals, shrinks, left moves (ascending), right moves
/// (descending), growths, then insertions.
pub fn diff_track(track: &Track, mut next: Vec<Clip>) -> Vec<Op> {
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
            clip: c.clone(),
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
            before: before.clone(),
            after: after.clone(),
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
            .map(|clip| Op::InsertClip { track: id, clip }),
    );
    out
}
