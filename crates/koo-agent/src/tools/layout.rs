//! Where things are on screen, and putting them somewhere: `get_layout`
//! reads every visible item's box at a time (`koo_core::compose::layout`,
//! as the viewer picks them), `place` aligns and sizes a clip by meaning
//! (corner, margin, fit) instead of numbers, and `animate` writes preset
//! moves as ordinary keyframes the user can edit.

use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::*;
use koo_core::Time;
use koo_core::command::Command;
use koo_core::compose::{self, Shape, Stills};
use koo_core::model::{Clip, ClipContent, Ease, Project, Property, Transform};
use koo_core::services::Services;
use serde_json::{Value, json};
use std::sync::Arc;

/// Margins of the action- and title-safe areas, as a fraction of the frame.
const ACTION_SAFE: f32 = 0.05;
const TITLE_SAFE: f32 = 0.1;
/// Default `place` margin, as a fraction of the frame height.
const MARGIN: f32 = 0.05;

pub const ALIGNS: [&str; 9] = [
    "top_left",
    "top",
    "top_right",
    "left",
    "center",
    "right",
    "bottom_left",
    "bottom",
    "bottom_right",
];

pub const FITS: [&str; 6] = ["keep", "contain", "cover", "fill", "width", "height"];

pub const PRESETS: [&str; 19] = [
    "fade_in",
    "fade_out",
    "pop_in",
    "pop_out",
    "slide_in_left",
    "slide_in_right",
    "slide_in_up",
    "slide_in_down",
    "slide_out_left",
    "slide_out_right",
    "slide_out_up",
    "slide_out_down",
    "spin_in",
    "spin_out",
    "grow_in",
    "shrink_out",
    "pulse",
    "shake",
    "drift",
];

/// Every layer at `t` on the sequence, topmost first.
fn shapes_at(project: &Project, services: &Services, t: Time) -> Vec<Shape> {
    let s = &project.timeline.settings;
    let mut stills = Stills::new(services.media.clone(), 640);
    compose::layout(project, t, (s.width, s.height), services, &mut stills)
}

/// Pixels to a tenth.
fn round(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}

/// Fractions to a thousandth.
fn norm(v: f32) -> f64 {
    (v as f64 * 1000.0).round() / 1000.0
}

/// What kind of thing a clip shows.
pub fn content_kind(project: &Project, clip: &Clip) -> &'static str {
    match &clip.content {
        ClipContent::Media { media, .. } => match project.media(*media).map(|m| m.kind) {
            Some(koo_core::model::MediaKind::Image) => "image",
            _ => "video",
        },
        ClipContent::Title { .. } => "title",
        ClipContent::Caption { .. } => "caption",
        ClipContent::Overlay { .. } => "overlay",
        ClipContent::Generator { .. } => "generator",
    }
}

/// Area of the intersection of two `[x0, y0, x1, y1]` boxes.
fn overlap(a: [f32; 4], b: [f32; 4]) -> f32 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    w * h
}

fn area(b: [f32; 4]) -> f32 {
    (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0)
}

/// `shapes` as JSON: each item's box in pixels and as a fraction of the
/// frame, and the pairs that overlap.
pub fn layout_json(project: &Project, shapes: &[Shape], t: Time) -> Value {
    let s = &project.timeline.settings;
    let (w, h) = (s.width.max(1) as f32, s.height.max(1) as f32);
    let safe = |f: f32| [w * f, h * f, w * (1.0 - f), h * (1.0 - f)];
    let title = safe(TITLE_SAFE);
    let items: Vec<Value> = shapes
        .iter()
        .filter_map(|shape| {
            let (track, clip) = project.timeline.clip(shape.clip)?;
            let b = shape.bounds();
            let tf = clip.transform_at(t - clip.start);
            let mut v = json!({
                "clip": short(clip.id),
                "track": track.name,
                "kind": content_kind(project, clip),
                "name": clip.display_name(project),
                "bounds": b.map(round),
                "bounds_fraction": [norm(b[0] / w), norm(b[1] / h), norm(b[2] / w), norm(b[3] / h)],
                "center": [round((b[0] + b[2]) / 2.0), round((b[1] + b[3]) / 2.0)],
                "frame_fraction": norm(overlap(b, [0.0, 0.0, w, h]) / (w * h)),
                "inside_title_safe": b[0] >= title[0] - 0.5
                    && b[1] >= title[1] - 0.5
                    && b[2] <= title[2] + 0.5
                    && b[3] <= title[3] + 0.5,
                "transform": transform_json(&tf),
            });
            let turned = shape.placement.rotation_deg.rem_euclid(90.0).abs() > 0.01
                || shape.placement.skew_deg.iter().any(|v| v.abs() > 0.01);
            if turned {
                v["corners"] = json!(shape.corners().map(|c| c.map(round)));
            }
            Some(v)
        })
        .collect();
    let mut overlaps = Vec::new();
    for (i, a) in shapes.iter().enumerate() {
        for b in &shapes[i + 1..] {
            let (ba, bb) = (a.bounds(), b.bounds());
            let common = overlap(ba, bb);
            let smaller = area(ba).min(area(bb));
            if common > 0.0 && smaller > 0.0 {
                overlaps.push(json!({
                    "above": short(a.clip),
                    "below": short(b.clip),
                    "fraction_of_smaller": norm(common / smaller),
                }));
            }
        }
    }
    json!({
        "t": secs(t),
        "frame": { "width": s.width, "height": s.height },
        "action_safe": safe(ACTION_SAFE).map(round),
        "title_safe": title.map(round),
        "items": items,
        "overlaps": overlaps,
    })
}

