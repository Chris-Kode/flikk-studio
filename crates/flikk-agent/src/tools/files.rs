//! Reading the files and folders the user attached to the chat. Attaching
//! is the user's leave: the agent reads inside an attachment and nowhere
//! else, and only in the editor's own chat. Every path goes through
//! [`resolve`], which canonicalizes it first, so `..` and symlinks that lead
//! out are refused like any other path outside.

use super::web::picture;
use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolError, ToolResultOf, fail};
use crate::bridge::{Content, ToolResult};
use flikk_core::agent::{Attachment, AttachmentKind};
use flikk_core::cloud;
use flikk_core::paths;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const TOOLS: [&str; 4] = ["list_files", "read_file", "search_files", "copy_file"];

/// The pool bin copied pictures go to.
const BIN: &str = "Attached";

/// Folders a listing or search never walks into: dependencies and builds.
const SKIP: [&str; 7] = [
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".turbo",
];

const LIST_MAX: usize = 500;
const DEPTH_DEFAULT: usize = 3;
const DEPTH_MAX: usize = 12;
const LINES_DEFAULT: usize = 2000;
/// Most text a `read_file` call returns.
const TEXT_MAX: usize = 256 << 10;
/// Biggest file read whole.
const READ_MAX: u64 = 10 << 20;
/// Bigger files are left out of a search.
const SEARCH_FILE_MAX: u64 = 2 << 20;
const MATCHES_MAX: usize = 200;
const MATCH_WIDTH: usize = 240;
/// Biggest file copied into the user's data.
const COPY_MAX: u64 = 100 << 20;

pub fn serve(ctx: &mut ToolCtx, tool: &str, args: &Args) -> ToolResultOf<Step> {
    if ctx.external {
        return fail("Attached files are for the editor's own chat.");
    }
    let roots = ctx.editor.attachments.clone();
    if roots.is_empty() {
        return fail(
            "Nothing is attached to this chat. Ask the user to attach the files or folder you need.",
        );
    }
    let stop = ctx.stop.flag();
    let path = args.opt_str("path")?.map(str::to_owned);
    let job: Job = match tool {
        "list_files" => {
            let depth = args
                .opt_f64("depth")?
                .map_or(DEPTH_DEFAULT, |d| (d.max(1.0) as usize).min(DEPTH_MAX));
            Box::new(move || done(list(&roots, path.as_deref(), depth, &stop)))
        }
        "read_file" => {
            let path = args.str("path")?.to_owned();
            let offset = args.opt_f64("offset")?.map_or(1, |o| o.max(1.0) as usize);
            let limit = args
                .opt_f64("limit")?
                .map_or(LINES_DEFAULT, |l| l.max(1.0) as usize);
            Box::new(move || done(read(&roots, &path, offset, limit)))
        }
        "search_files" => {
            let pattern = regex::Regex::new(args.str("pattern")?)
                .map_err(|e| ToolError(format!("`pattern` is not a valid regex: {e}")))?;
            let glob = args.opt_str("glob")?.map(str::to_owned);
            Box::new(move || {
                done(search(
                    &roots,
                    path.as_deref(),
                    &pattern,
                    glob.as_deref(),
                    &stop,
                ))
            })
        }
        "copy_file" => {
            let path = args.str("path")?.to_owned();
            let add = args.opt_bool("add_to_pool")?.unwrap_or(false);
            let media = ctx.services.media.clone();
            Box::new(move || match copy(&roots, &path, add, media.as_deref()) {
                Ok(out) => out,
                Err(ToolError(e)) => JobDone::Done(Done::error(e)),
            })
        }
        _ => return fail(format!("unknown tool `{tool}`")),
    };
    Ok(Step::Job(job))
}

fn done(out: ToolResultOf<Done>) -> JobDone {
    JobDone::Done(out.unwrap_or_else(|ToolError(e)| Done::error(e)))
}

// ------------------------------------------------------------------ paths

