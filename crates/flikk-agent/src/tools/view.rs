//! View tools: the model sees video as collages of at most 30 timestamped
//! frames plus the sound script (PRD V1–V7). Decoding runs off-thread on a
//! snapshot of the project. Where the range uses effects, transitions,
//! generators, blend modes, crop, flip or speed, frames are the composited
//! program (`flikk_core::compose`, as the viewer and the render show it);
//! otherwise the cached source collage.

use super::audio::{script, unheard_note};
use super::preview::{collage, columns, draw_text};
use super::{Done, Job, JobDone, Step, ToolCtx, on_gpu};
use crate::args::{Args, ToolError, range_json, short};
use crate::bridge::{Content, ToolResult};
use base64::Engine;
use flikk_core::agent::VisionSettings;
use flikk_core::compose::{self, Stills};
use flikk_core::memory::SourceFact;
use flikk_core::model::{ClipContent, Project, TrackKind};
use flikk_core::services::{CollageRequest, Frame, SamplePoint, Services};
use flikk_core::{Time, TimeRange, paths};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Longest span `view_range` with `detail` accepts.
pub(super) const DETAIL_MAX: f64 = 20.0;
/// Sound script lines `view_range` opens with; `get_audio` has the rest.
const SCRIPT_MAX: usize = 60;
pub(super) const FRAME_WIDTH: u32 = 1280;
/// Longest side of a zoomed view.
const ZOOM_SIDE: u32 = 1280;
/// Smallest zoom region side, in frame pixels.
const ZOOM_MIN: f64 = 16.0;
/// Widest frame rendered for a zoom; past it the view only magnifies.
const ZOOM_RENDER_MAX: u32 = 7680;

/// Whose seconds a view's times are: the timeline's, or a pool item's own
/// (`view_media` sees the item alone on a timeline of its own, where the
/// two are the same).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Clock {
    Timeline,
    Source,
}

impl Clock {
    fn name(self) -> &'static str {
        match self {
            Clock::Timeline => "timeline",
            Clock::Source => "source",
        }
    }

    /// Where the rest of a sound script cut short is.
    fn more(self) -> &'static str {
        match self {
            Clock::Timeline => "get_audio gives them all",
            Clock::Source => "view_media with `audio` gives them all",
        }
    }
}

/// The source frame under timeline time `t`, through the topmost video clip.
fn sample_at(project: &Project, t: Time) -> Option<SamplePoint> {
    project
        .timeline
        .tracks
        .iter()
        .filter(|tr| tr.kind == TrackKind::Video)
        .find_map(|tr| tr.clip_at(t))
        .and_then(|clip| {
            let ClipContent::Media { media, .. } = &clip.content else {
                return None;
            };
            let item = project.media(*media)?;
            Some(SamplePoint {
                timeline: t,
                media: *media,
                path: item.path.clone(),
                fingerprint: item.fingerprint.clone(),
                source: clip.to_source(t)?,
            })
        })
}

/// Whether frames of `range` come from the compositor: when the graph
/// would differ, or to letter captions and titles.
fn wants_program(project: &Project, range: TimeRange, services: &Services) -> bool {
    compose::needs_compositor(project, range)
        || (services.lettering.is_some()
            && project.timeline.tracks.iter().any(|tr| {
                !tr.mix.mute
                    && tr.clips.iter().any(|c| {
                        c.enabled && c.range().overlaps(&range) && compose::is_lettering(tr, c)
                    })
            }))
}

/// What `view_frame` with `masks` shows of the masks at `t`, and the
/// subjects not found yet.
fn masks_note(project: &Project, t: Time, services: &Services) -> String {
    let mut masked = 0;
    let mut waiting = Vec::new();
    for track in &project.timeline.tracks {
        let Some(clip) = track.clip_at(t).filter(|c| c.enabled) else {
            continue;
        };
        if !flikk_core::mask::any_enabled(&clip.masks) {
            continue;
        }
        masked += 1;
        let found = compose::subject_mattes(project, services, clip, t);
        for m in clip
            .masks
            .iter()
            .filter(|m| m.enabled && m.picks().is_some())
        {
            if !found.iter().any(|(id, _)| *id == m.id) {
                waiting.push(format!("{} on {}", short(m.id), short(clip.id)));
            }
        }
        let people = compose::person_mattes(project, services, clip, t);
        for m in clip.masks.iter().filter(|m| m.enabled && m.is_person()) {
            if !people.iter().any(|(id, _)| *id == m.id) {
                waiting.push(format!("person {} on {}", short(m.id), short(clip.id)));
            }
        }
    }
    let mut note = if masked == 0 {
        " No clip here has masks.".to_string()
    } else {
        " Pictures are shown whole: what their masks cut away is tinted red, what they keep is untinted.".to_string()
    };
    if !waiting.is_empty() {
        note.push_str(&format!(
            " Subject and person masks still being found (not drawn; look again shortly): {}.",
            waiting.join(", ")
        ));
    }
    note
}

