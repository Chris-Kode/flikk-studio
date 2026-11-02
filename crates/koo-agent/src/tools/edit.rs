//! Edit tools: arguments → `Command`s proposed through the bus.

use super::analysis::timeline_words;
use super::view::audio_segments;
use super::{Done, Job, JobDone, Step, ToolCtx, propose};
use crate::args::*;
use crate::bridge::Content;
use koo_core::audiofx::{AudioFx, AudioFxKind};
use koo_core::command::{Command, Edge, InsertMode, PlacedClip};
use koo_core::cuts::{FADE, Speech};
use koo_core::fx::ParamKind;
use koo_core::heard::{self, Target};
use koo_core::memory::Presets;
use koo_core::model::{
    BlendMode, ClipColor, ClipContent, Curves, Ducking, Ease, Fit, Format, Grade, MarkerKind,
    Property, TrackKind, Transform,
};
use koo_core::query;
use koo_core::{Time, TimeRange, TrackId};
use serde_json::Value;

pub fn serve(ctx: &mut ToolCtx, tool: &str, args: &Args) -> Result<Step, ToolError> {
    let reason = args
        .opt_str("reason")?
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or(tool)
        .to_owned();
    let commands = match tool {
        "blade" => {
            let (t, aligned_note) = aligned(ctx, args, args.time("at")?)?;
            let (at, note) = off_words(ctx, args, t)?;
            let commands = vec![Command::Blade {
                at,
                tracks: track_scope(ctx.project(), args)?,
            }];
            let notes = aligned_note.into_iter().chain(note);
            return Ok(with_notes(propose(ctx, commands, &reason), notes).into());
        }
        "ripple_delete" => {
            let range = args.range("range")?;
            let (start, start_aligned) = aligned(ctx, args, range.start)?;
            let (end, end_aligned) = aligned(ctx, args, range.end)?;
            let (start, a) = off_words(ctx, args, start)?;
            let (end, b) = off_words(ctx, args, end)?;
            if start >= end {
                return fail(format!(
                    "{range} lies inside one word; remove whole words with `cut_words`, \
                     or pass `snap: false` to cut exactly there"
                ));
            }
            let commands = vec![Command::RippleDelete {
                range: TimeRange::new(start, end),
                tracks: track_scope(ctx.project(), args)?,
                fade: FADE,
            }];
            let notes = [start_aligned, end_aligned, a, b].into_iter().flatten();
            return Ok(with_notes(propose(ctx, commands, &reason), notes).into());
        }
        "cut_words" => return cut_words(ctx, args, &reason),
        "insert_clip" => insert_clip(ctx, args)?,
        "move_clip" => {
            let p = ctx.project();
            vec![Command::MoveClip {
                clip: clip_id(p, args.str("clip")?)?,
                at: args.time("at")?,
                track: args.opt_str("track")?.map(|t| track_id(p, t)).transpose()?,
            }]
        }
        "trim" => vec![Command::Trim {
            clip: clip_id(ctx.project(), args.str("clip")?)?,
            edge: edge(args)?,
            delta: args.duration("delta")?,
            ripple: args.opt_bool("ripple")?.unwrap_or(false),
        }],
        "add_marker" => {
            let range = match (args.opt_range("range")?, args.opt_time("at")?) {
                (Some(r), _) => r,
                (None, Some(t)) => TimeRange::new(t, t),
                (None, None) => return Err(ToolError("give `at` or `range`".into())),
            };
            let kind = match args.opt_str("kind")? {
                None | Some("note") => MarkerKind::Note,
                Some("protected") => MarkerKind::Protected,
                Some(k) => return Err(ToolError(format!("unknown marker kind `{k}`"))),
            };
            vec![Command::AddMarker {
                range,
                kind,
                note: args.str("note")?.to_owned(),
            }]
        }

        "add_zoom" => {
            let p = ctx.project();
            let target = match args.0.get("target") {
                Some(Value::Array(xy)) if xy.len() == 2 => [
                    xy[0].as_f64().unwrap_or(0.0) as f32,
                    xy[1].as_f64().unwrap_or(0.0) as f32,
                ],
                None | Some(Value::Null) => [0.0, 0.0],
                Some(_) => return Err(ToolError("`target` is [x, y]".into())),
            };
            let presets = Presets::default();
            vec![Command::AddZoom {
                clip: clip_id(p, args.str("clip")?)?,
                range: args.range("range")?,
                target,
                scale: args
                    .opt_f64("scale")?
                    .map(|s| s as f32)
                    .unwrap_or(presets.zoom_scale),
                ease: ease(args)?.unwrap_or(presets.zoom_ease),
            }]
        }
        "set_keyframe" => {
            let p = ctx.project();
            let clip = clip_id(p, args.str("clip")?)?;
            let property = property(args)?;
            vec![Command::SetKeyframe {
                clip,
                property,
                // Timeline time; the planner converts to clip-relative.
                at: args.time("at")?,
                value: args.f64("value")? as f32,
                ease: ease(args)?.unwrap_or_default(),
            }]
        }

        "remove_keyframe" => {
            let p = ctx.project();
            vec![Command::RemoveKeyframe {
                clip: clip_id(p, args.str("clip")?)?,
                property: property(args)?,
                at: args.time("at")?,
            }]
        }
        "set_transform" => return set_transform(ctx, args, &reason),
        "set_mask" => return super::masks::set_mask(ctx, args, &reason),
        "remove_mask" => super::masks::remove_mask(ctx, args)?,
        "place" => return super::layout::place(ctx, args, &reason),
        "animate" => super::layout::animate(ctx, args)?,

        "set_speed" => super::speed::set_speed(ctx, args)?,
        "set_clips_enabled" => vec![Command::SetClipsEnabled {
            clips: some_clips(ctx, args)?,
            enabled: args
                .opt_bool("enabled")?
                .ok_or_else(|| ToolError("`enabled` is required".into()))?,
        }],
        "rename_clip" => vec![Command::RenameClip {
            clip: clip_id(ctx.project(), args.str("clip")?)?,
            name: args
                .opt_str("name")?
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_owned),
        }],
        "set_format" => {
            let s = &ctx.project().timeline.settings;
            let short = match args.opt_f64("resolution")? {
                Some(r) => r.round().clamp(16.0, 8192.0) as u32,
                None => s.short_side(),
            };
            let (width, height) = match (args.opt_f64("width")?, args.opt_f64("height")?) {
                (Some(w), Some(h)) => (w.round() as u32 & !1, h.round() as u32 & !1),
                (None, None) => match args.opt_str("format")?.map(str::trim) {
                    Some(name) => Format::ALL
                        .into_iter()
                        .find(|f| f.name() == name)
                        .ok_or_else(|| ToolError(format!("unknown format `{name}`")))?
                        .size(short),
                    None => s.scaled(short),
                },
                _ => return Err(ToolError("give both `width` and `height`".into())),
            };
            let mut out = vec![Command::SetFormat { width, height }];
            if args.opt_bool("fill")? == Some(true) {
                out.extend(
                    ctx.project()
                        .framed_clips()
                        .filter(|c| c.transform.fit != Fit::Cover)
                        .map(|c| Command::SetTransform {
                            clip: c.id,
                            transform: Transform {
                                fit: Fit::Cover,
                                ..c.transform.clone()
                            },
                        }),
                );
            }
            out
        }
        "set_clip_color" => {
            let color = match args.opt_str("color")?.map(str::trim) {
                None | Some("none") | Some("") => None,
                Some(name) => Some(
                    ClipColor::ALL
                        .into_iter()
                        .find(|c| c.label().eq_ignore_ascii_case(name))
                        .ok_or_else(|| ToolError(format!("unknown color `{name}`")))?,
                ),
            };
            vec![Command::SetClipColor {
                clips: some_clips(ctx, args)?,
                color,
            }]
        }
        "duplicate_clips" => duplicate_clips(ctx, args)?,

        "set_curves" => set_curves(ctx, args)?,

        "set_audio_effects" | "add_audio_effect" | "remove_audio_effect" => {
            audio_effects(ctx, tool, args)?
        }

        "generate_captions" => return generate_captions(ctx, args, &reason),
        "edit_caption" => {
            let p = ctx.project();
            let clip = clip_id(p, args.str("clip")?)?;
            if !matches!(
                p.timeline.clip(clip).map(|(_, c)| &c.content),
                Some(ClipContent::Caption { .. })
            ) {
                return Err(ToolError(format!("{clip} is not a caption")));
            }
            let text = args.opt_str("text")?.map(str::to_owned);
            let range = args.opt_range("range")?;
            if text.is_none() && range.is_none() {
                return Err(ToolError("give `text` and/or `range`".into()));
            }
            vec![Command::EditCaption { clip, text, range }]
        }
        "apply_title" => {
            let (mut commands, track) = track_or_new(ctx, args, TrackKind::Title, "T1")?;
            let Some(track) = track else {
                return Ok(
                    propose_then(ctx, commands, &reason, TrackKind::Title, |_, t| {
                        title(args, t)
                    })
                    .into(),
                );
            };
            commands.push(title(args, track)?);
            commands
        }

        "set_primary" => {
            let p = ctx.project();
            let clips = clip_ids(p, &args.strings("clips")?)?;
            if clips.is_empty() {
                return Err(ToolError("`clips` is empty".into()));
            }
            let mut out = Vec::new();
            for id in clips {
                let current = p
                    .timeline
                    .clip(id)
                    .map(|(_, c)| c.grade.clone())
                    .unwrap_or_default();
                out.push(Command::SetGrade {
                    clips: vec![id],
                    grade: merge_grade(current, args)?,
                });
            }
            out
        }
        "set_timeline_grade" => vec![Command::SetTimelineGrade {
            grade: merge_grade(ctx.project().timeline.grade.clone(), args)?,
        }],
        "apply_lut" => {
            let p = ctx.project();
            vec![Command::ApplyLut {
                clips: clip_ids(p, &args.strings("clips")?)?,
                lut: args.opt_str("lut")?.map(Into::into),
            }]
        }
        "copy_grade" => {
            let p = ctx.project();
            vec![Command::CopyGrade {
                from: clip_id(p, args.str("from")?)?,
                to: clip_ids(p, &args.strings("to")?)?,
            }]
        }

        "set_gain" => vec![Command::SetGain {
            clip: clip_id(ctx.project(), args.str("clip")?)?,
            db: args.f64("db")? as f32,
        }],
        "add_fade" => vec![Command::AddFade {
            clip: clip_id(ctx.project(), args.str("clip")?)?,
            edge: edge(args)?,
            duration: args.duration("duration")?.max(Time::ZERO),
        }],
        "set_ducking" => {
            let ducking = match args.opt_str("music_track")? {
                None => None,
                Some(t) => Some(Ducking {
                    music_track: track_id(ctx.project(), t)?,
                    reduction_db: args
                        .opt_f64("reduction_db")?
                        .map(|d| -(d.abs()) as f32)
                        .unwrap_or(Presets::default().ducking_db),
                    attack: Time::from_secs_f64(args.opt_f64("attack")?.unwrap_or(0.2)),
                    release: Time::from_secs_f64(args.opt_f64("release")?.unwrap_or(0.5)),
                }),
            };
            vec![Command::SetDucking { ducking }]
        }
        "set_playlist" => {
            let p = ctx.project();
            vec![Command::SetPlaylist {
                track: track_id(p, args.str("track")?)?,
                at: args.time("at")?,
                songs: args
                    .strings("songs")?
                    .iter()
                    .map(|s| media_id(p, s))
                    .collect::<ToolResultOf<_>>()?,
            }]
        }
        "normalize" => return normalize(ctx, args, reason),
        "place_sfx" => {
            let p = ctx.project();
            let media = media_id(p, args.str("media")?)?;
            let duration = media_duration(p, media)?;
            let track = match args.opt_str("track")? {
                Some(t) => track_id(p, t)?,
                None => p
                    .timeline
                    .tracks
                    .iter()
                    .rev()
                    .find(|t| t.kind == TrackKind::Audio && !t.locked)
                    .map(|t| t.id)
                    .ok_or_else(|| ToolError("there is no audio track".into()))?,
            };
            vec![Command::InsertClip {
                media,
                source: TimeRange::new(Time::ZERO, duration),
                track,
                at: args.time("at")?,
                mode: InsertMode::Overwrite,
            }]
        }

        "add_overlay" => {
            let (mut commands, track) = track_or_new(ctx, args, TrackKind::Overlay, "O1")?;
            let Some(track) = track else {
                return Ok(
                    propose_then(ctx, commands, &reason, TrackKind::Overlay, |ctx, t| {
                        overlay(ctx, args, t)
                    })
                    .into(),
                );
            };
            commands.push(overlay(ctx, args, track)?);
            commands
        }

        other => return Err(ToolError(format!("unknown tool `{other}`"))),
    };
    Ok(propose(ctx, commands, &reason).into())
}

