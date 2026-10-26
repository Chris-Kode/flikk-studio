//! Read-project tools. Results carry only the slice asked for.

use super::{Done, Step, ToolCtx};
use crate::args::{Args, ToolError, clip_id, kind_name, range_json, secs, short};
use koo_core::model::{Clip, ClipContent, Grade, MarkerKind, Project, Track, Transform};
use koo_core::{Time, TimeRange};
use serde_json::{Value, json};

/// Clips listed per call before the result says it was cut short.
const MAX_CLIPS: usize = 400;

pub fn media_name(project: &Project, media: koo_core::MediaId) -> String {
    project
        .media(media)
        .and_then(|m| m.path.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "missing media".into())
}

pub fn clip_brief(project: &Project, clip: &Clip) -> Value {
    let mut v = json!({
        "id": short(clip.id),
        "start": secs(clip.start),
        "end": secs(clip.end()),
    });
    match &clip.content {
        ClipContent::Media {
            media, source_in, ..
        } => {
            v["media"] = json!(short(media));
            v["name"] = json!(media_name(project, *media));
            v["source_in"] = secs(*source_in);
        }
        ClipContent::Title { template, text } => {
            v["title"] = json!(text);
            v["template"] = json!(template);
        }
        ClipContent::Caption { text, speaker } => {
            v["caption"] = json!(text);
            if let Some(s) = speaker {
                v["speaker"] = json!(short(s));
            }
        }
        ClipContent::Overlay { asset, speaker } => {
            v["overlay"] = json!(asset.display().to_string());
            if let Some(s) = speaker {
                v["speaker"] = json!(short(s));
            }
        }
    }
    if clip.audio.gain_db != 0.0 {
        v["gain_db"] = json!(clip.audio.gain_db);
    }
    if clip.audio.fade_in > Time::ZERO {
        v["fade_in"] = secs(clip.audio.fade_in);
    }
    if clip.audio.fade_out > Time::ZERO {
        v["fade_out"] = secs(clip.audio.fade_out);
    }
    if clip.transform != Transform::default() {
        v["transformed"] = json!(true);
    }
    if !clip.keyframes.is_empty() {
        v["keyframes"] = json!(clip.keyframes.len());
    }
    if clip.grade != Grade::default() {
        v["graded"] = json!(true);
    }
    v
}

fn marker_json(m: &koo_core::model::Marker) -> Value {
    let mut v = json!({
        "id": short(m.id),
        "note": m.note,
        "kind": match m.kind { MarkerKind::Note => "note", MarkerKind::Protected => "protected" },
    });
    if m.range.is_empty() {
        v["at"] = secs(m.range.start);
    } else {
        v["range"] = range_json(m.range);
    }
    v
}

fn marker_in(m: &koo_core::model::Marker, range: TimeRange) -> bool {
    m.range.overlaps(&range) || range.contains(m.range.start)
}

fn track_json(t: &Track) -> Value {
    let mut v = json!({
        "id": short(t.id),
        "name": t.name,
        "kind": kind_name(t.kind),
    });
    if t.locked {
        v["locked"] = json!(true);
    }
    if t.mix.mute {
        v["muted"] = json!(true);
    }
    if t.mix.volume_db != 0.0 {
        v["volume_db"] = json!(t.mix.volume_db);
    }
    v
}

pub fn get_timeline(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let range = args.range_or_all("range", project)?;
    let tl = &project.timeline;
    let mut listed = 0;
    let mut truncated = false;
    let tracks: Vec<_> = tl
        .tracks
        .iter()
        .map(|t| {
            let mut v = track_json(t);
            let clips: Vec<_> = t
                .clips
                .iter()
                .filter(|c| c.range().overlaps(&range))
                .filter(|_| {
                    listed += 1;
                    truncated |= listed > MAX_CLIPS;
                    listed <= MAX_CLIPS
                })
                .map(|c| clip_brief(project, c))
                .collect();
            v["clips"] = json!(clips);
            v
        })
        .collect();
    let markers: Vec<_> = tl
        .markers
        .iter()
        .filter(|m| marker_in(m, range))
        .map(marker_json)
        .collect();
    let mut out = json!({
        "range": range_json(range),
        "duration": secs(tl.duration()),
        "fps": tl.settings.rate.fps(),
        "tracks": tracks,
        "markers": markers,
    });
    if let Some(d) = &tl.ducking {
        out["ducking"] = json!({
            "music_track": tl.track(d.music_track).map(|t| t.name.clone()),
            "reduction_db": d.reduction_db,
        });
    }
    if truncated {
        out["truncated"] = json!(format!(
            "only the first {MAX_CLIPS} clips; ask for a smaller range"
        ));
    }
    Ok(Done::json(
        format!("timeline {range}: {} clips", listed.min(MAX_CLIPS)),
        &out,
    )
    .into())
}

