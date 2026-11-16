//! `lip_sync`: a face in the pool (a video, or a picture) made to say a
//! sound in the pool by the provider the user set for lip sync (the
//! Generate category). It spends the user's money, so each call waits for
//! their approval. The file goes to the pool's Video Lip-synced bin; with
//! `replace`, for a video, it is also proposed in place of the source in
//! its clips on picture tracks. Adding to the pool, proposing, and the
//! approval's summary and JSON are `generate`'s.

use crate::args::{Args, ToolError, ToolResultOf, fail, media_id, short};
use flikk_core::MediaId;
use flikk_core::cloud::{
    CloudError, FaceKind, GenerateRequest, LipSyncFace, LipSyncModel, LipSyncVoice,
};
use flikk_core::model::Project;

pub const TOOL: &str = "lip_sync";

/// The request, and the video it replaces in its clips when asked, from
/// the tool's arguments; checked against `model`, the one picked for lip
/// sync.
pub(crate) fn parse(
    project: &Project,
    model: Option<&LipSyncModel>,
    args: &Args,
) -> ToolResultOf<(GenerateRequest, Option<MediaId>)> {
    let source = media_id(project, args.str("face")?)?;
    let Some(item) = project.media(source) else {
        return fail(format!("no media {}", short(source)));
    };
    let Some(face) = LipSyncFace::of(item) else {
        return fail("`face` must be a video or a picture in the pool");
    };
    let sound = media_id(project, args.str("voice")?)?;
    let Some(voice) = project.media(sound).and_then(LipSyncVoice::of) else {
        return fail("`voice` must be a sound in the pool");
    };
    let replace = args.opt_bool("replace")?.unwrap_or(false);
    if replace && face.kind == FaceKind::Picture {
        return fail("`replace` is for a video face; leave it out for a picture");
    }
    if replace && !voice.covers(&face) {
        return fail(
            "`replace` needs a voice at least as long as the video: the result ends where the shorter of the two does, so its clips would run past it. Leave `replace` out, or use a voice that covers the video",
        );
    }
    let request = GenerateRequest::LipSync { face, voice };
    if let Some(model) = model {
        model
            .check(&request)
            .map_err(|e| ToolError(error_text(&e)))?;
    }
    Ok((request, replace.then_some(source)))
}

/// A provider's refusal as the model should read it.
pub fn error_text(e: &CloudError) -> String {
    match e {
        CloudError::BadPrompt { message, .. } => format!(
            "Refused: {message}. Pick a face and a voice the model takes (see the tool's description), or tell the user."
        ),
        e => super::generate::error_text(e),
    }
}
