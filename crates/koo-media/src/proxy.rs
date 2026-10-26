//! Playback proxies: 720p, short-GOP H.264 + AAC, keyed by fingerprint
//! (PRD M4).

use crate::decode::Process;
use crate::{FfmpegMedia, err, tmp_sibling};
use koo_core::model::{Fingerprint, MediaItem};
use koo_core::paths;
use koo_core::services::{ServiceError, ServiceResult};
use std::io::BufRead;
use std::io::BufReader;
use std::path::PathBuf;

/// Where the proxy for a fingerprint lives, whether or not it exists yet.
pub fn proxy_path(fingerprint: &Fingerprint) -> PathBuf {
    paths::cache()
        .join("proxies")
        .join(format!("{}.mp4", fingerprint.0))
}

pub(crate) fn ensure_proxy(
    media: &FfmpegMedia,
    item: &MediaItem,
    progress: &dyn Fn(f32),
) -> ServiceResult<PathBuf> {
    let fingerprint = media.fingerprint_of(item)?;
    let out = proxy_path(&fingerprint);
    if out.is_file() {
        progress(1.0);
        return Ok(out);
    }
    let info = media.info(item)?;
    let Some(video) = &info.video else {
        return Err(ServiceError(format!(
            "{} has no video to proxy",
            item.path.display()
        )));
    };
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| err("proxy dir", e))?;
    }
    let tmp = tmp_sibling(&out);

    let rate = format!("{}/{}", video.rate.num, video.rate.den);
    // Half-second GOPs keep scrubbing snappy.
    let gop = (video.rate.fps() / 2.0).round().max(1.0).to_string();
    // Short side at most 720, keeping aspect; never upscale.
    let scale = "scale=w='if(gte(iw,ih),-2,min(720,iw))':h='if(gte(iw,ih),min(720,ih),-2)'";

    let mut cmd = media.ffmpeg();
    cmd.arg("-i")
        .arg(&item.path)
        .args(["-map", "0:v:0", "-map", "0:a?", "-vf", scale])
        .args(["-fps_mode", "cfr", "-r", &rate])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "23"])
        .args(["-tune", "fastdecode", "-pix_fmt", "yuv420p", "-bf", "0"])
        .args(["-g", &gop, "-keyint_min", &gop, "-sc_threshold", "0"])
        .args(["-c:a", "aac", "-b:a", "128k"])
        .args(["-movflags", "+faststart", "-f", "mp4"])
        .args(["-progress", "pipe:1", "-nostats", "-y"])
        .arg(&tmp);

    let total_us = info.duration.as_secs_f64() * 1e6;
    let result = (|| {
        let mut process = Process::spawn(cmd, BufReader::new)?;
        let mut line = String::new();
        loop {
            line.clear();
            if process
                .stdout
                .read_line(&mut line)
                .map_err(|e| err("proxy", e))?
                == 0
            {
                break;
            }
            if let Some(us) = line.trim().strip_prefix("out_time_us=")
                && let Ok(us) = us.parse::<f64>()
                && total_us > 0.0
            {
                progress((us / total_us).clamp(0.0, 0.99) as f32);
            }
        }
        if let Some(message) = process.finish() {
            return Err(ServiceError(format!(
                "proxy {}: {message}",
                item.path.display()
            )));
        }
        std::fs::rename(&tmp, &out).map_err(|e| err("proxy", e))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result?;
    progress(1.0);
    Ok(out)
}
