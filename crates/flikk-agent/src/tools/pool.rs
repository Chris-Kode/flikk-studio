//! Seeing and hearing a media pool item without placing it (`view_media`).
//!
//! The item is put alone on a timeline of its own ([`alone`]), from its
//! first frame and at its own size and frame rate, so timeline time there
//! is its source time and the timeline's own views (collages, frames,
//! zooms, the sound script, spectrograms) see it as they see the program.
//! An item whose analysis is missing is analysed first, as the pool's
//! Analyze does, and the call waits for it.

use super::audio::{picture_step, pictured, script_done};
use super::read::media_name;
use super::view::{
    self, Clock, DETAIL_MAX, FRAME_WIDTH, image_content, save_png, zoom_region, zoom_width, zoomed,
};
use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolError, fail, media_id, short};
use crate::bridge::{Content, ToolResult};
use flikk_core::model::{
    Clip, ClipContent, MediaInfo, MediaItem, MediaKind, Project, Timeline, Track, TrackKind,
};
use flikk_core::services::{AnalysisService, Coverage, JobState, Services};
use flikk_core::time::FLICKS_PER_SECOND;
use flikk_core::{Time, TimeRange};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How often a call waiting for analysis looks at its job.
const POLL: Duration = Duration::from_millis(250);
/// What a picture lasts on its timeline: it has no length of its own.
const STILL: Time = Time(FLICKS_PER_SECOND);

/// What a call looks at.
enum Look {
    /// One picture: the item if it is one, else the video's frame at `at`;
    /// whole, or `zoom` (its own pixels) magnified.
    Picture {
        at: Option<Time>,
        zoom: Option<[f64; 4]>,
    },
    /// A stretch of a video or of audio, after analysis.
    Span { range: TimeRange, span: Span },
}

#[derive(Clone, Copy)]
enum Span {
    /// A collage of frames under the sound script, as `view_range`.
    Frames { detail: bool },
    /// The sound alone: the whole script as `get_audio`, and the
    /// spectrogram of `view_audio` when the range is short enough.
    Sound,
}

/// How waiting for an item's analysis ended.
enum Analysed {
    Done,
    /// The job failed; its results hold what was made before it did.
    Failed(String),
    /// The job left the queue unfinished: the item's file left the
    /// project.
    Cancelled,
    /// The turn was stopped meanwhile.
    Stopped,
}

pub fn view_media(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let id = media_id(project, args.str("media")?)?;
    let item = project
        .media(id)
        .cloned()
        .ok_or_else(|| ToolError("media not found".into()))?;
    let name = media_name(project, id);
    let Some(info) = item.info.clone() else {
        return fail(format!(
            "{name} could not be read, so there is nothing to see."
        ));
    };
    let alone = alone(project, &item);
    // Its times are its own seconds; timecode and frames at its own rate.
    let args = Args(args.0, alone.timeline.settings.rate);
    let look = look(&item, &info, &alone, &args)?;
    let about = about(&item, &info, &name);
    match look {
        Look::Picture { at, zoom } => {
            let Some(media) = ctx.services.media.clone() else {
                return fail("Frames are not available in this build yet.");
            };
            let frame_w = alone.timeline.settings.width;
            let job: Job = Box::new(move || {
                JobDone::Done(picture(&*media, &item, at, zoom, frame_w, about, &name))
            });
            Ok(Step::Job(job))
        }
        Look::Span { range, span } => {
            let analysis = to_analyse(ctx.services, &item);
            let unanalysable = item.kind != MediaKind::Image && item.fingerprint.is_none();
            let services = ctx.services.clone();
            let facts = ctx.memory.facts.clone();
            let vision = ctx.settings.vision.clone();
            let stop = ctx.stop.flag();
            let job: Job = Box::new(move || {
                let mut intro = about;
                match analysis.map(|a| analyse(&*a, &item, &stop)) {
                    None if unanalysable => {
                        intro.push_str(" Its file could not be read for analysis.")
                    }
                    None | Some(Analysed::Done) => {}
                    Some(Analysed::Failed(e)) => {
                        intro.push_str(&format!(" Analysing it failed: {e}."))
                    }
                    Some(Analysed::Cancelled) => {
                        return JobDone::Done(Done::error(format!(
                            "{name} left the pool while it was being analysed."
                        )));
                    }
                    Some(Analysed::Stopped) => return JobDone::Done(Done::error("Stopped.")),
                }
                let step = match span {
                    Span::Frames { detail } => view::range_step(
                        &alone,
                        &services,
                        &facts,
                        &vision,
                        range,
                        detail,
                        Clock::Source,
                    ),
                    Span::Sound if pictured(range) && services.media.is_some() => {
                        picture_step(&alone, &services, range)
                    }
                    Span::Sound if services.media.is_none() => {
                        script_done(&alone, &services, range).map(Into::into)
                    }
                    Span::Sound => {
                        intro.push_str(
                            " The spectrogram draws at most 30 s: ask for a `range` that short to see it.",
                        );
                        script_done(&alone, &services, range).map(Into::into)
                    }
                };
                let done = match step {
                    Ok(Step::Done(done)) => *done,
                    Ok(Step::Job(job)) => match job() {
                        JobDone::Done(done) => done,
                        other => return other,
                    },
                    Err(ToolError(e)) => Done::error(e),
                };
                JobDone::Done(introduced(done, &intro, &name))
            });
            Ok(Step::Job(job))
        }
    }
}