/// Words heard within 5 s of `t`, and the speech to place cuts in.
fn speech_near<'p>(
    ctx: &'p ToolCtx,
    t: Time,
) -> Option<(Speech<'p>, Vec<koo_core::query::TimelineWord>)> {
    let analysis = ctx.services.analysis.as_ref()?;
    let project = ctx.project();
    let around = Time::from_secs_f64(5.0);
    let words = query::words_in_range(project, TimeRange::new(t - around, t + around), |fp| {
        analysis.analysis(fp)
    });
    Some((Speech::new(project, |fp| analysis.analysis(fp)), words))
}

/// `t` moved onto the beat, downbeat or onset the call's `align` asks for,
/// preferring one clear of spoken words, with a line for the model saying
/// where.
fn aligned(ctx: &ToolCtx, args: &Args, t: Time) -> ToolResultOf<(Time, Option<String>)> {
    let Some(name) = args.opt_str("align")? else {
        return Ok((t, None));
    };
    let Some(target) = Target::parse(name) else {
        return fail(format!(
            "unknown align `{name}`: use beat, downbeat or onset"
        ));
    };
    let Some(analysis) = ctx.services.analysis.as_ref() else {
        return fail("Analysis is not available in this build yet.");
    };
    let found = heard::cut_targets(ctx.project(), t, target, |fp| analysis.analysis(fp));
    let Some(&nearest) = found.first() else {
        return Ok((
            t,
            Some(format!(
                "No {} near {t}; the cut stays there.",
                target.name()
            )),
        ));
    };
    let clear = speech_near(ctx, t).and_then(|(speech, words)| {
        found
            .iter()
            .copied()
            .find(|&c| speech.snap(&words, c).is_none())
    });
    let at = clear.unwrap_or(nearest);
    let mut note = if at == t {
        format!("{t} is on a {}.", target.name())
    } else {
        format!(
            "{t} moved {:+.2} s to the {} at {at}.",
            (at - t).as_secs_f64(),
            target.name()
        )
    };
    if at != nearest {
        note.push_str(&format!(
            " The {} at {nearest} is on a word.",
            target.name()
        ));
    }
    Ok((at, Some(note)))
}