/// How the program frames were drawn, for the model.
fn program_note(services: &Services) -> &'static str {
    if services.lettering.is_some() {
        "effects, transitions, generators, blend modes, captions and titles drawn"
    } else {
        "effects, transitions, generators and blend modes applied; captions not drawn"
    }
}

/// Colors of numbered boxes, by item.
const MARKS: [[u8; 3]; 6] = [
    [255, 80, 200],
    [60, 220, 255],
    [255, 210, 40],
    [120, 255, 120],
    [255, 140, 60],
    [180, 140, 255],
];

/// `frame` with each of `shapes` outlined and numbered from 1.
fn annotated(frame: &Frame, shapes: &[compose::Shape]) -> Frame {
    let (w, h) = (frame.width, frame.height);
    let mut buf = frame.rgba.to_vec();
    let dot = |buf: &mut [u8], x: f32, y: f32, rgb: [u8; 3]| {
        for dy in 0..2 {
            for dx in 0..2 {
                let (px, py) = (x as i64 + dx, y as i64 + dy);
                if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                    let d = ((py as u32 * w + px as u32) * 4) as usize;
                    buf[d..d + 3].copy_from_slice(&rgb);
                    buf[d + 3] = 255;
                }
            }
        }
    };
    for (i, shape) in shapes.iter().enumerate() {
        let rgb = MARKS[i % MARKS.len()];
        let corners = shape.corners();
        for k in 0..4 {
            let (a, b) = (corners[k], corners[(k + 1) % 4]);
            let len = (b[0] - a[0]).hypot(b[1] - a[1]).ceil().max(1.0) as usize * 2;
            for j in 0..=len {
                let f = j as f32 / len as f32;
                dot(
                    &mut buf,
                    a[0] + (b[0] - a[0]) * f,
                    a[1] + (b[1] - a[1]) * f,
                    rgb,
                );
            }
        }
        // The number on a dark tab above the box's top-left (inside it when
        // there is no room), kept on screen.
        let [x0, y0, ..] = shape.bounds();
        let label = (i + 1).to_string();
        let scale = 3;
        let (tw, th) = (label.len() as u32 * 6 * scale + 4, 7 * scale + 4);
        let x = (x0.max(0.0) as u32).min(w.saturating_sub(tw));
        let above = y0 - th as f32 - 2.0;
        let y = if above >= 0.0 {
            above as u32
        } else {
            y0.max(0.0) as u32 + 2
        };
        let y = y.min(h.saturating_sub(th));
        for py in y..(y + th).min(h) {
            for px in x..(x + tw).min(w) {
                let d = ((py * w + px) * 4) as usize;
                buf[d..d + 4].copy_from_slice(&[16, 16, 20, 255]);
            }
        }
        draw_text(&mut buf, w, &label, x + 2, y + 2, scale, rgb);
    }
    Frame {
        width: w,
        height: h,
        rgba: Arc::from(buf),
    }
}

pub(super) fn image_content(path: &Path) -> Result<Content, ToolError> {
    let bytes = std::fs::read(path).map_err(|e| ToolError(format!("reading the image: {e}")))?;
    Ok(Content::Image {
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        mime_type: "image/png".into(),
    })
}

pub fn view_range(ctx: &mut ToolCtx, args: &Args, detail: bool) -> Result<Step, ToolError> {
    let range = args.range("range")?;
    if detail && range.duration().as_secs_f64() > DETAIL_MAX + 0.001 {
        return Err(ToolError(format!(
            "view_range with `detail` takes at most {DETAIL_MAX} s; leave `detail` out for longer spans"
        )));
    }
    range_step(
        ctx.project(),
        ctx.services,
        &ctx.memory.facts,
        &ctx.settings.vision,
        range,
        detail,
        Clock::Timeline,
        ctx.stop.flag(),
    )
}

