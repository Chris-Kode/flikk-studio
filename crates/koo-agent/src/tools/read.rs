//! Read-project tools. Results carry only the slice asked for.

use super::fx::{asset_json, fx_asset, instance_json, kind_name as fx_kind, param_spec_json};
use super::layout::without_neutral;
use super::{Done, Step, ToolCtx};
use crate::args::{Args, ToolError, clip_id, kind_name, num, range_json, secs, short};
use koo_core::audiofx::{AudioFx, AudioFxKind};
use koo_core::fx::{self, FxKind, FxLanguage};
use koo_core::model::{
    Clip, ClipAudio, ClipContent, Curves, Grade, MarkerKind, Project, Track, Transform,
};
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
        ClipContent::Caption { text, speaker, .. } => {
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
        ClipContent::Generator { fx } => {
            v["generator"] = json!(fx.fx.to_string());
            v["name"] = json!(clip.display_name(project));
        }
    }
    if clip.audio.gain_db != 0.0 {
        v["gain_db"] = num(clip.audio.gain_db);
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
    if let Some(name) = &clip.name {
        v["label"] = json!(name);
    }
    if let Some(c) = clip.color {
        v["color"] = json!(c.label().to_lowercase());
    }
    if !clip.enabled {
        v["enabled"] = json!(false);
    }
    if clip.speed != 1.0 {
        v["speed"] = json!(clip.speed);
    }
    if !clip.effects.is_empty() {
        v["effects"] = clip
            .effects
            .iter()
            .map(|e| json!(format!("{} {}", e.fx, e.id)))
            .collect();
    }
    if let Some(t) = &clip.transition_in {
        v["transition_in"] = json!(format!("{} {}", t.fx.fx, secs(t.duration)));
    }
    if let Some(t) = &clip.transition_out {
        v["transition_out"] = json!(format!("{} {}", t.fx.fx, secs(t.duration)));
    }
    if !clip.audio.effects.is_empty() {
        v["audio_effects"] = json!(clip.audio.effects.len());
    }
    v
}

fn curves_json(c: &Curves) -> Value {
    let points = |c: &[[f32; 2]]| -> Vec<Value> { c.iter().map(|p| json!(p.map(num))).collect() };
    json!({
        "master": points(&c.master), "red": points(&c.red),
        "green": points(&c.green), "blue": points(&c.blue),
    })
}

/// The grade's values, leaving out those at their neutral value.
fn grade_json(g: &Grade) -> Value {
    let all = |g: &Grade| {
        json!({
            "exposure": num(g.exposure), "contrast": num(g.contrast),
            "saturation": num(g.saturation), "gamma": num(g.gamma),
            "temperature": num(g.temperature), "tint": num(g.tint),
            "lift": g.lift.map(num), "gamma_rgb": g.gamma_rgb.map(num), "gain": g.gain.map(num),
            "offset": g.offset.map(num), "shadows": num(g.shadows),
            "highlights": num(g.highlights), "hue": num(g.hue),
            "lut": g.lut.as_ref().map(|p| p.display().to_string()),
        })
    };
    let mut v = without_neutral(all(g), &all(&Grade::default()));
    if !g.curves.is_identity() {
        v["curves"] = curves_json(&g.curves);
    }
    v
}

pub fn audio_fx_json(list: &[AudioFx]) -> Value {
    list.iter()
        .map(|f| {
            let values: serde_json::Map<String, Value> = f
                .kind
                .params()
                .iter()
                .map(|p| (p.name.clone(), json!(f.value(&p.name))))
                .collect();
            let mut v = json!({ "id": short(f.id), "kind": f.kind.name(), "params": values });
            if !f.enabled {
                v["enabled"] = json!(false);
            }
            v
        })
        .collect()
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
        v["volume_db"] = num(t.mix.volume_db);
    }
    if !t.mix.effects.is_empty() {
        v["audio_effects"] = audio_fx_json(&t.mix.effects);
    }
    v
}