/// `t`, or the nearest pause when it falls on a word (unless `snap` is
/// false), with a line for the model saying so.
fn off_words(ctx: &ToolCtx, args: &Args, t: Time) -> ToolResultOf<(Time, Option<String>)> {
    if args.opt_bool("snap")? == Some(false) {
        return Ok((t, None));
    }
    let Some((speech, words)) = speech_near(ctx, t) else {
        return Ok((t, None));
    };
    let Some(snap) = speech.snap(&words, t) else {
        return Ok((t, None));
    };
    let w = &words[snap.word];
    let mut note = format!(
        "{t} fell on \"{}\" ({}); cut at {} instead.",
        w.word.text.trim(),
        w.timeline,
        snap.cut.at
    );
    if snap.cut.tight {
        note.push_str(" No pause there (the words run together), so the cut may be heard.");
    }
    Ok((snap.cut.at, Some(note)))
}

/// Append `notes` to a proposal's result.
pub(super) fn with_notes(mut done: Done, notes: impl IntoIterator<Item = String>) -> Done {
    for note in notes {
        if let Some(Content::Text { text }) = done.result.content.first_mut() {
            text.push('\n');
            text.push_str(&note);
        }
    }
    done
}

/// Remove runs of words, each given by its first and last word's `@` time,
/// cutting in the pauses around them.
fn cut_words(ctx: &mut ToolCtx, args: &Args, reason: &str) -> Result<Step, ToolError> {
    let Some(analysis) = ctx.services.analysis.clone() else {
        return fail("Analysis is not available in this build yet.");
    };
    let Some(Value::Array(runs)) = args.value("runs") else {
        return fail("`runs` is a list of {from, to, text}");
    };
    if runs.is_empty() {
        return fail("`runs` is empty");
    }
    let project = ctx.project();
    let all = TimeRange::new(Time::ZERO, project.timeline.duration());
    let words = query::words_in_range(project, all, |fp| analysis.analysis(fp));
    let speech = Speech::new(project, |fp| analysis.analysis(fp));
    let near = |t: Time| -> ToolResultOf<usize> {
        let k = words
            .iter()
            .enumerate()
            .min_by_key(|(_, w)| (w.timeline.start - t).0.abs())
            .map(|(k, _)| k)
            .ok_or_else(|| ToolError("there is no transcribed speech on the timeline".into()))?;
        if (words[k].timeline.start - t).0.abs() > Time::from_millis(250).0 {
            return fail(format!(
                "no word starts at {t}; the nearest is \"{}\" at {}",
                words[k].word.text.trim(),
                words[k].timeline.start
            ));
        }
        Ok(k)
    };
    let mut ranges = Vec::new();
    let mut lines = Vec::new();
    for run in runs {
        let run = Args(
            run.as_object()
                .ok_or_else(|| ToolError("each run is {from, to, text}".into()))?,
            args.1,
        );
        let first = near(run.time("from")?)?;
        let last = match run.opt_time("to")? {
            Some(t) => near(t)?,
            None => first,
        };
        if last < first {
            return fail(format!(
                "`to` {} is before `from` {}",
                words[last].timeline.start, words[first].timeline.start
            ));
        }
        let said: Vec<&str> = words[first..=last]
            .iter()
            .map(|w| w.word.text.trim())
            .collect();
        let said = said.join(" ");
        if let Some(expected) = run.opt_str("text")?
            && normalized(expected) != normalized(&said)
        {
            return fail(format!(
                "the words from {} to {} are \"{said}\", not \"{expected}\"",
                words[first].timeline.start, words[last].timeline.start
            ));
        }
        for removal in speech.removals(&words, first, last) {
            let mut line = format!("\"{said}\": {}", removal.range);
            if removal.tight {
                line.push_str(
                    ", no pause at an edge (the words run together), so the cut may be heard",
                );
            }
            lines.push(line);
            ranges.push(removal.range);
        }
    }
    let mut ranges = query::merge_ranges(ranges);
    ranges.reverse();
    let commands = ranges
        .into_iter()
        .map(|range| Command::RippleDelete {
            range,
            tracks: None,
            fade: FADE,
        })
        .collect();
    let mut done = propose(ctx, commands, reason);
    done.excerpts = lines.clone();
    Ok(with_notes(done, lines).into())
}

