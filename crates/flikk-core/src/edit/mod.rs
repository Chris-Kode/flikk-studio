//! Planners: [`Command`] → [`Plan`] against a read-only project.
//!
//! Planners never mutate. They compute the desired end state of each touched
//! track and let [`diff_track`] emit ops in an order that never overlaps
//! clips mid-way. The bus validates every plan by applying it to a copy.

mod bounds;
mod cut;
mod diff;
mod effects;
mod keyframes;
mod place;
mod protect;
mod retime;
mod slice;
mod target;
mod trim;
mod update;

pub(crate) use bounds::media_duration;
pub(crate) use diff::diff_track;
pub(crate) use place::fits_track;
pub(crate) use slice::{cut_out, split_clip, stretch_keyframes};

use crate::command::{Command, Edge, Plan, Warning};
use crate::error::{CoreError, Result};
use crate::fx::{self, FxKind, FxRef};
use crate::ids::*;
use crate::model::*;
use crate::ops::{Op, locked_track};
use crate::time::Rate;
use cut::{blade, delete_clips, ripple_delete};
use effects::{add_effect, coerced, fx_asset, set_effect_keyframe, set_transition, with_effect};
use keyframes::{add_zoom, remove_keyframe, set_keyframe};
use place::{insert_clip, move_clip, place_clips, set_playlist};
use protect::warn_protected;
use retime::{set_speed, set_speed_ramp};
use target::{editable_track, find_clip, locked};
use trim::{roll, slide, slip, trim};
use update::{insert_new, swap_media, update_clip, update_clips, update_track};

/// The plan for `command` on `project`, which stays unchanged. Refused when
/// it would change a locked track; carries protected-range warnings when
/// [`Command::checks_protected`].
pub(crate) fn plan(command: &Command, project: &Project) -> Result<Plan> {
    let mut plan = plan_command(command, project)?;
    if let Some(track) = locked_track(project, &plan.ops) {
        return Err(locked(track));
    }
    if command.checks_protected() {
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
            update_track(tl, *track, |p| p.caption_style = style.clone())
        }
        Command::SetBins { bins } => Ok(ops(vec![Op::SetBins {
            before: project.bins.clone(),
            after: bins.clone(),
        }])),
        Command::SetTrackMix { track, mix } => update_track(tl, *track, |p| p.mix = mix.clone()),
        Command::SetTrackLocked { track, locked } => {
            update_track(tl, *track, |p| p.locked = *locked)
        }
        Command::RenameTrack { track, name } => {
            tl.track(*track).ok_or(CoreError::TrackNotFound(*track))?;
            let name = name.trim();
            if name.is_empty() {
                return Err(CoreError::Invalid("a track needs a name".into()));
            }
            update_track(tl, *track, |p| p.name = name.to_owned())
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

        Command::SwapMedia { from, to } => swap_media(project, *from, *to),
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

fn ops(ops: Vec<Op>) -> Plan {
    Plan {
        ops,
        warnings: Vec::new(),
    }
}
