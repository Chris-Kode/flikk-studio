//! Files a request names, put where Pika can read them: Pika hands out an
//! upload URL for the file's type and length, the file is PUT there with
//! the headers that came with it, and the request names the hosted URL.

use super::{BASE, NAME, error};
use crate::http;
use flikk_core::cloud::{CloudError, CloudResult};
use serde_json::json;
use std::path::Path;

/// A URL Pika can read `path` from.
pub fn url(key: &str, path: &Path) -> CloudResult<String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let size = std::fs::metadata(path)
        .map_err(|e| CloudError::Failed(format!("could not read {}: {e}", path.display())))?
        .len();
    let mime = http::media_mime(path);
    let reply = http::post_json(
        &format!("{BASE}/v1/media/uploads"),
        &[("x-api-key", key)],
        &json!({ "content_type": mime, "size_bytes": size }),
        true,
    )?;
    if !reply.ok() {
        // Too big (413), or a type it won't take (415).
        return Err(match error(&reply) {
            e if e.needs_settings() => e,
            e => CloudError::Failed(format!("{NAME} could not take {name}: {e}")),
        });
    }
    let json = reply.json()?;
    let (Some(upload), Some(hosted)) = (json["upload_url"].as_str(), json["url"].as_str()) else {
        return Err(CloudError::Failed(format!("{NAME} could not take {name}")));
    };
    // Signed with the upload URL: sent as they came.
    let signed: Vec<(&str, &str)> = json["headers"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.as_str(), v.as_str()?)))
        .collect();
    let headers = if signed.is_empty() {
        vec![("content-type", mime)]
    } else {
        signed
    };
    let put = http::put_file(upload, path, &headers)?;
    if !put.ok() {
        return Err(CloudError::Failed(format!(
            "{NAME} could not take {name} ({})",
            put.status
        )));
    }
    Ok(hosted.to_owned())
}
