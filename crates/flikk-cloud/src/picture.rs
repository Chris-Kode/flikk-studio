//! What the image clients share: the prompt as image models read it, and
//! the picture a provider made, fetched next to the others.

use crate::http;
use flikk_core::cloud::{CloudResult, GenerationTask, Stage};
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// `prompt` with the editor's tags for its pictures (`@Image1`) as image
/// models read them: "image 1".
pub fn tags(prompt: &str) -> String {
    const TAG: &str = "@Image";
    let mut out = String::with_capacity(prompt.len());
    let mut rest = prompt;
    while let Some(at) = rest.find(TAG) {
        out.push_str(&rest[..at]);
        let after = &rest[at + TAG.len()..];
        let digits = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        if digits == 0 {
            out.push_str(TAG);
        } else {
            out.push_str("image ");
            out.push_str(&after[..digits]);
        }
        rest = &after[digits..];
    }
    out.push_str(rest);
    out
}

/// Fetch the picture at `url` to `<stem>.<its extension>`, reporting to
/// `task`. `content_type` is what the provider said it is; the file itself
/// has the last word.
pub fn fetch(
    url: &str,
    content_type: Option<&str>,
    stem: &Path,
    task: &GenerationTask,
) -> CloudResult<PathBuf> {
    let path = with_extension(stem, extension(content_type, url));
    task.report(Stage::Downloading(0.0));
    // A file the provider hands out on its own: the key doesn't go with it.
    http::download(url, &path, &|f| task.report(Stage::Downloading(f)))?;
    let Some(sniffed) = sniff(&path) else {
        return Ok(path);
    };
    let named = with_extension(stem, sniffed);
    if named != path && crate::settle(&path, &named).is_ok() {
        return Ok(named);
    }
    Ok(path)
}

/// `<stem>.<ext>`, keeping any dots in the stem.
fn with_extension(stem: &Path, ext: &str) -> PathBuf {
    let mut name = stem.as_os_str().to_owned();
    name.push(".");
    name.push(ext);
    PathBuf::from(name)
}

/// The extension of a picture, from its type or its address; png when
/// they say neither.
fn extension(content_type: Option<&str>, url: &str) -> &'static str {
    let named = url
        .split(['?', '#'])
        .next()
        .and_then(|u| u.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase());
    match (content_type, named.as_deref()) {
        (Some("image/jpeg" | "image/jpg"), _) | (_, Some("jpg" | "jpeg")) => "jpg",
        (Some("image/webp"), _) | (_, Some("webp")) => "webp",
        _ => "png",
    }
}

/// What the file at `path` is, from its first bytes.
fn sniff(path: &Path) -> Option<&'static str> {
    let mut head = [0u8; 12];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if head.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}
