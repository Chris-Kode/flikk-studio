//! Masks on picture clips (`flikk_core::mask`): `set_mask` adds one or
//! changes it (shape, mode, edge, opacity; keyed at a time with `at`), in
//! the picture's own coordinates or, with `space: frame`, in frame pixels
//! at that time; `set_mask` with `remove` takes them away. `get_clip` lists them.
//!
//! A `subject` shape keeps the object its `include` clicks and `box` pick
//! (`flikk_core::subject`): `at` names the frame they are on, and the model
//! finds the object's edge there before the change is proposed, so the
//! result says what it kept (its share of the picture, its box in frame
//! pixels, the model's confidence) and what to do when that looks wrong.
//! With `track` it is followed through the clip in the background.
//!
//! A `person` shape keeps the people a matting model finds in every frame
//! (`flikk_core::person`), with no prompt; `remove_background` adds one (or
//! with `keep: background`, one that cuts the people away) to several
//! clips at once, or takes them away again.

use super::edit::with_notes;
use super::{Done, Job, JobDone, Rebase, Step, ToolCtx, propose};
use crate::args::*;
use flikk_core::Time;
use flikk_core::command::Command;
use flikk_core::compose::{self, Shape, Stills};
use flikk_core::mask::{Mask, MaskKeyframe, MaskMode, MaskShape, MaskState};
use flikk_core::model::{Clip, ClipContent, MediaKind, Project, TrackKind};
use flikk_core::services::{Services, SubjectService};
use flikk_core::subject::{Matte, Subject, SubjectPoint, SubjectPrompt};
use serde_json::{Value, json};
use std::sync::Arc;

pub const SHAPES: [&str; 6] = ["rectangle", "ellipse", "path", "subject", "person", "depth"];
pub const MODES: [&str; 3] = ["add", "subtract", "intersect"];

/// `clip`'s masks as tools show them: short ids, times in timeline seconds,
/// a subject's prompts on the timeline and how far it is tracked.
pub fn masks_json(project: &Project, clip: &Clip, services: &Services) -> Value {
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
            if m.is_person()
                && let Some(status) = compose::person_ref(project, clip, m)
                    .zip(services.subjects.as_ref())
                    .map(|(r, s)| s.person_status(r))
            {
                v["matted"] = json!(format!("{}/{} frames", status.tracked, status.frames));
                if let Some(e) = status.error {
                    v["error"] = json!(e);
                }
            }
            if let Some(subject) = m.picks() {
                if let Some(o) = v.as_object_mut() {
                    o.remove("subject");
                }
                v["prompts"] = subject
                    .prompts
                    .iter()
                    .map(|p| prompt_json(clip, p))
                    .collect();
                v["track"] = json!(subject.track);
                v["refine"] = json!(subject.refine);
                let status = compose::subject_ref(project, clip, m)
                    .zip(services.subjects.as_ref())
                    .map(|(r, s)| s.status(r));
                if let Some(status) = status {
                    v["found"] = json!(status.prompted.0 == status.prompted.1);
                    if subject.track {
                        v["tracked"] =
                            json!(format!("{}/{} frames", status.tracked, status.frames));
                    }
                    if let Some(e) = status.error {
                        v["error"] = json!(e);
                    }
                }
            }
            v
        })
        .collect()
}

