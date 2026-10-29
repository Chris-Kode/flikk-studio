//! Masks on picture clips (`koo_core::mask`): `set_mask` adds one or
//! changes it (shape, mode, edge, opacity; keyed at a time with `at`), in
//! the picture's own coordinates or, with `space: frame`, in frame pixels
//! at that time; `set_mask` with `remove` takes them away. `get_clip` lists them.

use super::edit::with_notes;
use super::{Done, Job, JobDone, Step, ToolCtx, propose};
use crate::args::*;
use koo_core::Time;
use koo_core::command::Command;
use koo_core::compose::{self, Shape, Stills};
use koo_core::mask::{Mask, MaskKeyframe, MaskMode, MaskShape, MaskState};
use koo_core::model::{Clip, Project, TrackKind};
use serde_json::{Value, json};
use std::sync::Arc;

pub const SHAPES: [&str; 3] = ["rectangle", "ellipse", "path"];
pub const MODES: [&str; 3] = ["add", "subtract", "intersect"];

/// `clip`'s masks as tools show them: short ids, times in timeline seconds.
pub fn masks_json(clip: &Clip) -> Value {
    clip.masks
        .iter()
        .map(|m| {
            let mut v = serde_json::to_value(m).unwrap_or_default();
            v["id"] = json!(short(m.id));
            if let Some(keys) = v.get_mut("keyframes").and_then(Value::as_array_mut) {
                for (k, key) in keys.iter_mut().zip(&m.keyframes) {
                    k["at"] = secs(clip.start + key.at);
                }
            }
            v
        })
        .collect()
}

/// The clip a mask tool works on: a picture, not lettering or sound.
fn picture_clip<'a>(project: &'a Project, args: &Args) -> ToolResultOf<&'a Clip> {
    let id = clip_id(project, args.str("clip")?)?;
    let (track, clip) = project
        .timeline
        .clip(id)
        .ok_or_else(|| ToolError("clip not found".into()))?;
    if compose::is_lettering(track, clip) {
        return fail(
            "Captions and titles take no masks: the render burns them in with ASS, which cannot feather or combine shapes.",
        );
    }
    if track.kind == TrackKind::Audio {
        return fail("Masks go on picture clips, not audio.");
    }
    Ok(clip)
}

fn pair(v: &Value, key: &str) -> ToolResultOf<Option<[f32; 2]>> {
    match v.get(key) {
        None => Ok(None),
        Some(Value::Array(a)) if a.len() == 2 && a.iter().all(Value::is_number) => Ok(Some([
            a[0].as_f64().unwrap_or_default() as f32,
            a[1].as_f64().unwrap_or_default() as f32,
        ])),
        Some(_) => fail(format!("`shape.{key}` is [x, y]")),
    }
}

/// `shape` from the tool's `shape` object, missing values taken from
/// `current` when it is the same kind of shape.
fn parse_shape(v: &Value, current: Option<&MaskShape>) -> ToolResultOf<MaskShape> {
    let kind = match v.get("type").and_then(Value::as_str) {
        Some(k) if SHAPES.contains(&k) => k,
        Some(k) => return fail(format!("unknown shape type `{k}`")),
        None => match current {
            Some(MaskShape::Rectangle { .. }) => "rectangle",
            Some(MaskShape::Ellipse { .. }) => "ellipse",
            Some(MaskShape::Path { .. }) => "path",
            None => return fail("`shape.type` is one of rectangle, ellipse, path"),
        },
    };
    let num = |key: &str| -> ToolResultOf<Option<f32>> {
        match v.get(key) {
            None => Ok(None),
            Some(n) => n
                .as_f64()
                .map(|n| Some(n as f32))
                .ok_or_else(|| ToolError(format!("`shape.{key}` is a number"))),
        }
    };
    let (c0, s0, r0, o0) = match current {
        Some(MaskShape::Rectangle {
            center,
            size,
            rotation_deg,
            roundness,
        }) => (*center, *size, *rotation_deg, *roundness),
        Some(MaskShape::Ellipse {
            center,
            size,
            rotation_deg,
        }) => (*center, *size, *rotation_deg, 0.0),
        _ => ([0.5, 0.5], [0.5, 0.5], 0.0, 0.0),
    };
    Ok(match kind {
        "rectangle" => MaskShape::Rectangle {
            center: pair(v, "center")?.unwrap_or(c0),
            size: pair(v, "size")?.unwrap_or(s0),
            rotation_deg: num("rotation")?.unwrap_or(r0),
            roundness: num("roundness")?.unwrap_or(o0).clamp(0.0, 1.0),
        },
        "ellipse" => MaskShape::Ellipse {
            center: pair(v, "center")?.unwrap_or(c0),
            size: pair(v, "size")?.unwrap_or(s0),
            rotation_deg: num("rotation")?.unwrap_or(r0),
        },
        _ => {
            let (p0, smooth0) = match current {
                Some(MaskShape::Path { points, smooth }) => (points.clone(), *smooth),
                // Straight edges unless asked: points read off a zoomed
                // frame should be the corners of the shape.
                _ => (Vec::new(), false),
            };
            let points = match v.get("points") {
                None => p0,
                Some(Value::Array(a)) => a
                    .iter()
                    .map(|p| {
                        let p = p.as_array().filter(|p| p.len() == 2)?;
                        Some([p[0].as_f64()? as f32, p[1].as_f64()? as f32])
                    })
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| ToolError("`shape.points` is a list of [x, y]".into()))?,
                Some(_) => return fail("`shape.points` is a list of [x, y]"),
            };
            if points.len() < 3 {
                return fail("A path mask needs at least three `points`.");
            }
            MaskShape::Path {
                points,
                smooth: v.get("smooth").and_then(Value::as_bool).unwrap_or(smooth0),
            }
        }
    })
}

