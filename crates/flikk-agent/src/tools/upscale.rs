//! `upscale_video`: a video in the pool made larger by the provider the
//! user set for upscaling (the Generate category), with the source's
//! sound. It spends the user's money, so it waits for their approval
//! unless they allowed it. The file goes to the pool's Video Upscaled bin;
//! with `replace` it is also proposed in place of the source in its clips
//! on picture tracks. Adding to the pool and proposing are `generate`'s.

use super::generate::{Destination, unavailable};
use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolError, ToolResultOf, fail, media_id, short};
use flikk_core::MediaId;
use flikk_core::cloud::{CloudError, GenerateRequest, UpscaleModel, UpscaleSource};
use flikk_core::model::Project;
use serde_json::{Value, json};

pub const TOOL: &str = "upscale_video";

/// The request, and the video it replaces in its clips when asked, from
/// the tool's arguments; checked against `model`, the one picked for
/// upscaling, and made at its default resolution when none is asked for.
pub(crate) fn parse(
    project: &Project,
    model: Option<&UpscaleModel>,
    args: &Args,
) -> ToolResultOf<(GenerateRequest, Option<MediaId>)> {
    let media = media_id(project, args.str("media")?)?;
    let Some(item) = project.media(media) else {
        return fail(format!("no media {}", short(media)));
    };
    let Some(source) = UpscaleSource::of(item) else {
        return fail("`media` must be a video in the pool whose size is known");
    };
    let resolution = match args.opt_str("resolution")? {
        Some(asked) => Some(asked.trim().to_ascii_lowercase()),
        None => model
            .and_then(|m| m.default_target(&source))
            .map(str::to_owned),
    };
    let request = GenerateRequest::Upscale { source, resolution };
    if let Some(model) = model {
        model
            .check(&request)
            .map_err(|e| ToolError(error_text(&e)))?;
    }
    let replace = args.opt_bool("replace")?.unwrap_or(false);
    Ok((request, replace.then_some(media)))
}

/// A provider's refusal as the model should read it.
pub fn error_text(e: &CloudError) -> String {
    match e {
        CloudError::BadPrompt { message, .. } => format!(
            "Refused: {message}. Ask for a resolution the model can make (see the tool's description), or tell the user."
        ),
        e => super::generate::error_text(e),
    }
}

/// "Upscale to 1080P: Interview_A.mov", and whether its clips switch.
pub(crate) fn summary(request: &GenerateRequest, replaces: Option<MediaId>) -> String {
    let text = request.summary();
    match replaces {
        Some(_) => format!("{text}, in place of its clips"),
        None => text,
    }
}

/// The request with the video it replaces, as an approval carries it.
pub(crate) fn request_json(request: &GenerateRequest, replaces: Option<MediaId>) -> Value {
    json!({
        "request": request,
        "replace": replaces.map(|m| m.0.to_string()),
    })
}

/// What [`request_json`] made.
pub fn from_json(value: &Value) -> Option<(GenerateRequest, Destination)> {
    let request = serde_json::from_value(value.get("request")?.clone()).ok()?;
    let replaces = value
        .get("replace")
        .and_then(Value::as_str)
        .and_then(|m| uuid::Uuid::parse_str(m).ok())
        .map(MediaId);
    Some((
        request,
        replaces.map_or(Destination::Pool, Destination::Replace),
    ))
}

/// With the user's leave: a job that makes the video larger and comes back
/// to the UI thread to add it to the pool and, when asked, propose it in
/// the source's clips.
pub(crate) fn serve(ctx: &mut ToolCtx, args: &Args) -> ToolResultOf<Step> {
    let Some(generation) = ctx.services.generation.clone() else {
        return fail("Generation is not available in this build.");
    };
    let model = generation.upscale_model();
    let (request, replaces) = parse(ctx.project(), model.as_ref(), args)?;
    if generation.provider_for(request.capability()).is_none() {
        return fail(unavailable(&request));
    }
    let media = ctx.services.media.clone();
    let task = ctx.stop.task();
    let job: Job = Box::new(move || {
        match generation.generate(&request, &flikk_core::paths::generated(), &task) {
            Ok(made) => JobDone::Generated {
                item: Box::new(flikk_core::cloud::media_item(&made, media.as_deref())),
                destination: replaces.map_or(Destination::Pool, Destination::Replace),
                summary: request.summary(),
            },
            Err(e) => JobDone::Done(Done::error(error_text(&e))),
        }
    });
    Ok(Step::Job(job))
}