/// The transform's values as tools name them.
pub fn transform_json(tf: &Transform) -> Value {
    let n = |v: f32| (v as f64 * 10_000.0).round() / 10_000.0;
    json!({
        "position": tf.position.map(n),
        "scale": n(tf.scale),
        "stretch": tf.stretch.map(n),
        "rotation": n(tf.rotation_deg),
        "skew": tf.skew_deg.map(n),
        "anchor": tf.anchor.map(n),
        "opacity": n(tf.opacity),
        "crop": tf.crop.map(n),
        "flip": tf.flip,
        "blend": super::edit::blend_name(tf.blend),
    })
}

pub fn get_layout(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = Arc::new(ctx.project().clone());
    let rate = project.timeline.settings.rate;
    let t = args
        .opt_time("t")?
        .unwrap_or(ctx.editor.playhead)
        .max(Time::ZERO)
        .floor_to_frame(rate);
    let services = ctx.services.clone();
    let job: Job = Box::new(move || {
        let shapes = shapes_at(&project, &services, t);
        let value = layout_json(&project, &shapes, t);
        let n = shapes.len();
        JobDone::Done(Done::json(format!("layout at {t}: {n} items"), &value))
    });
    Ok(Step::Job(job))
}

/// A time inside `clip` to lay it out at: `at` if given, else its middle.
fn time_in(clip: &Clip, at: Option<Time>, project: &Project) -> ToolResultOf<Time> {
    let rate = project.timeline.settings.rate;
    match at {
        Some(t) if clip.range().contains(t) => Ok(t.floor_to_frame(rate).max(clip.start)),
        Some(t) => fail(format!(
            "{t} is outside the clip ({} – {})",
            clip.start,
            clip.end()
        )),
        None => Ok((clip.start + Time(clip.duration.0 / 2)).floor_to_frame(rate)),
    }
}

pub fn place(ctx: &mut ToolCtx, args: &Args, reason: &str) -> Result<Step, ToolError> {
    let project = ctx.project();
    let id = clip_id(project, args.str("clip")?)?;
    let (_, clip) = project
        .timeline
        .clip(id)
        .ok_or_else(|| ToolError("clip not found".into()))?;
    let t = time_in(clip, args.opt_time("at")?, project)?;
    let align = args.opt_str("align")?.unwrap_or("center");
    if !ALIGNS.contains(&align) {
        return fail(format!("`align` is one of {}", ALIGNS.join(", ")));
    }
    let fit = args.opt_str("fit")?.unwrap_or("keep");
    if !FITS.contains(&fit) {
        return fail(format!("`fit` is one of {}", FITS.join(", ")));
    }
    let size = args.opt_f64("size")?.map(|v| v as f32);
    if matches!(fit, "width" | "height") && !size.is_some_and(|s| s > 0.0) {
        return fail(
            "`fit: width` and `fit: height` need `size` (a fraction of the frame, e.g. 0.2)",
        );
    }
    let margin = match args.opt_bool("safe_area")? {
        Some(true) => TITLE_SAFE,
        _ => args
            .opt_f64("margin")?
            .map_or(MARGIN, |m| m as f32)
            .max(0.0),
    };
    let project = Arc::new(project.clone());
    let services = ctx.services.clone();
    let (align, fit, reason) = (align.to_owned(), fit.to_owned(), reason.to_owned());
    let job: Job = Box::new(move || {
        let shapes = shapes_at(&project, &services, t);
        let Some(shape) = shapes.iter().find(|s| s.clip == id) else {
            return JobDone::Done(Done::error(format!(
                "The clip draws nothing at {t} (hidden, transparent or covered by a transition); pass `at`."
            )));
        };
        let Some((_, clip)) = project.timeline.clip(id) else {
            return JobDone::Done(Done::error("clip not found"));
        };
        let tf = placed(&project, clip, shape, t, &align, &fit, size, margin);
        let commands = Command::transform_at(clip, t - clip.start, &tf);
        if commands.is_empty() {
            return JobDone::Done(Done::text(
                "already placed",
                "The clip is already there; nothing to change.",
            ));
        }
        JobDone::Propose {
            commands,
            reason,
            excerpts: Vec::new(),
        }
    });
    Ok(Step::Job(job))
}