pub fn get_selection(ctx: &mut ToolCtx) -> Result<Step, ToolError> {
    let project = ctx.project();
    let ctx_json = crate::prompt::editor_context(project, ctx.editor);
    let clips: Vec<_> = ctx
        .editor
        .selection
        .clips
        .iter()
        .filter_map(|id| project.timeline.clip(*id))
        .map(|(t, c)| {
            let mut v = clip_brief(project, c);
            v["track"] = json!(t.name);
            v
        })
        .collect();
    let mut out = ctx_json;
    out["selected_clips"] = json!(clips);
    Ok(Done::json(format!("{} selected clips", clips.len()), &out).into())
}

pub fn list_media(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let bin = match args.opt_str("bin")? {
        Some(name) => Some(
            project
                .bins
                .iter()
                .find(|b| b.name.eq_ignore_ascii_case(name))
                .map(|b| b.id)
                .ok_or_else(|| ToolError(format!("no bin named `{name}`")))?,
        ),
        None => None,
    };
    let analysis = ctx.services.analysis.as_ref();
    let used = |id| {
        project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| c.media() == Some(id))
            .count()
    };
    let items: Vec<_> = project
        .media
        .iter()
        .filter(|m| bin.is_none() || m.bin == bin)
        .map(|m| {
            let mut v = json!({
                "id": short(m.id),
                "name": m.path.file_name().map(|n| n.to_string_lossy().into_owned()),
                "kind": format!("{:?}", m.kind).to_lowercase(),
                "on_timeline": used(m.id),
            });
            if let Some(info) = &m.info {
                v["duration"] = secs(info.duration);
                if let Some(vs) = &info.video {
                    v["video"] = json!(format!(
                        "{}x{} @ {:.3} fps",
                        vs.width,
                        vs.height,
                        vs.rate.fps()
                    ));
                }
                if !info.audio.is_empty() {
                    v["audio_streams"] = json!(info.audio.len());
                }
            }
            if let Some(b) = m.bin.and_then(|b| project.bins.iter().find(|x| x.id == b)) {
                v["bin"] = json!(b.name);
            }
            if !m.tags.is_empty() {
                v["tags"] = json!(m.tags);
            }
            let transcribed = m
                .fingerprint
                .as_ref()
                .and_then(|f| analysis.and_then(|a| a.analysis(f)))
                .is_some_and(|a| a.transcript.is_some());
            v["transcribed"] = json!(transcribed);
            v
        })
        .collect();
    let n = items.len();
    Ok(Done::json(format!("{n} media items"), &json!({ "media": items })).into())
}

pub fn get_clip(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let id = clip_id(project, args.str("clip")?)?;
    let (track, clip) = project
        .timeline
        .clip(id)
        .ok_or_else(|| ToolError("clip not found".into()))?;
    let mut v = clip_brief(project, clip);
    v["track"] = json!(track.name);
    v["track_kind"] = json!(kind_name(track.kind));
    if let ClipContent::Media {
        audio_stream: Some(s),
        ..
    } = &clip.content
    {
        v["audio_stream"] = json!(s);
    }
    v["transform"] = json!({
        "position": clip.transform.position,
        "scale": clip.transform.scale,
        "rotation_deg": clip.transform.rotation_deg,
        "opacity": clip.transform.opacity,
    });
    v["keyframes"] = clip
        .keyframes
        .iter()
        .map(|k| {
            json!({
                "property": k.property,
                "at": secs(clip.start + k.at),
                "value": k.value,
                "ease": k.ease,
            })
        })
        .collect();
    let g = &clip.grade;
    v["grade"] = json!({
        "exposure": g.exposure, "contrast": g.contrast, "saturation": g.saturation,
        "gamma": g.gamma, "temperature": g.temperature, "tint": g.tint,
        "lift": g.lift, "gamma_rgb": g.gamma_rgb, "gain": g.gain,
        "lut": g.lut.as_ref().map(|p| p.display().to_string()),
    });
    v["audio"] = json!({
        "gain_db": clip.audio.gain_db,
        "fade_in": secs(clip.audio.fade_in),
        "fade_out": secs(clip.audio.fade_out),
    });
    Ok(Done::json(format!("clip {id}"), &v).into())
}

pub fn get_markers(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let range = args.range_or_all("range", project)?;
    let markers: Vec<_> = project
        .timeline
        .markers
        .iter()
        .filter(|m| marker_in(m, range) || project.timeline.duration() == Time::ZERO)
        .map(marker_json)
        .collect();
    let n = markers.len();
    Ok(Done::json(format!("{n} markers"), &json!({ "markers": markers })).into())
}