/// What `view_range` shows of `range` of `project`: the sound script, and
/// a collage of frames that the step's job decodes, or composes until
/// `stop` is set.
#[allow(clippy::too_many_arguments)]
pub(super) fn range_step(
    project: &Project,
    services: &Services,
    facts: &[SourceFact],
    vision: &VisionSettings,
    range: TimeRange,
    detail: bool,
    clock: Clock,
    stop: Arc<AtomicBool>,
) -> Result<Step, ToolError> {
    let n = vision.frames_per_collage.clamp(1, 30) as usize;
    let tile_width = vision.tile_width.clamp(64, 640);
    let samples = flikk_core::query::sample_points(project, range, n);

    // The sound script is cheap; build it here so the job only decodes.
    let transcript = match script(project, services, range) {
        Ok(lines) if lines.len() > SCRIPT_MAX => format!(
            "{}\n(… {} more lines; {})",
            lines[..SCRIPT_MAX].join("\n"),
            lines.len() - SCRIPT_MAX,
            clock.more()
        ),
        Ok(lines) if !lines.is_empty() => lines.join("\n"),
        Ok(_) => "(nothing heard)".to_owned(),
        Err(ToolError(e)) => format!("({e})"),
    };
    let facts = super::analysis::facts_for(project, facts, range);
    let name = if detail { "detail" } else { "range" };

    let mut header = format!("Audio {range}:\n{transcript}");
    if let Some(note) = unheard_note(project, services, range) {
        header.push_str(&format!("\n{note}"));
    }
    if !facts.is_empty() {
        header.push_str(&format!(
            "\n\nSource facts: {}",
            serde_json::to_string(&facts).unwrap_or_default()
        ));
    }
    if wants_program(project, range, services) {
        let times = program_times(project, range, n);
        let project = Arc::new(project.clone());
        let services = services.clone();
        let job: Job = Box::new(move || {
            program_collage(
                &project, &services, &times, tile_width, name, range, header, transcript, clock,
                &stop,
            )
        });
        return Ok(Step::Job(job));
    }
    let Some(media) = services.media.clone() else {
        let mut done = Done::text(
            format!("{name} {range}: transcript only"),
            format!("{header}\n\nFrames are not available in this build yet."),
        );
        done.excerpts.push(transcript);
        return Ok(done.into());
    };
    if samples.is_empty() {
        let none = match clock {
            Clock::Timeline => "No video on the timeline in this range.",
            Clock::Source => "No picture in this range.",
        };
        let mut done = Done::text(
            format!("{name} {range}: no video"),
            format!("{header}\n\n{none}"),
        );
        done.excerpts.push(transcript);
        return Ok(done.into());
    }

    let job: Job = Box::new(move || {
        let request = CollageRequest {
            samples,
            tile_width,
        };
        let collage = match media.collage(&request) {
            Ok(c) => c,
            Err(e) => {
                return JobDone::Done(Done::error(format!("Could not build the collage: {e}")));
            }
        };
        let image = match image_content(&collage.path) {
            Ok(i) => i,
            Err(ToolError(e)) => return JobDone::Done(Done::error(e)),
        };
        let tiles: Vec<_> = collage
            .tiles
            .iter()
            .enumerate()
            .map(|(i, t)| format!("{}: {}", i + 1, t.timeline))
            .collect();
        let text = format!(
            "{header}\n\nCollage: {} frames, {} columns × {} rows, left to right then top to bottom. Tile {} times: {}",
            collage.tiles.len(),
            collage.columns,
            collage.rows,
            clock.name(),
            tiles.join(", ")
        );
        JobDone::Done(Done {
            result: ToolResult {
                content: vec![Content::Text { text }, image],
                is_error: false,
            },
            summary: format!("{name} {range}: {} frames", collage.tiles.len()),
            images: vec![collage.path.clone()],
            excerpts: vec![transcript],
            ..Default::default()
        })
    });
    Ok(Step::Job(job))
}