/// Frame pixels → picture uv (before flip) through `shape`'s placement.
fn frame_to_uv(shape: &Shape, p: [f32; 2]) -> Option<[f32; 2]> {
    let [u, v] = shape.placement.to_picture(p)?;
    let flip = shape.placement.flip;
    Some([
        if flip[0] { 1.0 - u } else { u },
        if flip[1] { 1.0 - v } else { v },
    ])
}

/// `s`, given in frame pixels, in the picture's uv as laid out in `shape`:
/// points and centres mapped through it, a box's sides measured along its
/// own axes and its turn added to the picture's.
fn shape_from_frame(shape: &Shape, s: &MaskShape) -> Option<MaskShape> {
    let aspect = shape.rest_size[0].max(1e-3) / shape.rest_size[1].max(1e-3);
    let boxed = |center: [f32; 2], size: [f32; 2], rotation: f32| {
        let c = frame_to_uv(shape, center)?;
        let (sn, cs) = rotation.to_radians().sin_cos();
        let axis = |len: f32, dir: [f32; 2]| {
            let a = frame_to_uv(shape, [center[0] + dir[0] * len, center[1] + dir[1] * len])?;
            let b = frame_to_uv(shape, [center[0] - dir[0] * len, center[1] - dir[1] * len])?;
            Some([(a[0] - b[0]) * aspect, a[1] - b[1]])
        };
        let x = axis(size[0] / 2.0, [cs, sn])?;
        let y = axis(size[1] / 2.0, [-sn, cs])?;
        let turn = x[1].atan2(x[0]).to_degrees();
        Some((c, [x[0].hypot(x[1]) / aspect, y[0].hypot(y[1])], turn))
    };
    Some(match s {
        MaskShape::Rectangle {
            center,
            size,
            rotation_deg,
            roundness,
        } => {
            let (center, size, rotation_deg) = boxed(*center, *size, *rotation_deg)?;
            MaskShape::Rectangle {
                center,
                size,
                rotation_deg,
                roundness: *roundness,
            }
        }
        MaskShape::Ellipse {
            center,
            size,
            rotation_deg,
        } => {
            let (center, size, rotation_deg) = boxed(*center, *size, *rotation_deg)?;
            MaskShape::Ellipse {
                center,
                size,
                rotation_deg,
            }
        }
        MaskShape::Path { points, smooth } => MaskShape::Path {
            points: points
                .iter()
                .map(|p| frame_to_uv(shape, *p))
                .collect::<Option<_>>()?,
            smooth: *smooth,
        },
    })
}

/// `mask` showing `state` from clip time `at`: keyed there, or its static
/// state without `at` (a new mask, `new`, takes it as its static state
/// too).
fn with_state(mask: &Mask, state: MaskState, at: Option<Time>, new: bool) -> Mask {
    let mut out = mask.clone();
    if new {
        out.state = state.clone();
    }
    match at {
        None => out.state = state,
        Some(at) => {
            out.keyframes.retain(|k| k.at != at);
            out.keyframes.push(MaskKeyframe {
                at,
                state,
                ease: Default::default(),
            });
            out.keyframes.sort_by_key(|k| k.at);
        }
    }
    out
}

