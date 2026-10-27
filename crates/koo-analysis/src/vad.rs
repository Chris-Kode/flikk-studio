//! Energy and zero-crossing voice activity detection on 16 kHz mono audio.
//!
//! Good enough for talking-head material; music beds read as speech. A model
//! VAD (Silero via whisper.cpp, or webrtc-vad) can replace [`speech_ranges`]
//! without touching the callers.

use crate::ffmpeg::SAMPLE_RATE;
use koo_core::time::{Time, TimeRange};

/// 30 ms frames.
const FRAME: usize = SAMPLE_RATE as usize * 30 / 1000;
/// Never call anything quieter than this speech.
const FLOOR_DB: f32 = -50.0;
/// Speech must sit this far above the estimated noise floor.
const ABOVE_NOISE_DB: f32 = 9.0;
/// Pauses shorter than this stay inside one speech range.
const BRIDGE_MS: i64 = 250;
/// Bursts shorter than this are clicks, not speech.
const MIN_SPEECH_MS: i64 = 120;
/// Padding around each range so word onsets and tails are not clipped.
const PAD_MS: i64 = 60;

pub fn speech_ranges(samples: &[f32]) -> Vec<TimeRange> {
    let frames: Vec<(f32, f32)> = samples.chunks(FRAME).map(frame_stats).collect();
    if frames.is_empty() {
        return Vec::new();
    }
    let mut energies: Vec<f32> = frames.iter().map(|f| f.0).collect();
    energies.sort_by(f32::total_cmp);
    let noise = energies[energies.len() / 10];
    let threshold = (noise + ABOVE_NOISE_DB).max(FLOOR_DB);

    let voiced: Vec<bool> = frames
        .iter()
        .map(|&(db, zcr)| {
            // Unvoiced consonants (s, f, sh) are quiet but cross zero often.
            db >= threshold || (db >= threshold - 6.0 && db >= FLOOR_DB && zcr > 0.25)
        })
        .collect();

    let frame_time = |i: usize| Time::from_secs_f64((i * FRAME) as f64 / SAMPLE_RATE as f64);
    let end = Time::from_secs_f64(samples.len() as f64 / SAMPLE_RATE as f64);
    let mut ranges: Vec<TimeRange> = Vec::new();
    let mut start = None;
    for (i, v) in voiced.iter().chain([&false]).enumerate() {
        match (start, *v) {
            (None, true) => start = Some(i),
            (Some(s), false) => {
                ranges.push(TimeRange::new(frame_time(s), frame_time(i).min(end)));
                start = None;
            }
            _ => {}
        }
    }

    let bridge = Time::from_millis(BRIDGE_MS);
    let mut merged: Vec<TimeRange> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start - last.end < bridge => last.end = r.end,
            _ => merged.push(r),
        }
    }
    merged.retain(|r| r.duration() >= Time::from_millis(MIN_SPEECH_MS));

    let pad = Time::from_millis(PAD_MS);
    let mut padded: Vec<TimeRange> = Vec::new();
    for r in merged {
        let r = TimeRange::new((r.start - pad).max(Time::ZERO), (r.end + pad).min(end));
        match padded.last_mut() {
            Some(last) if r.start <= last.end => last.end = r.end,
            _ => padded.push(r),
        }
    }
    padded
}

/// RMS level in dBFS and zero-crossing rate per sample.
pub(crate) fn frame_stats(frame: &[f32]) -> (f32, f32) {
    let energy = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
    let db = 10.0 * energy.max(1e-12).log10();
    let crossings = frame
        .windows(2)
        .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0))
        .count();
    (db, crossings as f32 / frame.len() as f32)
}

/// Gaps in `[0, end)` not covered by `speech`, at least `min` long.
pub fn complement(speech: &[TimeRange], end: Time, min: Time) -> Vec<TimeRange> {
    let mut gaps = Vec::new();
    let mut cursor = Time::ZERO;
    let mut sorted = speech.to_vec();
    sorted.sort_by_key(|r| r.start);
    for r in sorted.iter().chain([&TimeRange::new(end, end)]) {
        if r.start > cursor {
            gaps.push(TimeRange::new(cursor, r.start));
        }
        cursor = cursor.max(r.end);
    }
    gaps.retain(|g| g.duration() >= min && !g.is_empty());
    gaps
}