/// `path` as a canonical path inside one of `roots`, with that root. A
/// relative path is taken inside the attached folder it exists in; none
/// means the only attachment.
fn resolve<'a>(
    roots: &'a [Attachment],
    path: Option<&str>,
) -> ToolResultOf<(PathBuf, &'a Attachment)> {
    let path = path.map(str::trim).filter(|p| !p.is_empty());
    let wanted = match path {
        None => match roots {
            [only] => only.path.clone(),
            _ => return fail(format!("Pass `path`: {}", attached(roots))),
        },
        Some(p) if Path::new(p).is_absolute() => PathBuf::from(p),
        Some(p) => {
            let found: Vec<_> = roots
                .iter()
                .filter(|r| r.kind == AttachmentKind::Folder)
                .map(|r| r.path.join(p))
                .filter(|c| c.exists())
                .collect();
            match found.as_slice() {
                [one] => one.clone(),
                [] => match roots.iter().find(|r| r.name() == p) {
                    Some(file) => file.path.clone(),
                    None => return fail(format!("No {p} in {}", attached(roots))),
                },
                _ => {
                    return fail(format!(
                        "{p} is in more than one attached folder; pass the full path"
                    ));
                }
            }
        }
    };
    let canonical = std::fs::canonicalize(&wanted)
        .map_err(|e| ToolError(format!("Could not open {}: {e}", wanted.display())))?;
    if let Ok(own) = std::fs::canonicalize(paths::home())
        && canonical.starts_with(own)
    {
        return fail("That is Flikk's own data, not something the user attached.");
    }
    match roots.iter().find(|r| r.covers(&canonical)) {
        Some(root) => Ok((canonical, root)),
        None => fail(format!(
            "{} is outside what the user attached: {}",
            wanted.display(),
            attached(roots)
        )),
    }
}