pub fn set_mask(ctx: &mut ToolCtx, args: &Args, reason: &str) -> Result<Step, ToolError> {
    let project = ctx.project();
    let clip = picture_clip(project, args)?;
    let existing = match args.opt_str("mask")? {
        Some(text) => {
            let id = mask_id(clip, text)?;
            clip.masks.iter().position(|m| m.id == id)
        }
        None => None,
    };
    let at = match args.opt_time("at")? {
        Some(t) if !clip.range().contains(t) => {
            return fail(format!(
                "{t} is outside the clip ({} – {})",
                clip.start,
                clip.end()
            ));
        }
        Some(t) => Some(t - clip.start),
        None => None,
    };
    let current = existing.map(|i| &clip.masks[i]);
    let base = match (current, at) {
        (Some(m), Some(at)) => m.state_at(at),
        (Some(m), None) => m.state.clone(),
        (None, _) => MaskState {
            shape: MaskShape::Ellipse {
                center: [0.5, 0.5],
                size: [0.5, 0.5],
                rotation_deg: 0.0,
            },
            feather: 0.0,
            expansion: 0.0,
            opacity: 1.0,
        },
    };
    let shape = match args.value("shape") {
        Some(v @ Value::Object(_)) => Some(parse_shape(v, current.map(|_| &base.shape))?),
        Some(_) => return fail("`shape` is an object with `type`"),
        None if current.is_none() => return fail("A new mask needs `shape`."),
        None => None,
    };
    let frame = match args.opt_str("space")? {
        None | Some("picture") => false,
        Some("frame") => true,
        Some(s) => return fail(format!("`space` is picture or frame, not `{s}`")),
    };
    let mut state = base.clone();
    if let Some(v) = args.opt_f64("feather")? {
        state.feather = (v as f32).max(0.0);
    }
    if let Some(v) = args.opt_f64("expansion")? {
        state.expansion = v as f32;
    }
    if let Some(v) = args.opt_f64("opacity")? {
        state.opacity = (v as f32).clamp(0.0, 1.0);
    }
    let mut mask = current
        .cloned()
        .unwrap_or_else(|| Mask::new(base.shape.clone()));
    if let Some(mode) = args.opt_str("mode")? {
        mask.mode = match mode {
            "add" => MaskMode::Add,
            "subtract" => MaskMode::Subtract,
            "intersect" => MaskMode::Intersect,
            m => return fail(format!("unknown mode `{m}`")),
        };
    }
    if let Some(v) = args.opt_bool("invert")? {
        mask.invert = v;
    }
    if let Some(v) = args.opt_bool("enabled")? {
        mask.enabled = v;
    }
    let (clip_id, masks) = (clip.id, clip.masks.clone());
    let note = match (current, at) {
        (Some(m), None) if !m.keyframes.is_empty() => Some(format!(
            "Mask {} has keyframes, which override its static shape; pass `at` to key it there.",
            short(m.id)
        )),
        _ => None,
    };
    let finish = move |mask: Mask| {
        let mut next = masks.clone();
        match existing {
            Some(i) => next[i] = mask,
            None => next.push(mask),
        }
        Command::SetMasks {
            clip: clip_id,
            masks: next,
        }
    };
    if !frame || shape.is_none() {
        if let Some(shape) = shape {
            state.shape = shape;
        }
        let summary = format!("Mask {}.", short(mask.id));
        let mask = with_state(&mask, state, at, existing.is_none());
        let done = propose(ctx, vec![finish(mask)], reason);
        return Ok(with_notes(done, std::iter::once(summary).chain(note)).into());
    }
    // In frame pixels: laid out where the clip is at `at` (or its middle).
    let t = clip.start + at.unwrap_or(Time(clip.duration.0 / 2));
    let project = Arc::new(project.clone());
    let services = ctx.services.clone();
    let reason = reason.to_owned();
    let given = shape.unwrap_or(base.shape);
    let job: Job = Box::new(move || {
        let s = &project.timeline.settings;
        let mut stills = Stills::new(services.media.clone(), 640);
        let shapes = compose::layout(&project, t, (s.width, s.height), &services, &mut stills);
        let Some(laid) = shapes.iter().find(|l| l.clip == clip_id) else {
            return JobDone::Done(Done::error(format!(
                "The clip draws nothing at {t}; pass `at`, or give the shape in picture coordinates."
            )));
        };
        let Some(shape) = shape_from_frame(laid, &given) else {
            return JobDone::Done(Done::error("The clip is scaled to nothing there."));
        };
        state.shape = shape;
        JobDone::Propose {
            commands: vec![finish(with_state(&mask, state, at, existing.is_none()))],
            reason,
            excerpts: Vec::new(),
        }
    });
    Ok(Step::Job(job))
}

pub fn remove_mask(ctx: &mut ToolCtx, args: &Args) -> ToolResultOf<Vec<Command>> {
    let project = ctx.project();
    let clip = picture_clip(project, args)?;
    let masks = match args.opt_str("mask")? {
        Some(text) => {
            let id = mask_id(clip, text)?;
            clip.masks.iter().filter(|m| m.id != id).cloned().collect()
        }
        None if args.opt_bool("all")? == Some(true) => Vec::new(),
        None => return fail("give `mask`, or `all: true`"),
    };
    Ok(vec![Command::SetMasks {
        clip: clip.id,
        masks,
    }])
}
