//! Looking at FX and color: `preview_fx` renders a program over time into
//! one stamped collage (the agent's write → look → fix loop), `get_scopes`
//! reads a frame's levels and `match_grade` proposes grades from them. All
//! decoding and rendering runs off-thread on owned copies.
//!
//! Frames are the source picture: no grade and no clip effects are applied
//! until the compositor lands in core.

use super::fx::{fx_asset, kind_name, preview_values};
use super::view::{image_content, save_png};
use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::*;
use crate::bridge::{Content, ToolResult};
use koo_core::command::Command;
use koo_core::fx::{self, FxAsset, FxInstance, FxKind, FxLanguage};
use koo_core::model::{Clip, ClipContent, Grade, MediaItem, Project};
use koo_core::query::topmost_video_clip;
use koo_core::scopes::{self, ScopeReadings, Stats};
use koo_core::services::{Frame, FxCall, Services};
use koo_core::{ClipId, Time};
use serde_json::{Value, json};
use std::sync::Arc;

const TILE_WIDTH: u32 = 384;
const SCOPE_WIDTH: u32 = 960;
const UNGRADED: &str = "Frames are the source picture, before grades and clip effects.";

/// A picture to feed a program, resolved on the UI thread.
#[derive(Clone)]
enum Source {
    Media {
        item: MediaItem,
        at: Time,
    },
    Generator {
        asset: FxAsset,
        fx: FxInstance,
        at: Time,
        duration: f64,
    },
    /// A generated card: 0 = "A" (warm), 1 = "B" (cool).
    Card(u8),
}

/// The picture of `clip` at timeline time `t` (clamped inside the clip).
fn clip_source(project: &Project, clip: &Clip, t: Time) -> Option<Source> {
    let t = t.clamp(clip.start, clip.end() - Time(1).min(clip.duration));
    match &clip.content {
        ClipContent::Media { media, .. } => Some(Source::Media {
            item: project.media(*media)?.clone(),
            at: clip.to_source(t)?,
        }),
        ClipContent::Generator { fx } => Some(Source::Generator {
            asset: fx::asset(project, &fx.fx)?.clone(),
            fx: fx.clone(),
            at: t - clip.start,
            duration: clip.duration.as_secs_f64(),
        }),
        _ => None,
    }
}

fn render_source(
    services: &Services,
    source: &Source,
    canvas: (u32, u32),
    size: (u32, u32),
) -> Result<Frame, String> {
    let frame = match source {
        Source::Media { item, at } => services
            .media
            .as_ref()
            .ok_or("frames are not available in this build")?
            .frame(item, *at, size.0)
            .map_err(|e| e.0)?,
        Source::Generator {
            asset,
            fx,
            at,
            duration,
        } => {
            let values = fx.values_at(asset, *at);
            let call = FxCall {
                asset,
                values: &values,
                time: at.as_secs_f64(),
                duration: *duration,
                canvas,
            };
            generate(services, &call, size)?
        }
        Source::Card(which) => test_card(*which, size),
    };
    Ok(fit(&frame, size))
}

fn generate(services: &Services, call: &FxCall, size: (u32, u32)) -> Result<Frame, String> {
    match call.asset.language {
        FxLanguage::Glsl => services
            .fx
            .as_ref()
            .ok_or_else(|| unavailable(FxLanguage::Glsl))?
            .generate(call, size.0, size.1)
            .map_err(|e| e.0),
        FxLanguage::Html => services
            .html
            .as_ref()
            .ok_or_else(|| unavailable(FxLanguage::Html))?
            .frame(call, size.0, size.1)
            .map_err(|e| e.0),
    }
}

fn unavailable(language: FxLanguage) -> String {
    match language {
        FxLanguage::Glsl => "the shader engine is not available in this build (no GPU engine)",
        FxLanguage::Html => "the HTML engine is not available in this build (no browser engine)",
    }
    .to_owned()
}

/// What `preview_fx` renders: the program, how its input is made, its clock.
struct Plan {
    asset: FxAsset,
    fx: FxInstance,
    duration: f64,
    inputs: (Option<Source>, Option<Source>),
    from_label: String,
}

