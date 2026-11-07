//! Locating and running the `ffmpeg`/`ffprobe` binaries, filtergraph
//! escaping, and the no-overwrite file rules for outputs.

use crate::queue::{claimed, claiming};
use anyhow::{Context as _, Result, anyhow, bail};
use flikk_core::JobId;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

/// `$FLIKK_FFMPEG`, else `ffmpeg` from `PATH`.
pub fn ffmpeg() -> PathBuf {
    binary("FLIKK_FFMPEG", "ffmpeg")
}

/// `$FLIKK_FFPROBE`, else `ffprobe` from `PATH`.
pub fn ffprobe() -> PathBuf {
    binary("FLIKK_FFPROBE", "ffprobe")
}

fn binary(var: &str, name: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(name))
}

/// A command with no console window on Windows.
pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Major version of the ffmpeg binary, 0 if unknown.
fn major_version() -> u32 {
    static VERSION: OnceLock<u32> = OnceLock::new();
    *VERSION.get_or_init(|| {
        let out = command(ffmpeg()).arg("-version").output().ok();
        let text = out
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        // "ffmpeg version 6.1.1-3ubuntu5 ..." or "ffmpeg version n7.0 ..."
        text.split_whitespace()
            .nth(2)
            .map(|v| v.trim_start_matches(|c: char| !c.is_ascii_digit()))
            .and_then(|v| v.split(['.', '-']).next()?.parse().ok())
            .unwrap_or(0)
    })
}

/// How this FFmpeg maps HDR to SDR (`zscale` or the baked LUT), found once
/// per process; the media engine finds the same from the same binary.
pub fn tone_method() -> flikk_core::hdr::Method {
    static METHOD: OnceLock<flikk_core::hdr::Method> = OnceLock::new();
    *METHOD.get_or_init(|| {
        let out = command(ffmpeg())
            .args(["-hide_banner", "-filters"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        flikk_core::hdr::Method::from_filters(&out)
    })
}

/// The filters mapping an HDR item's pictures to SDR with this FFmpeg,
/// as the media engine decodes them; `None` for SDR or tone mapping off.
pub(crate) fn tone_filters(item: &flikk_core::model::MediaItem) -> Result<Option<String>> {
    let Some(mapping) = flikk_core::hdr::Mapping::of(item) else {
        return Ok(None);
    };
    let filters = mapping
        .filters(tone_method())
        .context("could not prepare the HDR tone mapping")?;
    Ok(Some(filters))
}

/// Whether this FFmpeg has the `ass` filter. Builds without libass don't,
/// Homebrew's among them.
pub(crate) fn has_ass() -> bool {
    static HAS_ASS: OnceLock<bool> = OnceLock::new();
    *HAS_ASS.get_or_init(|| {
        let out = command(ffmpeg())
            .args(["-hide_banner", "-filters"])
            .output()
            .ok();
        out.is_some_and(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|l| l.split_whitespace().nth(1) == Some("ass"))
        })
    })
}

/// Arguments that read the filtergraph from `script`, so long timelines
/// never hit command-line length limits.
pub(crate) fn filter_script_args(script: &Path) -> [std::ffi::OsString; 2] {
    let flag = if major_version() >= 7 {
        "-/filter_complex"
    } else {
        "-filter_complex_script"
    };
    [flag.into(), script.as_os_str().to_owned()]
}

/// Escape a filter option value for use inside a filtergraph description
/// (both escaping levels).
pub(crate) fn escape(value: &str) -> String {
    let mut level1 = String::with_capacity(value.len() + 8);
    for c in value.chars() {
        if matches!(c, '\\' | '\'' | ':') {
            level1.push('\\');
        }
        level1.push(c);
    }
    let mut level2 = String::with_capacity(level1.len() + 8);
    for c in level1.chars() {
        if matches!(c, '\\' | '\'' | '[' | ']' | ',' | ';') {
            level2.push('\\');
        }
        level2.push(c);
    }
    level2
}

/// A path as an escaped filter option value. Forward slashes work on
/// Windows too and avoid a layer of backslashes.
pub(crate) fn escape_path(path: &Path) -> String {
    escape(&path.to_string_lossy().replace('\\', "/"))
}

/// Width and height of the first video stream.
pub(crate) fn probe_size(path: &Path) -> Option<(u32, u32)> {
    let out = command(ffprobe())
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_entries", "stream=width,height", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut it = text.trim().split(',');
    Some((
        it.next()?.trim().parse().ok()?,
        it.next()?.trim().parse().ok()?,
    ))
}

/// Run ffmpeg with `-progress pipe:1`, reporting `out_time / total_secs`
/// in 0..1. Kills the process when `cancel` is set. Returns stderr.
pub(crate) fn run(
    args: &[std::ffi::OsString],
    total_secs: f64,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<String> {
    let mut child = Process(
        command(ffmpeg())
            .args([
                "-hide_banner",
                "-nostdin",
                "-nostats",
                "-progress",
                "pipe:1",
            ])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("could not start {}", ffmpeg().display()))?,
    );

    let mut stderr = child.stderr.take().expect("piped");
    let errors = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    });

    // Read on a thread of its own, so a cancel is seen even while ffmpeg
    // prints nothing (a stalled source or encoder).
    let stdout = child.stdout.take().expect("piped");
    let (times, timed) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Some(us) = line.strip_prefix("out_time_us=")
                && let Ok(us) = us.trim().parse::<f64>()
                && times.send(us).is_err()
            {
                break;
            }
        }
    });
    let mut canceled = false;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            canceled = true;
            break;
        }
        match timed.recv_timeout(Duration::from_millis(50)) {
            Ok(us) if total_secs > 0.0 => {
                progress((us / 1e6 / total_secs).clamp(0.0, 1.0) as f32);
            }
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait()?;
    let stderr = errors.join().unwrap_or_default();
    if canceled || cancel.load(Ordering::Relaxed) {
        bail!("canceled");
    }
    if !status.success() {
        return Err(anyhow!(last_error(&stderr)));
    }
    progress(1.0);
    Ok(stderr)
}