/// A subject's prompt: its frame's timeline time (or source time, when the
/// clip no longer shows it), clicks and box.
fn prompt_json(clip: &Clip, p: &SubjectPrompt) -> Value {
    let mut v = json!({});
    match clip.to_timeline(p.at) {
        Some(t) => v["at"] = secs(t),
        None => v["source_at"] = secs(p.at),
    }
    let points = |include: bool| -> Vec<[f32; 2]> {
        p.points
            .iter()
            .filter(|q| q.include == include)
            .map(|q| q.at)
            .collect()
    };
    let (include, exclude) = (points(true), points(false));
    if !include.is_empty() {
        v["include"] = json!(include);
    }
    if !exclude.is_empty() {
        v["exclude"] = json!(exclude);
    }
    if let Some(r) = p.rect {
        v["box"] = json!(r);
    }
    v
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
            Some(MaskShape::Subject) => "subject",
            Some(MaskShape::Person { .. }) => "person",
            Some(MaskShape::Depth { .. }) => "depth",
            None => {
                return fail(
                    "`shape.type` is one of rectangle, ellipse, path, subject, person, depth",
                );
            }
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
        "subject" => MaskShape::Subject,
        "person" => MaskShape::Person {
            edge: num("edge")?
                .or(match current {
                    Some(MaskShape::Person { edge }) => Some(*edge),
                    _ => None,
                })
                .unwrap_or(0.5)
                .clamp(0.0, 1.0),
        },
        "depth" => {
            let (n0, f0, s0) = match current {
                Some(MaskShape::Depth {
                    near,
                    far,
                    softness,
                }) => (*near, *far, *softness),
                _ => (0.5, 1.0, 0.05),
            };
            MaskShape::Depth {
                near: num("near")?.unwrap_or(n0).clamp(0.0, 1.0),
                far: num("far")?.unwrap_or(f0).clamp(0.0, 1.0),
                softness: num("softness")?.unwrap_or(s0).clamp(0.0, 1.0),
            }
        }
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
        MaskShape::Subject => MaskShape::Subject,
        MaskShape::Person { edge } => MaskShape::Person { edge: *edge },
        MaskShape::Depth { .. } => s.clone(),
    })
}

/// Picture uv (before flip) → frame pixels through `shape`'s placement.
fn uv_to_frame(shape: &Shape, uv: [f32; 2]) -> [f32; 2] {
    let flip = shape.placement.flip;
    let u = if flip[0] { 1.0 - uv[0] } else { uv[0] };
    let v = if flip[1] { 1.0 - uv[1] } else { uv[1] };
    shape.placement.to_canvas([u, v])
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
    let named = args
        .value("shape")
        .and_then(|v| v.get("type"))
        .and_then(Value::as_str);
    let is_subject = match (named, existing) {
        (Some(kind), _) => kind == "subject",
        (None, Some(i)) => clip.masks[i].picks().is_some(),
        (None, None) => false,
    };
    if is_subject {
        return set_subject(ctx, args, reason, existing);
    }
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
    let seen = clip.clone();
    let since = ctx.version();
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
        let mask = with_state(&mask, state, at, existing.is_none());
        JobDone::Propose {
            commands: vec![finish(mask.clone())],
            reason,
            excerpts: Vec::new(),
            since,
            rebase: Some(rebase_mask(&seen, existing, mask)),
        }
    });
    Ok(Step::Job(job))
}

/// A job's `mask`, worked out on `seen` (the clip as its snapshot had it,
/// `existing` the index of the mask it changes), put among the clip's masks
/// as they are when it is done: the others may have changed meanwhile. Not
/// when the clip is gone, or that mask or anything else of the clip that
/// placed it was changed since.
fn rebase_mask(seen: &Clip, existing: Option<usize>, mask: Mask) -> Rebase {
    let was = existing.map(|i| seen.masks[i].clone());
    let mut seen = seen.clone();
    seen.masks.clear();
    Box::new(move |project: &Project| {
        let (_, clip) = project.timeline.clip(seen.id)?;
        let mut rest = clip.clone();
        let mut masks = std::mem::take(&mut rest.masks);
        if rest != seen {
            return None;
        }
        match was {
            Some(was) => {
                let slot = masks.iter_mut().find(|m| m.id == was.id)?;
                if *slot != was {
                    return None;
                }
                *slot = mask;
            }
            None if masks.iter().any(|m| m.id == mask.id) => return None,
            None => masks.push(mask),
        }
        Some(vec![Command::SetMasks {
            clip: seen.id,
            masks,
        }])
    })
}

