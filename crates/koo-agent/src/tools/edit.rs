//! Edit tools: arguments → `Command`s proposed through the bus.

use super::analysis::timeline_words;
use super::view::audio_segments;
use super::{Done, Job, JobDone, Step, ToolCtx, propose};
use crate::args::*;
use koo_core::command::{Command, Edge, InsertMode};
use koo_core::memory::Presets;
use koo_core::model::{ClipContent, Ducking, Ease, Grade, MarkerKind, Property, TrackKind};
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
        "blade" => vec![Command::Blade {
            at: args.time("at")?,
            tracks: track_scope(ctx.project(), args)?,
        }],
        "ripple_delete" => vec![Command::RippleDelete {
            range: args.range("range")?,
            tracks: track_scope(ctx.project(), args)?,
        }],
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
            delta: Time::from_secs_f64(args.f64("delta")?),
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
            let property = match args.str("property")? {
                "position_x" => Property::PositionX,
                "position_y" => Property::PositionY,
                "scale" => Property::Scale,
                "rotation" => Property::Rotation,
                "opacity" => Property::Opacity,
                other => return Err(ToolError(format!("unknown property `{other}`"))),
            };
            vec![Command::SetKeyframe {
                clip,
                property,
                // Timeline time; the planner stores it clip-relative.
                at: args.time("at")?,
                value: args.f64("value")? as f32,
                ease: ease(args)?.unwrap_or_default(),
            }]
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
                return Ok(propose_then(ctx, commands, &reason, args, TrackKind::Title).into());
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
        "match_grade" => {
            return Err(ToolError(
                "Shot matching needs scopes, which are not available in this build yet. Use view_frame and set_primary instead.".into(),
            ));
        }

        "set_gain" => vec![Command::SetGain {
            clip: clip_id(ctx.project(), args.str("clip")?)?,
            db: args.f64("db")? as f32,
        }],
        "add_fade" => vec![Command::AddFade {
            clip: clip_id(ctx.project(), args.str("clip")?)?,
            edge: edge(args)?,
            duration: Time::from_secs_f64(args.f64("duration")?.max(0.0)),
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
                return Ok(propose_then(ctx, commands, &reason, args, TrackKind::Overlay).into());
            };
            commands.push(overlay(ctx, args, track)?);
            commands
        }

        other => return Err(ToolError(format!("unknown tool `{other}`"))),
    };
    Ok(propose(ctx, commands, &reason).into())
}

fn edge(args: &Args) -> ToolResultOf<Edge> {
    match args.str("edge")? {
        "start" => Ok(Edge::Start),
        "end" => Ok(Edge::End),
        e => fail(format!("`edge` is start or end, not `{e}`")),
    }
}

fn ease(args: &Args) -> ToolResultOf<Option<Ease>> {
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
fn track_or_new(
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

/// Propose the new track, then the title or overlay on it.
fn propose_then(
    ctx: &mut ToolCtx,
    add_track: Vec<Command>,
    reason: &str,
    args: &Args,
    kind: TrackKind,
) -> Done {
    let first = propose(ctx, add_track, reason);
    if first.result.is_error {
        return first;
    }
    let Some(track) = first_track(ctx.project(), kind).map(|t| t.id) else {
        return Done::error("could not create the track");
    };
    let command = match kind {
        TrackKind::Title => title(args, track),
        _ => overlay(ctx, args, track),
    };
    match command {
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

fn rgb(args: &Args, key: &str, current: [f32; 3]) -> ToolResultOf<[f32; 3]> {
    match args.0.get(key) {
        None | Some(Value::Null) => Ok(current),
        Some(Value::Array(v)) if v.len() == 3 && v.iter().all(Value::is_number) => Ok([
            v[0].as_f64().unwrap_or_default() as f32,
            v[1].as_f64().unwrap_or_default() as f32,
            v[2].as_f64().unwrap_or_default() as f32,
        ]),
        Some(_) => fail(format!("`{key}` is [r, g, b]")),
    }
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
        commands.push(Command::AddCaption {
            track,
            range: TimeRange::new(
                words[i].range.start,
                end.max(words[i].range.start + Time::from_millis(300)),
            ),
            text,
            speaker: words[i].speaker,
        });
        i = j;
    }
    // Keep captions from overlapping after the minimum-length padding.
    for k in 1..commands.len() {
        let prev_end = match &commands[k - 1] {
            Command::AddCaption { range, .. } => range.end,
            _ => continue,
        };
        if let Command::AddCaption { range, .. } = &mut commands[k]
            && range.start < prev_end
        {
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
