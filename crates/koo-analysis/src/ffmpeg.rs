//! Thin wrappers around the `ffmpeg` binary: audio extraction, scene and
//! silence detection, EBU R128 loudness. Each run reports progress from
//! `-progress pipe:2` against the known duration.

use anyhow::{Context, Result, bail};
use koo_core::analysis::Loudness;
use koo_core::time::{Time, TimeRange};
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Sample rate Whisper and the VAD work at.
pub const SAMPLE_RATE: u32 = 16_000;

/// `$KOOEDITOR_FFMPEG`, else `ffmpeg` from `PATH`.
pub fn binary() -> PathBuf {
    std::env::var_os("KOOEDITOR_FFMPEG")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

/// What a run printed besides progress and its exit.
struct Output {
    log: Vec<String>,
}

/// Run ffmpeg with `args`, feeding stdout to `stdout` and calling `progress`
/// with 0..1 when `duration` is known.
fn run(
    args: &[OsString],
    duration: Option<Time>,
    progress: &(dyn Fn(f32) + Sync),
    stdout: impl FnOnce(&mut dyn Read) -> Result<()>,
) -> Result<Output> {
    let bin = binary();
    let mut child = Command::new(&bin)
        .args([
            "-nostdin",
            "-hide_banner",
            "-nostats",
            "-progress",
            "pipe:2",
        ])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not run {} (set KOOEDITOR_FFMPEG)", bin.display()))?;
    let mut out = child.stdout.take().expect("piped stdout");
    let err = child.stderr.take().expect("piped stderr");
    let total = duration.map(|d| d.as_secs_f64()).filter(|d| *d > 0.0);

    let (read, log) = std::thread::scope(|s| {
        let log = s.spawn(move || {
            let mut log = Vec::new();
            for line in BufReader::new(err).lines().map_while(|l| l.ok()) {
                if let Some(us) = line.strip_prefix("out_time_us=") {
                    if let (Some(total), Ok(us)) = (total, us.trim().parse::<f64>()) {
                        progress((us / 1e6 / total).clamp(0.0, 1.0) as f32);
                    }
                } else if !is_progress_line(&line) {
                    log.push(line);
                }
            }
            log
        });
        let read = stdout(&mut out);
        // Drain whatever is left so ffmpeg never blocks on a full pipe.
        let _ = std::io::copy(&mut out, &mut std::io::sink());
        (read, log.join().unwrap_or_default())
    });
    let status = child.wait()?;
    if !status.success() {
        let tail: Vec<_> = log.iter().rev().take(4).rev().cloned().collect();
        bail!("ffmpeg failed: {}", tail.join(" | "));
    }
    read?;
    Ok(Output { log })
}

fn is_progress_line(line: &str) -> bool {
    const KEYS: &[&str] = &[
        "frame=",
        "fps=",
        "stream_",
        "bitrate=",
        "total_size=",
        "out_time",
        "dup_frames=",
        "drop_frames=",
        "speed=",
        "progress=",
    ];
    KEYS.iter().any(|k| line.starts_with(k))
}

fn discard(r: &mut dyn Read) -> Result<()> {
    std::io::copy(r, &mut std::io::sink())?;
    Ok(())
}

fn input(path: &Path) -> [OsString; 2] {
    ["-i".into(), path.as_os_str().to_owned()]
}

fn strs<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(items: I) -> Vec<OsString> {
    items.into_iter().map(|s| s.as_ref().to_owned()).collect()
}

/// True when ffmpeg failed because the requested stream does not exist.
pub fn is_missing_stream(e: &anyhow::Error) -> bool {
    let msg = e.to_string();
    msg.contains("matches no streams") || msg.contains("does not contain any stream")
}

/// The `n`th audio stream (0 = first) as 16 kHz mono `f32`.
pub fn extract_audio(
    path: &Path,
    n: u32,
    duration: Option<Time>,
    progress: &(dyn Fn(f32) + Sync),
) -> Result<Vec<f32>> {
    let mut args = strs(["-v", "error"]);
    args.extend(input(path));
    args.extend(strs([
        "-map".to_string(),
        format!("0:a:{n}"),
        "-vn".into(),
        "-ac".into(),
        "1".into(),
        "-ar".into(),
        SAMPLE_RATE.to_string(),
        "-f".into(),
        "f32le".into(),
        "-".into(),
    ]));
    let mut bytes = Vec::new();
    run(&args, duration, progress, |r| {
        r.read_to_end(&mut bytes)?;
        Ok(())
    })?;
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect())
}