/// The parts of a subject prompt `set_mask` was given; `None` leaves the
/// frame's own.
struct PromptArgs {
    include: Option<Vec<[f32; 2]>>,
    exclude: Option<Vec<[f32; 2]>>,
    rect: Option<Option<[f32; 4]>>,
}

impl PromptArgs {
    fn parse(shape: Option<&Value>) -> ToolResultOf<Self> {
        let list = |key: &str| -> ToolResultOf<Option<Vec<[f32; 2]>>> {
            match shape.and_then(|v| v.get(key)) {
                None => Ok(None),
                Some(Value::Array(a)) => a
                    .iter()
                    .map(|p| {
                        let p = p.as_array().filter(|p| p.len() == 2)?;
                        Some([p[0].as_f64()? as f32, p[1].as_f64()? as f32])
                    })
                    .collect::<Option<Vec<_>>>()
                    .map(Some)
                    .ok_or_else(|| ToolError(format!("`shape.{key}` is a list of [x, y]"))),
                Some(_) => fail(format!("`shape.{key}` is a list of [x, y]")),
            }
        };
        let rect = match shape.and_then(|v| v.get("box")) {
            None => None,
            Some(Value::Null) => Some(None),
            Some(Value::Array(a)) if a.len() == 4 && a.iter().all(Value::is_number) => {
                let n: Vec<f32> = a
                    .iter()
                    .filter_map(Value::as_f64)
                    .map(|v| v as f32)
                    .collect();
                Some(Some([
                    n[0].min(n[2]),
                    n[1].min(n[3]),
                    n[0].max(n[2]),
                    n[1].max(n[3]),
                ]))
            }
            Some(_) => return fail("`shape.box` is [left, top, right, bottom], or null"),
        };
        Ok(Self {
            include: list("include")?,
            exclude: list("exclude")?,
            rect,
        })
    }

    fn given(&self) -> bool {
        self.include.is_some() || self.exclude.is_some() || self.rect.is_some()
    }

    /// In picture uv: as given, or from frame pixels through `shape`.
    fn to_uv(&self, shape: Option<&Shape>) -> Option<Self> {
        let Some(shape) = shape else {
            return Some(Self {
                include: self.include.clone(),
                exclude: self.exclude.clone(),
                rect: self.rect,
            });
        };
        let points = |v: &Option<Vec<[f32; 2]>>| -> Option<Option<Vec<[f32; 2]>>> {
            match v {
                None => Some(None),
                Some(v) => v
                    .iter()
                    .map(|p| frame_to_uv(shape, *p))
                    .collect::<Option<_>>()
                    .map(Some),
            }
        };
        let rect = match self.rect {
            Some(Some([x0, y0, x1, y1])) => {
                let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
                    .iter()
                    .map(|p| frame_to_uv(shape, *p))
                    .collect::<Option<Vec<_>>>()?;
                let fold = |f: fn(f32, f32) -> f32, i: usize, start: f32| {
                    corners.iter().map(|c| c[i]).fold(start, f)
                };
                Some(Some([
                    fold(f32::min, 0, f32::MAX),
                    fold(f32::min, 1, f32::MAX),
                    fold(f32::max, 0, f32::MIN),
                    fold(f32::max, 1, f32::MIN),
                ]))
            }
            other => other,
        };
        Some(Self {
            include: points(&self.include)?,
            exclude: points(&self.exclude)?,
            rect,
        })
    }

