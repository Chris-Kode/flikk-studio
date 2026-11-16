//! The video files a web page embeds or links to, found in its HTML.

use regex::Regex;
use std::sync::LazyLock;
use url::Url;

/// Most addresses listed for one page.
const MOST: usize = 30;
/// What the path of a video file's address ends in.
const VIDEO_FILES: [&str; 5] = [".mp4", ".m4v", ".mov", ".webm", ".mkv"];
/// What the path of a stream's manifest ends in (HLS, DASH): not a file to
/// bring in.
const STREAMS: [&str; 2] = [".m3u8", ".mpd"];

static VIDEO_TAG: LazyLock<Regex> = LazyLock::new(|| regex(r"(?is)<video\b[^>]*>"));
static SOURCE_TAG: LazyLock<Regex> = LazyLock::new(|| regex(r"(?is)<source\b[^>]*>"));
static SRC: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"(?is)\bsrc\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#));
static VIDEO_TYPE: LazyLock<Regex> = LazyLock::new(|| regex(r#"(?is)\btype\s*=\s*["']?\s*video/"#));
/// A quoted value: of an attribute (`href="…"`, `content='…'`), a JSON key
/// (`"url":"…"`) or a script's assignment.
static QUOTED: LazyLock<Regex> = LazyLock::new(|| regex(r#"[=:]\s*(?:"([^"]*)"|'([^']*)')"#));
/// An absolute or scheme-relative address anywhere in the text.
static ADDRESS: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"(?i)(?:https?:)?//[^\s"'<>()\[\]{}\\`^|]+"#));

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid pattern")
}

/// The video files on the page `html`, as absolute addresses resolved
/// against `base`, the page's own address: the `src` of each `<video>`,
/// and of each `<source>` of a video type, then every address whose path
/// ends in a video file's extension (links, `og:video`, players' JSON).
/// Streams (HLS, DASH) and what is not http or https are left out. In the
/// order found, each once, at most 30.
pub(super) fn videos_on(html: &str, base: &str) -> Vec<String> {
    let text = unescaped(html);
    let base = Url::parse(base).ok();
    let tagged = tag_sources(&text)
        .into_iter()
        .filter_map(|a| resolved(base.as_ref(), a));
    let quoted = QUOTED.captures_iter(&text).filter_map(|c| {
        let value = c.get(1).or_else(|| c.get(2))?;
        Some(value.as_str())
    });
    let written = ADDRESS
        .find_iter(&text)
        .map(|m| m.as_str().trim_end_matches(['.', ',', ';', ':', '!', '?']));
    let named = quoted
        .chain(written)
        .filter_map(|a| resolved(base.as_ref(), a))
        .filter(is_video_file);
    let mut found: Vec<String> = Vec::new();
    for url in tagged.chain(named).map(String::from) {
        if !found.contains(&url) {
            found.push(url);
            if found.len() == MOST {
                break;
            }
        }
    }
    found
}

/// The `src` of each `<video>` tag, then of each `<source>` tag whose
/// `type` is a video's.
fn tag_sources(text: &str) -> Vec<&str> {
    let videos = VIDEO_TAG.find_iter(text);
    let sources = SOURCE_TAG
        .find_iter(text)
        .filter(|tag| VIDEO_TYPE.is_match(tag.as_str()));
    videos
        .chain(sources)
        .filter_map(|tag| SRC.captures(tag.as_str()))
        .filter_map(|c| (1..=3).find_map(|i| c.get(i)))
        .map(|m| m.as_str())
        .collect()
}

/// `address` made absolute against `base`, without its fragment; `None`
/// when it is empty, not an http or https address, or a stream's.
fn resolved(base: Option<&Url>, address: &str) -> Option<Url> {
    let address = address.trim();
    if address.is_empty() {
        return None;
    }
    let mut url = match base {
        Some(base) => base.join(address),
        None => Url::parse(address),
    }
    .ok()?;
    url.set_fragment(None);
    let path = url.path().to_ascii_lowercase();
    let web = matches!(url.scheme(), "http" | "https");
    (web && !STREAMS.iter().any(|s| path.ends_with(s))).then_some(url)
}

/// Whether `url`'s path names a video file: `clip.mp4`, not `clip.mp4.jpg`.
fn is_video_file(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    VIDEO_FILES.iter().any(|ext| path.ends_with(ext))
}

/// `html` with the escapes undone that hide addresses in attributes and in
/// inline JSON.
fn unescaped(html: &str) -> String {
    html.replace("\\/", "/")
        .replace("\\u002F", "/")
        .replace("\\u002f", "/")
        .replace("\\u0026", "&")
        .replace("&#38;", "&")
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&amp;", "&")
}
