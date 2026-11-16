//! `view_web_video`: a video file on the web, seen and heard as
//! `view_media` sees a pool item, or, given a page's address, the video
//! files on that page. A file is downloaded once into the cache, under a
//! name made from its address that it gets only once whole; with `add` it
//! moves into the user's data and goes into the pool's Web Videos bin.

use super::super::pool::{Subject, Viewing, view_alone};
use super::super::{Done, Job, JobDone, Step, ToolCtx};
use super::{address, http_agent, page};
use crate::args::{Args, ToolError, ToolResultOf, fail};
use flikk_core::cloud;
use flikk_core::model::MediaItem;
use flikk_core::services::MediaService;
use flikk_core::{cache_sweep, paths};
use serde_json::{Map, Value};
use std::fs::{self, File, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use ureq::ResponseExt;

pub const TOOL: &str = "view_web_video";

/// Biggest video downloaded.
const MAX_BYTES: u64 = 500 << 20;
/// Most of a page read for the videos on it.
const PAGE_BYTES: u64 = 4 << 20;
/// What is read first, to tell a video file from a page.
const HEAD_BYTES: u64 = 4 << 10;
/// Longest a whole download may take.
const TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Read and written at a time; the turn's stop is checked between.
const CHUNK: usize = 256 << 10;

pub fn serve(ctx: &mut ToolCtx, args: &Args) -> ToolResultOf<Step> {
    let call = Call {
        url: address(
            ctx,
            args,
            "a video file, or of a page to list the videos on",
        )?,
        add: args.opt_bool("add")?.unwrap_or(false),
        looking: args.0.clone(),
        media: ctx.services.media.clone(),
        viewing: Viewing::of(ctx),
    };
    let job: Job = Box::new(move || call.run());
    Ok(Step::Job(job))
}

/// One call, taken on the UI thread and answered off it.
struct Call {
    url: String,
    add: bool,
    /// Its arguments, of which `view_media`'s looking ones (`range`,
    /// `detail`, `t`, `zoom`, `audio`) are read in the video's own clock.
    looking: Map<String, Value>,
    media: Option<Arc<dyn MediaService>>,
    viewing: Viewing,
}

impl Call {
    fn run(self) -> JobDone {
        let stop = self.viewing.stop().clone();
        match self.answer() {
            Ok(done) => done,
            Err(ToolError(e)) => JobDone::failed(&stop, e),
        }
    }

    /// The video seen as asked, and kept in the pool with `add`; or the
    /// video files on the page at the address.
    fn answer(self) -> ToolResultOf<JobDone> {
        let file = match fetch(&self.url, self.viewing.stop())? {
            Download::Video(file) => file,
            Download::Page(videos) => return Ok(JobDone::Done(page_done(&self.url, &videos))),
            Download::Stopped => return Ok(JobDone::Stopped),
        };
        let Some(media) = self.media.as_deref() else {
            return fail("Video is not available in this build.");
        };
        let item = cloud::video_item(file, Some(media));
        if item.info.as_ref().is_none_or(|i| i.video.is_none()) {
            // Not kept: the next call downloads it again, in case this
            // copy was the broken one.
            let _ = fs::remove_file(&item.path);
            return Err(not_a_video(&self.url, None));
        }
        // Called by its address, not by a media id it does not have until
        // it is added.
        let subject = Subject {
            item: item.clone(),
            name: name_of(&self.url),
            label: self.url.clone(),
        };
        let done = match view_alone(self.viewing, subject, &self.looking) {
            Ok(Step::Done(done)) => *done,
            Ok(Step::Job(job)) => match job() {
                JobDone::Done(done) => done,
                other => return Ok(other),
            },
            Err(ToolError(e)) => Done::error(e),
        };
        if !self.add || done.result.is_error {
            return Ok(JobDone::Done(done));
        }
        let path = keep(&item.path)?;
        Ok(JobDone::Fetched {
            item: Box::new(MediaItem { path, ..item }),
            done: Box::new(done),
            bin: cloud::WEB_VIDEOS,
        })
    }
}

/// What an address turned out to hold.
enum Download {
    /// A whole video file, in the cache.
    Video(PathBuf),
    /// A web page, with the addresses of the video files on it.
    Page(Vec<String>),
    /// The turn was stopped before the file was whole.
    Stopped,
}

/// The video file at `url` from the cache, downloaded into it first when
/// it is not there; or the video files on the page at `url`.
fn fetch(url: &str, stop: &AtomicBool) -> ToolResultOf<Download> {
    let cached = cached(url);
    if cached.is_file() {
        cache_sweep::mark_used(&cached);
        return Ok(Download::Video(cached));
    }
    download(url, &cached, stop)
}

/// Where the video file at `url` is cached. Only a whole file gets this
/// name, so one there is complete.
fn cached(url: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    paths::cache()
        .join("agent")
        .join("web")
        .join(format!("{:016x}", hasher.finish()))
}

/// Get `url`: a video file is streamed to `cached`, a page is read for
/// the video files on it, and anything else is an error.
fn download(url: &str, cached: &Path, stop: &AtomicBool) -> ToolResultOf<Download> {
    let mut response = http_agent(TIMEOUT)
        .get(url)
        .call()
        .map_err(|e| ToolError(format!("Could not reach {url}: {e}")))?;
    let status = response.status();
    if !status.is_success() {
        return fail(format!("{url} answered {status}."));
    }
    if response
        .body()
        .content_length()
        .is_some_and(|n| n > MAX_BYTES)
    {
        return Err(too_big());
    }
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    // Where redirects led: what the page's own addresses are relative to.
    let base = response.get_uri().to_string();
    let mut body = response.body_mut().as_reader();
    let mut head = Vec::new();
    (&mut body)
        .take(HEAD_BYTES)
        .read_to_end(&mut head)
        .map_err(broke_off)?;
    if container(&head).is_some() {
        return save(&head, &mut body, cached, stop);
    }
    if is_page(content_type.as_deref(), &head) {
        let mut html = head;
        body.take(PAGE_BYTES - HEAD_BYTES)
            .read_to_end(&mut html)
            .map_err(broke_off)?;
        let html = String::from_utf8_lossy(&html);
        return Ok(Download::Page(page::videos_on(&html, &base)));
    }
    Err(not_a_video(url, content_type.as_deref()))
}

/// Stream the video that starts with `head` and goes on in `body` to
/// `cached`, through a temp file that is removed unless it becomes
/// `cached`. When another call put the file there first, theirs is used.
fn save(
    head: &[u8],
    body: &mut impl Read,
    cached: &Path,
    stop: &AtomicBool,
) -> ToolResultOf<Download> {
    if let Some(dir) = cached.parent() {
        fs::create_dir_all(dir).map_err(saving)?;
    }
    let part = flikk_os::fs::temp_sibling(cached);
    let saved = match write_part(head, body, &part, stop) {
        Ok(Written::Whole) => flikk_os::fs::place_new(&part, cached)
            .map(|_| Download::Video(cached.to_path_buf()))
            .map_err(saving),
        Ok(Written::Stopped) => Ok(Download::Stopped),
        Err(e) => Err(e),
    };
    // Already gone once placed; left to remove in every other case.
    let _ = fs::remove_file(&part);
    saved
}

/// How far a download got.
enum Written {
    Whole,
    Stopped,
}

/// Write `head`, then the rest of `body`, to the new file `part`, until
/// the body ends, `stop` is set, or it passes [`MAX_BYTES`].
fn write_part(
    head: &[u8],
    body: &mut impl Read,
    part: &Path,
    stop: &AtomicBool,
) -> ToolResultOf<Written> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(part)
        .map_err(saving)?;
    file.write_all(head).map_err(saving)?;
    let mut total = head.len() as u64;
    let mut chunk = vec![0; CHUNK];
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(Written::Stopped);
        }
        let n = match body.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(broke_off(e)),
        };
        total += n as u64;
        if total > MAX_BYTES {
            return Err(too_big());
        }
        file.write_all(&chunk[..n]).map_err(saving)?;
    }
    file.sync_all().map_err(saving)?;
    Ok(Written::Whole)
}