pub fn preview_fx(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let p = ctx.project();
    let asset = fx_asset(p, args.str("fx")?, None)?.clone();
    let n = args
        .opt_f64("frames")?
        .map(|f| f.round().clamp(2.0, 16.0) as usize)
        .unwrap_or(9);
    let clip = |key: &str| -> ToolResultOf<Option<&Clip>> {
        args.opt_str(key)?
            .map(|text| {
                let id = clip_id(p, text)?;
                Ok(p.timeline.clip(id).map(|(_, c)| c))
            })
            .transpose()
            .map(Option::flatten)
    };
    let main = clip("clip")?;
    let duration_arg = args.opt_duration("duration")?.map(|d| d.as_secs_f64());
    let plan = match asset.kind {
        FxKind::Generator => {
            let base = main.and_then(|c| match &c.content {
                ClipContent::Generator { fx } if fx.fx == asset.id => Some(fx),
                _ => None,
            });
            Plan {
                fx: preview_values(&asset, base, args.value("params"))?,
                duration: duration_arg
                    .or(main.map(|c| c.duration.as_secs_f64()))
                    .unwrap_or(3.0),
                inputs: (None, None),
                from_label: "its own".into(),
                asset,
            }
        }
        FxKind::Effect => {
            let base = main.and_then(|c| c.effects.iter().find(|e| e.fx == asset.id));
            let input = match main {
                Some(c) => {
                    let at = args
                        .opt_time("at")?
                        .unwrap_or(c.start + Time(c.duration.0 / 2));
                    Some(clip_source(p, c, at).ok_or_else(|| {
                        ToolError(format!("{} has no picture to preview on", c.id))
                    })?)
                }
                None => None,
            };
            Plan {
                fx: preview_values(&asset, base, args.value("params"))?,
                duration: duration_arg
                    .or(main.map(|c| c.duration.as_secs_f64()))
                    .unwrap_or(3.0),
                from_label: main.map_or("a test card".into(), |c| format!("clip {}", c.id)),
                inputs: (Some(input.unwrap_or(Source::Card(0))), None),
                asset,
            }
        }
        FxKind::Transition => {
            let from = clip("from")?.or(main);
            let to = clip("to")?;
            let from_src = match from {
                Some(c) => clip_source(p, c, c.end()),
                None => None,
            };
            let to_src = match to {
                Some(c) => clip_source(p, c, c.start),
                None => None,
            };
            let base = main.and_then(|c| {
                [&c.transition_in, &c.transition_out]
                    .into_iter()
                    .flatten()
                    .find(|t| t.fx.fx == asset.id)
            });
            Plan {
                fx: preview_values(&asset, base.map(|t| &t.fx), args.value("params"))?,
                duration: duration_arg
                    .or(base.map(|t| t.duration.as_secs_f64()))
                    .unwrap_or(1.0),
                from_label: format!(
                    "{} → {}",
                    from.map_or("card A".into(), |c| format!("clip {}", c.id)),
                    to.map_or("card B".into(), |c| format!("clip {}", c.id))
                ),
                inputs: (
                    Some(from_src.unwrap_or(Source::Card(0))),
                    Some(to_src.unwrap_or(Source::Card(1))),
                ),
                asset,
            }
        }
    };
    let needed = match plan.asset.language {
        FxLanguage::Glsl => ctx.services.fx.is_none(),
        FxLanguage::Html => ctx.services.html.is_none(),
    };
    if needed {
        return Err(ToolError(format!(
            "Can't preview `{}`: {}. Its code was not run; describe the expected look instead and let the user check it in the viewer.",
            plan.asset.id,
            unavailable(plan.asset.language)
        )));
    }
    let settings = &p.timeline.settings;
    let canvas = (settings.width.max(16), settings.height.max(16));
    let size = (
        TILE_WIDTH,
        (TILE_WIDTH as u64 * canvas.1 as u64 / canvas.0 as u64).max(8) as u32,
    );
    let services = ctx.services.clone();
    let job: Job = Box::new(
        move || match render_preview(&services, &plan, n, canvas, size) {
            Ok(done) => JobDone::Done(done),
            Err(e) => JobDone::Done(Done::error(format!("Preview failed: {e}"))),
        },
    );
    Ok(Step::Job(job))
}