pub fn get_timeline(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let range = args.range_or_all("range", project)?;
    let tl = &project.timeline;
    let mut listed = 0;
    let mut truncated = false;
    // Media names once, by id, instead of on every clip that uses them.
    let mut media = serde_json::Map::new();
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
                .map(|c| {
                    let mut v = clip_brief(project, c);
                    if let Some(brief) = v.as_object_mut()
                        && let Some(Value::String(id)) = brief.get("media").cloned()
                        && let Some(name) = brief.remove("name")
                    {
                        media.insert(id, name);
                    }
                    v
                })
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
        "size": [tl.settings.width, tl.settings.height],
        "tracks": tracks,
        "markers": markers,
        "media": media,
    });
    if let Some(d) = &tl.ducking {
        out["ducking"] = json!({
            "music_track": tl.track(d.music_track).map(|t| t.name.clone()),
            "reduction_db": num(d.reduction_db),
        });
    }
    if tl.grade != Grade::default() {
        out["grade"] = grade_json(&tl.grade);
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
    // The brief already leaves out what is at its default (speed, enabled…);
    // its counts and flags give way to the full values below, which do too.
    let mut v = clip_brief(project, clip);
    if let Some(brief) = v.as_object_mut() {
        for key in [
            "transformed",
            "graded",
            "keyframes",
            "effects",
            "gain_db",
            "fade_in",
            "fade_out",
            "audio_effects",
        ] {
            brief.remove(key);
        }
    }
    v["track"] = json!(track.name);
    v["track_kind"] = json!(kind_name(track.kind));
    if let ClipContent::Media {
        audio_stream: Some(s),
        ..
    } = &clip.content
    {
        v["audio_stream"] = json!(s);
    }
    if clip.transform != Transform::default() {
        v["transform"] = super::layout::transform_json(&clip.transform);
    }
    if !clip.masks.is_empty() {
        v["masks"] = super::masks::masks_json(clip);
    }
    if !clip.effects.is_empty() {
        v["effects"] = clip
            .effects
            .iter()
            .map(|e| instance_json(project, e, clip.start))
            .collect();
    }
    let track_ref = project.timeline.track(track.id);
    for (key, edge, t) in [
        ("transition_in", "in", &clip.transition_in),
        ("transition_out", "out", &clip.transition_out),
    ] {
        if let Some(t) = t {
            // Where it actually plays, resolved against the neighbours.
            let placed = track_ref.and_then(|tr| {
                fx::transitions(tr)
                    .into_iter()
                    .find(|p| p.transition.fx.id == t.fx.id)
                    .map(|p| range_json(p.range))
            });
            let origin = placed
                .as_ref()
                .and_then(|r| r["start"].as_f64())
                .map(Time::from_secs_f64)
                .unwrap_or(clip.start);
            let mut j = instance_json(project, &t.fx, origin);
            j["ref"] = json!(format!("@transition {}/{edge}", clip.id));
            j["duration"] = secs(t.duration);
            j["align"] = json!(t.align);
            if let Some(r) = placed {
                j["plays"] = r;
            }
            v[key] = j;
        }
    }
    if let ClipContent::Generator { fx } = &clip.content {
        let mut values = serde_json::Map::new();
        if let Some(asset) = fx::asset(project, &fx.fx) {
            for (k, val) in fx.values_at(asset, Time::ZERO) {
                values.insert(k, json!(val));
            }
        }
        let mut j = instance_json(project, fx, clip.start);
        j["values_at_start"] = Value::Object(values);
        v["generator"] = j;
    }
    if !clip.keyframes.is_empty() {
        v["keyframes"] = clip
            .keyframes
            .iter()
            .map(|k| {
                json!({
                    "property": k.property,
                    "at": secs(clip.start + k.at),
                    "value": num(k.value),
                    "ease": k.ease,
                })
            })
            .collect();
    }
    if clip.grade != Grade::default() {
        v["grade"] = grade_json(&clip.grade);
    }
    if clip.audio != ClipAudio::default() {
        let all = |a: &ClipAudio| {
            json!({
                "gain_db": num(a.gain_db),
                "fade_in": secs(a.fade_in),
                "fade_out": secs(a.fade_out),
                "effects": audio_fx_json(&a.effects),
            })
        };
        v["audio"] = without_neutral(all(&clip.audio), &all(&ClipAudio::default()));
    }
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

pub fn list_fx(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let kind = args.opt_str("kind")?;
    let language = args.opt_str("language")?;
    let category = args.opt_str("category")?.map(str::to_lowercase);
    let text = args.opt_str("text")?.map(str::to_lowercase);
    let specs = args.opt_bool("params")?.unwrap_or(false);
    let wanted_kind = match kind {
        None | Some("audio") => None,
        Some("effect") => Some(FxKind::Effect),
        Some("transition") => Some(FxKind::Transition),
        Some("generator") => Some(FxKind::Generator),
        Some(k) => return Err(ToolError(format!("unknown kind `{k}`"))),
    };
    let wanted_language = match language {
        None => None,
        Some("glsl") => Some(FxLanguage::Glsl),
        Some("html") => Some(FxLanguage::Html),
        Some(l) => return Err(ToolError(format!("unknown language `{l}`"))),
    };
    let matches_text = |hay: &[&str]| {
        text.as_ref()
            .is_none_or(|t| hay.iter().any(|h| h.to_lowercase().contains(t.as_str())))
    };
    let programs: Vec<Value> = if kind == Some("audio") {
        Vec::new()
    } else {
        fx::library(project)
            .filter(|a| wanted_kind.is_none_or(|k| a.kind == k))
            .filter(|a| wanted_language.is_none_or(|l| a.language == l))
            .filter(|a| {
                category
                    .as_ref()
                    .is_none_or(|c| a.category.to_lowercase() == *c)
            })
            .filter(|a| matches_text(&[&a.name, &a.description, &a.category, &a.id.to_string()]))
            .map(|a| asset_json(project, a, false, specs))
            .collect()
    };
    let audio: Vec<Value> = if kind.is_none_or(|k| k == "audio") && language.is_none() {
        AudioFxKind::ALL
            .into_iter()
            .filter(|k| matches_text(&[k.name(), k.label()]))
            .map(|k| {
                let params: Vec<Value> = if specs {
                    k.params().iter().map(param_spec_json).collect()
                } else {
                    k.params().iter().map(|p| json!(p.name)).collect()
                };
                json!({ "kind": k.name(), "name": k.label(), "params": params })
            })
            .collect()
    } else {
        Vec::new()
    };
    let n = programs.len() + audio.len();
    let mut out = json!({ "programs": programs });
    if !audio.is_empty() {
        out["audio_effects"] = json!(audio);
    }
    Ok(Done::json(format!("{n} effects"), &out).into())
}

pub fn get_fx(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let asset = fx_asset(project, args.str("fx")?, None)?;
    let v = asset_json(project, asset, true, true);
    Ok(Done::json(format!("{} {}", fx_kind(asset.kind), asset.id), &v).into())
}
