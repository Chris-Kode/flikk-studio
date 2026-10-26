//! Planners: [`Command`] → [`Plan`] against a read-only project.
//!
//! Planners never mutate. They compute the desired end state of each touched
//! track and let [`diff_track`] emit ops in an order that never overlaps
//! clips mid-way. The bus validates every plan by applying it to a copy.

use crate::command::{Command, Edge, InsertMode, Plan, TrackScope, Warning};
use crate::error::{CoreError, Result};
use crate::ids::*;
use crate::model::*;
use crate::ops::apply_all;
use crate::ops::{Op, TrackProps};
use crate::time::{FLICKS_PER_SECOND, Time, TimeRange};
use std::collections::HashMap;

pub fn plan(command: &Command, project: &Project) -> Result<Plan> {
    let mut plan = plan_command(command, project)?;
    if !matches!(
        command,
        Command::AddMarker { .. }
            | Command::EditMarker { .. }
            | Command::RemoveMarker { .. }
            | Command::AddTrack { .. }
            | Command::SetTrackLocked { .. }
            | Command::ImportMedia { .. }
            | Command::UpdateMedia { .. }
            | Command::RemoveMedia { .. }
    ) {
        warn_protected(project, &mut plan);
    }
    Ok(plan)
}

fn plan_command(command: &Command, project: &Project) -> Result<Plan> {
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
            if let (
                Some(new),
                ClipContent::Caption { text: old, .. } | ClipContent::Title { text: old, .. },
            ) = (text, &mut c.content)
            {
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
        Command::SetCaptionStyle { track, style } => {
            let t = tl.track(*track).ok_or(CoreError::TrackNotFound(*track))?;
            let before = TrackProps::of(t);
            let after = TrackProps {
                caption_style: style.clone(),
                ..before.clone()
            };
            Ok(ops(vec![Op::UpdateTrack {
                track: *track,
                before,
                after,
            }]))
        }
        Command::SetBins { bins } => Ok(ops(vec![Op::SetBins {
            before: project.bins.clone(),
            after: bins.clone(),
        }])),
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
        Command::SetTrackLocked { track, locked } => {
            let t = tl.track(*track).ok_or(CoreError::TrackNotFound(*track))?;
            let before = TrackProps::of(t);
            let after = TrackProps {
                locked: *locked,
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

        Command::InsertClip {
            media,
            source,
            track,
            at,
            mode,
        } => insert_clip(project, *media, *source, *track, *at, *mode),
        Command::Trim {
            clip,
            edge,
            delta,
            ripple,
        } => trim(project, *clip, *edge, *delta, *ripple),
        Command::Roll { left, right, delta } => roll(project, *left, *right, *delta),
        Command::Slip { clip, delta } => slip(project, *clip, *delta),
        Command::Slide { clip, delta } => slide(project, *clip, *delta),
        Command::AddZoom {
            clip,
            range,
            target,
            scale,
            ease,
        } => add_zoom(tl, *clip, *range, *target, *scale, *ease),
        Command::SetKeyframe {
            clip,
            property,
            at,
            value,
            ease,
        } => set_keyframe(tl, *clip, *property, *at, *value, *ease),
        Command::RemoveKeyframe { clip, property, at } => {
            remove_keyframe(tl, *clip, *property, *at)
        }
        Command::SetPlaylist { track, at, songs } => set_playlist(project, *track, *at, songs),
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

fn move_clip(tl: &Timeline, id: ClipId, at: Time, to: Option<TrackId>) -> Result<Plan> {
    let (from, clip) = find_clip(tl, id)?;
    let to = to.unwrap_or(from.id);
    editable_track(tl, from.id)?;
    let target = editable_track(tl, to)?;
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

fn insert_clip(
    project: &Project,
    media: MediaId,
    source: TimeRange,
    track: TrackId,
    at: Time,
    mode: InsertMode,
) -> Result<Plan> {
    let tl = &project.timeline;
    let item = project
        .media(media)
        .ok_or(CoreError::MediaNotFound(media))?;
    let target = editable_track(tl, track)?;
    let fits = match target.kind {
        TrackKind::Audio => {
            item.kind != MediaKind::Image && item.info.as_ref().is_none_or(|i| !i.audio.is_empty())
        }
        TrackKind::Video | TrackKind::Overlay => item.kind != MediaKind::Audio,
        TrackKind::Title | TrackKind::Caption => false,
    };
    if !fits {
        return Err(CoreError::Invalid(format!(
            "{:?} media does not go on a {:?} track",
            item.kind, target.kind
        )));
    }
    if at < Time::ZERO || source.start < Time::ZERO {
        return Err(CoreError::Invalid("negative time".into()));
    }
    let source = match media_duration(project, media) {
        Some(d) => TimeRange::new(source.start, source.end.min(d)),
        None => source,
    };
    if source.is_empty() {
        return Err(CoreError::Invalid(
            "the source range is empty or outside the media".into(),
        ));
    }
    let len = source.duration();
    let audio_stream = (target.kind == TrackKind::Audio)
        .then(|| {
            item.info
                .as_ref()
                .and_then(|i| i.audio.first())
                .map(|a| a.index)
        })
        .flatten();
    let clip = Clip::new(
        at,
        len,
        ClipContent::Media {
            media,
            source_in: source.start,
            audio_stream,
        },
    );
    let mut next = match mode {
        InsertMode::Overwrite => cut_out(&target.clips, TimeRange::new(at, at + len)),
        InsertMode::Insert => {
            let mut next = Vec::with_capacity(target.clips.len() + 2);
            for c in &target.clips {
                if c.end() <= at {
                    next.push(c.clone());
                } else if c.start >= at {
                    next.push(Clip {
                        start: c.start + len,
                        ..c.clone()
                    });
                } else {
                    let (left, mut right) = split_clip(c, at);
                    right.start = right.start + len;
                    next.push(left);
                    next.push(right);
                }
            }
            next
        }
    };
    next.push(clip);
    Ok(ops(diff_track(target, next)))
}

fn trim(project: &Project, id: ClipId, edge: Edge, delta: Time, ripple: bool) -> Result<Plan> {
    let tl = &project.timeline;
    let (track, clip) = find_clip(tl, id)?;
    editable_track(tl, track.id)?;
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
    let mut clips: Vec<Clip> = track.clips[..ix].to_vec();
    clips.push(trimmed);
    clips.extend(track.clips[ix + 1..].iter().map(|c| Clip {
        start: c.start + shift,
        ..c.clone()
    }));
    Ok(ops(diff_track(track, clips)))
}

fn roll(project: &Project, left: ClipId, right: ClipId, delta: Time) -> Result<Plan> {
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

fn slip(project: &Project, id: ClipId, delta: Time) -> Result<Plan> {
    let tl = &project.timeline;
    let (track, clip) = find_clip(tl, id)?;
    editable_track(tl, track.id)?;
    let Some(before) = room_before(clip) else {
        return Err(CoreError::Invalid("only media clips can slip".into()));
    };
    let delta = clamp(delta, Some(-before), room_after(project, clip));
    update_clip(tl, id, |c| {
        if let ClipContent::Media { source_in, .. } = &mut c.content {
            *source_in = *source_in + delta;
        }
    })
}

fn slide(project: &Project, id: ClipId, delta: Time) -> Result<Plan> {
    let tl = &project.timeline;
    let (track, clip) = find_clip(tl, id)?;
    editable_track(tl, track.id)?;
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

/// Longest ease in and out of a zoom; shorter ranges use 20% of their length.
pub const ZOOM_RAMP: Time = Time(FLICKS_PER_SECOND * 2 / 5);

/// Keyframes `Scale`, and `PositionX/Y` unless `target` is the centre, so the
/// clip eases from its current value to `scale` over the ramp, holds, then
/// eases back by `range.end`. `target` is a point in the normalized space of
/// [`Transform::position`] ((0, 0) = centre); position is keyframed to
/// `-target * (scale - 1)` so that point stays in place while the frame grows
/// around it. Existing keyframes of those properties inside the range go.
fn add_zoom(
    tl: &Timeline,
    id: ClipId,
    range: TimeRange,
    target: [f32; 2],
    scale: f32,
    ease: Ease,
) -> Result<Plan> {
    let (_, clip) = find_clip(tl, id)?;
    if !(scale.is_finite() && scale > 0.0) || !target.iter().all(|v| v.is_finite()) {
        return Err(CoreError::Invalid("invalid zoom scale or target".into()));
    }
    let r = range
        .intersect(&clip.range())
        .filter(|r| r.duration() >= min_len(tl))
        .ok_or_else(|| CoreError::Invalid("the zoom range is outside the clip".into()))?;
    let (a, b) = (r.start - clip.start, r.end - clip.start);
    let ramp = ZOOM_RAMP.min(Time(r.duration().0 / 5));
    let mut props = vec![Property::Scale];
    if target != [0.0, 0.0] {
        props.extend([Property::PositionX, Property::PositionY]);
    }
    let peak = |property: Property, from: f32| match property {
        Property::Scale => scale,
        Property::PositionX => from - target[0] * (scale - 1.0),
        _ => from - target[1] * (scale - 1.0),
    };
    let keys: Vec<Keyframe> = props
        .iter()
        .flat_map(|&property| {
            let (from, to) = (clip.value_at(property, a), clip.value_at(property, b));
            let peak = peak(property, from);
            [
                (a, from, ease),
                (a + ramp, peak, Ease::Linear),
                (b - ramp, peak, ease),
                (b, to, ease),
            ]
            .map(|(at, value, ease)| Keyframe {
                property,
                at,
                value,
                ease,
            })
        })
        .collect();
    update_clip(tl, id, |c| {
        c.keyframes
            .retain(|k| !(props.contains(&k.property) && a <= k.at && k.at <= b));
        c.keyframes.extend(keys);
        c.keyframes.sort_by_key(|k| k.at);
    })
}

/// `at` is timeline time, like every other command; it is stored relative to
/// the clip's start and must fall inside the clip.
fn set_keyframe(
    tl: &Timeline,
    id: ClipId,
    property: Property,
    at: Time,
    value: f32,
    ease: Ease,
) -> Result<Plan> {
    let (_, clip) = find_clip(tl, id)?;
    let rel = at - clip.start;
    if rel < Time::ZERO || rel > clip.duration {
        return Err(CoreError::Invalid(
            "the keyframe is outside the clip".into(),
        ));
    }
    if !value.is_finite() {
        return Err(CoreError::Invalid("invalid keyframe value".into()));
    }
    update_clip(tl, id, |c| {
        c.keyframes
            .retain(|k| !(k.property == property && k.at == rel));
        c.keyframes.push(Keyframe {
            property,
            at: rel,
            value,
            ease,
        });
        c.keyframes.sort_by_key(|k| k.at);
    })
}

/// Removes the keyframe of `property` nearest to timeline time `at`, within
/// half a frame.
fn remove_keyframe(tl: &Timeline, id: ClipId, property: Property, at: Time) -> Result<Plan> {
    let (_, clip) = find_clip(tl, id)?;
    let rel = at - clip.start;
    let tolerance = Time(tl.settings.rate.frame_duration().0 / 2);
    let ix = clip
        .keyframes
        .iter()
        .enumerate()
        .filter(|(_, k)| k.property == property && (k.at - rel).0.abs() <= tolerance.0)
        .min_by_key(|(_, k)| (k.at - rel).0.abs())
        .map(|(i, _)| i)
        .ok_or_else(|| CoreError::Invalid(format!("no {property:?} keyframe at {at}")))?;
    update_clip(tl, id, |c| {
        c.keyframes.remove(ix);
    })
}

/// Songs back to back from `at`, each for its full duration, replacing
/// whatever the track has in that span (PRD A4).
fn set_playlist(project: &Project, track: TrackId, at: Time, songs: &[MediaId]) -> Result<Plan> {
    let tl = &project.timeline;
    let target = editable_track(tl, track)?;
    if target.kind != TrackKind::Audio {
        return Err(CoreError::Invalid("playlists go on audio tracks".into()));
    }
    if songs.is_empty() || at < Time::ZERO {
        return Err(CoreError::Invalid("no songs, or a negative start".into()));
    }
    let mut cursor = at;
    let mut clips = Vec::with_capacity(songs.len());
    for &media in songs {
        let item = project
            .media(media)
            .ok_or(CoreError::MediaNotFound(media))?;
        let duration = media_duration(project, media).ok_or_else(|| {
            CoreError::Invalid(format!("{} has no known duration", item.path.display()))
        })?;
        clips.push(Clip::new(
            cursor,
            duration,
            ClipContent::Media {
                media,
                source_in: Time::ZERO,
                audio_stream: item
                    .info
                    .as_ref()
                    .and_then(|i| i.audio.first())
                    .map(|a| a.index),
            },
        ));
        cursor = cursor + duration;
    }
    let mut next = cut_out(&target.clips, TimeRange::new(at, cursor));
    next.extend(clips);
    Ok(ops(diff_track(target, next)))
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

fn editable_track(tl: &Timeline, id: TrackId) -> Result<&Track> {
    let track = tl.track(id).ok_or(CoreError::TrackNotFound(id))?;
    if track.locked {
        return Err(CoreError::Invalid(format!(
            "track {} is locked",
            track.name
        )));
    }
    Ok(track)
}

fn clip_index(track: &Track, id: ClipId) -> usize {
    track.clips.iter().position(|c| c.id == id).unwrap_or(0)
}

/// Shortest clip a trim, roll or slide leaves behind: one sequence frame.
fn min_len(tl: &Timeline) -> Time {
    tl.settings.rate.frame_duration()
}

/// Known length of a pooled source; `None` for stills and unprobed media.
pub fn media_duration(project: &Project, id: MediaId) -> Option<Time> {
    project
        .media(id)
        .filter(|m| m.kind != MediaKind::Image)
        .and_then(|m| m.info.as_ref())
        .map(|i| i.duration)
        .filter(|d| *d > Time::ZERO)
}

/// Source media available before the clip's first frame; `None` = unlimited.
fn room_before(clip: &Clip) -> Option<Time> {
    match &clip.content {
        ClipContent::Media { source_in, .. } => Some((*source_in).max(Time::ZERO)),
        _ => None,
    }
}

/// Source media available after the clip's last frame; `None` = unlimited or
/// unknown.
fn room_after(project: &Project, clip: &Clip) -> Option<Time> {
    let ClipContent::Media {
        media, source_in, ..
    } = &clip.content
    else {
        return None;
    };
    media_duration(project, *media).map(|d| (d - *source_in - clip.duration).max(Time::ZERO))
}

/// The tighter of two optional lower bounds.
fn lower(a: Option<Time>, b: Option<Time>) -> Option<Time> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// The tighter of two optional upper bounds.
fn upper(a: Option<Time>, b: Option<Time>) -> Option<Time> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Clamp `delta` into `[lo, hi]`; bounds never exclude zero, so a clip that is
/// already past a limit simply cannot move further that way.
fn clamp(delta: Time, lo: Option<Time>, hi: Option<Time>) -> Time {
    let mut d = delta;
    if let Some(hi) = hi {
        d = d.min(hi.max(Time::ZERO));
    }
    if let Some(lo) = lo {
        d = d.max(lo.min(Time::ZERO));
    }
    d
}

/// `clips` with everything inside `range` removed, without closing the gap.
/// A clip spanning the whole range is split; the right part gets a new id.
pub fn cut_out(clips: &[Clip], range: TimeRange) -> Vec<Clip> {
    let mut next = Vec::with_capacity(clips.len() + 1);
    for c in clips {
        if c.end() <= range.start || c.start >= range.end {
            next.push(c.clone());
            continue;
        }
        if c.start < range.start {
            next.push(with_duration(c, range.start - c.start));
        }
        if c.end() > range.end {
            let mut right = trim_start(c, range.end);
            if c.start < range.start {
                right.id = ClipId::new();
            }
            next.push(right);
        }
    }
    next
}

/// The same clip with its tail set so it lasts `duration`. Keyframes past
/// the new end are replaced by one at the end holding the value there.
pub fn with_duration(clip: &Clip, duration: Time) -> Clip {
    let mut c = clip.clone();
    c.duration = duration;
    if duration < clip.duration {
        c.keyframes = keyframes_until(clip, duration);
    }
    c.audio.fade_in = c.audio.fade_in.min(duration);
    c.audio.fade_out = c.audio.fade_out.min(duration);
    c
}

/// Keyframes at or before clip time `t`, plus one at `t` for each property
/// that had keyframes after it, so the value at the cut does not jump.
fn keyframes_until(clip: &Clip, t: Time) -> Vec<Keyframe> {
    let mut keys: Vec<Keyframe> = clip
        .keyframes
        .iter()
        .filter(|k| k.at <= t)
        .cloned()
        .collect();
    for k in clip.keyframes.iter().filter(|k| k.at > t) {
        if !keys.iter().any(|x| x.property == k.property && x.at == t) {
            keys.push(Keyframe {
                property: k.property,
                at: t,
                value: clip.value_at(k.property, t),
                ease: k.ease,
            });
        }
    }
    keys.sort_by_key(|k| k.at);
    keys
}

/// Keyframes at or after clip time `t`, shifted to start at zero, plus one at
/// zero for each property that had keyframes before `t`.
fn keyframes_from(clip: &Clip, t: Time) -> Vec<Keyframe> {
    let mut keys: Vec<Keyframe> = clip
        .keyframes
        .iter()
        .filter(|k| k.at >= t)
        .map(|k| Keyframe {
            at: k.at - t,
            ..k.clone()
        })
        .collect();
    for k in clip.keyframes.iter().filter(|k| k.at < t) {
        if !keys
            .iter()
            .any(|x| x.property == k.property && x.at == Time::ZERO)
        {
            let ease = clip
                .keyframes
                .iter()
                .filter(|x| x.property == k.property && x.at < t)
                .max_by_key(|x| x.at)
                .map_or(k.ease, |x| x.ease);
            keys.push(Keyframe {
                property: k.property,
                at: Time::ZERO,
                value: clip.value_at(k.property, t),
                ease,
            });
        }
    }
    keys.sort_by_key(|k| k.at);
    keys
}

// --------------------------------------------------------- protected ranges

/// Add [`Warning::TouchesProtected`] for every protected marker whose content
/// the plan removes, shifts or replaces (PRD F4). A marker is untouched when,
/// after the plan, it still exists with the same length and every piece of
/// media it covered still plays at the same offset inside it. Adding content
/// over a protected range, or moving the marker together with its content,
/// does not warn.
fn warn_protected(project: &Project, plan: &mut Plan) {
    let protected: Vec<&Marker> = project
        .timeline
        .markers
        .iter()
        .filter(|m| m.kind == MarkerKind::Protected && !m.range.is_empty())
        .collect();
    if protected.is_empty() || plan.ops.is_empty() {
        return;
    }
    let mut after = project.clone();
    if apply_all(&mut after, &plan.ops).is_err() {
        return;
    }
    for m in protected {
        let touched = match after.timeline.marker(m.id) {
            Some(n) if n.range.duration() == m.range.duration() => {
                project.timeline.tracks.iter().any(|t| {
                    let now = after.timeline.track(t.id).map_or(&[][..], |t| &t.clips);
                    let kept = segments(now, n.range);
                    segments(&t.clips, m.range).iter().any(|old| {
                        !kept.iter().any(|new| {
                            new.key == old.key && new.start <= old.start && old.end <= new.end
                        })
                    })
                })
            }
            _ => true,
        };
        let known = plan
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::TouchesProtected { marker, .. } if *marker == m.id));
        if touched && !known {
            plan.warnings.push(Warning::TouchesProtected {
                marker: m.id,
                note: m.note.clone(),
            });
        }
    }
}

#[derive(PartialEq)]
enum SegmentKey<'a> {
    /// Source time minus offset into the range: equal for continuous media.
    Media {
        media: MediaId,
        stream: Option<u32>,
        offset: Time,
    },
    Other(&'a ClipContent),
}

struct Segment<'a> {
    /// Relative to the range start.
    start: Time,
    end: Time,
    key: SegmentKey<'a>,
}

/// What plays inside `range`, relative to its start, with continuous pieces
/// (e.g. the two halves of a blade) merged.
fn segments(clips: &[Clip], range: TimeRange) -> Vec<Segment<'_>> {
    let mut out: Vec<Segment> = Vec::new();
    for c in clips {
        let Some(r) = c.range().intersect(&range) else {
            continue;
        };
        let key = match &c.content {
            ClipContent::Media {
                media,
                source_in,
                audio_stream,
            } => SegmentKey::Media {
                media: *media,
                stream: *audio_stream,
                offset: *source_in - c.start + range.start,
            },
            other => SegmentKey::Other(other),
        };
        let (start, end) = (r.start - range.start, r.end - range.start);
        match out.last_mut() {
            Some(last) if last.end == start && last.key == key => last.end = end,
            _ => out.push(Segment { start, end, key }),
        }
    }
    out
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
/// the id; the right half is new. Fades and keyframes go with their side,
/// and each half gets a keyframe at the cut so animated values don't jump.
pub fn split_clip(clip: &Clip, at: Time) -> (Clip, Clip) {
    let mut left = with_duration(clip, at - clip.start);
    left.audio.fade_out = Time::ZERO;
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
    c.keyframes = if offset > Time::ZERO {
        keyframes_from(clip, offset)
    } else {
        clip.keyframes
            .iter()
            .map(|k| Keyframe {
                at: k.at - offset,
                ..k.clone()
            })
            .collect()
    };
    c.audio.fade_in = c.audio.fade_in.min(c.duration);
    c.audio.fade_out = c.audio.fade_out.min(c.duration);
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
