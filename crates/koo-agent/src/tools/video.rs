//! `generate_video`: a video made by the provider the user set for it (the
//! Generate category), from a description, from a first frame (a picture
//! in the pool, or the program at a time), or after references (pictures,
//! clips and sounds in the pool). It spends the user's money, so it waits
//! for their approval unless they allowed it. The file goes to the pool's
//! Video Generated bin; with `at` (or `range`) it is also proposed on a
//! video track, its sound on an audio track beside it. Placing and adding
//! to the pool are `generate`'s, as for audio.

use super::generate::{Placement, unavailable};
use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolResultOf, fail, media_id, secs, track_id};
use koo_core::cloud::{CloudError, GenerateRequest, VideoModel, VideoReference};
use koo_core::compose::{self, Stills};
use koo_core::model::{MediaKind, Project, TrackKind};
use koo_core::services::Services;
use koo_core::{Time, paths};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Widest frame sent as a first or last frame.
const FRAME_WIDTH: u32 = 1920;

/// A frame of the program to grab before the video is made, into `path`
/// (which the request already names).
#[derive(Clone, Debug, PartialEq)]
pub struct FrameGrab {
    pub at: Time,
    pub path: PathBuf,
}

impl FrameGrab {
    /// A grab of the program at `at`, into a new file under the cache.
    pub fn new(at: Time) -> Self {
        let name = format!("{}.png", uuid::Uuid::new_v4().simple());
        Self {
            at,
            path: paths::cache().join("frames").join(name),
        }
    }

    fn to_json(&self) -> Value {
        json!({ "at": secs(self.at), "path": self.path })
    }

    fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            at: Time::from_secs_f64(value.get("at")?.as_f64()?),
            path: PathBuf::from(value.get("path")?.as_str()?),
        })
    }
}

/// Write the program at `at` as a PNG at `path`: the sequence's picture,
/// at most [`FRAME_WIDTH`] wide, as the viewer shows it.
pub fn grab_frame(
    project: &Project,
    services: &Services,
    at: Time,
    path: &Path,
) -> Result<(), String> {
    let settings = &project.timeline.settings;
    let width = settings.width.clamp(2, FRAME_WIDTH);
    let size = compose::size_for_width(project, width);
    let mut stills = Stills::new(services.media.clone(), width);
    let frame = compose::program(project, at, size, services, &mut stills);
    let fail = |e: String| format!("could not grab the frame at {:.2} s: {e}", at.as_secs_f64());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(e.to_string()))?;
    }
    let file = std::fs::File::create(path).map_err(|e| fail(e.to_string()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| fail(e.to_string()))?;
    writer
        .write_image_data(&frame.rgba)
        .map_err(|e| fail(e.to_string()))?;
    writer.finish().map_err(|e| fail(e.to_string()))
}

/// Grab each of `grabs`, so the request's frames are on disk.
pub fn grab_all(project: &Project, services: &Services, grabs: &[FrameGrab]) -> Result<(), String> {
    grabs
        .iter()
        .try_for_each(|g| grab_frame(project, services, g.at, &g.path))
}

/// The request, the frames to grab for it and its placement, from the
/// tool's arguments; checked against `model`, the one picked for video.
pub fn parse(
    project: &Project,
    model: Option<&VideoModel>,
    args: &Args,
) -> ToolResultOf<(GenerateRequest, Vec<FrameGrab>, Option<Placement>)> {
    let prompt = args.str("prompt")?.trim().to_owned();
    if prompt.is_empty() {
        return fail("`prompt` describes what to make");
    }
    let range = args.opt_range("range")?;
    let length = match (args.opt_f64("duration")?, range) {
        (Some(secs), _) if secs > 0.0 => Some(Time::from_secs_f64(secs.round())),
        (Some(_), _) => return fail("`duration` is in seconds, more than 0"),
        (None, Some(r)) => Some(Time::from_secs_f64(r.duration().as_secs_f64().round())),
        (None, None) => None,
    };
    let mut grabs = Vec::new();
    let mut frame = |media: &str, at: &str| -> ToolResultOf<Option<PathBuf>> {
        match (args.opt_str(media)?, args.opt_time(at)?) {
            (Some(_), Some(_)) => fail(format!("give `{media}` or `{at}`, not both")),
            (Some(id), None) => {
                let item = project
                    .media(media_id(project, id)?)
                    .filter(|m| m.kind == MediaKind::Image);
                match item {
                    Some(item) => Ok(Some(item.path.clone())),
                    None => fail(format!("`{media}` must be a picture in the pool")),
                }
            }
            (None, Some(t)) => {
                let grab = FrameGrab::new(t);
                let path = grab.path.clone();
                grabs.push(grab);
                Ok(Some(path))
            }
            (None, None) => Ok(None),
        }
    };
    let first_frame = frame("first_frame", "first_frame_at")?;
    let last_frame = frame("last_frame", "last_frame_at")?;
    let mut references = Vec::new();
    for id in args.strings("references")? {
        let media = media_id(project, &id)?;
        let Some(item) = project.media(media) else {
            return fail(format!("no media {id}"));
        };
        references.push(VideoReference {
            kind: item.kind,
            path: item.path.clone(),
            length: item.info.as_ref().map(|i| i.duration),
        });
    }
    let request = GenerateRequest::Video {
        prompt,
        length,
        aspect: args.opt_str("aspect")?.map(str::to_owned),
        resolution: args.opt_str("resolution")?.map(str::to_owned),
        first_frame,
        last_frame,
        references,
        audio: args.opt_bool("audio")?.unwrap_or(true),
    };
    if let Some(model) = model {
        model
            .check(&request)
            .map_err(|e| crate::args::ToolError(error_text(&e)))?;
    }
    let at = args.opt_time("at")?.or(range.map(|r| r.start));
    let track = args
        .opt_str("track")?
        .map(|t| track_id(project, t))
        .transpose()?;
    if let Some(track) = track
        && project
            .timeline
            .track(track)
            .is_some_and(|t| !matches!(t.kind, TrackKind::Video | TrackKind::Overlay))
    {
        return fail("`track` must be a video track");
    }
    let placement = at.map(|at| Placement { at, track });
    Ok((request, grabs, placement))
}