/// `item` alone on a timeline of its own, from its first frame, at its own
/// size and frame rate (the sequence's, for what has no picture): timeline
/// time there is its source time.
fn alone(project: &Project, item: &MediaItem) -> Project {
    let info = item.info.as_ref();
    let video = info.and_then(|i| i.video.as_ref());
    let sequence = &project.timeline.settings;
    let mut timeline = Timeline::default();
    let settings = &mut timeline.settings;
    settings.sample_rate = sequence.sample_rate;
    settings.rate = match video {
        Some(v) if item.kind == MediaKind::Video => v.rate,
        _ => sequence.rate,
    };
    if let Some(v) = video {
        settings.width = v.width.max(2);
        settings.height = v.height.max(2);
    }
    let (kind, length) = match item.kind {
        MediaKind::Image => (TrackKind::Video, STILL),
        MediaKind::Video => (TrackKind::Video, info.map_or(Time::ZERO, |i| i.duration)),
        MediaKind::Audio => (TrackKind::Audio, info.map_or(Time::ZERO, |i| i.duration)),
    };
    let mut track = Track::new(kind, media_name(project, item.id));
    track.clips.push(Clip::new(
        Time::ZERO,
        length,
        ClipContent::Media {
            media: item.id,
            source_in: Time::ZERO,
            audio_stream: None,
        },
    ));
    timeline.tracks = vec![track];
    let mut out = Project::new(project.name.clone());
    out.id = project.id;
    out.media = vec![item.clone()];
    out.timeline = timeline;
    out
}

/// What the arguments ask to look at, checked against what the item is.
fn look(
    item: &MediaItem,
    info: &MediaInfo,
    alone: &Project,
    args: &Args,
) -> Result<Look, ToolError> {
    let zoom = zoom_region(alone, args)?;
    let t = args.opt_time("t")?;
    let range = args.opt_range("range")?;
    let detail = args.opt_bool("detail")?.unwrap_or(false);
    let audio = args.opt_bool("audio")?.unwrap_or(false);
    let length = info.duration;
    let whole = TimeRange::new(Time::ZERO, length);
    if item.kind != MediaKind::Image && whole.is_empty() {
        return fail("Its length could not be read, so there is nothing to see.");
    }
    let outside = |what: String| {
        ToolError(format!(
            "{what} is outside the item, which runs {whole}; times are its own seconds"
        ))
    };
    let span = |r: Option<TimeRange>| match r {
        None => Ok(whole),
        Some(r) => r
            .intersect(&whole)
            .ok_or_else(|| outside(format!("`range` {r}"))),
    };
    match item.kind {
        MediaKind::Image => {
            if audio || detail || t.is_some() || range.is_some() {
                return fail(
                    "This is a picture: it has no time or sound. Leave out `t`, `range`, `detail` and `audio`; `zoom` looks closer.",
                );
            }
            Ok(Look::Picture { at: None, zoom })
        }
        MediaKind::Audio => {
            if detail || t.is_some() || zoom.is_some() {
                return fail("This is audio: it has no picture. `range` picks the part to hear.");
            }
            Ok(Look::Span {
                range: span(range)?,
                span: Span::Sound,
            })
        }
        MediaKind::Video if audio => {
            if detail || t.is_some() || zoom.is_some() {
                return fail("`audio` hears a `range`; leave out `t`, `zoom` and `detail`.");
            }
            if info.audio.is_empty() {
                return fail("This video has no sound.");
            }
            Ok(Look::Span {
                range: span(range)?,
                span: Span::Sound,
            })
        }
        MediaKind::Video => match t {
            Some(t) => {
                if detail || range.is_some() {
                    return fail(
                        "`t` looks at one frame; leave out `range` and `detail`, or leave out `t` for a collage.",
                    );
                }
                if !(Time::ZERO..=length).contains(&t) {
                    return Err(outside(format!("`t` {t}")));
                }
                Ok(Look::Picture { at: Some(t), zoom })
            }
            None => {
                if zoom.is_some() {
                    return fail("`zoom` needs `t`, the frame to look closer at.");
                }
                let range = span(range)?;
                if detail && range.duration().as_secs_f64() > DETAIL_MAX + 0.001 {
                    return fail(format!(
                        "`detail` takes at most {DETAIL_MAX} s; leave it out for longer spans"
                    ));
                }
                Ok(Look::Span {
                    range,
                    span: Span::Frames { detail },
                })
            }
        },
    }
}

