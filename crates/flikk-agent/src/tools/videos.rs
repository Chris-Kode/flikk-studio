//! Videos to reuse: `list_library_videos` lists the user's library, and
//! `use_library_video` adds one to the pool and proposes it on a video
//! track, its sound on an audio track. `save_video_to_library` goes through
//! approval.

use super::sounds::add_to_pool;
use super::{Done, Step, ToolCtx};
use crate::args::{Args, ToolError, secs, short, strip_token};
use flikk_core::library::{self, Library, Video};
use flikk_core::model::Project;
use serde_json::{Value, json};

/// What the model is told of library video `v`, with its media id when
/// `project` has it in the pool.
fn described(project: &Project, v: &Video) -> Value {
    let mut value = json!({ "library_id": v.id.to_string(), "name": v.name });
    let (media, added) = library::pool_media(project, v.item());
    if added.is_empty() {
        value["media"] = json!(short(media));
    }
    if let Some(info) = &v.info {
        value["duration"] = secs(info.duration);
        value["sound"] = json!(!info.audio.is_empty());
        if let Some(picture) = &info.video {
            value["size"] = json!(format!("{}x{}", picture.width, picture.height));
        }
    }
    if !v.tags.is_empty() {
        value["tags"] = json!(v.tags);
    }
    if let Some(generated) = &v.generated {
        value["prompt"] = json!(generated.prompt);
    }
    value
}

/// Whether `v`'s name, tags or what a provider was asked carry one of
/// `words` (lower case).
fn carries(v: &Video, words: &[String]) -> bool {
    let prompt = v.generated.as_ref().map_or("", |g| g.prompt.as_str());
    let text = format!("{} {} {prompt}", v.name, v.tags.join(" ")).to_lowercase();
    words.iter().any(|w| text.contains(w.as_str()))
}

/// The videos in the user's library, all or those `text` names.
pub fn list(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let words: Vec<String> = args
        .opt_str("text")?
        .unwrap_or_default()
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let limit = args.opt_f64("limit")?.unwrap_or(20.0).clamp(1.0, 50.0) as usize;
    let lib = Library::load();
    let found: Vec<&Video> = lib
        .videos
        .iter()
        .filter(|v| words.is_empty() || carries(v, &words))
        .collect();
    let total = found.len();
    let project = ctx.project();
    let videos: Vec<Value> = found
        .into_iter()
        .take(limit)
        .map(|v| described(project, v))
        .collect();
    Ok(Done::json(
        format!("{total} library videos"),
        &json!({ "videos": videos, "total": total }),
    )
    .into())
}

/// Add a video to the pool as the user's edit, like a generated one, and
/// propose it where asked.
pub fn use_video(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let lib = Library::load();
    let text = strip_token(args.str("video")?).trim().to_lowercase();
    let video = lib
        .videos
        .iter()
        .find(|v| v.id.to_string() == text || v.id.0.to_string() == text)
        .or_else(|| lib.videos.iter().find(|v| v.name.to_lowercase() == text))
        .ok_or_else(|| {
            ToolError(format!(
                "no library video `{text}`; list_library_videos lists them"
            ))
        })?;
    let text = add_to_pool(ctx, args, video.item())?;
    Ok(Done::text(format!("video {}", video.name), text).into())
}