/// `clip`'s transform at `t` moved so its drawn part sits at `align`,
/// `margin` (a fraction of the frame height) in from the edges, sized by
/// `fit`.
#[allow(clippy::too_many_arguments)]
fn placed(
    project: &Project,
    clip: &Clip,
    shape: &Shape,
    t: Time,
    align: &str,
    fit: &str,
    size: Option<f32>,
    margin: f32,
) -> Transform {
    let s = &project.timeline.settings;
    let canvas = (s.width.max(1), s.height.max(1));
    let (w, h) = (canvas.0 as f32, canvas.1 as f32);
    let m = margin * h;
    let mut tf = clip.transform_at(t - clip.start);
    // The drawn part's size at scale 1 and no stretch.
    let [u0, v0, u1, v1] = shape.content;
    let cw = ((u1 - u0) * shape.rest_size[0]).max(1e-3);
    let ch = ((v1 - v0) * shape.rest_size[1]).max(1e-3);
    let [sx, sy] = tf.stretch.map(|v| v.max(1e-3));
    match fit {
        "contain" => tf.scale = ((w - 2.0 * m) / (cw * sx)).min((h - 2.0 * m) / (ch * sy)),
        "cover" => tf.scale = (w / (cw * sx)).max(h / (ch * sy)),
        "fill" => {
            tf.scale = 1.0;
            tf.stretch = [w / cw, h / ch];
        }
        "width" => tf.scale = size.unwrap_or(1.0) * w / (cw * sx),
        "height" => tf.scale = size.unwrap_or(1.0) * h / (ch * sy),
        _ => {}
    }
    tf.scale = tf.scale.max(0.001);
    // Where the drawn part lands with no offset.
    tf.position = [0.0, 0.0];
    let geometry = if shape.lettering {
        compose::lettering_transform(tf.clone())
    } else {
        tf.clone()
    };
    let at_rest = Shape {
        placement: compose::placement_from(&geometry, shape.rest_center, shape.rest_size, canvas),
        ..shape.clone()
    };
    let [x0, y0, x1, y1] = at_rest.bounds();
    let dx = if align.ends_with("left") || align == "left" {
        m - x0
    } else if align.ends_with("right") {
        w - m - x1
    } else {
        w / 2.0 - (x0 + x1) / 2.0
    };
    let dy = if align.starts_with("top") {
        m - y0
    } else if align.starts_with("bottom") {
        h - m - y1
    } else {
        h / 2.0 - (y0 + y1) / 2.0
    };
    tf.position = [dx / w, dy / h];
    tf
}

/// One keyframe to write: property, clip-relative time, value, easing.
type Key = (Property, Time, f32, Ease);

pub fn animate(ctx: &mut ToolCtx, args: &Args) -> ToolResultOf<Vec<Command>> {
    let project = ctx.project();
    let id = clip_id(project, args.str("clip")?)?;
    let (_, clip) = project
        .timeline
        .clip(id)
        .ok_or_else(|| ToolError("clip not found".into()))?;
    let preset = args.str("preset")?;
    if !PRESETS.contains(&preset) {
        return fail(format!("`preset` is one of {}", PRESETS.join(", ")));
    }
    let looping = matches!(preset, "pulse" | "shake" | "drift");
    let default = if looping {
        clip.duration
    } else {
        Time::from_secs_f64(0.4)
    };
    let duration = args
        .opt_f64("duration")?
        .map(Time::from_secs_f64)
        .unwrap_or(default)
        .max(Time::from_millis(40))
        .min(clip.duration);
    let out = preset.ends_with("_out");
    let start = match args.opt_time("at")? {
        Some(t) => t - clip.start,
        None if out => clip.duration - duration,
        None => Time::ZERO,
    }
    .clamp(Time::ZERO, clip.duration - duration);
    let end = start + duration;
    let distance = args.opt_f64("distance")?.map_or(0.3, |d| d as f32);
    let intensity = args.opt_f64("intensity")?.map_or(1.0, |v| v as f32);
    let keys = preset_keys(clip, preset, start, end, distance, intensity);
    Ok(keys
        .into_iter()
        .map(|(property, at, value, ease)| Command::SetKeyframe {
            clip: id,
            property,
            at: clip.start + at,
            value,
            ease,
        })
        .collect())
}