/// The line a view opens with: which item, what it is, and its clock.
fn about(item: &MediaItem, info: &MediaInfo, name: &str) -> String {
    let id = short(item.id);
    let video = info.video.as_ref();
    let length = info.duration.as_secs_f64();
    let sound = if info.audio.is_empty() {
        "without sound"
    } else {
        "with sound"
    };
    match item.kind {
        MediaKind::Image => match video {
            Some(v) => format!("Media {id} ({name}): a {}×{} picture.", v.width, v.height),
            None => format!("Media {id} ({name}): a picture."),
        },
        MediaKind::Video => {
            let picture = video
                .map(|v| format!(" {}×{} at {:.3} fps", v.width, v.height, v.rate.fps()))
                .unwrap_or_default();
            format!(
                "Media {id} ({name}): video{picture}, {length:.2} s, {sound}. Times are its own seconds."
            )
        }
        MediaKind::Audio => {
            format!("Media {id} ({name}): audio, {length:.2} s. Times are its own seconds.")
        }
    }
}

/// The analysis to run and wait for before `item` is heard: when it has
/// sound or pictures to analyse and its results don't cover what analysis
/// makes now. `None` when there is none to run, or nothing to analyse.
fn to_analyse(services: &Services, item: &MediaItem) -> Option<Arc<dyn AnalysisService>> {
    let service = services.analysis.as_ref()?;
    let analysable = item.kind != MediaKind::Image && item.fingerprint.is_some();
    (analysable && service.coverage(item) != Coverage::Full).then(|| service.clone())
}

/// Analyse `item` as the pool's Analyze does (only what its results lack)
/// and wait until that is done, unless the turn is stopped first. The pool
/// shows the job's progress meanwhile; a job of the item already queued or
/// running is the one waited on.
fn analyse(service: &dyn AnalysisService, item: &MediaItem, stop: &AtomicBool) -> Analysed {
    let job = service.analyze(item);
    loop {
        if stop.load(Ordering::Relaxed) {
            return Analysed::Stopped;
        }
        let state = service
            .jobs()
            .into_iter()
            .find(|j| j.id == job)
            .map(|j| j.state);
        match state {
            Some(JobState::Done) => return Analysed::Done,
            Some(JobState::Failed { message }) => return Analysed::Failed(message),
            // Gone from the list: cancelled, unless it finished and the
            // list was cleared since.
            None if service.coverage(item) == Coverage::Full => return Analysed::Done,
            None => return Analysed::Cancelled,
            Some(JobState::Queued | JobState::Running { .. }) => std::thread::sleep(POLL),
        }
    }
}

/// A picture, or a video's frame at `at` (`frame_w` pixels wide), whole or
/// with `zoom` magnified under a grid in its own pixels.
fn picture(
    media: &dyn flikk_core::services::MediaService,
    item: &MediaItem,
    at: Option<Time>,
    zoom: Option<[f64; 4]>,
    frame_w: u32,
    about: String,
    name: &str,
) -> Done {
    let width = zoom.map_or(FRAME_WIDTH, |r| zoom_width(frame_w, r));
    let frame = match media.frame(item, at.unwrap_or(Time::ZERO), width) {
        Ok(f) => f,
        Err(e) => return Done::error(format!("Could not decode {name}: {e}")),
    };
    let mut text = about;
    if let Some(t) = at {
        text.push_str(&format!(" Frame at {t}."));
    }
    let frame = match zoom {
        Some(region) => {
            let (zoomed, note) = zoomed(&frame, region, frame_w, Clock::Source);
            text.push('\n');
            text.push_str(&note);
            zoomed
        }
        None => frame,
    };
    let saved = save_png(&frame).and_then(|path| Ok((image_content(&path)?, path)));
    let (image, path) = match saved {
        Ok(x) => x,
        Err(ToolError(e)) => return Done::error(format!("Could not save the frame: {e}")),
    };
    let what = match (at, zoom) {
        (Some(t), Some(_)) => format!("zoomed frame at {t}"),
        (Some(t), None) => format!("frame at {t}"),
        (None, Some(_)) => "zoomed picture".to_owned(),
        (None, None) => "picture".to_owned(),
    };
    Done {
        result: ToolResult {
            content: vec![Content::Text { text }, image],
            is_error: false,
        },
        summary: format!("{name}: {what}"),
        images: vec![path],
        ..Default::default()
    }
}

/// `done` opening with `intro`, and its summary naming the item.
fn introduced(mut done: Done, intro: &str, name: &str) -> Done {
    match done.result.content.first_mut() {
        Some(Content::Text { text }) => *text = format!("{intro}\n\n{text}"),
        _ => done.result.content.insert(
            0,
            Content::Text {
                text: intro.to_owned(),
            },
        ),
    }
    done.summary = format!("{name}: {}", done.summary);
    done
}