    /// `old` (the frame's prompt) with what was given in place of its own.
    fn applied(&self, old: Option<&SubjectPrompt>, at: Time) -> SubjectPrompt {
        let old_points = |include: bool| -> Vec<[f32; 2]> {
            old.map(|p| {
                p.points
                    .iter()
                    .filter(|q| q.include == include)
                    .map(|q| q.at)
                    .collect()
            })
            .unwrap_or_default()
        };
        let include = self.include.clone().unwrap_or_else(|| old_points(true));
        let exclude = self.exclude.clone().unwrap_or_else(|| old_points(false));
        let point = |include: bool| {
            move |at: [f32; 2]| SubjectPoint {
                at: [at[0].clamp(0.0, 1.0), at[1].clamp(0.0, 1.0)],
                include,
            }
        };
        let rect = self.rect.unwrap_or_else(|| old.and_then(|p| p.rect));
        SubjectPrompt {
            at,
            points: include
                .into_iter()
                .map(point(true))
                .chain(exclude.into_iter().map(point(false)))
                .collect(),
            rect: rect.map(|r| r.map(|v| v.clamp(0.0, 1.0))),
        }
    }
}

/// `set_mask` for a subject: the prompt on the frame at `at` (the clip's
/// middle by default) found by the model before it is proposed.
fn set_subject(
    ctx: &mut ToolCtx,
    args: &Args,
    reason: &str,
    existing: Option<usize>,
) -> Result<Step, ToolError> {
    let project = ctx.project();
    let clip = picture_clip(project, args)?;
    let ClipContent::Media { media, .. } = &clip.content else {
        return fail("Subject masks go on video and image clips from the pool.");
    };
    let item = project
        .media(*media)
        .cloned()
        .ok_or_else(|| ToolError("media not found".into()))?;
    if item.kind == MediaKind::Audio {
        return fail("Masks go on picture clips, not audio.");
    }
    let Some(service) = ctx.services.subjects.clone() else {
        return fail("Subject masks are not available in this build.");
    };
    if let Err(e) = service.available() {
        return fail(format!(
            "{e} Ask the user to download it; until then use a rectangle, ellipse or path."
        ));
    }
    let t = match args.opt_time("at")? {
        Some(t) if !clip.range().contains(t) => {
            return fail(format!(
                "{t} is outside the clip ({} – {})",
                clip.start,
                clip.end()
            ));
        }
        Some(t) => t,
        None => clip.start + Time(clip.duration.0 / 2),
    };
    let rate = item
        .info
        .as_ref()
        .and_then(|i| i.video.as_ref())
        .map(|v| v.rate)
        .unwrap_or_default();
    let source = match item.kind {
        MediaKind::Image => Time::ZERO,
        _ => clip
            .to_source(t)
            .ok_or_else(|| ToolError("the clip has no source time there".into()))?
            .max(Time::ZERO)
            .floor_to_frame(rate),
    };
    let given = PromptArgs::parse(args.value("shape"))?;
    let current = existing.map(|i| clip.masks[i].clone());
    if current.is_none() && !given.given() {
        return fail(
            "A new subject mask needs `shape.include` points on the object, or a `shape.box` around it.",
        );
    }
    let frame = match args.opt_str("space")? {
        None | Some("picture") => false,
        Some("frame") => true,
        Some(s) => return fail(format!("`space` is picture or frame, not `{s}`")),
    };
    let mut mask = current.clone().unwrap_or_else(|| {
        Mask::subject(Subject::new(SubjectPrompt {
            at: source,
            ..Default::default()
        }))
    });
    // A shape mask turned into a subject: its shape's keys go with it.
    if mask.state.shape != MaskShape::Subject {
        mask.state.shape = MaskShape::Subject;
        mask.keyframes.clear();
    }
    if let Some(v) = args.opt_f64("feather")? {
        mask.state.feather = (v as f32).max(0.0);
    }
    if let Some(v) = args.opt_f64("expansion")? {
        mask.state.expansion = v as f32;
    }
    if let Some(v) = args.opt_f64("opacity")? {
        mask.state.opacity = (v as f32).clamp(0.0, 1.0);
    }
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
    let subject = mask.subject.get_or_insert_with(|| Subject {
        prompts: Vec::new(),
        track: false,
        refine: true,
    });
    if let Some(v) = args.opt_bool("track")? {
        subject.track = v && item.kind != MediaKind::Image;
    }
    if let Some(v) = args.opt_bool("refine")? {
        subject.refine = v;
    }
    let (clip_id, masks) = (clip.id, clip.masks.clone());
    let clip = clip.clone();
    let since = ctx.version();
    let project = Arc::new(project.clone());
    let services = ctx.services.clone();
    let reason = reason.to_owned();
    let job: Job = Box::new(move || {
        let s = &project.timeline.settings;
        let mut stills = Stills::new(services.media.clone(), 640);
        let shapes = compose::layout(&project, t, (s.width, s.height), &services, &mut stills);
        let laid = shapes.iter().find(|l| l.clip == clip_id);
        if frame && laid.is_none() {
            return JobDone::Done(Done::error(format!(
                "The clip draws nothing at {t}; pass `at`, or give the points in picture coordinates."
            )));
        }
        let Some(given) = given.to_uv(laid.filter(|_| frame)) else {
            return JobDone::Done(Done::error("The clip is scaled to nothing there."));
        };
        let subject = mask.subject.clone().expect("set above");
        let old = subject.prompt_on(source.to_frame(rate), rate).cloned();
        let prompt = if given.given() {
            given.applied(old.as_ref(), source)
        } else {
            old.clone().unwrap_or(SubjectPrompt {
                at: source,
                ..Default::default()
            })
        };
        let mut mask = mask;
        mask.subject = Some(subject.with_prompt(prompt.clone(), rate));
        if mask.picks().is_some_and(|s| s.prompts.is_empty()) {
            return JobDone::Done(Done::error(
                "That leaves the subject with nothing picking it; remove the mask instead.",
            ));
        }
        let mut notes = vec![format!("Subject mask {}.", short(mask.id))];
        if !prompt.is_empty() {
            match service.preview(&item, &prompt) {
                Ok(matte) => notes.extend(found_notes(&matte, laid, t)),
                Err(e) => return JobDone::Done(Done::error(format!("The model failed: {}", e.0))),
            }
        }
        if let Some(r) = compose::subject_ref(&project, &clip, &mask) {
            notes.extend(tracking_note(service.as_ref(), r, item.kind));
        }
        notes.push(
            "Check it with view_frame `masks: true`: what the masks cut away is tinted red.".into(),
        );
        let rebase = rebase_mask(&clip, existing, mask.clone());
        let mut next = masks.clone();
        match existing {
            Some(i) => next[i] = mask,
            None => next.push(mask),
        }
        JobDone::Propose {
            commands: vec![Command::SetMasks {
                clip: clip_id,
                masks: next,
            }],
            reason,
            excerpts: notes,
            since,
            rebase: Some(rebase),
        }
    });
    Ok(Step::Job(job))
}