/// Lowercase words without punctuation, for comparing what was said.
fn normalized(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '\'')
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

pub(super) fn edge(args: &Args) -> ToolResultOf<Edge> {
    match args.str("edge")? {
        "start" | "in" => Ok(Edge::Start),
        "end" | "out" => Ok(Edge::End),
        e => fail(format!("`edge` is start or end, not `{e}`")),
    }
}

fn property(args: &Args) -> ToolResultOf<Property> {
    let name = args.str("property")?;
    Property::from_name(name).ok_or_else(|| {
        let names: Vec<_> = Property::ALL.iter().map(|p| p.name()).collect();
        ToolError(format!(
            "unknown property `{name}`; one of {}",
            names.join(", ")
        ))
    })
}

/// `clips`, required and non-empty.
fn some_clips(ctx: &ToolCtx, args: &Args) -> ToolResultOf<Vec<koo_core::ClipId>> {
    let clips = clip_ids(ctx.project(), &args.strings("clips")?)?;
    if clips.is_empty() {
        return fail("`clips` is empty");
    }
    Ok(clips)
}

pub(super) fn ease(args: &Args) -> ToolResultOf<Option<Ease>> {
    Ok(match args.opt_str("ease")? {
        None => None,
        Some("linear") => Some(Ease::Linear),
        Some("ease_in_out") => Some(Ease::EaseInOut),
        Some("ease_in") => Some(Ease::EaseIn),
        Some("ease_out") => Some(Ease::EaseOut),
        Some("hold") => Some(Ease::Hold),
        Some(e) => return fail(format!("unknown ease `{e}`")),
    })
}

fn media_duration(p: &koo_core::model::Project, media: koo_core::MediaId) -> ToolResultOf<Time> {
    p.media(media)
        .and_then(|m| m.info.as_ref())
        .map(|i| i.duration)
        .filter(|d| *d > Time::ZERO)
        .ok_or_else(|| ToolError(format!("{media} has not been probed yet; give `source`")))
}

fn insert_clip(ctx: &mut ToolCtx, args: &Args) -> ToolResultOf<Vec<Command>> {
    let p = ctx.project();
    let media = media_id(p, args.str("media")?)?;
    let source = match args.opt_range("source")? {
        Some(r) => r,
        None => TimeRange::new(Time::ZERO, media_duration(p, media)?),
    };
    let item = p
        .media(media)
        .ok_or_else(|| ToolError("media not found".into()))?;
    let kind = if item.kind == koo_core::model::MediaKind::Audio {
        TrackKind::Audio
    } else {
        TrackKind::Video
    };
    let track = match args.opt_str("track")? {
        Some(t) => track_id(p, t)?,
        None => first_track(p, kind)
            .map(|t| t.id)
            .ok_or_else(|| ToolError(format!("there is no {} track", kind_name(kind))))?,
    };
    let mode = match args.opt_str("mode")? {
        None | Some("insert") => InsertMode::Insert,
        Some("overwrite") => InsertMode::Overwrite,
        Some(m) => return fail(format!("unknown mode `{m}`")),
    };
    Ok(vec![Command::InsertClip {
        media,
        source,
        track,
        at: args.time("at")?,
        mode,
    }])
}

/// The given track, the first of `kind`, or (when there is none) an
/// `AddTrack` command and `None`: the caller proposes it first and looks
/// the new track up in the draft.
pub(super) fn track_or_new(
    ctx: &ToolCtx,
    args: &Args,
    kind: TrackKind,
    name: &str,
) -> ToolResultOf<(Vec<Command>, Option<TrackId>)> {
    let p = ctx.project();
    if let Some(t) = args.opt_str("track")? {
        return Ok((vec![], Some(track_id(p, t)?)));
    }
    if let Some(t) = first_track(p, kind) {
        return Ok((vec![], Some(t.id)));
    }
    Ok((
        vec![Command::AddTrack {
            kind,
            index: 0,
            name: name.into(),
        }],
        None,
    ))
}