/// Up to `n` frame times spread evenly over `range`, each at the centre of
/// its slice and snapped to a sequence frame (as `query::sample_points`).
fn program_times(project: &Project, range: TimeRange, n: usize) -> Vec<Time> {
    let n = n.min(flikk_core::query::MAX_SAMPLES);
    if n == 0 || range.is_empty() {
        return Vec::new();
    }
    let rate = project.timeline.settings.rate;
    let len = range.duration().0 as i128;
    let mut out: Vec<Time> = Vec::with_capacity(n);
    for i in 0..n as i128 {
        let t = range.start + Time((len * (2 * i + 1) / (2 * n as i128)) as i64);
        let t = t.floor_to_frame(rate).max(range.start);
        if out.last() != Some(&t) {
            out.push(t);
        }
    }
    out
}

/// The composited program at `times`, as one stamped collage; stopped
/// once `stop` is set.
#[allow(clippy::too_many_arguments)]
fn program_collage(
    project: &Project,
    services: &Services,
    times: &[Time],
    tile_width: u32,
    name: &str,
    range: TimeRange,
    header: String,
    transcript: String,
    clock: Clock,
    stop: &AtomicBool,
) -> JobDone {
    let size = compose::size_for_width(project, tile_width);
    let mut stills = Stills::new(services.media.clone(), tile_width);
    let mut tiles: Vec<(Frame, String)> = Vec::with_capacity(times.len());
    for t in times {
        let Some(frame) = on_gpu(services, stop, || {
            compose::program(project, *t, size, services, &mut stills)
        }) else {
            return JobDone::Stopped;
        };
        tiles.push((frame, format!("{:.2}s", t.as_secs_f64())));
    }
    let path = match save_png(&collage(&tiles, size)) {
        Ok(p) => p,
        Err(ToolError(e)) => return JobDone::Done(Done::error(e)),
    };
    let image = match image_content(&path) {
        Ok(i) => i,
        Err(ToolError(e)) => return JobDone::Done(Done::error(e)),
    };
    let stamps: Vec<_> = times
        .iter()
        .enumerate()
        .map(|(i, t)| format!("{}: {t}", i + 1))
        .collect();
    let cols = columns(tiles.len());
    let text = format!(
        "{header}\n\nComposited program ({}): {} frames, {cols} columns × {} rows, left to right then top to bottom. Tile {} times: {}",
        program_note(services),
        tiles.len(),
        tiles.len().div_ceil(cols),
        clock.name(),
        stamps.join(", ")
    );
    JobDone::Done(Done {
        result: ToolResult {
            content: vec![Content::Text { text }, image],
            is_error: false,
        },
        summary: format!("{name} {range}: {} program frames", tiles.len()),
        images: vec![path],
        excerpts: vec![transcript],
        ..Default::default()
    })
}

