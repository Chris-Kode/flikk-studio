//! Energy and zero-crossing voice activity detection on 16 kHz mono audio.
//!
//! Good enough for talking-head material; music beds read as speech. It
//! stands in for a voice activity model in a build without the `whisper`
//! feature, or while the model can't be had, and shapes its ranges as the
//! models do (`koo_models::voice_activity::ranges`).

use crate::ffmpeg::SAMPLE_RATE;
use koo_core::time::{Time, TimeRange};
use koo_models::voice_activity::{PAD_MS, ranges};

/// 30 ms frames.
const FRAME: usize = SAMPLE_RATE as usize * 30 / 1000;
/// Never call anything quieter than this speech.
const FLOOR_DB: f32 = -50.0;
/// Speech must sit this far above the estimated noise floor.
const ABOVE_NOISE_DB: f32 = 9.0;

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

    ranges(&voiced, FRAME, samples.len(), Time::from_millis(PAD_MS))
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