/// Propose the new track, then what `make` puts on it.
pub(super) fn propose_then(
    ctx: &mut ToolCtx,
    add_track: Vec<Command>,
    reason: &str,
    kind: TrackKind,
    make: impl FnOnce(&ToolCtx, TrackId) -> ToolResultOf<Command>,
) -> Done {
    let first = propose(ctx, add_track, reason);
    if first.result.is_error {
        return first;
    }
    let Some(track) = first_track(ctx.project(), kind).map(|t| t.id) else {
        return Done::error("could not create the track");
    };
    match make(ctx, track) {
        Ok(c) => {
            let mut done = propose(ctx, vec![c], reason);
            done.summary = format!("new {} track + {}", kind_name(kind), done.summary);
            done
        }
        Err(ToolError(e)) => Done::error(e),
    }
}

fn title(args: &Args, track: TrackId) -> ToolResultOf<Command> {
    Ok(Command::ApplyTitle {
        track,
        template: args.str("template")?.to_owned(),
        range: args.range("range")?,
        text: args.str("text")?.to_owned(),
    })
}

fn overlay(ctx: &ToolCtx, args: &Args, track: TrackId) -> ToolResultOf<Command> {
    let speaker = match args.opt_str("speaker")? {
        None => None,
        Some(s) => {
            let range = args.range("range")?;
            let ids: Vec<_> = timeline_words(ctx.project(), ctx.services, range)
                .map(|(_, speakers)| speakers.iter().map(|s| s.id).collect())
                .unwrap_or_default();
            Some(speaker_id(&ids, s)?)
        }
    };
    Ok(Command::AddOverlay {
        track,
        asset: args.str("asset")?.into(),
        range: args.range("range")?,
        speaker,
    })
}

fn numbers<const N: usize>(
    args: &Args,
    key: &str,
    current: [f32; N],
    shape: &str,
) -> ToolResultOf<[f32; N]> {
    match args.value(key) {
        None => Ok(current),
        Some(Value::Array(v)) if v.len() == N && v.iter().all(Value::is_number) => {
            let mut out = [0.0; N];
            for (o, x) in out.iter_mut().zip(v) {
                *o = x.as_f64().unwrap_or_default() as f32;
            }
            Ok(out)
        }
        Some(_) => fail(format!("`{key}` is {shape}")),
    }
}

fn rgb(args: &Args, key: &str, current: [f32; 3]) -> ToolResultOf<[f32; 3]> {
    numbers(args, key, current, "[r, g, b]")
}

/// `clip` and `clips` together: the clips a transform tool changes.
pub(super) fn target_clips(ctx: &ToolCtx, args: &Args) -> ToolResultOf<Vec<koo_core::ClipId>> {
    let p = ctx.project();
    let mut ids = Vec::new();
    if let Some(c) = args.opt_str("clip")? {
        ids.push(clip_id(p, c)?);
    }
    if args.has("clips") {
        ids.extend(clip_ids(p, &args.strings("clips")?)?);
    }
    if ids.is_empty() {
        return fail("give `clip` or `clips`");
    }
    Ok(ids)
}

/// The static transform of each clip, or with `at`, the transform there:
/// keyframed properties get a keyframe at `at`, the rest change statically.
fn set_transform(ctx: &mut ToolCtx, args: &Args, reason: &str) -> Result<Step, ToolError> {
    if args.opt_bool("group")? == Some(true) {
        return super::layout::transform_group(ctx, args, reason);
    }
    let p = ctx.project();
    let at = args.opt_time("at")?;
    let relative = args.opt_bool("relative")?.unwrap_or(false);
    let mut commands = Vec::new();
    let mut notes = Vec::new();
    for id in target_clips(ctx, args)? {
        let (_, clip) = p
            .timeline
            .clip(id)
            .ok_or_else(|| ToolError("clip not found".into()))?;
        match at {
            Some(t) => {
                if t < clip.start || t > clip.end() {
                    return fail(format!(
                        "{t} is outside clip {} ({} – {})",
                        short(id),
                        clip.start,
                        clip.end()
                    ));
                }
                let local = t - clip.start;
                let tf = merge_transform(clip.transform_at(local), args, relative)?;
                commands.extend(Command::transform_at(clip, local, &tf));
            }
            None => {
                let tf = merge_transform(clip.transform.clone(), args, relative)?;
                let keyed: Vec<&str> = Property::TRANSFORM
                    .into_iter()
                    .filter(|p| (tf.get(*p) - clip.transform.get(*p)).abs() > 1e-6)
                    .filter(|p| clip.keyframes.iter().any(|k| k.property == *p))
                    .map(|p| p.name())
                    .collect();
                if !keyed.is_empty() {
                    notes.push(format!(
                        "Clip {}: {} has keyframes, which override the static value; pass `at` to key it there.",
                        short(id),
                        keyed.join(", ")
                    ));
                }
                if tf != clip.transform {
                    commands.push(Command::SetTransform {
                        clip: id,
                        transform: tf,
                    });
                }
            }
        }
    }
    if commands.is_empty() {
        return Ok(Done::text("no change", "Nothing changes.").into());
    }
    Ok(with_notes(propose(ctx, commands, reason), notes).into())
}