/// What the model found on the prompted frame, and what to do when it
/// looks wrong.
fn found_notes(matte: &Matte, shape: Option<&Shape>, t: Time) -> Vec<String> {
    let area = matte.area();
    let Some(bounds) = matte.bounds().filter(|_| area > 0.0005) else {
        return vec![format!(
            "On the frame at {t} the model found no object: put `include` points on the object itself, or a `box` around it."
        )];
    };
    let mut where_ = String::new();
    if let Some(shape) = shape {
        let [u0, v0, u1, v1] = bounds;
        let corners = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]].map(|c| uv_to_frame(shape, c));
        let (x0, x1) = corners
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), c| (a.min(c[0]), b.max(c[0])));
        let (y0, y1) = corners
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), c| (a.min(c[1]), b.max(c[1])));
        where_ = format!(", frame pixels x {x0:.0}–{x1:.0}, y {y0:.0}–{y1:.0}");
    }
    let mut notes = vec![format!(
        "On the frame at {t} it keeps {:.1} % of the picture{where_}; the model's confidence {:.2}.",
        area * 100.0,
        matte.score
    )];
    if area > 0.8 {
        notes.push("That is nearly the whole picture: the clicks may be on the background. Put them on the object, or `exclude` the background.".into());
    } else if matte.score < 0.7 {
        notes.push("The model is unsure: `exclude` points on parts it took by mistake and `include` points on parts it missed sharpen it.".into());
    }
    notes
}

