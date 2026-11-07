//! Playback proxies: 720p, short-GOP H.264 + AAC, keyed by fingerprint
//! (PRD M4).

use crate::decode::{Process, tone_then};
use crate::{FfmpegMedia, err, tmp_sibling};
use flikk_core::hdr::Mapping;
use flikk_core::model::{Fingerprint, MediaItem};
use flikk_core::paths;
use flikk_core::services::{ServiceError, ServiceResult};
use flikk_deliver::hw::{self, Codec};
use std::io::BufRead;
use std::io::BufReader;
use std::path::PathBuf;
use std::process::Command;

/// Where the proxy for a fingerprint lives, whether or not it exists yet.
pub fn proxy_path(fingerprint: &Fingerprint) -> PathBuf {
    proxy_path_mapped(fingerprint, None)
}

/// Where the item's proxy lives for its tone mapping: an HDR item's proxy
/// is made mapped to SDR, so each mapping has its own file.
pub fn proxy_path_for(item: &MediaItem) -> Option<PathBuf> {
    let fp = item.fingerprint.as_ref()?;
    Some(proxy_path_mapped(fp, Mapping::of(item).as_ref()))
}

pub(crate) fn proxy_path_mapped(fingerprint: &Fingerprint, mapping: Option<&Mapping>) -> PathBuf {
    let name = match mapping {
        Some(m) => format!("{}-{}.mp4", fingerprint.0, m.key()),
        None => format!("{}.mp4", fingerprint.0),
    };
    paths::cache().join("proxies").join(name)
}

pub(crate) fn ensure_proxy(
    media: &FfmpegMedia,
    item: &MediaItem,
    progress: &dyn Fn(f32),
) -> ServiceResult<PathBuf> {
    let fingerprint = media.fingerprint_of(item)?;
    let mapping = Mapping::of(item);
    let out = proxy_path_mapped(&fingerprint, mapping.as_ref());
    if out.is_file() {
        flikk_core::cache_sweep::mark_used(&out);
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
    let filters = tone_then(
        media.tone_filters(mapping.as_ref())?.as_deref(),
        scale.into(),
    );

    // The GPU first (VideoToolbox, NVENC, Quick Sync, AMF or VA-API, as
    // the probe confirmed), all in software when that run fails.
    let encoder = hw::probe().best(Codec::H264).cloned();
    let (width, height) = {
        let short = video.width.min(video.height).max(1) as f64;
        let k = (720.0 / short).min(1.0);
        (
            (video.width as f64 * k) as u32,
            (video.height as f64 * k) as u32,
        )
    };
    let command = |gpu: bool| {
        let encoder = encoder.as_ref().filter(|_| gpu);
        let mut cmd = media.ffmpeg();
        if let Some(encoder) = encoder {
            cmd.args(encoder.global_args());
        }
        if gpu {
            cmd.args(["-hwaccel", "auto"]);
        }
        let filters = match encoder.and_then(|e| e.upload_filter()) {
            Some(upload) => format!("{filters},{upload}"),
            None => filters.clone(),
        };
        cmd.arg("-i")
            .arg(&item.path)
            .args(["-map", "0:v:0", "-map", "0:a?", "-vf", &filters])
            .args(["-fps_mode", "cfr", "-r", &rate]);
        // Only ever watched in the viewer, mostly smaller than 720p.
        match encoder {
            Some(encoder) => {
                cmd.args(encoder.codec_args(26, width, height, video.rate.fps()));
            }
            None => {
                cmd.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "26"])
                    .args(["-tune", "fastdecode", "-pix_fmt", "yuv420p"])
                    .args(["-keyint_min", &gop, "-sc_threshold", "0"]);
            }
        }
        cmd.args(["-bf", "0", "-g", &gop])
            .args(["-color_primaries", "bt709", "-color_trc", "bt709"])
            .args(["-colorspace", "bt709"])
            .args(["-c:a", "aac", "-b:a", "128k"])
            .args(["-movflags", "+faststart", "-f", "mp4"])
            .args(["-progress", "pipe:1", "-nostats", "-y"])
            .arg(&tmp);
        cmd
    };

    let total_us = info.duration.as_secs_f64() * 1e6;
    let run = |cmd: Command| -> ServiceResult<()> {
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
        match process.finish() {
            Some(message) => Err(ServiceError(format!(
                "proxy {}: {message}",
                item.path.display()
            ))),
            None => Ok(()),
        }
    };
    // Only an encode that failed is tried again in software: a rename that
    // failed would fail the same way after it.
    let result = run(command(true))
        .or_else(|_| {
            let _ = std::fs::remove_file(&tmp);
            progress(0.0);
            run(command(false))
        })
        .and_then(|()| {
            std::fs::rename(&tmp, &out).or_else(|e| {
                // Another build of the same file got there first (on
                // Windows, a proxy being played can't be replaced).
                if out.is_file() {
                    let _ = std::fs::remove_file(&tmp);
                    Ok(())
                } else {
                    Err(err("proxy", e))
                }
            })
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result?;
    progress(1.0);
    Ok(out)
}