/// `t` with the values in `args`. With `relative`, position, rotation,
/// skew, anchor and corner pin add to the current values, and scale,
/// stretch and opacity multiply them; crop, flip, blend and fit are always set.
fn merge_transform(mut t: Transform, args: &Args, relative: bool) -> ToolResultOf<Transform> {
    let add = |current: f32, v: f32| if relative { current + v } else { v };
    let times = |current: f32, v: f32| if relative { current * v } else { v };
    let pair = |key: &str, current: [f32; 2], f: &dyn Fn(f32, f32) -> f32| {
        let given = numbers(args, key, [f32::NAN; 2], "[x, y]")?;
        Ok::<_, ToolError>(if given[0].is_nan() {
            current
        } else {
            [f(current[0], given[0]), f(current[1], given[1])]
        })
    };
    t.position = pair("position", t.position, &add)?;
    t.stretch = pair("stretch", t.stretch, &times)?.map(|v| v.max(0.0));
    t.skew_deg = pair("skew", t.skew_deg, &add)?
        .map(|v| v.clamp(-koo_core::compose::MAX_SKEW, koo_core::compose::MAX_SKEW));
    t.anchor = pair("anchor", t.anchor, &add)?;
    if let Some(pin) = corner_points(args, "corner_pin")? {
        for (corner, given) in t.corner_pin.iter_mut().zip(pin) {
            *corner = [add(corner[0], given[0]), add(corner[1], given[1])];
        }
    }
    if let Some(v) = args.opt_f64("scale")? {
        t.scale = times(t.scale, v as f32).max(0.0);
    }
    if let Some(v) = args.opt_f64("rotation")? {
        t.rotation_deg = add(t.rotation_deg, v as f32);
    }
    if let Some(v) = args.opt_f64("opacity")? {
        t.opacity = times(t.opacity, v as f32).clamp(0.0, 1.0);
    }
    t.crop = numbers(
        args,
        "crop",
        t.crop,
        "[left, top, right, bottom], each 0..1",
    )?
    .map(|c| c.clamp(0.0, 1.0));
    match args.value("flip") {
        None => {}
        Some(Value::Array(v)) if v.len() == 2 && v.iter().all(Value::is_boolean) => {
            t.flip = [v[0].as_bool() == Some(true), v[1].as_bool() == Some(true)];
        }
        Some(_) => return fail("`flip` is [horizontal, vertical] booleans"),
    }
    if let Some(name) = args.opt_str("fit")? {
        t.fit = Fit::ALL
            .into_iter()
            .find(|f| f.name() == name)
            .ok_or_else(|| ToolError(format!("unknown fit `{name}`")))?;
    }
    if let Some(name) = args.opt_str("blend")? {
        t.blend = BlendMode::ALL
            .into_iter()
            .find(|b| blend_name(*b) == name)
            .ok_or_else(|| ToolError(format!("unknown blend mode `{name}`")))?;
    }
    Ok(t)
}

/// Four `[x, y]` points (top-left, top-right, bottom-right, bottom-left)
/// under `key`, if given.
pub(super) fn corner_points(args: &Args, key: &str) -> ToolResultOf<Option<[[f32; 2]; 4]>> {
    let Some(v) = args.value(key) else {
        return Ok(None);
    };
    let points: Option<Vec<[f32; 2]>> = v.as_array().filter(|a| a.len() == 4).and_then(|a| {
        a.iter()
            .map(|p| {
                let p = p.as_array().filter(|p| p.len() == 2)?;
                Some([p[0].as_f64()? as f32, p[1].as_f64()? as f32])
            })
            .collect()
    });
    match points {
        Some(p) => Ok(Some([p[0], p[1], p[2], p[3]])),
        None => fail(format!(
            "`{key}` is four [x, y] points: top-left, top-right, bottom-right, bottom-left"
        )),
    }
}

