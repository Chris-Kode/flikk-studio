//! View tools: the model sees video as collages of at most 30 timestamped
//! frames plus the transcript (PRD V1–V7). Decoding runs off-thread on a
//! snapshot of the project.

use super::analysis::{timeline_words, transcript_lines};
use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolError, range_json};
use crate::bridge::{Content, ToolResult};
use base64::Engine;
use koo_core::model::{ClipContent, Project, TrackKind};
use koo_core::services::{CollageRequest, Frame, SamplePoint};
use koo_core::{Time, TimeRange, paths};
use serde_json::json;
use std::path::{Path, PathBuf};

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

fn image_content(path: &Path) -> Result<Content, ToolError> {
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

pub fn view_frame(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let t = args.time("t")?;
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
fn save_png(frame: &Frame) -> Result<PathBuf, ToolError> {
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
