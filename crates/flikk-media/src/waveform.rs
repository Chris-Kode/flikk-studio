//! Waveform peaks: one full decode per source and stream, stored as a
//! downsampled peak file, so drawing at any zoom just reads it back.

use crate::decode::{Process, audio_map, pcm_output};
use crate::{FfmpegMedia, err, write_atomic};
use flikk_core::model::{Fingerprint, MediaItem};
use flikk_core::paths;
use flikk_core::services::{ServiceError, ServiceResult};
use flikk_core::time::TimeRange;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

/// Peaks per second of audio in the stored file (5 ms each).
const PEAKS_PER_SECOND: u32 = 200;
const DECODE_RATE: u32 = 16_000;
const MAGIC: &[u8; 8] = b"FLKPEAK1";

/// Max absolute sample per 1/200 s, quantized to 0..=255.
pub(crate) struct Peaks {
    per_second: u32,
    values: Vec<u8>,
}

pub(crate) fn waveform(
    media: &FfmpegMedia,
    item: &MediaItem,
    stream: u32,
    range: TimeRange,
    buckets: usize,
) -> ServiceResult<Vec<f32>> {
    if buckets == 0 {
        return Ok(Vec::new());
    }
    let peaks = peaks(media, item, stream)?;
    let rate = peaks.per_second as f64;
    let start = range.start.as_secs_f64() * rate;
    let end = range.end.as_secs_f64() * rate;
    let step = (end - start) / buckets as f64;
    let n = peaks.values.len() as i64;
    Ok((0..buckets)
        .map(|b| {
            let a = (start + step * b as f64).floor() as i64;
            let z = ((start + step * (b + 1) as f64).ceil() as i64).max(a + 1);
            let (a, z) = (a.clamp(0, n), z.clamp(0, n));
            let peak = peaks.values[a as usize..z as usize]
                .iter()
                .copied()
                .max()
                .unwrap_or(0);
            peak as f32 / 255.0
        })
        .collect())
}

fn peaks(media: &FfmpegMedia, item: &MediaItem, stream: u32) -> ServiceResult<Arc<Peaks>> {
    let fingerprint = media.fingerprint_of(item)?;
    let key = (fingerprint.clone(), stream);
    if let Some(p) = media.peaks.lock().unwrap().get(&key) {
        return Ok(p.clone());
    }
    let file = peak_path(&fingerprint, stream);
    let peaks = match std::fs::read(&file).ok().and_then(|b| parse(&b)) {
        Some(p) => p,
        None => {
            let p = decode(media, item, stream)?;
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(|e| err("waveform dir", e))?;
            }
            write_atomic(&file, &serialize(&p)).map_err(|e| err("waveform", e))?;
            p
        }
    };
    let peaks = Arc::new(peaks);
    media.peaks.lock().unwrap().insert(key, peaks.clone());
    Ok(peaks)
}

fn peak_path(fingerprint: &Fingerprint, stream: u32) -> PathBuf {
    paths::cache()
        .join("waveforms")
        .join(format!("{}-{stream}.peaks", fingerprint.0))
}

fn decode(media: &FfmpegMedia, item: &MediaItem, stream: u32) -> ServiceResult<Peaks> {
    let map = audio_map(media, item, stream)?;
    let mut cmd = media.ffmpeg();
    cmd.arg("-i").arg(&item.path);
    pcm_output(&mut cmd, &map, DECODE_RATE);
    let mut process = Process::spawn(cmd, |out| out)?;

    // Stereo f32 frames per peak.
    let block = (DECODE_RATE / PEAKS_PER_SECOND) as usize * 2;
    let mut values = Vec::new();
    let (mut peak, mut in_block) = (0f32, 0usize);
    let mut buf = vec![0u8; 1 << 16];
    let mut carry = 0;
    loop {
        let n = process
            .stdout
            .read(&mut buf[carry..])
            .map_err(|e| err("waveform", e))?;
        if n == 0 {
            break;
        }
        let have = carry + n;
        let whole = have / 4 * 4;
        for b in buf[..whole].as_chunks::<4>().0 {
            peak = peak.max(f32::from_le_bytes(*b).abs());
            in_block += 1;
            if in_block == block {
                values.push(quantize(peak));
                (peak, in_block) = (0.0, 0);
            }
        }
        buf.copy_within(whole..have, 0);
        carry = have - whole;
    }
    if in_block > 0 {
        values.push(quantize(peak));
    }
    if let Some(message) = process.finish() {
        return Err(ServiceError(format!(
            "waveform {}: {message}",
            item.path.display()
        )));
    }
    Ok(Peaks {
        per_second: PEAKS_PER_SECOND,
        values,
    })
}

fn quantize(peak: f32) -> u8 {
    (peak.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn serialize(p: &Peaks) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + p.values.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&p.per_second.to_le_bytes());
    out.extend_from_slice(&(p.values.len() as u32).to_le_bytes());
    out.extend_from_slice(&p.values);
    out
}

fn parse(bytes: &[u8]) -> Option<Peaks> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let per_second = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?);
    let len = u32::from_le_bytes(rest.get(4..8)?.try_into().ok()?) as usize;
    let values = rest.get(8..8 + len)?.to_vec();
    (per_second > 0).then_some(Peaks { per_second, values })
}