pub(super) fn blend_name(b: BlendMode) -> String {
    serde_json::to_value(b)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Copies of clips at a new time (the earliest lands at `at`, the others
/// keep their offsets), on their own tracks or one target track.
fn duplicate_clips(ctx: &ToolCtx, args: &Args) -> ToolResultOf<Vec<Command>> {
    let p = ctx.project();
    let ids = some_clips(ctx, args)?;
    let target = args.opt_str("track")?.map(|t| track_id(p, t)).transpose()?;
    let found: Vec<_> = ids
        .iter()
        .filter_map(|id| p.timeline.clip(*id))
        .map(|(t, c)| (t.id, c.clone()))
        .collect();
    let earliest = found.iter().map(|(_, c)| c.start).min().unwrap_or_default();
    let at = match args.opt_time("at")? {
        Some(t) => t,
        // Right after the last of them.
        None => found.iter().map(|(_, c)| c.end()).max().unwrap_or_default(),
    };
    let mode = match args.opt_str("mode")? {
        None | Some("overwrite") => InsertMode::Overwrite,
        Some("insert") => InsertMode::Insert,
        Some(m) => return fail(format!("unknown mode `{m}`")),
    };
    let clips = found
        .into_iter()
        .map(|(track, mut clip)| {
            clip.start = at + (clip.start - earliest);
            PlacedClip {
                track: target.unwrap_or(track),
                clip,
            }
        })
        .collect();
    Ok(vec![Command::PlaceClips { clips, mode }])
}

/// `[[x, y], …]` control points in 0..1, sorted by input.
fn curve(args: &Args, key: &str, current: &[[f32; 2]]) -> ToolResultOf<Vec<[f32; 2]>> {
    let Some(v) = args.value(key) else {
        return Ok(current.to_vec());
    };
    let shape = || {
        ToolError(format!(
            "`{key}` is a list of [input, output] points in 0..1"
        ))
    };
    let mut points = v
        .as_array()
        .ok_or_else(shape)?
        .iter()
        .map(|p| match p.as_array().map(Vec::as_slice) {
            Some([x, y]) => Some([
                x.as_f64()?.clamp(0.0, 1.0) as f32,
                y.as_f64()?.clamp(0.0, 1.0) as f32,
            ]),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(shape)?;
    points.sort_by(|a, b| a[0].total_cmp(&b[0]));
    Ok(points)
}

fn merge_curves(mut c: Curves, args: &Args) -> ToolResultOf<Curves> {
    c.master = curve(args, "master", &c.master)?;
    c.red = curve(args, "red", &c.red)?;
    c.green = curve(args, "green", &c.green)?;
    c.blue = curve(args, "blue", &c.blue)?;
    Ok(c)
}

fn set_curves(ctx: &ToolCtx, args: &Args) -> ToolResultOf<Vec<Command>> {
    let p = ctx.project();
    let clips = clip_ids(p, &args.strings("clips")?)?;
    if clips.is_empty() {
        let mut grade = p.timeline.grade.clone();
        grade.curves = merge_curves(grade.curves, args)?;
        return Ok(vec![Command::SetTimelineGrade { grade }]);
    }
    clips
        .into_iter()
        .map(|id| {
            let mut grade = p
                .timeline
                .clip(id)
                .map(|(_, c)| c.grade.clone())
                .unwrap_or_default();
            grade.curves = merge_curves(grade.curves, args)?;
            Ok(Command::SetGrade {
                clips: vec![id],
                grade,
            })
        })
        .collect()
}

/// `{kind, params, enabled, id?}` → an audio effect; `id` keeps an existing
/// one's identity.
fn audio_fx(v: &Value, current: &[AudioFx]) -> ToolResultOf<AudioFx> {
    let bad = || ToolError("an audio effect is {kind, params?, enabled?, id?}".into());
    let obj = v.as_object().ok_or_else(bad)?;
    let kind_name = obj.get("kind").and_then(Value::as_str).ok_or_else(bad)?;
    let kind = AudioFxKind::from_name(kind_name)
        .ok_or_else(|| ToolError(format!("unknown audio effect `{kind_name}`")))?;
    let existing = obj
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| find_audio_fx(current, id));
    let mut fx = existing.cloned().unwrap_or_else(|| AudioFx::new(kind));
    fx.kind = kind;
    if let Some(e) = obj.get("enabled").and_then(Value::as_bool) {
        fx.enabled = e;
    }
    if let Some(params) = obj.get("params") {
        let params = params
            .as_object()
            .ok_or_else(|| ToolError("`params` is {name: number}".into()))?;
        let specs = kind.params();
        for (name, value) in params {
            let Some(spec) = specs.iter().find(|s| &s.name == name) else {
                let names: Vec<_> = specs.iter().map(|s| s.name.as_str()).collect();
                return fail(format!(
                    "{kind_name} has no parameter `{name}`; it has {}",
                    names.join(", ")
                ));
            };
            let v = value
                .as_f64()
                .ok_or_else(|| ToolError(format!("`{name}` must be a number")))?;
            let v = match spec.kind {
                ParamKind::Float { min, max } => v.clamp(min, max),
                _ => v,
            };
            fx.params.insert(name.clone(), v);
        }
    }
    Ok(fx)
}

fn find_audio_fx<'a>(list: &'a [AudioFx], text: &str) -> Option<&'a AudioFx> {
    let text = strip_token(text);
    let text = text.rsplit('/').next().unwrap_or(text).to_ascii_lowercase();
    list.iter()
        .find(|f| text.len() >= 4 && f.id.0.simple().to_string().starts_with(&text))
}

/// Clip audio effects (`SetAudioEffects`) or a track's inserts
/// (`SetTrackMix`), replaced, added to or removed from.
fn audio_effects(ctx: &ToolCtx, tool: &str, args: &Args) -> ToolResultOf<Vec<Command>> {
    let p = ctx.project();
    enum Target {
        Clip(koo_core::ClipId),
        Track(TrackId),
    }
    let (target, current) = match (args.opt_str("clip")?, args.opt_str("track")?) {
        (Some(c), None) => {
            let id = clip_id(p, c)?;
            let (_, clip) = p
                .timeline
                .clip(id)
                .ok_or_else(|| ToolError("clip not found".into()))?;
            (Target::Clip(id), clip.audio.effects.clone())
        }
        (None, Some(t)) => {
            let id = track_id(p, t)?;
            let track = p
                .timeline
                .track(id)
                .ok_or_else(|| ToolError("track not found".into()))?;
            (Target::Track(id), track.mix.effects.clone())
        }
        _ => return fail("give `clip` or `track`"),
    };
    let effects = match tool {
        "set_audio_effects" => {
            let list = args
                .value("effects")
                .and_then(Value::as_array)
                .ok_or_else(|| ToolError("`effects` is a list".into()))?;
            list.iter()
                .map(|v| audio_fx(v, &current))
                .collect::<ToolResultOf<Vec<_>>>()?
        }
        "add_audio_effect" => {
            let mut spec = serde_json::Map::new();
            spec.insert("kind".into(), Value::String(args.str("kind")?.to_owned()));
            if let Some(params) = args.value("params") {
                spec.insert("params".into(), params.clone());
            }
            let fx = audio_fx(&Value::Object(spec), &[])?;
            let mut list = current;
            let at = args
                .opt_f64("index")?
                .map(|i| i.max(0.0) as usize)
                .unwrap_or(list.len())
                .min(list.len());
            list.insert(at, fx);
            list
        }
        _ => {
            let effect = args.str("effect")?;
            let id = find_audio_fx(&current, effect)
                .map(|f| f.id)
                .ok_or_else(|| ToolError(format!("no audio effect `{effect}` there")))?;
            current.into_iter().filter(|f| f.id != id).collect()
        }
    };
    Ok(vec![match target {
        Target::Clip(clip) => Command::SetAudioEffects { clip, effects },
        Target::Track(track) => {
            let mut mix = p
                .timeline
                .track(track)
                .map(|t| t.mix.clone())
                .unwrap_or_default();
            mix.effects = effects;
            Command::SetTrackMix { track, mix }
        }
    }])
}

fn merge_grade(mut g: Grade, args: &Args) -> ToolResultOf<Grade> {
    let f = |key: &str, cur: f32| -> ToolResultOf<f32> {
        Ok(args.opt_f64(key)?.map(|v| v as f32).unwrap_or(cur))
    };
    g.exposure = f("exposure", g.exposure)?;
    g.contrast = f("contrast", g.contrast)?;
    g.saturation = f("saturation", g.saturation)?;
    g.gamma = f("gamma", g.gamma)?;
    g.temperature = f("temperature", g.temperature)?;
    g.tint = f("tint", g.tint)?;
    g.lift = rgb(args, "lift", g.lift)?;
    g.gamma_rgb = rgb(args, "gamma_rgb", g.gamma_rgb)?;
    g.gain = rgb(args, "gain", g.gain)?;
    g.offset = rgb(args, "offset", g.offset)?;
    g.shadows = f("shadows", g.shadows)?;
    g.highlights = f("highlights", g.highlights)?;
    g.hue = f("hue", g.hue)?;
    g.curves = merge_curves(g.curves, args)?;
    Ok(g)
}

/// Captions from transcript words: break on sentence ends, pauses and
/// length (PRD C1).
fn generate_captions(ctx: &mut ToolCtx, args: &Args, reason: &str) -> Result<Step, ToolError> {
    let p = ctx.project();
    let range = args.range_or_all("range", p)?;
    let track = match args.opt_str("track")? {
        Some(t) => track_id(p, t)?,
        None => first_track(p, TrackKind::Caption)
            .map(|t| t.id)
            .ok_or_else(|| ToolError("there is no caption track".into()))?,
    };
    let style = koo_core::memory::CaptionStyle::default();
    let max_chars = args
        .opt_f64("max_chars")?
        .map(|m| m.clamp(8.0, 200.0) as usize)
        .unwrap_or(style.max_chars_per_line as usize);
    let (words, _) = timeline_words(p, ctx.services, range)?;
    if words.is_empty() {
        return Err(ToolError(format!("No transcribed speech in {range}.")));
    }
    let pause = Time::from_millis(600);
    let longest = Time::from_millis(4000);
    let mut commands = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let mut text = words[i].text.trim().to_owned();
        let mut j = i + 1;
        while j < words.len() {
            let prev = &words[j - 1];
            let next = &words[j];
            let ends_sentence = prev.text.trim_end().ends_with(['.', '?', '!']);
            let candidate_len = text.chars().count() + 1 + next.text.trim().chars().count();
            if ends_sentence
                || next.range.start - prev.range.end >= pause
                || candidate_len > max_chars
                || next.range.end - words[i].range.start > longest
                || next.speaker != words[i].speaker
            {
                break;
            }
            text.push(' ');
            text.push_str(next.text.trim());
            j += 1;
        }
        // Hold until the next caption starts, if that is soon.
        let mut end = words[j - 1].range.end;
        if let Some(next) = words.get(j)
            && next.range.start - end < pause
        {
            end = next.range.start;
        }
        let start = words[i].range.start;
        let spoken = words[i..j]
            .iter()
            .flat_map(|w| {
                std::iter::repeat_n(w.range.start - start, w.text.split_whitespace().count())
            })
            .collect();
        commands.push(Command::AddCaption {
            track,
            range: TimeRange::new(start, end.max(start + Time::from_millis(300))),
            text,
            speaker: words[i].speaker,
            words: spoken,
        });
        i = j;
    }
    // Keep captions from overlapping after the minimum-length padding.
    for k in 1..commands.len() {
        let prev_end = match &commands[k - 1] {
            Command::AddCaption { range, .. } => range.end,
            _ => continue,
        };
        if let Command::AddCaption { range, words, .. } = &mut commands[k]
            && range.start < prev_end
        {
            for w in words.iter_mut() {
                *w = *w + range.start - prev_end;
            }
            range.start = prev_end;
            if range.end <= range.start {
                range.end = range.start + Time::from_millis(200);
            }
        }
    }
    let n = commands.len();
    let mut done = propose(ctx, commands, reason);
    done.summary = format!("generate_captions {range}: {n} captions");
    Ok(done.into())
}

