//! Depth effects (`flikk_core::fx::depth`): `add_depth_effect` puts a lens
//! blur, depth fog or 3D camera move on clips, and `set_depth_focus`
//! focuses one where a point of the picture is, as a click in the viewer
//! does: the depth map of that frame is made first (off the UI thread), and
//! the distance there becomes the effect's focus, keyed at that time for a
//! rack focus. Depth masks are `set_mask`'s `depth` shape.

use super::fx::param_value;
use super::{Done, Job, JobDone, Rebase, Step, ToolCtx, propose};
use crate::args::*;
use flikk_core::command::Command;
use flikk_core::depth;
use flikk_core::fx::{self, FxInstance, FxRef};
use flikk_core::model::{Clip, Project};
use flikk_core::{EffectId, Time, TimeRange};
use flikk_os::gpu::Level;
use serde_json::Value;
use std::sync::Arc;

pub const TOOLS: [&str; 2] = ["add_depth_effect", "set_depth_focus"];

/// What `add_depth_effect` calls each effect, and its built-in key.
pub const EFFECTS: [(&str, &str); 3] = [
    ("lens_blur", fx::depth::LENS_BLUR),
    ("fog", fx::depth::FOG),
    ("camera", fx::depth::PARALLAX),
];

pub fn serve(ctx: &mut ToolCtx, tool: &str, args: &Args) -> Result<Step, ToolError> {
    match tool {
        "add_depth_effect" => add(ctx, args),
        _ => focus(ctx, args),
    }
}

fn reason(args: &Args, tool: &str) -> ToolResultOf<String> {
    Ok(args
        .opt_str("reason")?
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or(tool)
        .to_owned())
}

/// Why depth effects show nothing yet, when they can't.
fn missing(ctx: &ToolCtx) -> Option<String> {
    match &ctx.services.depth {
        None => Some("There is no depth service in this editor.".into()),
        Some(d) => d.available().err(),
    }
}

fn add(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let reason = reason(args, "add_depth_effect")?;
    let name = args.str("effect")?;
    let Some((_, key)) = EFFECTS.iter().find(|(n, _)| *n == name) else {
        let names: Vec<&str> = EFFECTS.iter().map(|(n, _)| *n).collect();
        return fail(format!("`effect` is one of {}", names.join(", ")));
    };
    let p = ctx.project();
    let asset =
        fx::asset(p, &FxRef::builtin(key)).ok_or_else(|| ToolError("no such effect".into()))?;
    let clips = clip_ids(p, &args.strings("clips")?)?;
    if clips.is_empty() {
        return fail("`clips` is empty");
    }
    let mut effect = FxInstance::new(asset.id.clone());
    if let Some(Value::Object(values)) = args.value("params") {
        for (k, v) in values {
            let spec = asset
                .param(k)
                .ok_or_else(|| ToolError(format!("`{}` has no parameter `{k}`", asset.name)))?;
            effect.params.insert(k.clone(), param_value(spec, v)?);
        }
    }
    let before: Vec<Vec<EffectId>> = clips
        .iter()
        .map(|id| {
            p.timeline
                .clip(*id)
                .map(|(_, c)| c.effects.iter().map(|e| e.id).collect())
                .unwrap_or_default()
        })
        .collect();
    let command = Command::AddEffect {
        clips: clips.clone(),
        effect,
        index: None,
    };
    let mut done = propose(ctx, vec![command], &reason);
    if !done.result.is_error {
        let added: Vec<String> = clips
            .iter()
            .zip(&before)
            .filter_map(|(id, before)| {
                let (_, c) = ctx.project().timeline.clip(*id)?;
                let e = c.effects.iter().find(|e| !before.contains(&e.id))?;
                Some(format!("@effect {id}/{}", e.id))
            })
            .collect();
        let mut note = format!("New effects: {}", added.join(", "));
        if let Some(why) = missing(ctx) {
            note.push_str(&format!(
                "\nThe picture shows unchanged until depth maps can be made: {why}"
            ));
        }
        if let Some(crate::bridge::Content::Text { text }) = done.result.content.first_mut() {
            text.push('\n');
            text.push_str(&note);
        }
    }
    Ok(done.into())
}

