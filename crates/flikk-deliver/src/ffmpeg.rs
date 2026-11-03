//! Locating and running the `ffmpeg`/`ffprobe` binaries, filtergraph
//! escaping, and the no-overwrite file rules for outputs.

use anyhow::{Context as _, Result, anyhow, bail};
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

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
    let mut child = command(ffmpeg())
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
        .with_context(|| format!("could not start {}", ffmpeg().display()))?;

    let mut stderr = child.stderr.take().expect("piped");
    let errors = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    });

    let stdout = child.stdout.take().expect("piped");
    let mut canceled = false;
    for line in BufReader::new(stdout).lines() {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            canceled = true;
            break;
        }
        let Ok(line) = line else { break };
        if let Some(us) = line.strip_prefix("out_time_us=")
            && let Ok(us) = us.trim().parse::<f64>()
            && total_secs > 0.0
        {
            progress((us / 1e6 / total_secs).clamp(0.0, 1.0) as f32);
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
pub fn free_name(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    (2..)
        .map(|n| {
            let name = match &ext {
                Some(ext) => format!("{stem} ({n}).{ext}"),
                None => format!("{stem} ({n})"),
            };
            path.with_file_name(name)
        })
        .find(|p| !p.exists())
        .expect("unbounded")
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
pub(crate) fn temp_path(out: &Path, tag: &str) -> PathBuf {
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

/// Move a finished temp file into place without ever replacing a file that
/// appeared in the meantime.
pub(crate) fn publish(tmp: &Path, out: &Path) -> Result<()> {
    match std::fs::hard_link(tmp, out) {
        Ok(()) => {
            let _ = std::fs::remove_file(tmp);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(tmp);
            bail!("{} appeared during the render; not replaced", out.display())
        }
        // Filesystems without hard links (FAT, exFAT, some network shares).
        Err(_) => {
            if out.exists() {
                let _ = std::fs::remove_file(tmp);
                bail!("{} appeared during the render; not replaced", out.display());
            }
            std::fs::rename(tmp, out).context("could not move the render into place")
        }
    }
}
