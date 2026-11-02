//! `fit_music`: make a song end exactly when the video does, on its real
//! ending, by skipping or repeating bars at seamless points
//! (`koo_core::music_fit`). Decoding and planning run as a job; the result
//! is one `fit_music` change the user reviews like any other.

use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolError, clip_id, secs, short};
use koo_core::heard;
use koo_core::model::{Fingerprint, TrackKind};
use koo_core::music_fit::{FitOptions, JumpKind, Request, Target, describe};
use koo_core::{ClipId, Time, TimeRange};
use serde_json::json;

pub fn fit_music(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let reason = args
        .opt_str("reason")?
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or("Fit the music to length")
        .to_owned();
    let clip = match args.opt_str("clip")? {
        Some(text) => clip_id(project, text)?,
        None => only_music_clip(ctx)?,
    };
    let given = [args.has("end"), args.has("duration"), args.has("range")];
    if given.iter().filter(|g| **g).count() > 1 {
        return Err(ToolError(
            "give one of `end`, `duration` or `range` (or none for the sequence end)".into(),
        ));
    }
    let target = if let Some(t) = args.opt_time("end")? {
        Target::End(t)
    } else if let Some(d) = args.opt_duration("duration")? {
        Target::Duration(d)
    } else if let Some(r) = args.opt_range("range")? {
        Target::Range(r)
    } else {
        Target::SequenceEnd
    };
    let mut options = FitOptions::default();
    if let Some(ms) = args.opt_f64("crossfade_ms")? {
        options.crossfade = Time::from_secs_f64(ms.clamp(0.0, 500.0) / 1000.0);
    }
    if let Some(loops) = args.opt_bool("loops")? {
        options.loops = loops;
    }
    let request = Request::new(project, clip, target, options).map_err(ToolError)?;
    let services = ctx.services.clone();
    let job: Job = Box::new(move || match request.fit(&services) {
        Err(e) => JobDone::Done(Done::error(e)),
        Ok((command, fit)) => {
            let at = request.at;
            let mut lines = vec![format!(
                "Fitted {}: {:.2} s from {:.2} to {:.2} s in {} segment{}, quality {:.0}%.",
                short(request.clip.id),
                fit.length.as_secs_f64(),
                at.as_secs_f64(),
                (at + fit.length).as_secs_f64(),
                fit.segments.len(),
                if fit.segments.len() == 1 { "" } else { "s" },
                fit.quality * 100.0
            )];
            lines.extend(describe(&fit));
            let skips = fit
                .jumps
                .iter()
                .filter(|j| j.kind == JumpKind::Skip)
                .count();
            let loops = fit.jumps.len() - skips;
            let detail = json!({
                "jumps": fit.jumps.iter().map(|j| json!({
                    "at": secs(at + j.at),
                    "kind": match j.kind { JumpKind::Skip => "skip", JumpKind::Loop => "loop" },
                    "source": { "start": secs(j.span.start), "end": secs(j.span.end) },
                    "quality": (j.quality * 100.0).round() / 100.0,
                })).collect::<Vec<_>>(),
                "skips": skips,
                "loops": loops,
                "end": secs(at + fit.length),
                "fade_out": secs(fit.fade_out),
            });
            lines.push(detail.to_string());
            JobDone::Propose {
                commands: vec![command],
                reason,
                excerpts: lines,
            }
        }
    });
    Ok(Step::Job(job))
}

/// The one clip on an audio track whose source analysis heard music.
fn only_music_clip(ctx: &ToolCtx) -> Result<ClipId, ToolError> {
    let project = ctx.project();
    let Some(service) = ctx.services.analysis.as_ref() else {
        return Err(ToolError("Name the music clip with `clip`.".into()));
    };
    let get = |fp: &Fingerprint| service.analysis(fp);
    let whole = TimeRange::new(Time::ZERO, project.timeline.duration().max(Time(1)));
    let mut clips: Vec<ClipId> = heard::music_in_range(project, whole, get)
        .into_iter()
        .filter(|m| {
            project
                .timeline
                .track(m.track)
                .is_some_and(|t| t.kind == TrackKind::Audio)
        })
        .map(|m| m.clip)
        .collect();
    clips.dedup();
    match clips.as_slice() {
        [one] => Ok(*one),
        [] => Err(ToolError(
            "No analysed music on an audio track; name the clip with `clip`.".into(),
        )),
        _ => Err(ToolError(format!(
            "Several music clips ({}); name one with `clip`.",
            clips.iter().map(short).collect::<Vec<_>>().join(", ")
        ))),
    }
}