fn render_preview(
    services: &Services,
    plan: &Plan,
    n: usize,
    canvas: (u32, u32),
    size: (u32, u32),
) -> Result<Done, String> {
    let from = plan
        .inputs
        .0
        .as_ref()
        .map(|s| render_source(services, s, canvas, size))
        .transpose()?;
    let to = plan
        .inputs
        .1
        .as_ref()
        .map(|s| render_source(services, s, canvas, size))
        .transpose()?;
    let asset = &plan.asset;
    let mut tiles = Vec::with_capacity(n);
    let mut labels = Vec::with_capacity(n);
    for i in 0..n {
        let u = i as f64 / (n - 1).max(1) as f64;
        // Generators and effects stop a frame short of the end, where
        // exit animations have finished; transitions show both ends.
        let time = match asset.kind {
            FxKind::Transition => u * plan.duration,
            _ => u * (plan.duration - 1.0 / 30.0).max(0.0),
        };
        let values = plan.fx.values_at(asset, Time::from_secs_f64(time));
        let call = FxCall {
            asset,
            values: &values,
            time,
            duration: plan.duration,
            canvas,
        };
        let frame = match asset.kind {
            FxKind::Generator => generate(services, &call, size)?,
            FxKind::Effect => services
                .fx
                .as_ref()
                .ok_or_else(|| unavailable(FxLanguage::Glsl))?
                .apply(from.as_ref().ok_or("no input")?, &[call])
                .map_err(|e| e.0)?,
            FxKind::Transition => services
                .fx
                .as_ref()
                .ok_or_else(|| unavailable(FxLanguage::Glsl))?
                .transition(
                    from.as_ref().ok_or("no input")?,
                    to.as_ref().ok_or("no input")?,
                    &call,
                )
                .map_err(|e| e.0)?,
        };
        let label = match asset.kind {
            FxKind::Transition => format!("p={:.2}", u),
            _ => format!("{time:.2}s"),
        };
        labels.push(format!("{}: {label}", i + 1));
        tiles.push((fit(&frame, size), label));
    }
    let collage = collage(&tiles, size);
    let path = save_png(&collage).map_err(|e| e.0)?;
    let image = image_content(&path).map_err(|e| e.0)?;
    let cols = columns(n);
    let text = format!(
        "{} `{}` ({}) over {:.2} s on {}: {n} frames, {cols} columns, left to right then top to bottom; transparent areas show a checkerboard. {}",
        kind_name(asset.kind),
        asset.id,
        asset.name,
        plan.duration,
        plan.from_label,
        labels.join(", ")
    );
    Ok(Done {
        result: ToolResult {
            content: vec![Content::Text { text }, image],
            is_error: false,
        },
        summary: format!("preview {}: {n} frames", asset.id),
        images: vec![path],
        ..Default::default()
    })
}

// ------------------------------------------------------------------ scopes

fn r3(v: f32) -> f64 {
    (v as f64 * 1000.0).round() / 1000.0
}

fn stats_json(s: &Stats) -> Value {
    json!({
        "min": r3(s.min), "p5": r3(s.p5), "mean": r3(s.mean),
        "median": r3(s.median), "p95": r3(s.p95), "max": r3(s.max),
    })
}

fn readings_json(r: &ScopeReadings) -> Value {
    // 64 bins → 16 for reading.
    let luma: Vec<f64> = r.histogram[0]
        .chunks(4)
        .map(|c| r3(c.iter().sum()))
        .collect();
    json!({
        "luma": stats_json(&r.luma),
        "red": stats_json(&r.red),
        "green": stats_json(&r.green),
        "blue": stats_json(&r.blue),
        "saturation": stats_json(&r.saturation),
        "crushed": r3(r.crushed),
        "clipped": r3(r.clipped),
        "cast_cb_cr": [r3(r.cast[0]), r3(r.cast[1])],
        "luma_histogram_16": luma,
    })
}

/// The clip and time `get_scopes` / `match_grade` read: the given clip (at
/// `t`, default its middle) or the topmost video clip at `t` (default the
/// playhead).
fn scope_target<'a>(
    ctx: &'a ToolCtx,
    args: &Args,
    clip_key: &str,
) -> ToolResultOf<(&'a Clip, Time)> {
    let p = ctx.project();
    let t = args.opt_time("t")?;
    match args.opt_str(clip_key)? {
        Some(text) => {
            let id = clip_id(p, text)?;
            let (_, c) = p
                .timeline
                .clip(id)
                .ok_or_else(|| ToolError("clip not found".into()))?;
            Ok((c, t.unwrap_or(c.start + Time(c.duration.0 / 2))))
        }
        None => {
            let t = t.unwrap_or(ctx.editor.playhead);
            let (_, c) = topmost_video_clip(&p.timeline, t)
                .ok_or_else(|| ToolError(format!("No video on the timeline at {t}.")))?;
            Ok((c, t))
        }
    }
}

