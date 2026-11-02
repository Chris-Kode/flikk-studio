//! `sync_audio`: line clips (or pool items) up by their sound, off-thread,
//! and propose the move with what was measured.

use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolError, clip_id, clip_ids, media_id, short};
use koo_core::sync::{self, Measured};

pub fn serve(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let Some(media) = ctx.services.media.clone() else {
        return Err(ToolError("Syncing is not available in this build.".into()));
    };
    let reason = args
        .opt_str("reason")?
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or("sync by sound")
        .to_owned();
    let mute = args.opt_bool("replace_camera_audio")?.unwrap_or(false);
    let project = ctx.project().clone();
    let pool: Vec<_> = args
        .strings("media")?
        .iter()
        .map(|m| media_id(&project, m))
        .collect::<Result<_, _>>()?;
    if !pool.is_empty() {
        let at = args.opt_time("at")?.unwrap_or(ctx.editor.playhead);
        let mut order = sync::pool_order(&project, &pool);
        if let Some(r) = args.opt_str("reference")? {
            let r = media_id(&project, r)?;
            if let Some(i) = order.iter().position(|m| *m == r) {
                order.swap(0, i);
            }
        }
        let sources: Option<Vec<_>> = order
            .iter()
            .map(|m| sync::source(&project, *m, 0))
            .collect();
        let sources = sources
            .filter(|s| s.len() >= 2)
            .ok_or_else(|| ToolError("Give at least two pool items with sound.".into()))?;
        let names: Vec<String> = order
            .iter()
            .map(|m| format!("media {}", short(m)))
            .collect();
        let job: Job = Box::new(move || {
            match sync::measure_group(media.as_ref(), &sources[0], &sources[1..]) {
                Err(e) => JobDone::Done(Done::error(format!("Could not sync: {e}"))),
                Ok(results) => {
                    let excerpts = report(&names, &results);
                    let synced = sync::place_command(&project, &order, at, results, mute);
                    finish(synced.command, reason, excerpts)
                }
            }
        });
        return Ok(Step::Job(job));
    }
    let clips = clip_ids(&project, &args.strings("clips")?)?;
    let mut recordings = sync::plan::recordings(&project, &clips);
    if let Some(r) = args.opt_str("reference")? {
        let r = clip_id(&project, r)?;
        if let Some(i) = recordings.iter().position(|x| x.clips.contains(&r)) {
            recordings.swap(0, i);
        }
    }
    if recordings.len() < 2 {
        return Err(ToolError(
            "Give at least two clips with sound from different recordings.".into(),
        ));
    }
    let sources: Option<Vec<_>> = recordings
        .iter()
        .map(|r| sync::source(&project, r.media, r.stream))
        .collect();
    let sources =
        sources.ok_or_else(|| ToolError("A clip's media has not been probed yet.".into()))?;
    let names: Vec<String> = recordings
        .iter()
        .map(|r| format!("clip {}", short(r.clip)))
        .collect();
    let job: Job =
        Box::new(
            move || match sync::measure_group(media.as_ref(), &sources[0], &sources[1..]) {
                Err(e) => JobDone::Done(Done::error(format!("Could not sync: {e}"))),
                Ok(results) => {
                    let excerpts = report(&names, &results);
                    let synced = sync::clips_command(&project, &recordings, results, mute);
                    finish(synced.command, reason, excerpts)
                }
            },
        );
    Ok(Step::Job(job))
}

fn finish(
    command: Option<koo_core::command::Command>,
    reason: String,
    excerpts: Vec<String>,
) -> JobDone {
    match command {
        Some(c) => JobDone::Propose {
            commands: vec![c],
            reason,
            excerpts,
        },
        None => JobDone::Done(Done::error(format!(
            "Nothing matched, so nothing was proposed.\n{}",
            excerpts.join("\n")
        ))),
    }
}

/// One line per recording: the reference, then each follower's offset,
/// drift and confidence, or why it was not synced.
fn report(names: &[String], results: &[Result<Measured, String>]) -> Vec<String> {
    let mut out = vec![format!("{}: reference", names[0])];
    for (name, r) in names[1..].iter().zip(results) {
        out.push(match r {
            Ok(m) => {
                let mut line = format!(
                    "{name}: its source runs {:+.4} s from the reference's, confidence {:.2}",
                    m.offset, m.confidence
                );
                if m.drift != 0.0 {
                    line.push_str(&format!(
                        ", clock drift {:+.4}% (corrected by its speed)",
                        m.drift * 100.0
                    ));
                }
                line
            }
            Err(e) => format!("{name}: not synced ({e})"),
        });
    }
    out
}