/// Measure, then propose gains that reach the target (off-thread).
fn normalize(ctx: &mut ToolCtx, args: &Args, reason: String) -> Result<Step, ToolError> {
    let p = ctx.project();
    let Some(analysis) = ctx.services.analysis.clone() else {
        return Err(ToolError(
            "Loudness measurement is not available in this build yet.".into(),
        ));
    };
    let target = args
        .opt_f64("target_lufs")?
        .map(|t| t as f32)
        .unwrap_or(Presets::default().loudness_lufs);
    let wanted = clip_ids(p, &args.strings("clips")?)?;
    let range = args.range_or_all("range", p)?;
    let all = if wanted.is_empty() {
        audio_segments(p, range)
    } else {
        let whole = TimeRange::new(Time::ZERO, p.timeline.duration().max(Time(1)));
        audio_segments(p, whole)
            .into_iter()
            .filter(|s| wanted.contains(&s.0))
            .collect()
    };
    if all.is_empty() {
        return Err(ToolError("No audio clips to normalize.".into()));
    }
    let job: Job = Box::new(move || {
        let mut commands = Vec::new();
        let mut excerpts = Vec::new();
        for (clip, _, path, source, _) in all.iter().take(48) {
            match analysis.loudness(path, Some(*source)) {
                Ok(l) if l.integrated_lufs.is_finite() => {
                    let db = ((target - l.integrated_lufs) * 10.0).round() / 10.0;
                    excerpts.push(format!(
                        "{clip}: {:.1} LUFS → gain {db:+.1} dB",
                        l.integrated_lufs
                    ));
                    commands.push(Command::SetGain { clip: *clip, db });
                }
                Ok(_) => excerpts.push(format!("{clip}: silent, skipped")),
                Err(e) => excerpts.push(format!("{clip}: {e}")),
            }
        }
        if commands.is_empty() {
            return JobDone::Done(Done::error(format!(
                "Could not measure any clip: {}",
                excerpts.join("; ")
            )));
        }
        JobDone::Propose {
            commands,
            reason: format!("{reason} (target {target} LUFS)"),
            excerpts,
        }
    });
    Ok(Step::Job(job))
}