pub fn get_scopes(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let p = ctx.project();
    let (clip, t) = scope_target(ctx, args, "clip")?;
    let source = clip_source(p, clip, t)
        .ok_or_else(|| ToolError(format!("{} has no picture to measure", clip.id)))?;
    let id = clip.id;
    let services = ctx.services.clone();
    let canvas = (p.timeline.settings.width, p.timeline.settings.height);
    let size = (
        SCOPE_WIDTH,
        (SCOPE_WIDTH as u64 * canvas.1 as u64 / canvas.0.max(1) as u64) as u32,
    );
    let job: Job = Box::new(
        move || match render_source(&services, &source, canvas, size) {
            Ok(frame) => {
                let mut v = readings_json(&scopes::measure(&frame));
                v["clip"] = json!(id.to_string());
                v["t"] = secs(t);
                v["note"] = json!(format!(
                    "{UNGRADED} Values are 0..1; cast is the vectorscope's centre (Cb, Cr), away from 0 means a color cast."
                ));
                JobDone::Done(Done::json(format!("scopes {id} at {t}"), &v))
            }
            Err(e) => JobDone::Done(Done::error(format!("Could not read the frame: {e}"))),
        },
    );
    Ok(Step::Job(job))
}

pub fn match_grade(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let p = ctx.project();
    let (reference, t) = scope_target(ctx, args, "reference")?;
    let reference_src = clip_source(p, reference, t)
        .ok_or_else(|| ToolError(format!("{} has no picture", reference.id)))?;
    let targets: Vec<(ClipId, Source, Grade)> = clip_ids(p, &args.strings("clips")?)?
        .into_iter()
        .map(|id| {
            let (_, c) = p
                .timeline
                .clip(id)
                .ok_or_else(|| ToolError("clip not found".into()))?;
            let src = clip_source(p, c, c.start + Time(c.duration.0 / 2))
                .ok_or_else(|| ToolError(format!("{id} has no picture")))?;
            Ok((id, src, c.grade.clone()))
        })
        .collect::<ToolResultOf<_>>()?;
    if targets.is_empty() {
        return fail("`clips` is empty");
    }
    let reason = args
        .opt_str("reason")?
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Match to {}", reference.id));
    let reference_id = reference.id;
    let services = ctx.services.clone();
    let canvas = (p.timeline.settings.width, p.timeline.settings.height);
    let size = (480, (480 * canvas.1 as u64 / canvas.0.max(1) as u64) as u32);
    let job: Job = Box::new(move || {
        let measure =
            |s: &Source| render_source(&services, s, canvas, size).map(|f| scopes::measure(&f));
        let reference = match measure(&reference_src) {
            Ok(r) => r,
            Err(e) => {
                return JobDone::Done(Done::error(format!(
                    "Could not read the reference {reference_id}: {e}"
                )));
            }
        };
        let mut commands = Vec::new();
        let mut excerpts = Vec::new();
        for (id, src, current) in &targets {
            match measure(src) {
                Ok(r) => {
                    let grade = scopes::match_grade(&reference, &r, current);
                    excerpts.push(format!(
                        "{id}: luma median {:.3} → reference {:.3}",
                        r.luma.median, reference.luma.median
                    ));
                    commands.push(Command::SetGrade {
                        clips: vec![*id],
                        grade,
                    });
                }
                Err(e) => excerpts.push(format!("{id}: {e}")),
            }
        }
        if commands.is_empty() {
            return JobDone::Done(Done::error(format!(
                "Could not read any clip: {}",
                excerpts.join("; ")
            )));
        }
        excerpts.push(UNGRADED.into());
        JobDone::Propose {
            commands,
            reason,
            excerpts,
        }
    });
    Ok(Step::Job(job))
}

// ------------------------------------------------------------------ images

fn columns(n: usize) -> usize {
    match n {
        0..=3 => n.max(1),
        4 => 2,
        5..=9 => 3,
        _ => 4,
    }
}

/// `frame` scaled (nearest) to exactly `size`, letterboxed with
/// transparency to keep its aspect.
fn fit(frame: &Frame, size: (u32, u32)) -> Frame {
    let (w, h) = size;
    if frame.width == w && frame.height == h {
        return frame.clone();
    }
    let (fw, fh) = (frame.width.max(1) as f64, frame.height.max(1) as f64);
    let scale = (w as f64 / fw).min(h as f64 / fh);
    let (dw, dh) = ((fw * scale).round() as u32, (fh * scale).round() as u32);
    let (ox, oy) = ((w - dw.min(w)) / 2, (h - dh.min(h)) / 2);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..dh.min(h) {
        let sy = ((y as f64 / scale) as u32).min(frame.height - 1);
        for x in 0..dw.min(w) {
            let sx = ((x as f64 / scale) as u32).min(frame.width - 1);
            let s = ((sy * frame.width + sx) * 4) as usize;
            let d = (((y + oy) * w + x + ox) * 4) as usize;
            rgba[d..d + 4].copy_from_slice(&frame.rgba[s..s + 4]);
        }
    }
    Frame {
        width: w,
        height: h,
        rgba: Arc::from(rgba),
    }
}