pub fn view_frame(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let t = args.time("t")?;
    let annotate = args.opt_bool("annotate")?.unwrap_or(false);
    let masks = args.opt_bool("masks")?.unwrap_or(false);
    let zoom = zoom_region(project, args)?;
    let rate = project.timeline.settings.rate;
    let t = t.max(Time::ZERO).floor_to_frame(rate);
    let span = TimeRange::new(t, t + rate.frame_duration());
    if annotate || masks || zoom.is_some() || wants_program(project, span, ctx.services) {
        let project = Arc::new(project.clone());
        let services = ctx.services.clone();
        let stop = ctx.stop.flag();
        let job: Job = Box::new(move || {
            let frame_w = project.timeline.settings.width.max(2);
            let width = zoom.map_or(FRAME_WIDTH, |r| zoom_width(frame_w, r));
            let size = compose::size_for_width(&project, width);
            let mut stills = Stills::new(services.media.clone(), width);
            let view = if masks {
                compose::MaskView::Tinted(None)
            } else {
                compose::MaskView::Cut
            };
            let composed = on_gpu(&services, &stop, || {
                let frame = compose::program_with(&project, t, size, &services, &mut stills, view);
                let shapes =
                    annotate.then(|| compose::layout(&project, t, size, &services, &mut stills));
                (frame, shapes)
            });
            let Some((mut frame, shapes)) = composed else {
                return JobDone::Stopped;
            };
            let mut text = format!("Program frame at {t} ({}).", program_note(&services));
            if masks {
                text.push_str(&masks_note(&project, t, &services));
            }
            if let Some(shapes) = shapes {
                frame = annotated(&frame, &shapes);
                let legend = super::layout::legend(&project, &shapes);
                if legend.is_empty() {
                    text.push_str("\nNothing is drawn.");
                } else {
                    text.push_str("\nNumbered boxes, topmost first:");
                    for (i, line) in legend.iter().enumerate() {
                        text.push_str(&format!("\n{}: {line}", i + 1));
                    }
                }
            }
            if let Some(region) = zoom {
                let (zoomed, note) = zoomed(&frame, region, frame_w, Clock::Timeline);
                frame = zoomed;
                text.push('\n');
                text.push_str(&note);
            }
            match save_png(&frame).and_then(|path| Ok((image_content(&path)?, path))) {
                Ok((image, path)) => JobDone::Done(Done {
                    result: ToolResult {
                        content: vec![Content::Text { text }, image],
                        is_error: false,
                    },
                    summary: if zoom.is_some() {
                        format!("zoomed frame at {t}")
                    } else {
                        format!("frame at {t}")
                    },
                    images: vec![path],
                    ..Default::default()
                }),
                Err(ToolError(e)) => {
                    JobDone::Done(Done::error(format!("Could not save the frame: {e}")))
                }
            }
        });
        return Ok(Step::Job(job));
    }
    let point = sample_at(project, t)
        .ok_or_else(|| ToolError(format!("No video on the timeline at {t}.")))?;
    let item = project
        .media(point.media)
        .cloned()
        .ok_or_else(|| ToolError("media not found".into()))?;
    let Some(media) = ctx.services.media.clone() else {
        return Err(ToolError(
            "Frames are not available in this build yet.".into(),
        ));
    };
    let job: Job = Box::new(move || {
        let result = media
            .frame(&item, point.source, FRAME_WIDTH)
            .map_err(ToolError::from)
            .and_then(|frame| save_png(&frame));
        match result.and_then(|path| Ok((image_content(&path)?, path))) {
            Ok((image, path)) => JobDone::Done(Done {
                result: ToolResult {
                    content: vec![
                        Content::Text {
                            text: format!("Frame at {t} (source {}).", point.source),
                        },
                        image,
                    ],
                    is_error: false,
                },
                summary: format!("frame at {t}"),
                images: vec![path],
                ..Default::default()
            }),
            Err(ToolError(e)) => {
                JobDone::Done(Done::error(format!("Could not decode the frame: {e}")))
            }
        }
    });
    Ok(Step::Job(job))
}

/// How wide to render a frame `frame_w` pixels wide for a zoom on
/// `region`: wide enough that the region fills the view at its own
/// resolution (the sources' detail, not an upscaled preview).
pub(super) fn zoom_width(frame_w: u32, region: [f64; 4]) -> u32 {
    ((frame_w as f64 * ZOOM_SIDE as f64 / region[2].max(region[3])).ceil() as u32)
        .clamp(FRAME_WIDTH, ZOOM_RENDER_MAX.max(frame_w))
}

/// `zoom` as [left, top, width, height] in frame pixels, kept inside the
/// frame.
pub(super) fn zoom_region(project: &Project, args: &Args) -> Result<Option<[f64; 4]>, ToolError> {
    let Some(value) = args.value("zoom") else {
        return Ok(None);
    };
    let n: Vec<f64> = value
        .as_array()
        .map(|a| a.iter().filter_map(serde_json::Value::as_f64).collect())
        .unwrap_or_default();
    let [x, y, w, h] = n[..] else {
        return Err(ToolError(
            "`zoom` is [left, top, width, height] in frame pixels".into(),
        ));
    };
    let s = &project.timeline.settings;
    let (fw, fh) = (s.width.max(2) as f64, s.height.max(2) as f64);
    let (w, h) = (w.clamp(ZOOM_MIN, fw), h.clamp(ZOOM_MIN, fh));
    let (x, y) = (x.clamp(0.0, fw - w), y.clamp(0.0, fh - h));
    Ok(Some([x, y, w, h]))
}

