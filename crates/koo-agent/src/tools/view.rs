//! View tools: the model sees video as collages of at most 30 timestamped
//! frames plus the transcript (PRD V1–V7). Decoding runs off-thread on a
//! snapshot of the project. Where the range uses effects, transitions,
//! generators, blend modes, crop, flip or speed, frames are the composited
//! program (`koo_core::compose`, as the viewer and the render show it);
//! otherwise the cached source collage.

use super::analysis::{timeline_words, transcript_lines};
use super::preview::{collage, columns};
use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolError, range_json};
use crate::bridge::{Content, ToolResult};
use base64::Engine;
use koo_core::compose::{self, Stills};
use koo_core::model::{ClipContent, Project, TrackKind};
use koo_core::services::{CollageRequest, Frame, SamplePoint, Services};
use koo_core::{Time, TimeRange, paths};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Longest span `view_detail` accepts.
const DETAIL_MAX: f64 = 20.0;
const FRAME_WIDTH: u32 = 1280;

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

pub(super) fn image_content(path: &Path) -> Result<Content, ToolError> {
    let bytes = std::fs::read(path).map_err(|e| ToolError(format!("reading the image: {e}")))?;
    Ok(Content::Image {
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        mime_type: "image/png".into(),
    })
}

pub fn view_range(ctx: &mut ToolCtx, args: &Args, detail: bool) -> Result<Step, ToolError> {
    let project = ctx.project();
    let range = args.range("range")?;
    if detail && range.duration().as_secs_f64() > DETAIL_MAX + 0.001 {
        return Err(ToolError(format!(
            "view_detail takes at most {DETAIL_MAX} s; use view_range for longer spans"
        )));
    }
    let n = ctx.settings.vision.frames_per_collage.clamp(1, 30) as usize;
    let tile_width = ctx.settings.vision.tile_width.clamp(64, 640);
    let samples = koo_core::query::sample_points(project, range, n);

    // Transcript is cheap; build it here so the job only decodes.
    let transcript = match timeline_words(project, ctx.services, range) {
        Ok((words, speakers)) if !words.is_empty() => {
            transcript_lines(&words, &speakers, false).join("\n")
        }
        Ok(_) => "(no speech)".to_owned(),
        Err(ToolError(e)) => format!("({e})"),
    };
    let facts = super::analysis::facts_for(project, &ctx.memory.facts, range);
    let name = if detail { "detail" } else { "range" };

    let mut header = format!("Transcript {range}:\n{transcript}");
    if !facts.is_empty() {
        header.push_str(&format!(
            "\n\nSource facts: {}",
            serde_json::to_string(&facts).unwrap_or_default()
        ));
    }
    if compose::needs_compositor(project, range) {
        let times = program_times(project, range, n);
        let project = Arc::new(project.clone());
        let services = ctx.services.clone();
        let job: Job = Box::new(move || {
            program_collage(
                &project, &services, &times, tile_width, name, range, header, transcript,
            )
        });
        return Ok(Step::Job(job));
    }
    let Some(media) = ctx.services.media.clone() else {
        let mut done = Done::text(
            format!("{name} {range}: transcript only"),
            format!("{header}\n\nFrames are not available in this build yet."),
        );
        done.excerpts.push(transcript);
        return Ok(done.into());
    };
    if samples.is_empty() {
        let mut done = Done::text(
            format!("{name} {range}: no video"),
            format!("{header}\n\nNo video on the timeline in this range."),
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
            "{header}\n\nCollage: {} frames, {} columns × {} rows, left to right then top to bottom. Tile timeline times: {}",
            collage.tiles.len(),
            collage.columns,
            collage.rows,
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
    let n = n.min(koo_core::query::MAX_SAMPLES);
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

/// The composited program at `times`, as one stamped collage.
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
) -> JobDone {
    let size = compose::size_for_width(project, tile_width);
    let mut stills = Stills::new(services.media.clone(), tile_width);
    let tiles: Vec<(Frame, String)> = times
        .iter()
        .map(|t| {
            let frame = compose::program(project, *t, size, services, &mut stills);
            (frame, format!("{:.2}s", t.as_secs_f64()))
        })
        .collect();
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
        "{header}\n\nComposited program (effects, transitions, generators and blend modes applied; captions not drawn): {} frames, {cols} columns × {} rows, left to right then top to bottom. Tile timeline times: {}",
        tiles.len(),
        tiles.len().div_ceil(cols),
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
    let rate = project.timeline.settings.rate;
    let t = t.max(Time::ZERO).floor_to_frame(rate);
    if compose::needs_compositor(project, TimeRange::new(t, t + rate.frame_duration())) {
        let project = Arc::new(project.clone());
        let services = ctx.services.clone();
        let job: Job = Box::new(move || {
            let size = compose::size_for_width(&project, FRAME_WIDTH);
            let mut stills = Stills::new(services.media.clone(), FRAME_WIDTH);
            let frame = compose::program(&project, t, size, &services, &mut stills);
            match save_png(&frame).and_then(|path| Ok((image_content(&path)?, path))) {
                Ok((image, path)) => JobDone::Done(Done {
                    result: ToolResult {
                        content: vec![
                            Content::Text {
                                text: format!(
                                    "Program frame at {t}, composited (captions not drawn)."
                                ),
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

/// PNG under `paths::cache()/agent/frames`, kept for the activity log.
pub(super) fn save_png(frame: &Frame) -> Result<PathBuf, ToolError> {
    let dir = paths::cache().join("agent").join("frames");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.png", uuid::Uuid::new_v4().simple()));
    let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
    let mut encoder = png::Encoder::new(file, frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&frame.rgba)?;
    writer.finish()?;
    Ok(path)
}

/// (clip id, track name, source path, source range) of the audio to measure.
pub fn audio_segments(
    project: &Project,
    range: TimeRange,
) -> Vec<(koo_core::ClipId, String, PathBuf, TimeRange, f32)> {
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