/// Two distinct pictures to run effects and transitions on: "A" warm with a
/// grid, "B" cool with rings.
fn test_card(which: u8, (w, h): (u32, u32)) -> Frame {
    let mut rgba = vec![255u8; (w * h * 4) as usize];
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let i = ((y * w + x) * 4) as usize;
            let (r, g, b) = if which == 0 {
                let line = x % 32 < 2 || y % 32 < 2;
                let k = if line { 0.55 } else { 1.0 };
                (240.0 * k, (90.0 + 120.0 * u) * k, (40.0 + 60.0 * v) * k)
            } else {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                let ring = if (d as u32 / 12).is_multiple_of(2) {
                    1.0
                } else {
                    0.7
                };
                (
                    (30.0 + 60.0 * v) * ring,
                    (90.0 + 80.0 * u) * ring,
                    235.0 * ring,
                )
            };
            rgba[i] = r as u8;
            rgba[i + 1] = g as u8;
            rgba[i + 2] = b as u8;
        }
    }
    let mut f = Frame {
        width: w,
        height: h,
        rgba: Arc::from(rgba),
    };
    let letter = if which == 0 { "A" } else { "B" };
    let mut buf = f.rgba.to_vec();
    draw_text(&mut buf, w, letter, 12, 12, 6, [255, 255, 255]);
    f.rgba = Arc::from(buf);
    f
}

const GAP: u32 = 6;
const BAND: u32 = 20;

/// Tiles in a grid on a dark background, alpha over a checkerboard, each
/// with its label in a band below.
fn collage(tiles: &[(Frame, String)], (tw, th): (u32, u32)) -> Frame {
    let cols = columns(tiles.len()) as u32;
    let rows = (tiles.len() as u32).div_ceil(cols);
    let cell_h = th + BAND;
    let w = cols * tw + (cols + 1) * GAP;
    let h = rows * cell_h + (rows + 1) * GAP;
    let mut out = vec![0u8; (w * h * 4) as usize];
    for px in out.as_chunks_mut::<4>().0 {
        *px = [24, 24, 28, 255];
    }
    for (i, (frame, label)) in tiles.iter().enumerate() {
        let (c, r) = (i as u32 % cols, i as u32 / cols);
        let (ox, oy) = (GAP + c * (tw + GAP), GAP + r * (cell_h + GAP));
        for y in 0..th.min(frame.height) {
            for x in 0..tw.min(frame.width) {
                let s = ((y * frame.width + x) * 4) as usize;
                let p = &frame.rgba[s..s + 4];
                let check = if (x / 8 + y / 8).is_multiple_of(2) {
                    72.0
                } else {
                    104.0
                };
                let a = p[3] as f32 / 255.0;
                let d = (((oy + y) * w + ox + x) * 4) as usize;
                for k in 0..3 {
                    out[d + k] = (p[k] as f32 * a + check * (1.0 - a)).round() as u8;
                }
                out[d + 3] = 255;
            }
        }
        draw_text(&mut out, w, label, ox + 2, oy + th + 3, 2, [230, 230, 235]);
    }
    Frame {
        width: w,
        height: h,
        rgba: Arc::from(out),
    }
}

/// 5 × 7 glyphs, one row per byte (low 5 bits, MSB on the left).
fn glyph(c: char) -> [u8; 7] {
    match c {
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 30, 1, 1, 17, 14],
        '6' => [6, 8, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 2, 12],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '=' => [0, 0, 31, 0, 31, 0, 0],
        's' => [0, 0, 14, 16, 14, 1, 30],
        'p' => [0, 0, 30, 17, 30, 16, 16],
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        _ => [0; 7],
    }
}

fn draw_text(buf: &mut [u8], width: u32, text: &str, x: u32, y: u32, scale: u32, rgb: [u8; 3]) {
    let height = buf.len() as u32 / 4 / width.max(1);
    for (i, c) in text.chars().enumerate() {
        let gx = x + i as u32 * 6 * scale;
        for (row, bits) in glyph(c).iter().enumerate() {
            for col in 0..5 {
                if bits & (16 >> col) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (px, py) = (gx + col * scale + dx, y + row as u32 * scale + dy);
                        if px < width && py < height {
                            let d = ((py * width + px) * 4) as usize;
                            buf[d..d + 3].copy_from_slice(&rgb);
                            buf[d + 3] = 255;
                        }
                    }
                }
            }
        }
    }
}