/// Move the cached video at `cached` into the user's data, under a new
/// name with its container's extension, for the pool.
fn keep(cached: &Path) -> ToolResultOf<PathBuf> {
    let keeping = |e: io::Error| ToolError(format!("Could not keep the video: {e}"));
    let mut head = Vec::new();
    File::open(cached)
        .and_then(|f| f.take(HEAD_BYTES).read_to_end(&mut head))
        .map_err(keeping)?;
    let ext = container(&head).unwrap_or("mp4");
    let kept = paths::web().join(format!("{}.{ext}", uuid::Uuid::new_v4().simple()));
    if !flikk_os::fs::place_new(cached, &kept).map_err(keeping)? {
        return fail("Could not keep the video: its new name was taken.");
    }
    Ok(kept)
}

/// The extension of the video container a file starting with `head` is:
/// an ISO media file (MP4, M4V, MOV) by its `ftyp` box and brand, or a
/// Matroska one (WebM, MKV) by its EBML header. `None` for anything else.
fn container(head: &[u8]) -> Option<&'static str> {
    if head.get(4..8) == Some(b"ftyp") {
        return Some(match head.get(8..12) {
            Some(b"qt  ") => "mov",
            Some(b"M4V ") => "m4v",
            _ => "mp4",
        });
    }
    if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        let webm = head.windows(4).any(|w| w == b"webm");
        return Some(if webm { "webm" } else { "mkv" });
    }
    None
}

