//! Stabilization for the agent: `stabilize` proposes it on video clips (or
//! turns it off), `get_shake` says how much each clip's camera shakes and
//! what stabilizing it would zoom in by, measuring the camera's path first
//! when it has to (see `koo_core::stabilize`).

use super::catalog::ToolSpec;
use super::{Done, Job, JobDone, Step, ToolCtx, propose};
use crate::args::{Args, ToolError, clip_ids, short};
use koo_core::ClipId;
use koo_core::command::{Command, ToolCategory};
use koo_core::model::Project;
use koo_core::stabilize::{self, Plan, Stabilization, StabilizeCrop, StabilizeMode};
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;

pub fn specs() -> Vec<ToolSpec> {
    let ids = json!({ "type": "array", "items": { "type": "string" }, "description": "More clip ids, changed alike" });
    vec![
        ToolSpec {
            name: "stabilize",
            category: Some(ToolCategory::Transform),
            description: "Propose stabilizing video clips: the camera's shake is smoothed out of the picture, which is zoomed in just enough to hide the moving edges. Only the values given change; `enabled: false` turns it off. The camera's path is measured in the background the first time. Check with get_shake first: a steady clip gains nothing and loses some of its edges.",
            schema: json!({
                "type": "object",
                "properties": {
                    "clip": { "type": "string", "description": "Clip id" },
                    "clips": ids,
                    "enabled": { "type": "boolean", "description": "Default true" },
                    "smoothness": { "type": "number", "description": "0..1 (default 0.5): how slow a move still counts as shake; higher keeps only slow pans" },
                    "mode": { "type": "string", "enum": ["smooth", "locked"], "description": "smooth (default) keeps the camera's intended move; locked holds the frame still, as on a tripod, as far as the zoom allows" },
                    "crop": { "type": "string", "enum": ["auto", "none"], "description": "auto (default) zooms in up to 1.4× so no edge shows; none leaves the moving edges black" },
                    "reason": { "type": "string", "description": "One line shown to the user with the change" }
                },
                "required": []
            }),
        },
        ToolSpec {
            name: "get_shake",
            category: Some(ToolCategory::ReadAnalysis),
            description: "How shaky video clips are (measuring the camera's path when it has not been: several seconds per minute of footage), and how much stabilizing each would zoom in. `score` under 0.25 is steady, 0.25–0.75 slightly shaky, 0.75–2 shaky, over 2 very shaky.",
            schema: json!({
                "type": "object",
                "properties": {
                    "clip": { "type": "string", "description": "Clip id" },
                    "clips": { "type": "array", "items": { "type": "string" }, "description": "More clip ids" }
                },
                "required": []
            }),
        },
    ]
}

/// `clip` and `clips`, at least one.
fn targets(project: &Project, args: &Args) -> Result<Vec<ClipId>, ToolError> {
    let mut texts: Vec<String> = args
        .opt_str("clip")?
        .map(str::to_owned)
        .into_iter()
        .collect();
    texts.extend(args.strings("clips")?);
    if texts.is_empty() {
        return Err(ToolError("Give `clip` or `clips`.".into()));
    }
    let mut ids = clip_ids(project, &texts)?;
    ids.dedup();
    Ok(ids)
}

pub fn stabilize(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let reason = args
        .opt_str("reason")?
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or("stabilize")
        .to_owned();
    let project = ctx.project();
    let ids = targets(project, args)?;
    let smoothness = args.opt_f64("smoothness")?;
    if smoothness.is_some_and(|s| !(0.0..=1.0).contains(&s)) {
        return Err(ToolError("`smoothness` is 0..1.".into()));
    }
    let mode = match args.opt_str("mode")? {
        None => None,
        Some("smooth") => Some(StabilizeMode::Smooth),
        Some("locked") => Some(StabilizeMode::Locked),
        Some(other) => return Err(ToolError(format!("unknown mode `{other}`"))),
    };
    let crop = match args.opt_str("crop")? {
        None => None,
        Some("auto") => Some(StabilizeCrop::Auto),
        Some("none") => Some(StabilizeCrop::None),
        Some(other) => return Err(ToolError(format!("unknown crop `{other}`"))),
    };
    let enabled = args.opt_bool("enabled")?.unwrap_or(true);
    let mut commands = Vec::new();
    let mut measure = Vec::new();
    for id in ids {
        let Some((_, clip)) = project.timeline.clip(id) else {
            continue;
        };
        let current = &clip.stabilization;
        let s = Stabilization {
            enabled,
            smoothness: smoothness.map_or(current.smoothness, |v| v as f32),
            mode: mode.unwrap_or(current.mode),
            crop: crop.unwrap_or(current.crop),
        };
        if enabled && let Some(item) = clip.media().and_then(|m| project.media(m)) {
            measure.push(item.clone());
        }
        commands.push(Command::SetStabilization {
            clips: vec![id],
            stabilization: s,
        });
    }
    if let Some(stabilizer) = &ctx.services.stabilizer {
        for item in &measure {
            stabilizer.measure(item);
        }
    }
    Ok(propose(ctx, commands, &reason).into())
}

pub fn get_shake(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project().clone();
    let ids = targets(&project, args)?;
    let Some(stabilizer) = ctx.services.stabilizer.clone() else {
        return Err(ToolError(
            "Stabilization is not available here (no media engine).".into(),
        ));
    };
    let job: Job = Box::new(move || {
        let never = AtomicBool::new(false);
        let rows: Vec<Value> = ids
            .iter()
            .map(|id| {
                let mut row = json!({ "clip": short(id) });
                let Some((_, clip)) = project.timeline.clip(*id) else {
                    row["error"] = json!("clip not found");
                    return row;
                };
                // Its source as if stabilized, to measure it either way.
                let mut on = clip.clone();
                on.stabilization.enabled = true;
                let Some((item, range)) = stabilize::source(&project, &on) else {
                    row["error"] = json!("not a video clip from the pool");
                    return row;
                };
                let path = match stabilizer.complete(item, &|_| {}, &never) {
                    Ok(p) => p,
                    Err(e) => {
                        row["error"] = json!(e.to_string());
                        return row;
                    }
                };
                if let Some(shake) = path.shake(range) {
                    row["score"] = json!((shake.score() * 100.0).round() / 100.0);
                    row["shake"] = json!(shake.label());
                    row["jitter_percent"] = json!((shake.translation * 100.0).round() / 100.0);
                    row["jitter_degrees"] = json!((shake.rotation_deg * 100.0).round() / 100.0);
                }
                if let Some(plan) = Plan::new(&path, &on.stabilization, range) {
                    row["zoom"] = json!((plan.zoom * 100.0).round() / 100.0);
                }
                let s = &clip.stabilization;
                if s.enabled {
                    row["stabilized"] = json!({
                        "mode": s.mode.name(),
                        "smoothness": s.smoothness,
                        "crop": s.crop.name(),
                    });
                }
                row
            })
            .collect();
        let summary = format!("shake of {} clips", rows.len());
        JobDone::Done(Done::json(summary, &json!({ "clips": rows })))
    });
    Ok(Step::Job(job))
}

/// What `get_clip` says of a clip's stabilization; `None` when off.
pub fn clip_json(s: &Stabilization) -> Option<Value> {
    s.enabled.then(|| {
        json!({
            "mode": s.mode.name(),
            "smoothness": s.smoothness,
            "crop": s.crop.name(),
        })
    })
}