/// The effect `args.effect` names on `clip`, else its first that focuses.
fn focus_effect(project: &Project, clip: &Clip, args: &Args) -> ToolResultOf<EffectId> {
    let effects = depth::focus_effects(project, clip);
    match args.opt_str("effect")? {
        Some(text) => {
            let t = strip_token(text);
            let t = t
                .rsplit('/')
                .next()
                .unwrap_or(t)
                .trim()
                .to_ascii_lowercase();
            effects
                .iter()
                .find(|e| short(e.id).starts_with(&t) || e.id.to_string().starts_with(&t))
                .map(|e| e.id)
                .ok_or_else(|| {
                    ToolError(format!(
                        "`{text}` is not a lens blur or 3D camera on the clip"
                    ))
                })
        }
        None => effects.first().map(|e| e.id).ok_or_else(|| {
            ToolError(
                "The clip has no lens blur or 3D camera; add one with add_depth_effect.".into(),
            )
        }),
    }
}

fn focus(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let reason = reason(args, "set_depth_focus")?;
    let p = ctx.project();
    let id = clip_id(p, args.str("clip")?)?;
    let (track, clip) = p
        .timeline
        .clip(id)
        .ok_or_else(|| ToolError("clip not found".into()))?;
    let effect = focus_effect(p, clip, args)?;
    let point = match args.value("point") {
        Some(Value::Array(a)) if a.len() == 2 && a.iter().all(Value::is_number) => [
            a[0].as_f64().unwrap_or(0.5) as f32,
            a[1].as_f64().unwrap_or(0.5) as f32,
        ],
        _ => return fail("`point` is [x, y], fractions of the picture from its top-left"),
    };
    let at = match args.opt_time("at")? {
        Some(t) => t,
        None => clip.start + Time(clip.duration.0 / 2),
    };
    if !clip.range().contains(at) {
        return fail("`at` is outside the clip");
    }
    let key = args.opt_bool("key")?.unwrap_or(false);
    let Some(service) = ctx.services.depth.clone() else {
        return fail("There is no depth service in this editor.");
    };
    service.available().map_err(ToolError)?;
    let (item, _) = depth::source(p, track, clip, clip.range())
        .ok_or_else(|| ToolError("Depth needs a video or image clip from the pool.".into()))?;
    let item = item.clone();
    let source = clip
        .to_source(at)
        .ok_or_else(|| ToolError("the clip shows no source there".into()))?;
    let clip = Arc::new(clip.clone());
    let since = ctx.version();
    let cancel = ctx.stop.flag();
    let job: Job = Box::new(move || {
        let frame = TimeRange::new(source, source + Time(1));
        if let Err(e) = service.complete(&item, frame, Level::Asked, &|_| {}, &cancel) {
            return JobDone::Done(Done::error(format!("Could not make the depth map: {e}")));
        }
        let Some(map) = service.depth(&item, source) else {
            return JobDone::Done(Done::error("The depth map is not there yet; try again."));
        };
        let distance = map.around([point[0].clamp(0.0, 1.0), point[1].clamp(0.0, 1.0)]);
        let Some(command) = depth::focus_command(&clip, effect, at, distance, key) else {
            return JobDone::Done(Done::error("the effect is gone"));
        };
        // The focus is set on the clip as it is by then, so other changes
        // to it are kept, as long as the effect and the frame are the ones
        // measured.
        let seen = clip.effects.iter().find(|e| e.id == effect).cloned();
        let rebase: Rebase = Box::new(move |project: &Project| {
            let (_, now) = project.timeline.clip(clip.id)?;
            let same = now.effects.iter().find(|e| e.id == effect) == seen.as_ref()
                && now.to_source(at) == Some(source);
            same.then(|| depth::focus_command(now, effect, at, distance, key))
                .flatten()
                .map(|c| vec![c])
        });
        JobDone::Propose {
            commands: vec![command],
            reason,
            excerpts: vec![format!(
                "Focus {distance:.3} (distance at the point: 0 nearest, 1 farthest)"
            )],
            since,
            rebase: Some(rebase),
        }
    });
    Ok(Step::Job(job))
}
