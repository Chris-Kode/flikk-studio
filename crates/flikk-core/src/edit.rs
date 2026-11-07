//! Planners: [`Command`] → [`Plan`] against a read-only project.
//!
//! Planners never mutate. They compute the desired end state of each touched
//! track and let [`diff_track`] emit ops in an order that never overlaps
//! clips mid-way. The bus validates every plan by applying it to a copy.

use crate::command::{Command, Edge, InsertMode, PlacedClip, Plan, TrackScope, Warning};
use crate::error::{CoreError, Result};
use crate::fx::{self, FxAsset, FxInstance, FxKind, FxRef, ParamKeyframe, ParamValue, Transition};
use crate::ids::*;
use crate::model::*;
use crate::ops::apply_all;
use crate::ops::{Op, TrackProps};
use crate::speed::{MAX_SPEED, MIN_SPEED, SpeedKey};
use crate::time::{FLICKS_PER_SECOND, Rate, Time, TimeRange};
use std::collections::HashMap;

pub fn plan(command: &Command, project: &Project) -> Result<Plan> {
    let mut plan = plan_command(command, project)?;
    if let Some(track) = locked_track(project, &plan.ops) {
        return Err(locked(track));
    }
    if !matches!(
        command,
        Command::AddMarker { .. }
            | Command::EditMarker { .. }
            | Command::RemoveMarker { .. }
            | Command::AddTrack { .. }
            | Command::SetTrackLocked { .. }
            | Command::RenameTrack { .. }
            | Command::MoveTrack { .. }
            | Command::ImportMedia { .. }
            | Command::UpdateMedia { .. }
            | Command::RemoveMedia { .. }
            | Command::RenameClip { .. }
            | Command::SetClipColor { .. }
            | Command::SetFormat { .. }
            | Command::SetRates { .. }
            | Command::AddEffect { .. }
            | Command::SetEffect { .. }
            | Command::RemoveEffect { .. }
            | Command::MoveEffect { .. }
            | Command::SetEffectKeyframe { .. }
            | Command::RemoveEffectKeyframe { .. }
            | Command::SetFxAsset { .. }
            | Command::RemoveFxAsset { .. }
            | Command::SetAudioEffects { .. }
            | Command::SetToneMap { .. }
    ) {
        warn_protected(project, &mut plan);
    }
    Ok(plan)
}