/// A provider's refusal as the model should read it.
pub fn error_text(e: &CloudError) -> String {
    match e {
        CloudError::BadPrompt { message, .. } => format!(
            "Refused: {message}. Change the request to fit (the model's limits are in the tool's description), or tell the user."
        ),
        e => super::generate::error_text(e),
    }
}

/// "Video, 5 s: …", then what it starts on or follows, and where it goes.
pub fn summary(request: &GenerateRequest, placement: Option<&Placement>) -> String {
    let mut text = request.summary();
    if let GenerateRequest::Video {
        first_frame,
        references,
        ..
    } = request
    {
        if first_frame.is_some() {
            text.push_str(" (from a frame)");
        } else if !references.is_empty() {
            text.push_str(&format!(" (after {} references)", references.len()));
        }
    }
    match placement {
        Some(p) => format!("{text} → {:.1}s", p.at.as_secs_f64()),
        None => text,
    }
}

/// The request with its frames and placement, as an approval carries it.
pub fn request_json(
    request: &GenerateRequest,
    grabs: &[FrameGrab],
    placement: Option<&Placement>,
) -> Value {
    json!({
        "request": request,
        "frames": grabs.iter().map(FrameGrab::to_json).collect::<Vec<_>>(),
        "place": placement.map(Placement::to_json),
    })
}

/// What [`request_json`] made.
pub fn from_json(value: &Value) -> Option<(GenerateRequest, Vec<FrameGrab>, Option<Placement>)> {
    let request = serde_json::from_value(value.get("request")?.clone()).ok()?;
    let grabs = value
        .get("frames")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(FrameGrab::from_json).collect())
        .unwrap_or_default();
    let placement = value.get("place").and_then(Placement::from_json);
    Some((request, grabs, placement))
}

/// With the user's leave: a job that grabs its frames, makes the video and
/// comes back to the UI thread to add it to the pool and place it.
pub fn serve(ctx: &mut ToolCtx, args: &Args) -> ToolResultOf<Step> {
    let Some(generation) = ctx.services.generation.clone() else {
        return fail("Generation is not available in this build.");
    };
    let model = generation.video_model();
    let (request, grabs, placement) = parse(ctx.project(), model.as_ref(), args)?;
    if generation.provider_for(request.capability()).is_none() {
        return fail(unavailable(&request));
    }
    let services = ctx.services.clone();
    let project = (!grabs.is_empty()).then(|| ctx.project().clone());
    let job: Job = Box::new(move || {
        if let Some(project) = &project
            && let Err(e) = grab_all(project, &services, &grabs)
        {
            return JobDone::Done(Done::error(e));
        }
        let task = koo_core::cloud::GenerationTask::default();
        match generation.generate(&request, &paths::generated(), &task) {
            Ok(made) => JobDone::Generated {
                item: Box::new(koo_core::cloud::media_item(
                    &made,
                    services.media.as_deref(),
                )),
                placement,
                summary: request.summary(),
            },
            Err(e) => JobDone::Done(Done::error(error_text(&e))),
        }
    });
    Ok(Step::Job(job))
}
