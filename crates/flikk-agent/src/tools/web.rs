//! `view_web_image`: a picture from the web, shown to the agent. The
//! backends' own web tools read pages as text only, so the agent cannot see
//! the pictures on them. With `add`, the picture also goes into the pool's
//! Web Images bin. Only offered when the user allows the web.

use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolError, ToolResultOf, fail, short};
use crate::bridge::{Content, ToolResult};
use base64::Engine;
use flikk_core::bus::{Author, CommandBus};
use flikk_core::cloud;
use flikk_core::model::MediaItem;
use flikk_core::paths;
use flikk_core::services::MediaService;
use image::ImageFormat;
use std::io::Cursor;
use std::time::Duration;

pub const TOOL: &str = "view_web_image";

/// The pool bin added pictures go to.
pub const BIN: &str = "Web Images";

/// Biggest picture downloaded.
const MAX_BYTES: u64 = 25 << 20;

/// Longest side of the copy the agent sees; bigger pictures are scaled down.
const MAX_SIDE: u32 = 1568;

pub fn serve(ctx: &mut ToolCtx, args: &Args) -> ToolResultOf<Step> {
    if !ctx.settings.web {
        return fail("The user has not allowed the web for the agent.");
    }
    let url = args.str("url")?.trim().to_owned();
    let scheme = url.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    if !matches!(scheme.as_deref(), Some("http" | "https")) {
        return fail("`url` is the http or https address of a picture");
    }
    let add = args.opt_bool("add")?.unwrap_or(false);
    let media = ctx.services.media.clone();
    let job: Job = Box::new(move || match fetch(&url, add, media.as_deref()) {
        Ok(done) => done,
        Err(ToolError(e)) => JobDone::Done(Done::error(e)),
    });
    Ok(Step::Job(job))
}

fn fetch(url: &str, add: bool, media: Option<&dyn MediaService>) -> ToolResultOf<JobDone> {
    let (bytes, content_type) = download(url)?;
    let not_a_picture = || {
        let kind = content_type
            .as_deref()
            .map(|k| format!(" ({k})"))
            .unwrap_or_default();
        ToolError(format!(
            "{url} is not a PNG, JPEG, GIF or WebP picture{kind}. Pass the address of the image itself, such as an <img> src or the page's og:image, not the page."
        ))
    };
    let format = image::guess_format(&bytes).map_err(|_| not_a_picture())?;
    let ext = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Gif => "gif",
        ImageFormat::WebP => "webp",
        _ => return Err(not_a_picture()),
    };
    let picture = image::load_from_memory_with_format(&bytes, format)
        .map_err(|e| ToolError(format!("Could not read the picture at {url}: {e}")))?;
    let (width, height) = (picture.width(), picture.height());
    let shown = if width.max(height) > MAX_SIDE {
        picture.thumbnail(MAX_SIDE, MAX_SIDE)
    } else {
        picture
    };
    let mut png = Vec::new();
    shown
        .to_rgba8()
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| ToolError(format!("Could not show the picture: {e}")))?;
    let dir = paths::cache().join("agent").join("web");
    std::fs::create_dir_all(&dir)?;
    let seen = dir.join(format!("{}.png", uuid::Uuid::new_v4().simple()));
    std::fs::write(&seen, &png)?;

    let text = format!("{width}×{height} {ext} picture from {url}.");
    let done = Done {
        result: ToolResult {
            content: vec![
                Content::Text { text },
                Content::Image {
                    data: base64::engine::general_purpose::STANDARD.encode(&png),
                    mime_type: "image/png".into(),
                },
            ],
            is_error: false,
        },
        summary: url.to_owned(),
        images: vec![seen],
        ..Default::default()
    };
    if !add {
        return Ok(JobDone::Done(done));
    }
    // The original file, kept with the user's data so the project can use it.
    let kept = paths::web().join(format!("{}.{ext}", uuid::Uuid::new_v4().simple()));
    std::fs::write(&kept, &bytes)?;
    Ok(JobDone::Fetched {
        item: Box::new(cloud::image_item(kept, media)),
        done: Box::new(done),
    })
}

/// The body of `url` and its content type.
fn download(url: &str) -> ToolResultOf<(Vec<u8>, Option<String>)> {
    use ureq::tls::{RootCerts, TlsConfig};
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .user_agent(concat!("flikk/", env!("CARGO_PKG_VERSION")))
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .map_err(|e| ToolError(format!("Could not reach {url}: {e}")))?;
    let status = response.status();
    if !status.is_success() {
        return fail(format!("{url} answered {status}."));
    }
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_vec()
        .map_err(|e| ToolError(format!("Could not download {url}: {e}")))?;
    Ok((bytes, content_type))
}

/// Back on the UI thread: add the picture to the pool's Web Images bin at
/// once, as `author`'s change (the user's when the turn is gone), so a
/// proposal can use it.
pub fn finish(
    bus: &mut CommandBus,
    author: Option<&Author>,
    item: MediaItem,
    mut done: Done,
) -> Done {
    let id = item.id;
    let commands = cloud::pool_commands(bus.project(), item, BIN);
    let author = author.cloned().unwrap_or(Author::User);
    let note = match bus.execute_shared_as(commands, "Add web image", author) {
        Ok(_) => format!(
            " Added to the pool as media {} in the {BIN} bin.",
            short(id)
        ),
        Err(e) => format!(" Could not add it to the pool: {e}"),
    };
    if let Some(Content::Text { text }) = done.result.content.first_mut() {
        text.push_str(&note);
    }
    done
}
