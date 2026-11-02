//! ffprobe JSON → [`MediaInfo`].

use crate::{FfmpegMedia, command, err, stderr_tail};
use koo_core::hdr::{Hdr, Transfer};
use koo_core::model::{AudioStream, MediaInfo, MediaKind, VideoStream};
use koo_core::services::{ServiceError, ServiceResult};
use koo_core::time::{Rate, Time};
use serde_json::Value;
use std::path::Path;

pub(crate) fn probe(media: &FfmpegMedia, path: &Path) -> ServiceResult<MediaInfo> {
    let out = command(&media.ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .map_err(|e| err("ffprobe", e))?;
    if !out.status.success() {
        return Err(ServiceError(format!(
            "ffprobe {}: {}",
            path.display(),
            stderr_tail(&out.stderr)
        )));
    }
    let json: Value = serde_json::from_slice(&out.stdout).map_err(|e| err("ffprobe json", e))?;
    let mut info = parse(&json);
    // PQ's peak is in the first frame's metadata, not the stream's.
    if let Some(hdr) = info.video.as_mut().and_then(|v| v.hdr.as_mut())
        && hdr.transfer == Transfer::Pq
    {
        hdr.peak = frame_peak(media, path);
    }
    Ok(info)
}

/// MaxCLL, else the mastering display's peak, from the first video frame.
fn frame_peak(media: &FfmpegMedia, path: &Path) -> Option<f32> {
    let out = command(&media.ffprobe)
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-read_intervals", "%+#1", "-show_frames"])
        .args([
            "-show_entries",
            "frame=side_data_list",
            "-print_format",
            "json",
        ])
        .arg(path)
        .output()
        .ok()?;
    let json: Value = serde_json::from_slice(&out.stdout).ok()?;
    let side = json["frames"][0]["side_data_list"].as_array()?;
    let cll = side
        .iter()
        .filter_map(|d| d["max_content"].as_f64())
        .find(|v| *v > 0.0);
    let mastering = side
        .iter()
        .filter_map(|d| ratio(&d["max_luminance"]))
        .find(|v| *v > 0.0);
    cll.or(mastering).map(|v| v as f32)
}

/// "10000000/10000" or a number.
fn ratio(v: &Value) -> Option<f64> {
    if let Some((n, d)) = v.as_str().and_then(|s| s.split_once('/')) {
        let (n, d): (f64, f64) = (n.trim().parse().ok()?, d.trim().parse().ok()?);
        return (d != 0.0).then(|| n / d);
    }
    num_of(v)
}

/// PQ and HLG streams: their colour description and bit depth.
fn hdr(s: &Value) -> Option<Box<Hdr>> {
    let transfer = Transfer::from_ffprobe(s["color_transfer"].as_str()?)?;
    let pix_fmt = str_of(&s["pix_fmt"]);
    let bit_depth = ["p16", "p14", "p12", "p10", "p9"]
        .iter()
        .find(|p| pix_fmt.contains(*p))
        .and_then(|p| p[1..].parse().ok())
        .or_else(|| num_of(&s["bits_per_raw_sample"]).map(|b| b as u8))
        .unwrap_or(8);
    let dolby_vision = s["side_data_list"].as_array().and_then(|list| {
        list.iter()
            .find_map(|d| d["dv_profile"].as_u64())
            .map(|p| p as u8)
    });
    Some(Box::new(Hdr {
        transfer,
        primaries: str_of(&s["color_primaries"]),
        matrix: str_of(&s["color_space"]),
        range: str_of(&s["color_range"]),
        bit_depth,
        peak: None,
        dolby_vision,
    }))
}

fn parse(json: &Value) -> MediaInfo {
    let format = &json["format"];
    let streams = json["streams"].as_array().map(Vec::as_slice).unwrap_or(&[]);

    let video = streams
        .iter()
        .filter(|s| s["codec_type"] == "video")
        // Cover art in audio files is a video stream too.
        .find(|s| s["disposition"]["attached_pic"].as_i64() != Some(1))
        .map(|s| VideoStream {
            index: index(s),
            width: s["width"].as_u64().unwrap_or(0) as u32,
            height: s["height"].as_u64().unwrap_or(0) as u32,
            rate: frame_rate(s),
            codec: str_of(&s["codec_name"]),
            hdr: hdr(s),
        });

    let audio = streams
        .iter()
        .filter(|s| s["codec_type"] == "audio")
        .map(|s| AudioStream {
            index: index(s),
            channels: s["channels"].as_u64().unwrap_or(0) as u32,
            sample_rate: num_of(&s["sample_rate"]).unwrap_or(0.0) as u32,
            codec: str_of(&s["codec_name"]),
            language: s["tags"]["language"]
                .as_str()
                .filter(|l| !l.is_empty() && *l != "und")
                .map(str::to_string),
        })
        .collect();

    let duration = num_of(&format["duration"])
        .or_else(|| {
            streams
                .iter()
                .filter_map(|s| num_of(&s["duration"]))
                .reduce(f64::max)
        })
        .unwrap_or(0.0);

    MediaInfo {
        duration: Time::from_secs_f64(duration),
        video,
        audio,
        container: str_of(&format["format_name"]),
    }
}

/// What kind of pool item a probed file is: a single-frame video stream in an
/// image demuxer is an image; otherwise video wins over audio.
pub fn media_kind(info: &MediaInfo) -> Option<MediaKind> {
    let image_demuxer =
        info.container == "image2" || info.container.ends_with("_pipe") || info.container == "webp";
    match &info.video {
        Some(_) if image_demuxer || (info.duration == Time::ZERO && info.audio.is_empty()) => {
            Some(MediaKind::Image)
        }
        Some(_) => Some(MediaKind::Video),
        None if !info.audio.is_empty() => Some(MediaKind::Audio),
        None => None,
    }
}

fn index(s: &Value) -> u32 {
    s["index"].as_u64().unwrap_or(0) as u32
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// ffprobe writes most numbers as strings.
fn num_of(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .filter(|n: &f64| n.is_finite())
}

/// `r_frame_rate` is the exact rate for constant-rate media; variable-rate
/// files can report a timebase there instead, so fall back to the average.
/// Rates within 0.1% of a standard one snap to it.
fn frame_rate(s: &Value) -> Rate {
    let plausible = |r: &Rate| (1.0..=240.0).contains(&r.fps());
    let r = parse_rate(&s["r_frame_rate"]).filter(plausible);
    let avg = parse_rate(&s["avg_frame_rate"]).filter(plausible);
    let rate = match (r, avg) {
        // Far apart means r_frame_rate is a timebase multiple, not the rate.
        (Some(r), Some(a)) if (r.fps() / a.fps() - 1.0).abs() > 0.05 => a,
        (Some(r), _) => r,
        (None, Some(a)) => a,
        (None, None) => Rate::default(),
    };
    const STANDARD: [(u32, u32); 10] = [
        (24000, 1001),
        (24, 1),
        (25, 1),
        (30000, 1001),
        (30, 1),
        (48, 1),
        (50, 1),
        (60000, 1001),
        (60, 1),
        (120, 1),
    ];
    // Closest, not first: 30000/1001 is itself within 0.1% of 30.
    let off = |std: &Rate| (std.fps() / rate.fps() - 1.0).abs();
    STANDARD
        .iter()
        .map(|&(num, den)| Rate { num, den })
        .filter(|std| off(std) < 0.001)
        .min_by(|a, b| off(a).total_cmp(&off(b)))
        .unwrap_or(rate)
}

fn parse_rate(v: &Value) -> Option<Rate> {
    let (num, den) = v.as_str()?.split_once('/')?;
    let (num, den): (u64, u64) = (num.trim().parse().ok()?, den.trim().parse().ok()?);
    if num == 0 || den == 0 {
        return None;
    }
    let g = gcd(num, den);
    let (num, den) = (num / g, den / g);
    Some(Rate {
        num: u32::try_from(num).ok()?,
        den: u32::try_from(den).ok()?,
    })
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}