/// The keyframes of `preset` between clip times `start` and `end`, around
/// the clip's own values there.
fn preset_keys(
    clip: &Clip,
    preset: &str,
    start: Time,
    end: Time,
    distance: f32,
    intensity: f32,
) -> Vec<Key> {
    let out = preset.ends_with("_out");
    // At rest: where an entrance ends and an exit begins.
    let rest = |p: Property| clip.value_at(p, if out { start } else { end });
    let mut keys: Vec<Key> = Vec::new();
    // From `from` to rest for an entrance, rest to `to` for an exit.
    let mut change = |p: Property, away: f32| {
        let v = rest(p);
        if out {
            keys.push((p, start, v, Ease::EaseIn));
            keys.push((p, end, away, Ease::EaseIn));
        } else {
            keys.push((p, start, away, Ease::EaseOut));
            keys.push((p, end, v, Ease::EaseOut));
        }
    };
    let slide = |dir: &str| -> (Property, f32) {
        // Which way the item travels while on screen.
        match dir {
            "left" => (Property::PositionX, -distance),
            "right" => (Property::PositionX, distance),
            "up" => (Property::PositionY, -distance),
            _ => (Property::PositionY, distance),
        }
    };
    match preset {
        "fade_in" | "fade_out" => change(Property::Opacity, 0.0),
        "pop_in" | "pop_out" => {
            change(Property::Scale, rest(Property::Scale) * 0.6);
            change(Property::Opacity, 0.0);
        }
        "grow_in" | "shrink_out" => change(Property::Scale, 0.0),
        "spin_in" | "spin_out" => {
            let turn = if out { 360.0 } else { -360.0 };
            change(Property::Rotation, rest(Property::Rotation) + turn);
            change(Property::Scale, rest(Property::Scale) * 0.3);
            change(Property::Opacity, 0.0);
        }
        p if p.starts_with("slide_in_") => {
            // Enters from that side: starts offset toward it.
            let side = &p["slide_in_".len()..];
            let (prop, d) = slide(side);
            change(prop, rest(prop) + d);
            change(Property::Opacity, 0.0);
        }
        p if p.starts_with("slide_out_") => {
            // Leaves toward that side.
            let side = &p["slide_out_".len()..];
            let (prop, d) = slide(side);
            change(prop, rest(prop) + d);
            change(Property::Opacity, 0.0);
        }
        "pulse" => {
            let v = clip.value_at(Property::Scale, start);
            let mid = start + Time((end.0 - start.0) / 2);
            keys.push((Property::Scale, start, v, Ease::EaseInOut));
            keys.push((
                Property::Scale,
                mid,
                v * (1.0 + 0.12 * intensity),
                Ease::EaseInOut,
            ));
            keys.push((Property::Scale, end, v, Ease::EaseInOut));
        }
        "shake" => {
            let (x, r) = (
                clip.value_at(Property::PositionX, start),
                clip.value_at(Property::Rotation, start),
            );
            let a = 0.012 * intensity;
            let swings = [0.0, 1.0, -1.0, 0.7, -0.5, 0.25, 0.0];
            let n = swings.len() as i64 - 1;
            for (i, s) in swings.iter().enumerate() {
                let at = start + Time((end.0 - start.0) * i as i64 / n);
                keys.push((Property::PositionX, at, x + a * s, Ease::EaseInOut));
                keys.push((
                    Property::Rotation,
                    at,
                    r + 2.0 * intensity * s,
                    Ease::EaseInOut,
                ));
            }
        }
        "drift" => {
            let v = clip.value_at(Property::Scale, start);
            keys.push((Property::Scale, start, v, Ease::Linear));
            keys.push((
                Property::Scale,
                end,
                v * (1.0 + 0.08 * intensity),
                Ease::Linear,
            ));
        }
        _ => {}
    }
    keys
}

/// One line per shape naming its clip, for `view_frame`'s numbered boxes.
pub fn legend(project: &Project, shapes: &[Shape]) -> Vec<String> {
    shapes
        .iter()
        .filter_map(|s| {
            let (_, clip) = project.timeline.clip(s.clip)?;
            Some(format!(
                "@clip {} ({}, {})",
                short(clip.id),
                content_kind(project, clip),
                clip.display_name(project)
            ))
        })
        .collect()
}