/// The `region` (frame pixels) of `frame`, rendered for a frame `frame_w`
/// pixels wide, scaled up to fit [`ZOOM_SIDE`] with a grid labelled in
/// frame pixels, and the line that explains it.
pub(super) fn zoomed(
    frame: &Frame,
    region: [f64; 4],
    frame_w: u32,
    clock: Clock,
) -> (Frame, String) {
    let k = frame.width as f64 / frame_w as f64;
    let [rx, ry, rw, rh] = region;
    let fit = ZOOM_SIDE as f64 / rw.max(rh);
    let (ow, oh) = (
        ((rw * fit).round() as u32).max(1),
        ((rh * fit).round() as u32).max(1),
    );
    let mut buf = vec![0u8; (ow * oh * 4) as usize];
    // Bilinear, sampling the render at the frame pixel each output pixel
    // stands for.
    let px = |x: i64, y: i64| -> [f32; 4] {
        let x = x.clamp(0, frame.width as i64 - 1) as u32;
        let y = y.clamp(0, frame.height as i64 - 1) as u32;
        let d = ((y * frame.width + x) * 4) as usize;
        let p = &frame.rgba[d..d + 4];
        [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
    };
    for oy in 0..oh {
        for ox in 0..ow {
            let fx = (rx + (ox as f64 + 0.5) / fit) * k - 0.5;
            let fy = (ry + (oy as f64 + 0.5) / fit) * k - 0.5;
            let (x0, y0) = (fx.floor(), fy.floor());
            let (tx, ty) = ((fx - x0) as f32, (fy - y0) as f32);
            let (x0, y0) = (x0 as i64, y0 as i64);
            let (a, b, c, d) = (
                px(x0, y0),
                px(x0 + 1, y0),
                px(x0, y0 + 1),
                px(x0 + 1, y0 + 1),
            );
            let o = ((oy * ow + ox) * 4) as usize;
            for i in 0..4 {
                let top = a[i] + (b[i] - a[i]) * tx;
                let bottom = c[i] + (d[i] - c[i]) * tx;
                buf[o + i] = (top + (bottom - top) * ty).round() as u8;
            }
        }
    }
    // Grid lines every `step` frame pixels, about eight across.
    let step = [
        1.0, 2.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 250.0, 500.0,
    ]
    .into_iter()
    .find(|s| rw.max(rh) / s <= 10.0)
    .unwrap_or(1000.0);
    let line = [255u8, 220, 40];
    let blend = |buf: &mut [u8], x: u32, y: u32| {
        let o = ((y * ow + x) * 4) as usize;
        for i in 0..3 {
            buf[o + i] = ((buf[o + i] as u16 + line[i] as u16 * 2) / 3) as u8;
        }
        buf[o + 3] = 255;
    };
    let label = |buf: &mut [u8], text: &str, x: u32, y: u32| {
        let (tw, th) = (text.len() as u32 * 12 + 4, 18);
        let x = x.min(ow.saturating_sub(tw));
        let y = y.min(oh.saturating_sub(th));
        for py in y..(y + th).min(oh) {
            for qx in x..(x + tw).min(ow) {
                let o = ((py * ow + qx) * 4) as usize;
                buf[o..o + 4].copy_from_slice(&[16, 16, 20, 255]);
            }
        }
        draw_text(buf, ow, text, x + 2, y + 2, 2, line);
    };
    let first = |from: f64| (from / step).ceil() * step;
    let mut v = first(rx);
    while v <= rx + rw {
        let x = ((v - rx) * fit).round() as u32;
        if x < ow {
            for y in 0..oh {
                blend(&mut buf, x, y);
            }
        }
        v += step;
    }
    let mut v = first(ry);
    while v <= ry + rh {
        let y = ((v - ry) * fit).round() as u32;
        if y < oh {
            for x in 0..ow {
                blend(&mut buf, x, y);
            }
        }
        v += step;
    }
    // Labels last, so lines never cross them: x along the top, y down the
    // left.
    let mut v = first(rx);
    while v <= rx + rw {
        let x = ((v - rx) * fit).round() as u32;
        label(&mut buf, &format!("{v}"), x + 2, 0);
        v += step * 2.0;
    }
    let mut v = first(ry) + step;
    while v <= ry + rh {
        let y = ((v - ry) * fit).round() as u32;
        label(&mut buf, &format!("{v}"), 0, y + 2);
        v += step * 2.0;
    }
    let note = match clock {
        Clock::Timeline => format!(
            "Zoomed ×{fit:.1} on frame pixels x {rx:.0}–{:.0}, y {ry:.0}–{:.0}. Grid lines every {step} px, labelled in frame pixels: use those numbers with `space: frame` in set_mask, or in place's `pin_to`.",
            rx + rw,
            ry + rh
        ),
        Clock::Source => format!(
            "Zoomed ×{fit:.1} on the item's own pixels x {rx:.0}–{:.0}, y {ry:.0}–{:.0}. Grid lines every {step} px, labelled in its pixels (not the sequence frame's).",
            rx + rw,
            ry + rh
        ),
    };
    (
        Frame {
            width: ow,
            height: oh,
            rgba: Arc::from(buf),
        },
        note,
    )
}

/// PNG under `paths::cache()/agent/frames`, kept for the activity log.
pub(super) fn save_png(frame: &Frame) -> Result<PathBuf, ToolError> {
    let path = paths::cache()
        .join("agent")
        .join("frames")
        .join(format!("{}.png", uuid::Uuid::new_v4().simple()));
    write_png(&path, frame)?;
    Ok(path)
}

/// `frame` as an 8-bit RGBA PNG at `path`, creating its folder; written
/// whole or not at all. An encoder error comes back as an `io::Error` with
/// the encoder's own message.
pub(super) fn write_png(path: &Path, frame: &Frame) -> std::io::Result<()> {
    flikk_os::fs::write_atomic_with(path, |file| {
        let mut encoder =
            png::Encoder::new(std::io::BufWriter::new(file), frame.width, frame.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&frame.rgba)?;
        writer.finish()?;
        Ok(())
    })
}

/// (clip id, track name, source path, source range) of the audio to measure.
pub fn audio_segments(
    project: &Project,
    range: TimeRange,
) -> Vec<(flikk_core::ClipId, String, PathBuf, TimeRange, f32)> {
    let collect = |kind: TrackKind| {
        let mut out = Vec::new();
        for track in project.timeline.tracks.iter().filter(|t| t.kind == kind) {
            for clip in &track.clips {
                let Some(span) = clip.range().intersect(&range) else {
                    continue;
                };
                let Some(item) = clip.media().and_then(|m| project.media(m)) else {
                    continue;
                };
                let (Some(a), Some(b)) = (clip.to_source(span.start), clip.to_source(span.end))
                else {
                    continue;
                };
                out.push((
                    clip.id,
                    track.name.clone(),
                    item.path.clone(),
                    TimeRange::new(a, b),
                    clip.audio.gain_db,
                ));
            }
        }
        out
    };
    let audio = collect(TrackKind::Audio);
    if audio.is_empty() {
        collect(TrackKind::Video)
    } else {
        audio
    }
}

pub fn measure_loudness(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let range = args.range_or_all("range", project)?;
    let Some(analysis) = ctx.services.analysis.clone() else {
        return Err(ToolError(
            "Loudness measurement is not available in this build yet.".into(),
        ));
    };
    let segments = audio_segments(project, range);
    if segments.is_empty() {
        return Err(ToolError(format!("No audio in {range}.")));
    }
    let job: Job = Box::new(move || {
        let mut rows = Vec::new();
        for (clip, track, path, source, gain) in segments.iter().take(24) {
            let row = match analysis.loudness(path, Some(*source)) {
                Ok(l) => json!({
                    "clip": clip.to_string(), "track": track,
                    "integrated_lufs": l.integrated_lufs, "true_peak_db": l.true_peak_db,
                    "range_lu": l.range_lu, "clip_gain_db": gain,
                }),
                Err(e) => {
                    json!({ "clip": clip.to_string(), "track": track, "error": e.to_string() })
                }
            };
            rows.push(row);
        }
        let mut out = json!({ "range": range_json(range), "clips": rows, "note": "measured on the source, before clip gain" });
        if segments.len() > 24 {
            out["truncated"] = json!(format!("first 24 of {} clips", segments.len()));
        }
        JobDone::Done(Done::json(
            format!("loudness {range}: {} clips", rows_len(&out)),
            &out,
        ))
    });
    Ok(Step::Job(job))
}

fn rows_len(v: &serde_json::Value) -> usize {
    v["clips"].as_array().map_or(0, |a| a.len())
}
