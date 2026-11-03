//! Energy and zero-crossing voice activity detection on 16 kHz mono audio.
//!
//! Good enough for talking-head material; music beds read as speech. It
//! stands in for a voice activity model in a build without the `whisper`
//! feature, or while the model can't be had, and shapes its ranges as the
//! models do (`flikk_models::voice_activity::ranges`).

use crate::ffmpeg::SAMPLE_RATE;
use flikk_core::analysis::Word;
use flikk_core::time::{FLICKS_PER_SECOND, Time, TimeRange};
use flikk_models::voice_activity::{PAD_MS, ranges};

/// 30 ms frames.
const FRAME: usize = SAMPLE_RATE as usize * 30 / 1000;
/// Never call anything quieter than this speech.
const FLOOR_DB: f32 = -50.0;
/// Speech must sit this far above the estimated noise floor.
const ABOVE_NOISE_DB: f32 = 9.0;
/// A hole in a transcript: this much speech (600 ms)…
const MIN_HOLE: Time = Time(FLICKS_PER_SECOND * 3 / 5);
/// …that no word comes within this of (250 ms).
const HOLE_REACH: Time = Time(FLICKS_PER_SECOND / 4);
/// Speech from words: pauses shorter than this stay inside it (250 ms)…
const WORDS_PAUSE: Time = Time(FLICKS_PER_SECOND / 4);
/// …and it starts this far before its first word, as Silero's do.
const WORDS_LEAD: Time = Time(FLICKS_PER_SECOND * PAD_MS / 1000);

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

/// Where someone talks, from the words a provider heard: words less than a
/// short pause apart are one range, each starting a little before its
/// first word, as Silero's ranges are.
pub fn from_words(words: &[Word]) -> Vec<TimeRange> {
    let mut ranges: Vec<TimeRange> = Vec::new();
    for w in words {
        let start = (w.range.start - WORDS_LEAD).max(Time::ZERO);
        match ranges.last_mut() {
            Some(last) if w.range.start <= last.end + WORDS_PAUSE => {
                last.end = last.end.max(w.range.end)
            }
            _ => ranges.push(TimeRange::new(start, w.range.end)),
        }
    }
    ranges
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

/// The holes `words` leave in `speech`: at least [`MIN_HOLE`] of it that no
/// word comes within [`HOLE_REACH`] of, widened by that much so that each
/// reaches the words either side.
pub fn holes(speech: &[TimeRange], words: &[Word]) -> Vec<TimeRange> {
    let mut near: Vec<TimeRange> = words
        .iter()
        .map(|w| TimeRange::new(w.range.start - HOLE_REACH, w.range.end + HOLE_REACH))
        .collect();
    near.sort_by_key(|r| r.start);
    let mut holes: Vec<TimeRange> = Vec::new();
    for s in speech {
        let mut from = s.start;
        let end = TimeRange::new(s.end, s.end);
        for r in near.iter().filter(|r| r.overlaps(s)).chain([&end]) {
            if r.start - from >= MIN_HOLE {
                let hole =
                    TimeRange::new((from - HOLE_REACH).max(Time::ZERO), r.start + HOLE_REACH);
                match holes.last_mut() {
                    Some(last) if hole.start <= last.end => last.end = hole.end,
                    _ => holes.push(hole),
                }
            }
            from = from.max(r.end);
        }
    }
    holes
}