/// The attachments, for errors that point the agent at them.
fn attached(roots: &[Attachment]) -> String {
    roots
        .iter()
        .map(|r| r.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// `path` as shown in results: inside its folder, or whole for a file.
fn shown(path: &Path, root: &Attachment) -> String {
    match root.kind {
        AttachmentKind::Folder => path
            .strip_prefix(&root.path)
            .ok()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".into()),
        AttachmentKind::File => path.display().to_string(),
    }
}

/// Files under `dir`, as git would see them: `.gitignore` honoured,
/// hidden files and [`SKIP`] folders left out.
fn walker(dir: &Path, depth: Option<usize>, glob: Option<&str>) -> ToolResultOf<ignore::Walk> {
    let mut walk = ignore::WalkBuilder::new(dir);
    walk.max_depth(depth)
        .require_git(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|e| {
            !(e.file_type().is_some_and(|t| t.is_dir())
                && e.file_name().to_str().is_some_and(|n| SKIP.contains(&n)))
        });
    if let Some(glob) = glob {
        let mut over = ignore::overrides::OverrideBuilder::new(dir);
        over.add(glob)
            .map_err(|e| ToolError(format!("`glob` is not valid: {e}")))?;
        walk.overrides(
            over.build()
                .map_err(|e| ToolError(format!("`glob` is not valid: {e}")))?,
        );
    }
    Ok(walk.build())
}

fn stopped(stop: &AtomicBool) -> ToolResultOf<()> {
    if stop.load(Ordering::Relaxed) {
        return fail("Stopped.");
    }
    Ok(())
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b < 1 << 10 => format!("{b} B"),
        b if b < 1 << 20 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}

// ------------------------------------------------------------------ tools

fn list(
    roots: &[Attachment],
    path: Option<&str>,
    depth: usize,
    stop: &Arc<AtomicBool>,
) -> ToolResultOf<Done> {
    // With several attachments and no path, each one.
    let targets: Vec<(PathBuf, &Attachment)> = match (path, roots) {
        (None, [_, _, ..]) => roots.iter().map(|r| (r.path.clone(), r)).collect(),
        _ => vec![resolve(roots, path)?],
    };
    let mut out = String::new();
    let mut entries = 0;
    let mut more = 0;
    for (dir, root) in &targets {
        if !dir.is_dir() {
            let len = std::fs::metadata(dir).map(|m| m.len()).unwrap_or(0);
            let _ = writeln!(out, "{}  {}", dir.display(), size(len));
            entries += 1;
            continue;
        }
        let _ = writeln!(out, "{}/", dir.display());
        for entry in walker(dir, Some(depth), None)? {
            stopped(stop)?;
            let Ok(entry) = entry else { continue };
            if entry.depth() == 0 {
                continue;
            }
            if entries >= LIST_MAX {
                more += 1;
                continue;
            }
            entries += 1;
            let rel = shown(entry.path(), root);
            if entry.file_type().is_some_and(|t| t.is_dir()) {
                let _ = writeln!(out, "  {rel}/");
            } else {
                let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
                let _ = writeln!(out, "  {rel}  {}", size(len));
            }
        }
    }
    if more > 0 {
        let _ = writeln!(
            out,
            "… {more} more; list a subfolder, or lower `depth`, to see them"
        );
    }
    let summary = match targets.as_slice() {
        [(dir, root)] => shown(dir, root),
        _ => format!("{} attachments", targets.len()),
    };
    Ok(Done::text(summary, out))
}

fn read(roots: &[Attachment], path: &str, offset: usize, limit: usize) -> ToolResultOf<Done> {
    let (file, root) = resolve(roots, Some(path))?;
    let meta = std::fs::metadata(&file)?;
    if meta.is_dir() {
        return fail(format!("{} is a folder; use list_files", file.display()));
    }
    let name = shown(&file, root);
    if meta.len() > READ_MAX {
        return fail(format!(
            "{name} is {}; search it with search_files instead",
            size(meta.len())
        ));
    }
    let bytes = std::fs::read(&file)?;
    if let Some(shown_picture) = picture(&bytes, "files") {
        let p = shown_picture?;
        let text = format!(
            "{}×{} {} picture, {}.",
            p.width,
            p.height,
            p.ext,
            size(meta.len())
        );
        return Ok(Done {
            result: ToolResult {
                content: vec![Content::Text { text }, p.content()],
                is_error: false,
            },
            summary: name,
            images: vec![p.seen],
            ..Default::default()
        });
    }
    if is_binary(&bytes) {
        let kind = file
            .extension()
            .map(|e| format!(".{} ", e.to_string_lossy()))
            .unwrap_or_default();
        return fail(format!(
            "{name} is a binary {kind}file of {}, not text or a picture. copy_file brings a font or other asset into the project.",
            size(meta.len())
        ));
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let first = offset.min(total.max(1));
    let mut out = String::new();
    let mut last = first - 1;
    for (i, line) in lines.iter().enumerate().skip(first - 1).take(limit) {
        if out.len() + line.len() > TEXT_MAX {
            break;
        }
        let _ = writeln!(out, "{:>6}\t{line}", i + 1);
        last = i + 1;
    }
    if total == 0 {
        out.push_str("(empty file)\n");
    } else if last < total {
        let _ = writeln!(
            out,
            "(lines {first}–{last} of {total}; pass offset {} for more)",
            last + 1
        );
    }
    Ok(Done::text(name, out))
}

/// Text has no NUL bytes in its first few kilobytes.
fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8 << 10).any(|b| *b == 0)
}

fn search(
    roots: &[Attachment],
    path: Option<&str>,
    pattern: &regex::Regex,
    glob: Option<&str>,
    stop: &Arc<AtomicBool>,
) -> ToolResultOf<Done> {
    let targets: Vec<(PathBuf, &Attachment)> = match (path, roots) {
        (None, [_, _, ..]) => roots.iter().map(|r| (r.path.clone(), r)).collect(),
        _ => vec![resolve(roots, path)?],
    };
    let mut out = String::new();
    let mut matches = 0;
    let mut more = 0;
    let mut files = 0;
    for (dir, root) in &targets {
        for entry in walker(dir, None, glob)? {
            stopped(stop)?;
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            if entry.metadata().map_or(true, |m| m.len() > SEARCH_FILE_MAX) {
                continue;
            }
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            if is_binary(&bytes) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            let mut hit = false;
            for (i, line) in text.lines().enumerate() {
                if !pattern.is_match(line) {
                    continue;
                }
                hit = true;
                if matches >= MATCHES_MAX {
                    more += 1;
                    continue;
                }
                matches += 1;
                let line = line.trim();
                let line: String = if line.chars().count() > MATCH_WIDTH {
                    line.chars().take(MATCH_WIDTH).chain(['…']).collect()
                } else {
                    line.to_owned()
                };
                let _ = writeln!(out, "{}:{}: {line}", shown(entry.path(), root), i + 1);
            }
            files += hit as usize;
        }
    }
    if matches == 0 {
        out.push_str("No matches.\n");
    } else if more > 0 {
        let _ = writeln!(
            out,
            "… {more} more matches; narrow `pattern`, `path` or `glob`"
        );
    }
    Ok(Done::text(
        format!("{} · {matches} in {files} files", pattern.as_str()),
        out,
    ))
}

/// Copy a file out of an attachment into the user's data, so the project
/// can use it after the attachment moves; with `add`, a picture also goes
/// into the pool.
fn copy(
    roots: &[Attachment],
    path: &str,
    add: bool,
    media: Option<&dyn flikk_core::services::MediaService>,
) -> ToolResultOf<JobDone> {
    let (file, root) = resolve(roots, Some(path))?;
    let meta = std::fs::metadata(&file)?;
    if meta.is_dir() {
        return fail(format!(
            "{} is a folder; copy one file at a time",
            file.display()
        ));
    }
    if meta.len() > COPY_MAX {
        return fail(format!(
            "{} is too big to copy ({})",
            shown(&file, root),
            size(meta.len())
        ));
    }
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let picture = add
        .then(|| std::fs::read(&file).ok().and_then(|b| picture(&b, "files")))
        .flatten();
    if add && picture.is_none() {
        return fail(format!(
            "{name} is not a PNG, JPEG, GIF or WebP picture; copy it without `add_to_pool`"
        ));
    }
    let dest = place(&file, &name)?;
    let text = format!(
        "Copied to {}. Use this path, not the attachment's: in HTML as a file:// URL, or as an image parameter's value.",
        dest.display()
    );
    let mut done = Done::text(name, text);
    match picture {
        Some(Ok(p)) => {
            done.result.content.push(p.content());
            done.images.push(p.seen);
            Ok(JobDone::Fetched {
                item: Box::new(cloud::image_item(dest, media)),
                done: Box::new(done),
                bin: BIN,
            })
        }
        Some(Err(e)) => Err(e),
        None => Ok(JobDone::Done(done)),
    }
}

/// `file` copied as `name` into a folder of its own under
/// [`paths::attached`]: claimed with `create_dir`, written to a part file
/// and renamed, and removed again if anything fails.
fn place(file: &Path, name: &str) -> ToolResultOf<PathBuf> {
    let dir = loop {
        let dir = paths::attached().join(uuid::Uuid::new_v4().simple().to_string());
        match std::fs::create_dir(&dir) {
            Ok(()) => break dir,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    };
    let part = dir.join(format!(".{name}.part"));
    let dest = dir.join(name);
    let written = (|| -> std::io::Result<()> {
        let mut out = std::fs::File::create_new(&part)?;
        std::io::copy(&mut std::fs::File::open(file)?, &mut out)?;
        out.flush()?;
        out.sync_all()?;
        std::fs::rename(&part, &dest)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(ToolError(format!("Could not copy {name}: {e}")));
    }
    Ok(dest)
}