/// The most useful lines of an ffmpeg error log.
pub(crate) fn last_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let tail = &lines[lines.len().saturating_sub(3)..];
    if tail.is_empty() {
        "ffmpeg failed".into()
    } else {
        tail.join("\n")
    }
}

/// `path` if free, else the first free `name (2).ext`, `name (3).ext`, …
/// A name is free when no file is there and no queued or running render
/// will write it.
pub fn free_name(path: &Path) -> PathBuf {
    names(path)
        .find(|p| !p.exists() && !claimed(p, None))
        .expect("unbounded")
}

/// `path`, then `name (2).ext`, `name (3).ext`, …
fn names(path: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    std::iter::once(path.to_path_buf()).chain((2..).map(move |n| {
        let name = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        path.with_file_name(name)
    }))
}

/// Refuse to touch an existing output (PRD hard limit).
pub(crate) fn ensure_free(path: &Path) -> Result<()> {
    if path.exists() {
        bail!(
            "{} already exists; try {}",
            path.display(),
            free_name(path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
        && !dir.is_dir()
    {
        bail!("folder {} does not exist", dir.display());
    }
    Ok(())
}

/// Hidden sibling of `out` with the same extension, so the muxer is chosen
/// from it and the final rename stays on one filesystem.
fn temp_path(out: &Path, tag: &str) -> PathBuf {
    let stem = out
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "render".into());
    let name = match out.extension() {
        Some(ext) => format!(".{stem}.{tag}.part.{}", ext.to_string_lossy()),
        None => format!(".{stem}.{tag}.part"),
    };
    out.with_file_name(name)
}

/// A hidden temp file beside an output, removed when dropped: after it was
/// published, or when the work ends any other way (an error, a cancel, a
/// panic), so no partial file stays in the user's folder.
pub(crate) struct TempOutput(PathBuf);

impl TempOutput {
    pub(crate) fn new(out: &Path) -> Self {
        Self(temp_path(out, &JobId::new().to_string()))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempOutput {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// A spawned ffmpeg, killed and reaped when dropped while still running (on
/// an early return or a panic), so it never finishes a partial output on
/// its own.
pub(crate) struct Process(pub(crate) Child);

impl std::ops::Deref for Process {
    type Target = Child;

    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for Process {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // Both are no-ops once it has been waited for.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// How many names [`publish`] tries before giving up.
const PUBLISH_TRIES: usize = 1000;

/// Move a finished temp file into place without ever replacing a file:
/// when one appeared at `out` during the render, to the next free name
/// (like [`free_name`]) instead, skipping the outputs of jobs other than
/// `job` still queued or running. Returns the path written.
pub(crate) fn publish(tmp: &TempOutput, out: &Path, job: Option<JobId>) -> Result<PathBuf> {
    let _claiming = claiming();
    for target in names(out).take(PUBLISH_TRIES) {
        if claimed(&target, job) {
            continue;
        }
        if place(tmp.path(), &target).context("could not move the render into place")? {
            return Ok(target);
        }
    }
    bail!("could not find a free name for {}", out.display())
}

/// Move a finished temp file to exactly `out`, failing if a file appeared
/// there in the meantime or a queued or running render will write it.
pub(crate) fn publish_exact(tmp: &TempOutput, out: &Path) -> Result<()> {
    let _claiming = claiming();
    if claimed(out, None) {
        bail!(
            "{} is the output of a queued render; not written",
            out.display()
        );
    }
    if !place(tmp.path(), out).context("could not move the file into place")? {
        bail!("{} appeared in the meantime; not replaced", out.display());
    }
    Ok(())
}

/// Move `tmp` to `target` unless a file is there; `false` if one is.
fn place(tmp: &Path, target: &Path) -> io::Result<bool> {
    match fs::hard_link(tmp, target) {
        Ok(()) => {
            let _ = fs::remove_file(tmp);
            return Ok(true);
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        // Filesystems without hard links (FAT, exFAT, some network shares).
        Err(_) => {}
    }
    // Claim the name with an exclusive create, then rename over the claim:
    // only our own empty file is ever replaced.
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
    {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e),
    }
    fs::rename(tmp, target).inspect_err(|_| {
        if fs::metadata(target).is_ok_and(|m| m.len() == 0) {
            let _ = fs::remove_file(target);
        }
    })?;
    Ok(true)
}

/// Write `contents` to `path` through a temp file beside it, renamed into
/// place once complete: a failure never leaves a partial file, and an
/// existing file is replaced whole or not at all. Refuses the output of a
/// queued or running render.
pub(crate) fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    let _claiming = claiming();
    if claimed(path, None) {
        bail!(
            "{} is the output of a queued render; not written",
            path.display()
        );
    }
    let write = || -> io::Result<()> {
        // Through a link, like `fs::write`: replace what it points to.
        let path = match fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_symlink() => fs::canonicalize(path)?,
            _ => path.to_path_buf(),
        };
        let tmp = TempOutput::new(&path);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(tmp.path())?;
        file.write_all(contents.as_ref())?;
        file.sync_all()?;
        drop(file);
        fs::rename(tmp.path(), &path)
    };
    write().with_context(|| format!("could not write {}", path.display()))
}