/// Source times where the picture cuts, from ffmpeg's scene score.
pub fn shot_changes(
    path: &Path,
    threshold: f32,
    duration: Option<Time>,
    progress: &(dyn Fn(f32) + Sync),
) -> Result<Vec<Time>> {
    let mut args = strs(["-v", "info"]);
    args.extend(input(path));
    args.extend(strs([
        "-map".to_string(),
        "0:v:0".into(),
        "-an".into(),
        "-sn".into(),
        "-vf".into(),
        // Scoring a small picture is much faster and finds the same cuts.
        format!("scale=320:-2,select='gt(scene,{threshold})',showinfo"),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]));
    let out = run(&args, duration, progress, discard)?;
    let mut times: Vec<Time> = out
        .log
        .iter()
        .filter(|l| l.contains("Parsed_showinfo"))
        .filter_map(|l| field(l, "pts_time:"))
        .map(Time::from_secs_f64)
        .collect();
    times.sort();
    times.dedup();
    Ok(times)
}

/// Silent gaps of at least `min`, from `silencedetect` on the `n`th audio
/// stream. Uses the duration ffmpeg reports when `duration` is unknown.
pub fn silences(
    path: &Path,
    n: u32,
    min: Time,
    duration: Option<Time>,
    progress: &(dyn Fn(f32) + Sync),
) -> Result<Vec<TimeRange>> {
    let mut args = strs(["-v", "info"]);
    args.extend(input(path));
    args.extend(strs([
        "-map".to_string(),
        format!("0:a:{n}"),
        "-vn".into(),
        "-ac".into(),
        "1".into(),
        "-af".into(),
        format!("silencedetect=n=-40dB:d={:.3}", min.as_secs_f64().max(0.01)),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]));
    let out = run(&args, duration, progress, discard)?;
    let end = duration
        .or_else(|| out.log.iter().find_map(|l| parse_duration(l)))
        .unwrap_or(Time::ZERO);
    let mut gaps = Vec::new();
    let mut open: Option<Time> = None;
    for line in &out.log {
        if let Some(s) = field(line, "silence_start:") {
            open = Some(Time::from_secs_f64(s.max(0.0)));
        } else if let Some(e) = field(line, "silence_end:") {
            let start = open.take().unwrap_or(Time::ZERO);
            gaps.push(TimeRange::new(start, Time::from_secs_f64(e)));
        }
    }
    if let Some(start) = open {
        gaps.push(TimeRange::new(start, end.max(start)));
    }
    gaps.retain(|g| g.duration() >= min);
    Ok(gaps)
}

/// Integrated loudness, true peak and loudness range of the first audio
/// stream, over `range` when given (PRD A2).
pub fn loudness(path: &Path, range: Option<TimeRange>) -> Result<Loudness> {
    let mut args = strs(["-v", "info"]);
    if let Some(r) = range {
        args.extend(strs([
            "-ss".to_string(),
            format!("{:.6}", r.start.as_secs_f64()),
            "-t".into(),
            format!("{:.6}", r.duration().as_secs_f64().max(0.0)),
        ]));
    }
    args.extend(input(path));
    args.extend(strs([
        "-map",
        "0:a:0",
        "-vn",
        "-af",
        "ebur128=peak=true",
        "-f",
        "null",
        "-",
    ]));
    let out = run(&args, None, &|_| {}, discard)?;
    let summary = out
        .log
        .iter()
        .position(|l| l.contains("Summary:"))
        .context("ebur128 printed no summary")?;
    let mut lufs = None;
    let mut lra = None;
    let mut peak = None;
    for line in &out.log[summary..] {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("I:") {
            lufs = first_number(v);
        } else if let Some(v) = line.strip_prefix("LRA:") {
            lra = first_number(v);
        } else if let Some(v) = line.strip_prefix("Peak:") {
            peak = first_number(v);
        }
    }
    // Digital silence reports -inf; keep the values finite for JSON.
    let finite = |v: f32| if v.is_finite() { v } else { -144.0 };
    Ok(Loudness {
        integrated_lufs: finite(lufs.context("no integrated loudness in ebur128 summary")?),
        true_peak_db: finite(peak.context("no true peak in ebur128 summary")?),
        range_lu: lra.unwrap_or(0.0),
    })
}

fn first_number(s: &str) -> Option<f32> {
    s.split_whitespace().next()?.parse().ok()
}

/// The number after `key` in a log line, e.g. `pts_time:12.3`.
fn field(line: &str, key: &str) -> Option<f64> {
    let rest = line[line.find(key)? + key.len()..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | 'e' | '+')))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// `  Duration: 00:01:00.04, start: …` from the input banner.
fn parse_duration(line: &str) -> Option<Time> {
    let rest = line.trim().strip_prefix("Duration:")?.trim();
    Time::parse_clock(rest.split(',').next()?)
}