/// How the subject `r` spans the clip: held from its prompted frames, or
/// followed (and how long that takes here).
fn tracking_note(
    service: &dyn SubjectService,
    r: flikk_core::services::SubjectRef<'_>,
    kind: MediaKind,
) -> Option<String> {
    if kind == MediaKind::Image {
        return None;
    }
    if !r.subject.track {
        return Some("Each frame shows the nearest prompted frame's shape; pass `track: true` to follow the object as it moves.".into());
    }
    let status = service.status(r);
    let left = status.frames.saturating_sub(status.tracked);
    if left == 0 {
        return Some("It is followed through every frame of the clip.".into());
    }
    let pace = match status.seconds_per_frame {
        Some(spf) => format!(
            "about {:.0} min on this machine",
            (left as f32 * spf / 60.0).ceil()
        ),
        None => "a few seconds a frame".into(),
    };
    Some(format!(
        "It is followed through the clip's {left} frames in the background ({pace}); renders wait for it, and get_clip shows how far it got. Prompts on other frames (`at`) correct it where it drifts."
    ))
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

/// `remove_background`: on each of `clips`, a person mask keeping the
/// people (or with `keep: background`, cutting them away), the clip's own
/// changed when it has one; with `restore`, its person masks taken away.
pub fn remove_background(ctx: &mut ToolCtx, args: &Args) -> ToolResultOf<Vec<Command>> {
    let project = ctx.project();
    let mut ids = args.strings("clips")?;
    if let Some(one) = args.opt_str("clip")? {
        ids.push(one.to_owned());
    }
    if ids.is_empty() {
        return fail("give `clips`");
    }
    let invert = match args.opt_str("keep")? {
        None | Some("people") => false,
        Some("background") => true,
        Some(k) => return fail(format!("`keep` is people or background, not `{k}`")),
    };
    let restore = args.opt_bool("restore")? == Some(true);
    let edge = args.opt_f64("edge")?.map(|v| (v as f32).clamp(0.0, 1.0));
    let feather = args.opt_f64("feather")?.map(|v| (v as f32).max(0.0));
    let expansion = args.opt_f64("expansion")?.map(|v| v as f32);
    let mut commands = Vec::new();
    for text in ids {
        let id = clip_id(project, &text)?;
        let (track, clip) = project
            .timeline
            .clip(id)
            .ok_or_else(|| ToolError(format!("clip {text} not found")))?;
        if track.kind == TrackKind::Audio || compose::is_lettering(track, clip) {
            return fail(format!("{text} is not a picture clip"));
        }
        if !matches!(clip.content, ClipContent::Media { .. }) {
            return fail(format!(
                "{text} is not video or an image from the pool: only those have people to find"
            ));
        }
        let mut masks = clip.masks.clone();
        if restore {
            masks.retain(|m| !m.is_person());
        } else {
            let i = match masks.iter().position(Mask::is_person) {
                Some(i) => i,
                None => {
                    masks.push(Mask::person());
                    masks.len() - 1
                }
            };
            let mask = &mut masks[i];
            mask.invert = invert;
            mask.enabled = true;
            let mut states: Vec<&mut MaskState> = std::iter::once(&mut mask.state)
                .chain(mask.keyframes.iter_mut().map(|k| &mut k.state))
                .collect();
            for state in &mut states {
                if let (Some(e), MaskShape::Person { edge }) = (edge, &mut state.shape) {
                    *edge = e;
                }
                if let Some(f) = feather {
                    state.feather = f;
                }
                if let Some(x) = expansion {
                    state.expansion = x;
                }
            }
        }
        commands.push(Command::SetMasks {
            clip: clip.id,
            masks,
        });
    }
    Ok(commands)
}