/// Whether a response of `content_type` starting with `head` is a page.
fn is_page(content_type: Option<&str>, head: &[u8]) -> bool {
    let html = content_type.is_some_and(|t| t.to_ascii_lowercase().contains("html"));
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    html || text.starts_with('<')
}

/// What a page's address gives: the video files on it, or that there are
/// none.
fn page_done(url: &str, videos: &[String]) -> Done {
    if videos.is_empty() {
        return Done::text(
            format!("{url}: no video files"),
            format!(
                "There are no video files on {url}. Videos that play as streams (YouTube, HLS or DASH players) cannot be brought in."
            ),
        );
    }
    Done::text(
        format!("{url}: {} video files", videos.len()),
        format!(
            "The video files on {url}; pass one as `url` to see it:\n{}",
            videos.join("\n")
        ),
    )
}

/// What the video at `url` is called: the last part of its path, else its
/// host.
fn name_of(url: &str) -> String {
    let parsed = url::Url::parse(url).ok();
    let last = parsed
        .as_ref()
        .and_then(|u| u.path_segments()?.next_back())
        .filter(|s| !s.is_empty());
    last.or_else(|| parsed.as_ref().and_then(|u| u.host_str()))
        .unwrap_or(url)
        .to_owned()
}

fn not_a_video(url: &str, content_type: Option<&str>) -> ToolError {
    let kind = content_type.map(|k| format!(" ({k})")).unwrap_or_default();
    ToolError(format!(
        "{url} is not an MP4, MOV, M4V, WebM or MKV file{kind}. Pass the address of the video file itself; a page's address lists the video files on it."
    ))
}

fn too_big() -> ToolError {
    ToolError(format!(
        "The video is over {} MB, the most brought in from the web.",
        MAX_BYTES >> 20
    ))
}

fn broke_off(e: io::Error) -> ToolError {
    ToolError(format!("The download broke off: {e}"))
}

fn saving(e: io::Error) -> ToolError {
    ToolError(format!("Could not save the video: {e}"))
}
