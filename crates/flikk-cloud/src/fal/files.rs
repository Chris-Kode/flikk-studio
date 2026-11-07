//! Files a request names, put where fal can read them: uploaded to fal's
//! storage (the flow fal's own clients use: ask for an upload URL, PUT the
//! file there), or sent inline as a data URI when the storage won't take
//! it and the file is small.

use super::{NAME, auth, error};
use crate::http;
use base64::Engine as _;
use flikk_core::cloud::{CloudError, CloudResult, GenerationTask};
use serde_json::json;
use std::path::Path;

const INITIATE: &str = "https://rest.alpha.fal.ai/storage/upload/initiate?storage_type=fal-cdn-v3";

/// Biggest file sent inline when the storage won't take it.
const INLINE: u64 = 8 << 20;

/// A URL fal can read `path` from; [`CloudError::Cancelled`] when `task`
/// is stopped while an upload waits to be tried again.
pub fn url(key: &str, path: &Path, task: &GenerationTask) -> CloudResult<String> {
    let mime = http::media_mime(path);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let auth = auth(key);
    let reply = http::post_json(
        INITIATE,
        &[("authorization", &auth)],
        &json!({ "content_type": mime, "file_name": name }),
        true,
        Some(task),
    )?;
    if reply.ok() {
        let json = reply.json()?;
        if let (Some(upload), Some(file)) = (json["upload_url"].as_str(), json["file_url"].as_str())
        {
            let put = http::put_file(upload, path, &[("content-type", mime)], Some(task))?;
            if put.ok() {
                return Ok(file.to_owned());
            }
        }
    } else {
        let e = error(&reply);
        if e.needs_settings() {
            return Err(e);
        }
    }
    inline(path, mime)
}

/// `path` as a data URI, when it is small enough to send that way.
fn inline(path: &Path, mime: &str) -> CloudResult<String> {
    let fail =
        |e: std::io::Error| CloudError::Failed(format!("could not read {}: {e}", path.display()));
    let size = std::fs::metadata(path).map_err(fail)?.len();
    if size > INLINE {
        return Err(CloudError::Failed(format!(
            "{NAME} could not take {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        )));
    }
    let bytes = std::fs::read(path).map_err(fail)?;
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{data}"))
}