fn plan_command(command: &Command, project: &Project) -> Result<Plan> {
    let tl = &project.timeline;
    match command {
        Command::Blade { at, tracks } => blade(tl, *at, tracks),
        Command::RippleDelete {
            range,
            tracks,
            fade,
        } => ripple_delete(tl, *range, tracks, *fade),
        Command::DeleteClips { clips, ripple } => delete_clips(tl, clips, *ripple),
        Command::MoveClip { clip, at, track } => move_clip(project, *clip, *at, *track),
        Command::SyncClips {
            reference,
            followers,
            mute,
        } => crate::sync::plan::sync_clips(project, *reference, followers, mute),
        Command::PlaceSynced {
            at,
            reference,
            followers,
            mute,
        } => crate::sync::plan::place_synced(project, *at, *reference, followers, mute),

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
            let mut out = vec![Op::RemoveTrack {
                index,
                track: tl.tracks[index].clone(),
            }];
            // Ducking follows the music track it names.
            if tl.ducking.as_ref().is_some_and(|d| d.music_track == *track) {
                out.push(Op::SetDucking {
                    before: tl.ducking.clone(),
                    after: None,
                });
            }
            Ok(ops(out))
        }

        Command::SetTransform { clip, transform } => {
            update_clip(tl, *clip, |c| c.transform = transform.clone())
        }
        Command::SetMasks { clip, masks } => update_clip(tl, *clip, |c| c.masks = masks.clone()),

        Command::AddCaption {
            track,
            range,
            text,
            speaker,
            words,
        } => insert_new(
            tl,
            *track,
            *range,
            ClipContent::Caption {
                text: text.clone(),
                speaker: *speaker,
                words: words.clone(),
            },
        ),
        Command::EditCaption { clip, text, range } => {
            if range.is_some_and(|r| r.is_empty()) {
                return Err(CoreError::Invalid("empty range".into()));
            }
            update_clip(tl, *clip, |c| {
                match (text, &mut c.content) {
                    (
                        Some(new),
                        ClipContent::Caption {
                            text: old, words, ..
                        },
                    ) => {
                        // A fixed typo keeps the word timing; other rewrites lose it,
                        // the same number of words too when most of them changed.
                        let (was, now): (Vec<&str>, Vec<&str>) = (
                            old.split_whitespace().collect(),
                            new.split_whitespace().collect(),
                        );
                        let changed = was.iter().zip(&now).filter(|(a, b)| a != b).count();
                        if was.len() != now.len()
                            || words.len() != now.len()
                            || changed * 2 > now.len()
                        {
                            words.clear();
                        }
                        *old = new.clone();
                    }
                    (Some(new), ClipContent::Title { text: old, .. }) => *old = new.clone(),
                    _ => {}
                }
                if let Some(r) = range {
                    // Retiming leaves the words where they are spoken.
                    if let ClipContent::Caption { words, .. } = &mut c.content {
                        for w in words {
                            *w = *w + c.start - r.start;
                        }
                    }
                    c.start = r.start;
                    c.duration = r.duration();
                }
            })
        }
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
        Command::SetTimelineGrade { grade } => Ok(ops(vec![Op::SetTimelineGrade {
            before: tl.grade.clone(),
            after: grade.clone(),
        }])),

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
        Command::RenameTrack { track, name } => {
            let t = tl.track(*track).ok_or(CoreError::TrackNotFound(*track))?;
            let name = name.trim();
            if name.is_empty() {
                return Err(CoreError::Invalid("a track needs a name".into()));
            }
            let before = TrackProps::of(t);
            let after = TrackProps {
                name: name.to_owned(),
                ..before.clone()
            };
            Ok(ops(vec![Op::UpdateTrack {
                track: *track,
                before,
                after,
            }]))
        }
        Command::MoveTrack { track, index } => {
            let from = tl
                .tracks
                .iter()
                .position(|t| t.id == *track)
                .ok_or(CoreError::TrackNotFound(*track))?;
            let to = (*index).min(tl.tracks.len() - 1);
            if from == to {
                return Err(CoreError::Invalid("the track is already there".into()));
            }
            Ok(ops(vec![Op::MoveTrack {
                track: *track,
                from,
                to,
            }]))
        }
        Command::SetDucking { ducking } => {
            if let Some(d) = ducking {
                tl.track(d.music_track)
                    .ok_or(CoreError::TrackNotFound(d.music_track))?;
            }
            Ok(ops(vec![Op::SetDucking {
                before: tl.ducking.clone(),
                after: ducking.clone(),
            }]))
        }

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
        Command::SetToneMap { media, tone_map } => {
            let before = project
                .media(*media)
                .ok_or(CoreError::MediaNotFound(*media))?
                .clone();
            let after = MediaItem {
                tone_map: tone_map.clone(),
                ..before.clone()
            };
            Ok(ops(vec![Op::UpdateMedia { before, after }]))
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
        Command::FitMusic {
            clip,
            at,
            segments,
            crossfade,
            fade_out,
        } => crate::music_fit::plan_command(project, *clip, *at, segments, *crossfade, *fade_out),

        Command::PlaceClips { clips, mode } => place_clips(project, clips, *mode),
        Command::SetSpeed {
            clip,
            speed,
            ripple,
        } => set_speed(project, *clip, *speed, *ripple),
        Command::SetSpeedRamp {
            clip,
            points,
            keep_length,
            ripple,
        } => set_speed_ramp(project, *clip, points, *keep_length, *ripple),
        Command::SetRetime {
            clip,
            interpolation,
            audio,
        } => {
            let (_, c) = find_clip(tl, *clip)?;
            if c.media().is_none() {
                return Err(CoreError::Invalid("only media clips are retimed".into()));
            }
            update_clip(tl, *clip, |c| {
                if let Some(i) = interpolation {
                    c.retime.interpolation = *i;
                }
                if let Some(a) = audio {
                    c.retime.audio = *a;
                }
            })
        }
        Command::SetClipsEnabled { clips, enabled } => {
            update_clips(tl, clips, |c| c.enabled = *enabled)
        }
        Command::RenameClip { clip, name } => update_clip(tl, *clip, |c| {
            c.name = name
                .as_ref()
                .map(|n| n.trim().to_owned())
                .filter(|n| !n.is_empty())
        }),
        Command::SetClipColor { clips, color } => update_clips(tl, clips, |c| c.color = *color),
        Command::SetFormat { width, height } => {
            let valid = |v: u32| (16..=8192).contains(&v) && v.is_multiple_of(2);
            if !valid(*width) || !valid(*height) {
                return Err(CoreError::Invalid(format!(
                    "frame size {width}×{height}: each side must be even, 16 to 8192"
                )));
            }
            Ok(ops(vec![Op::SetSequence {
                before: tl.settings.clone(),
                after: SequenceSettings {
                    width: *width,
                    height: *height,
                    ..tl.settings.clone()
                },
            }]))
        }
        Command::SetRates { rate, sample_rate } => {
            let known = Rate::SEQUENCE.iter().any(|(r, _)| r == rate);
            let mix = SequenceSettings::SAMPLE_RATES
                .iter()
                .any(|(s, _)| s == sample_rate);
            if !known || !mix {
                return Err(CoreError::Invalid(format!(
                    "{}/{} fps at {sample_rate} Hz is not a sequence rate",
                    rate.num, rate.den
                )));
            }
            Ok(ops(vec![Op::SetSequence {
                before: tl.settings.clone(),
                after: SequenceSettings {
                    rate: *rate,
                    sample_rate: *sample_rate,
                    ..tl.settings.clone()
                },
            }]))
        }

        Command::AddEffect {
            clips,
            effect,
            index,
        } => add_effect(project, clips, effect, *index),
        Command::SetEffect { clip, effect } => {
            let asset = fx_asset(project, &effect.fx, FxKind::Effect)?;
            let effect = coerced(effect, asset);
            with_effect(tl, *clip, effect.id, |list, ix| list[ix] = effect.clone())
        }
        Command::RemoveEffect { clip, effect } => with_effect(tl, *clip, *effect, |list, ix| {
            list.remove(ix);
        }),
        Command::MoveEffect {
            clip,
            effect,
            index,
        } => with_effect(tl, *clip, *effect, |list, ix| {
            let e = list.remove(ix);
            list.insert((*index).min(list.len()), e);
        }),
        Command::SetEffectKeyframe {
            clip,
            effect,
            param,
            at,
            value,
            ease,
        } => set_effect_keyframe(project, *clip, *effect, param, *at, Some((value, *ease))),
        Command::RemoveEffectKeyframe {
            clip,
            effect,
            param,
            at,
        } => set_effect_keyframe(project, *clip, *effect, param, *at, None),
        Command::SetTransition {
            clip,
            edge,
            transition,
        } => set_transition(project, *clip, *edge, transition.as_ref()),
        Command::AddGenerator { track, range, fx } => {
            let asset = fx_asset(project, &fx.fx, FxKind::Generator)?;
            let t = editable_track(tl, *track)?;
            if !matches!(
                t.kind,
                TrackKind::Video | TrackKind::Overlay | TrackKind::Title
            ) {
                return Err(CoreError::Invalid(
                    "generators go on video, overlay or title tracks".into(),
                ));
            }
            insert_new(
                tl,
                *track,
                *range,
                ClipContent::Generator {
                    fx: coerced(fx, asset),
                },
            )
        }
        Command::SetGenerator { clip, fx } => {
            let asset = fx_asset(project, &fx.fx, FxKind::Generator)?;
            let (_, c) = find_clip(tl, *clip)?;
            if !matches!(c.content, ClipContent::Generator { .. }) {
                return Err(CoreError::Invalid(format!(
                    "{clip} is not a generator clip"
                )));
            }
            let fx = coerced(fx, asset);
            update_clip(tl, *clip, |c| c.content = ClipContent::Generator { fx })
        }
        Command::SetFxAsset { asset } => {
            if !matches!(asset.id, FxRef::Project(_)) {
                return Err(CoreError::Invalid(
                    "built-in programs cannot be replaced; save a copy under a new id".into(),
                ));
            }
            asset.validate().map_err(CoreError::Invalid)?;
            // Clips use a program as what it is: an effect stack must not
            // end up holding a transition.
            if let Some(old) = project.fx.iter().find(|a| a.id == asset.id)
                && old.kind != asset.kind
            {
                let used = fx::uses(project, &asset.id);
                if !used.is_empty() {
                    return Err(CoreError::Invalid(format!(
                        "the {} is used by {} clip(s); save the {} under a new id",
                        old.kind.label().to_lowercase(),
                        used.len(),
                        asset.kind.label().to_lowercase()
                    )));
                }
            }
            let mut after = project.fx.clone();
            match after.iter_mut().find(|a| a.id == asset.id) {
                Some(a) => *a = asset.clone(),
                None => after.push(asset.clone()),
            }
            Ok(ops(vec![Op::SetFxAssets {
                before: project.fx.clone(),
                after,
            }]))
        }
        Command::RemoveFxAsset { asset } => {
            let r = FxRef::Project(*asset);
            if !project.fx.iter().any(|a| a.id == r) {
                return Err(CoreError::Invalid(format!("no FX program {asset}")));
            }
            let used = fx::uses(project, &r);
            if !used.is_empty() {
                return Err(CoreError::Invalid(format!(
                    "the program is still used by {} clip(s)",
                    used.len()
                )));
            }
            let after = project.fx.iter().filter(|a| a.id != r).cloned().collect();
            Ok(ops(vec![Op::SetFxAssets {
                before: project.fx.clone(),
                after,
            }]))
        }
        Command::ReplaceSpeech {
            range,
            track,
            media,
            fade,
        } => crate::respeak::plan(project, *range, *track, *media, *fade),
        Command::SetAudioEffects { clip, effects } => {
            let (_, c) = find_clip(tl, *clip)?;
            if c.media().is_none() {
                return Err(CoreError::Invalid(format!("{clip} has no audio")));
            }
            update_clip(tl, *clip, |c| c.audio.effects = effects.clone())
        }
        Command::SetStabilization {
            clips,
            stabilization,
        } => {
            for id in clips {
                let (track, c) = find_clip(tl, *id)?;
                let video = c
                    .media()
                    .and_then(|m| project.media(m))
                    .is_some_and(|m| m.kind == MediaKind::Video);
                let picture = matches!(track.kind, TrackKind::Video | TrackKind::Overlay);
                if !video || !picture {
                    return Err(CoreError::Invalid(format!("{id} is not a video clip")));
                }
            }
            let mut s = stabilization.clone();
            s.smoothness = s.smoothness.clamp(0.0, 1.0);
            update_clips(tl, clips, |c| c.stabilization = s.clone())
        }
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

fn ripple_delete(tl: &Timeline, range: TimeRange, scope: &TrackScope, fade: Time) -> Result<Plan> {
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

fn move_clip(project: &Project, id: ClipId, at: Time, to: Option<TrackId>) -> Result<Plan> {
    let tl = &project.timeline;
    let (from, clip) = find_clip(tl, id)?;
    let to = to.unwrap_or(from.id);
    editable_track(tl, from.id)?;
    let target = editable_track(tl, to)?;
    if to != from.id && !fits_track(project, &clip.content, target.kind) {
        return Err(CoreError::Invalid(format!(
            "that clip does not go on {} ({:?})",
            target.name, target.kind
        )));
    }
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
            before: Box::new(clip.clone()),
            after: Box::new(moved),
        }]
    } else {
        vec![
            Op::RemoveClip {
                track: from.id,
                clip: Box::new(clip.clone()),
            },
            Op::InsertClip {
                track: to,
                clip: Box::new(moved),
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
    // `delta` is timeline time; the source moves by that much at the clip's speed.
    update_clip(tl, id, |c| {
        if let ClipContent::Media { source_in, .. } = &mut c.content {
            *source_in = *source_in + delta.scale(c.speed);
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

/// Copies of clips on their target tracks (paste, duplicate).
fn place_clips(project: &Project, items: &[PlacedClip], mode: InsertMode) -> Result<Plan> {
    let tl = &project.timeline;
    if items.is_empty() {
        return Err(CoreError::Invalid("nothing to place".into()));
    }
    let mut groups: Vec<(TrackId, Vec<Clip>)> = Vec::new();
    for item in items {
        let track = editable_track(tl, item.track)?;
        if !fits_track(project, &item.clip.content, track.kind) {
            return Err(CoreError::Invalid(format!(
                "that clip does not go on {} ({:?})",
                track.name, track.kind
            )));
        }
        if item.clip.start < Time::ZERO || item.clip.duration <= Time::ZERO {
            return Err(CoreError::Invalid("negative start or empty clip".into()));
        }
        let clip = placed_clip(project, &item.clip)?;
        match groups.iter_mut().find(|(t, _)| *t == item.track) {
            Some((_, clips)) => clips.push(clip),
            None => groups.push((item.track, vec![clip])),
        }
    }
    let mut out = Vec::new();
    for (track_id, mut placed) in groups {
        placed.sort_by_key(|c| c.start);
        if placed.windows(2).any(|w| w[0].end() > w[1].start) {
            return Err(CoreError::Invalid("placed clips overlap each other".into()));
        }
        let track = tl
            .track(track_id)
            .ok_or(CoreError::TrackNotFound(track_id))?;
        let first = placed[0].start;
        let span = placed.iter().map(Clip::end).max().unwrap_or(first) - first;
        let mut next = match mode {
            InsertMode::Overwrite => placed
                .iter()
                .fold(track.clips.clone(), |clips, c| cut_out(&clips, c.range())),
            InsertMode::Insert => {
                let mut next = Vec::with_capacity(track.clips.len() + 1);
                for c in &track.clips {
                    if c.end() <= first {
                        next.push(c.clone());
                    } else if c.start >= first {
                        next.push(Clip {
                            start: c.start + span,
                            ..c.clone()
                        });
                    } else {
                        let (left, mut right) = split_clip(c, first);
                        right.start = right.start + span;
                        next.push(left);
                        next.push(right);
                    }
                }
                next
            }
        };
        next.extend(placed);
        out.extend(diff_track(track, next));
    }
    Ok(ops(out))
}

/// A copy of a caller's clip for [`place_clips`], new ids and all, held to
/// what the other planners keep: a speed and ramp in range, fades inside
/// the clip, keyframes in order, and programs of the right kind.
fn placed_clip(project: &Project, clip: &Clip) -> Result<Clip> {
    let speed_ok = |s: f64| s.is_finite() && (MIN_SPEED..=MAX_SPEED).contains(&s);
    if !speed_ok(clip.speed) || !clip.retime.keys.iter().all(|k| speed_ok(k.speed)) {
        return Err(CoreError::Invalid(format!(
            "speed must be between {MIN_SPEED} and {MAX_SPEED}"
        )));
    }
    if !clip.keyframes.iter().all(|k| k.value.is_finite()) {
        return Err(CoreError::Invalid("invalid keyframe value".into()));
    }
    let mut c = clip.clone();
    c.id = ClipId::new();
    if c.media().is_some() {
        c.retime.keys.sort_by_key(|k| k.at);
        c.settle_ramp();
    } else {
        c.retime.keys.clear();
    }
    c.keyframes.sort_by_key(|k| k.at);
    c.audio.fade_in = c.audio.fade_in.clamp(Time::ZERO, c.duration);
    c.audio.fade_out = c.audio.fade_out.clamp(Time::ZERO, c.duration);
    // Each copy owns its effect instances.
    let instance = |fx: &FxInstance, kind: FxKind| -> Result<FxInstance> {
        let mut out = coerced(fx, fx_asset(project, &fx.fx, kind)?);
        out.id = EffectId::new();
        out.keyframes.sort_by_key(|k| k.at);
        Ok(out)
    };
    if let ClipContent::Generator { fx } = &mut c.content {
        *fx = instance(fx, FxKind::Generator)?;
    }
    c.effects = c
        .effects
        .iter()
        .map(|e| instance(e, FxKind::Effect))
        .collect::<Result<_>>()?;
    let caption = matches!(c.content, ClipContent::Caption { .. });
    for t in [&mut c.transition_in, &mut c.transition_out]
        .into_iter()
        .flatten()
    {
        if caption {
            return Err(CoreError::Invalid("captions take no transitions".into()));
        }
        if t.duration <= Time::ZERO {
            return Err(CoreError::Invalid("a transition needs a duration".into()));
        }
        t.fx = instance(&t.fx, FxKind::Transition)?;
    }
    for m in &mut c.masks {
        m.keyframes.sort_by_key(|k| k.at);
    }
    Ok(c)
}

/// Whether `content` may sit on a `kind` track. [`Project::normalize`]
/// holds files read from disk to the same rule.
pub(crate) fn fits_track(project: &Project, content: &ClipContent, kind: TrackKind) -> bool {
    match content {
        ClipContent::Media { media, .. } => {
            let Some(item) = project.media(*media) else {
                return false;
            };
            match kind {
                TrackKind::Audio => {
                    item.kind != MediaKind::Image
                        && item.info.as_ref().is_none_or(|i| !i.audio.is_empty())
                }
                TrackKind::Video | TrackKind::Overlay => item.kind != MediaKind::Audio,
                TrackKind::Title | TrackKind::Caption => false,
            }
        }
        ClipContent::Caption { .. } => kind == TrackKind::Caption,
        ClipContent::Title { .. } => kind == TrackKind::Title,
        ClipContent::Overlay { .. } => matches!(kind, TrackKind::Overlay | TrackKind::Video),
        ClipContent::Generator { .. } => matches!(
            kind,
            TrackKind::Video | TrackKind::Overlay | TrackKind::Title
        ),
    }
}

/// Retime a media clip: the source it plays stays the same, so the clip
/// lasts `duration × old / new`. Keyframes stretch with it.
fn set_speed(project: &Project, id: ClipId, speed: f64, ripple: bool) -> Result<Plan> {
    let tl = &project.timeline;
    let (track, clip) = find_clip(tl, id)?;
    editable_track(tl, track.id)?;
    if !(speed.is_finite() && (0.01..=100.0).contains(&speed)) {
        return Err(CoreError::Invalid(
            "speed must be between 0.01 and 100".into(),
        ));
    }
    if clip.media().is_none() {
        return Err(CoreError::Invalid("only media clips change speed".into()));
    }
    let ratio = clip.speed / speed;
    let mut duration = clip.duration.scale(ratio).max(min_len(tl));
    let ix = clip_index(track, id);
    if !ripple && let Some(next) = track.clips.get(ix + 1) {
        duration = duration.min(next.start - clip.start);
    }
    let mut c = clip.clone();
    c.speed = speed;
    c.duration = duration;
    // A ramp keeps its shape at the new pace.
    for k in &mut c.retime.keys {
        k.at = k.at.scale(ratio);
        k.speed = (k.speed * speed / clip.speed).clamp(MIN_SPEED, MAX_SPEED);
    }
    if c.is_ramped() {
        c.settle_ramp();
    }
    for k in &mut c.keyframes {
        k.at = k.at.scale(ratio);
    }
    for e in &mut c.effects {
        e.scale_keyframes(ratio);
    }
    for m in &mut c.masks {
        m.scale_keyframes(ratio);
    }
    c.audio.fade_in = c.audio.fade_in.min(duration);
    c.audio.fade_out = c.audio.fade_out.min(duration);
    let shift = if ripple {
        duration - clip.duration
    } else {
        Time::ZERO
    };
    let mut clips: Vec<Clip> = track.clips[..ix].to_vec();
    clips.push(c);
    clips.extend(track.clips[ix + 1..].iter().map(|c| Clip {
        start: c.start + shift,
        ..c.clone()
    }));
    Ok(ops(diff_track(track, clips)))
}

/// Give a media clip a speed ramp (see [`Command::SetSpeedRamp`]). The
/// source it plays stays; keyframes stretch with its length.
fn set_speed_ramp(
    project: &Project,
    id: ClipId,
    points: &[[f64; 2]],
    keep_length: bool,
    ripple: bool,
) -> Result<Plan> {
    let tl = &project.timeline;
    let (track, clip) = find_clip(tl, id)?;
    editable_track(tl, track.id)?;
    if clip.media().is_none() {
        return Err(CoreError::Invalid("only media clips change speed".into()));
    }
    let mut points = crate::speed::clean_points(points).map_err(CoreError::Invalid)?;
    let source = clip.source_length().as_secs_f64();
    let duration = clip.duration.as_secs_f64().max(1e-9);
    let flat = points.windows(2).all(|w| (w[0][1] - w[1][1]).abs() < 1e-9);
    let mut c = clip.clone();
    let length = if points.len() < 2 || flat {
        // One speed again: the one given, else what the clip averages.
        c.retime.keys.clear();
        c.speed = match points.first() {
            Some(p) if !keep_length => p[1],
            _ => clip.speed,
        };
        Time::from_secs_f64(source / c.speed)
    } else {
        let mut m = crate::speed::shape_mean(&points);
        if keep_length {
            let scale = (source / duration) / m;
            for p in &mut points {
                p[1] = (p[1] * scale).clamp(MIN_SPEED, MAX_SPEED);
            }
            m = crate::speed::shape_mean(&points);
        }
        let length = Time::from_secs_f64(source / m.max(MIN_SPEED));
        c.retime.keys = points
            .iter()
            .map(|[u, s]| SpeedKey {
                at: length.scale(*u),
                speed: *s,
            })
            .collect();
        length
    };
    let length = length.max(min_len(tl));
    let ratio = length.as_secs_f64() / duration;
    for k in &mut c.keyframes {
        k.at = k.at.scale(ratio);
    }
    for e in &mut c.effects {
        e.scale_keyframes(ratio);
    }
    for m in &mut c.masks {
        m.scale_keyframes(ratio);
    }
    let ix = clip_index(track, id);
    let mut end = length;
    if !ripple && let Some(next) = track.clips.get(ix + 1) {
        end = end.min(next.start - clip.start);
    }
    c.duration = length;
    let mut c = if end < length {
        with_duration(&c, end)
    } else {
        c
    };
    c.settle_ramp();
    c.audio.fade_in = c.audio.fade_in.min(c.duration);
    c.audio.fade_out = c.audio.fade_out.min(c.duration);
    let shift = if ripple {
        c.duration - clip.duration
    } else {
        Time::ZERO
    };
    let mut clips: Vec<Clip> = track.clips[..ix].to_vec();
    clips.push(c);
    clips.extend(track.clips[ix + 1..].iter().map(|c| Clip {
        start: c.start + shift,
        ..c.clone()
    }));
    Ok(ops(diff_track(track, clips)))
}

/// The asset `r` names, which must be of `kind`.
fn fx_asset<'a>(project: &'a Project, r: &FxRef, kind: FxKind) -> Result<&'a FxAsset> {
    let asset =
        fx::asset(project, r).ok_or_else(|| CoreError::Invalid(format!("no FX program `{r}`")))?;
    if asset.kind != kind {
        return Err(CoreError::Invalid(format!(
            "`{}` is a {}, not a {}",
            asset.name,
            asset.kind.label().to_lowercase(),
            kind.label().to_lowercase()
        )));
    }
    Ok(asset)
}

/// `fx` with unknown parameters dropped and values coerced to their kinds.
fn coerced(fx: &FxInstance, asset: &FxAsset) -> FxInstance {
    let mut out = fx.clone();
    out.params = fx
        .params
        .iter()
        .filter_map(|(name, v)| {
            let spec = asset.param(name)?;
            Some((name.clone(), spec.kind.coerce(v, &spec.default)))
        })
        .collect();
    out.keyframes = fx
        .keyframes
        .iter()
        .filter_map(|k| {
            let spec = asset.param(&k.param)?;
            Some(ParamKeyframe {
                value: spec.kind.coerce(&k.value, &spec.default),
                ..k.clone()
            })
        })
        .collect();
    out
}

/// Whether the clip shows a picture effects can process.
fn has_picture(track: &Track, clip: &Clip) -> bool {
    track.kind != TrackKind::Audio && !matches!(clip.content, ClipContent::Caption { .. })
}

fn add_effect(
    project: &Project,
    clips: &[ClipId],
    effect: &FxInstance,
    index: Option<usize>,
) -> Result<Plan> {
    let tl = &project.timeline;
    let asset = fx_asset(project, &effect.fx, FxKind::Effect)?;
    if clips.is_empty() {
        return Err(CoreError::Invalid("no clips given".into()));
    }
    let effect = coerced(effect, asset);
    let mut out = Vec::new();
    for (i, id) in clips.iter().enumerate() {
        let (track, clip) = find_clip(tl, *id)?;
        if !has_picture(track, clip) {
            return Err(CoreError::Invalid(format!(
                "{id} has no picture to apply an effect to"
            )));
        }
        out.extend(
            update_clip(tl, *id, |c| {
                let mut e = effect.clone();
                // The first clip keeps the caller's id so it can refer to it.
                if i > 0 || c.effects.iter().any(|x| x.id == e.id) {
                    e.id = EffectId::new();
                }
                let at = index.unwrap_or(c.effects.len()).min(c.effects.len());
                c.effects.insert(at, e);
            })?
            .ops,
        );
    }
    Ok(ops(out))
}

/// Edit the effect stack of `clip` where effect `effect` is at `ix`.
fn with_effect(
    tl: &Timeline,
    clip: ClipId,
    effect: EffectId,
    f: impl FnOnce(&mut Vec<FxInstance>, usize),
) -> Result<Plan> {
    let (_, c) = find_clip(tl, clip)?;
    let ix = c
        .effects
        .iter()
        .position(|e| e.id == effect)
        .ok_or_else(|| CoreError::Invalid(format!("{clip} has no effect {effect}")))?;
    update_clip(tl, clip, |c| f(&mut c.effects, ix))
}

/// Set (`Some`) or remove (`None`, nearest within half a frame) a keyframe
/// on an effect, generator or transition instance of `clip`.
fn set_effect_keyframe(
    project: &Project,
    clip: ClipId,
    effect: EffectId,
    param: &str,
    at: Time,
    key: Option<(&ParamValue, crate::model::Ease)>,
) -> Result<Plan> {
    let tl = &project.timeline;
    let (track, c) = find_clip(tl, clip)?;
    // The instance's clock origin in timeline time.
    let origin = if c.effects.iter().any(|e| e.id == effect)
        || matches!(&c.content, ClipContent::Generator { fx } if fx.id == effect)
    {
        c.start
    } else {
        fx::transitions(track)
            .into_iter()
            .find(|p| p.transition.fx.id == effect)
            .map(|p| p.range.start)
            .ok_or_else(|| CoreError::Invalid(format!("{clip} has no effect {effect}")))?
    };
    let rel = at - origin;
    let tolerance = Time(tl.settings.rate.frame_duration().0 / 2);
    let mut after = c.clone();
    let inst = instance_mut(&mut after, effect)
        .ok_or_else(|| CoreError::Invalid(format!("{clip} has no effect {effect}")))?;
    let asset = fx::asset(project, &inst.fx)
        .ok_or_else(|| CoreError::Invalid(format!("no FX program `{}`", inst.fx)))?;
    let spec = asset.param(param).ok_or_else(|| {
        CoreError::Invalid(format!("`{}` has no parameter `{param}`", asset.name))
    })?;
    match key {
        Some((value, ease)) => {
            if !spec.kind.accepts(value) && spec.kind.coerce(value, &spec.default) == spec.default {
                return Err(CoreError::Invalid(format!(
                    "value does not fit parameter `{param}`"
                )));
            }
            let value = spec.kind.coerce(value, &spec.default);
            inst.keyframes
                .retain(|k| !(k.param == param && (k.at - rel).0.abs() <= tolerance.0));
            inst.keyframes.push(ParamKeyframe {
                param: param.to_owned(),
                at: rel,
                value,
                ease,
            });
            inst.keyframes.sort_by_key(|k| k.at);
        }
        None => {
            let ix = inst
                .keyframes
                .iter()
                .enumerate()
                .filter(|(_, k)| k.param == param && (k.at - rel).0.abs() <= tolerance.0)
                .min_by_key(|(_, k)| (k.at - rel).0.abs())
                .map(|(i, _)| i)
                .ok_or_else(|| CoreError::Invalid(format!("no `{param}` keyframe at {at}")))?;
            inst.keyframes.remove(ix);
        }
    }
    Ok(ops(vec![Op::UpdateClip {
        track: track.id,
        before: Box::new(c.clone()),
        after: Box::new(after),
    }]))
}

/// The effect, generator or transition instance `id` of a clip.
fn instance_mut(c: &mut Clip, id: EffectId) -> Option<&mut FxInstance> {
    if let Some(e) = c.effects.iter_mut().find(|e| e.id == id) {
        return Some(e);
    }
    if let ClipContent::Generator { fx } = &mut c.content
        && fx.id == id
    {
        return Some(fx);
    }
    [&mut c.transition_in, &mut c.transition_out]
        .into_iter()
        .flatten()
        .find(|t| t.fx.id == id)
        .map(|t| &mut t.fx)
}

fn set_transition(
    project: &Project,
    id: ClipId,
    edge: Edge,
    transition: Option<&Transition>,
) -> Result<Plan> {
    let tl = &project.timeline;
    let (track, clip) = find_clip(tl, id)?;
    editable_track(tl, track.id)?;
    if matches!(clip.content, ClipContent::Caption { .. }) {
        return Err(CoreError::Invalid("captions take no transitions".into()));
    }
    let transition = match transition {
        Some(t) => {
            let asset = fx_asset(project, &t.fx.fx, FxKind::Transition)?;
            if t.duration <= Time::ZERO {
                return Err(CoreError::Invalid("a transition needs a duration".into()));
            }
            Some(Transition {
                fx: coerced(&t.fx, asset),
                ..t.clone()
            })
        }
        None => None,
    };
    let mut plan = update_clip(tl, id, |c| match edge {
        Edge::Start => c.transition_in = transition,
        Edge::End => c.transition_out = transition,
    })?;
    // One transition per cut: the clip it touches there lets go of its
    // own, so this one is what plays and removing it leaves a plain cut.
    let neighbour = track.clips.iter().find(|n| {
        n.id != id
            && n.enabled
            && match edge {
                Edge::Start => n.end() == clip.start,
                Edge::End => n.start == clip.end(),
            }
    });
    if let Some(n) = neighbour {
        plan.ops.extend(
            update_clip(tl, n.id, |n| match edge {
                Edge::Start => n.transition_out = None,
                Edge::End => n.transition_in = None,
            })?
            .ops,
        );
    }
    Ok(plan)
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
    Ok(ops(vec![Op::InsertClip {
        track,
        clip: Box::new(clip),
    }]))
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
            before: Box::new(before.clone()),
            after: Box::new(after),
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
        return Err(locked(track));
    }
    Ok(track)
}

fn locked(track: &Track) -> CoreError {
    CoreError::Invalid(format!("track {} is locked", track.name))
}

/// The first locked track `ops` would change. A lock keeps a track's clips
/// and the track itself; its mix, caption style and lock stay editable, as
/// in the track header.
pub(crate) fn locked_track<'a>(project: &'a Project, ops: &[Op]) -> Option<&'a Track> {
    ops.iter().find_map(|op| {
        let id = match op {
            Op::InsertClip { track, .. }
            | Op::RemoveClip { track, .. }
            | Op::UpdateClip { track, .. } => *track,
            Op::RemoveTrack { track, .. } => track.id,
            _ => return None,
        };
        project.timeline.track(id).filter(|t| t.locked)
    })
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

/// Source media available before the clip's first frame, in timeline time
/// at the clip's speed; `None` = unlimited.
fn room_before(clip: &Clip) -> Option<Time> {
    match &clip.content {
        ClipContent::Media { source_in, .. } => {
            Some(-clip.timeline_offset(-(*source_in).max(Time::ZERO)))
        }
        _ => None,
    }
}

/// Source media available after the clip's last frame, in timeline time at
/// the clip's speed; `None` = unlimited or unknown.
fn room_after(project: &Project, clip: &Clip) -> Option<Time> {
    let ClipContent::Media {
        media, source_in, ..
    } = &clip.content
    else {
        return None;
    };
    media_duration(project, *media).map(|d| {
        let left = (d - *source_in - clip.source_length()).max(Time::ZERO);
        (clip.timeline_offset(clip.source_length() + left) - clip.duration).max(Time::ZERO)
    })
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
/// An edge that is cut away loses its transition.
pub fn cut_out(clips: &[Clip], range: TimeRange) -> Vec<Clip> {
    let mut next = Vec::with_capacity(clips.len() + 1);
    for c in clips {
        if c.end() <= range.start || c.start >= range.end {
            next.push(c.clone());
            continue;
        }
        if c.start < range.start {
            let mut left = with_duration(c, range.start - c.start);
            left.transition_out = None;
            next.push(left);
        }
        if c.end() > range.end {
            let mut right = trim_start(c, range.end);
            right.transition_in = None;
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
        for m in &mut c.masks {
            m.keep_until(duration);
        }
    }
    if c.is_ramped() {
        c.settle_ramp();
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
        speed: u64,
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
                offset: *source_in - c.source_offset(c.start - range.start),
                speed: c.speed.to_bits(),
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
    left.transition_out = None;
    let mut right = trim_start(clip, at);
    right.id = ClipId::new();
    right.audio.fade_in = Time::ZERO;
    right.transition_in = None;
    // Each half owns its effect instances.
    for e in &mut right.effects {
        e.id = EffectId::new();
    }
    if let ClipContent::Generator { fx } = &mut right.content {
        fx.id = EffectId::new();
    }
    (left, right)
}

/// The same clip with its head trimmed so it starts at timeline time `at`.
pub fn trim_start(clip: &Clip, at: Time) -> Clip {
    let offset = at - clip.start;
    let mut c = clip.clone();
    c.start = at;
    c.duration = clip.duration - offset;
    match &mut c.content {
        ClipContent::Media { source_in, .. } => {
            *source_in = *source_in + clip.source_offset(offset)
        }
        ClipContent::Generator { fx } => fx.shift_keyframes(-offset),
        ClipContent::Caption { words, .. } => {
            for w in words {
                *w = *w - offset;
            }
        }
        _ => {}
    }
    for e in &mut c.effects {
        e.shift_keyframes(-offset);
    }
    for m in &mut c.masks {
        if offset > Time::ZERO {
            m.keep_from(offset);
        } else {
            m.keyframes.iter_mut().for_each(|k| k.at = k.at - offset);
        }
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
    if clip.is_ramped() {
        c.retime.keys = if offset > Time::ZERO {
            crate::speed::slice(&clip.retime.keys, offset, clip.duration)
        } else {
            let mut keys = clip.retime.keys.clone();
            keys.iter_mut().for_each(|k| k.at = k.at - offset);
            keys
        };
        c.settle_ramp();
    }
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
