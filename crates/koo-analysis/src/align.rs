//! Word edges measured on the audio.
//!
//! Whisper's word times are estimates, and its own refinement keeps only
//! the loud core of each word (it snaps to half the local mean energy), so
//! starts come late and ends early: a cut placed on them clips the word.
//! Here each pair of neighbouring words is measured again on 10 ms levels
//! against the local noise floor. Their edges move to the pause between
//! them or, where they run together, meet at the quietest frame.

use crate::ffmpeg::SAMPLE_RATE;
use crate::vad::frame_stats;
use koo_core::analysis::{Levels, Word};
use koo_core::time::{Time, TimeRange};

/// 10 ms frames, as [`Levels`].
const FRAME: usize = SAMPLE_RATE as usize / 100;
/// Sound sits this far above the local noise floor…
const ABOVE_NOISE_DB: f32 = 9.0;
/// …and never below this.
const MIN_THRESHOLD_DB: f32 = -60.0;
/// Unvoiced consonants (s, f, sh) are quiet but cross zero often; they
/// count as sound this far below the threshold.
const FRICATIVE_DB: f32 = 5.0;
const FRICATIVE_ZCR: f32 = 0.3;
/// The noise floor is this quantile of the frames within `FLOOR_SPAN`
/// either side (10 s), taken every `FLOOR_STEP` frames (1 s).
const FLOOR_QUANTILE: f32 = 0.1;
const FLOOR_SPAN: usize = 1000;
const FLOOR_STEP: usize = 100;
/// Shortest pause between words: 30 ms.
const MIN_PAUSE: usize = 3;
/// Farthest an edge moves from Whisper's: 0.5 s.
const REACH: usize = 50;
/// Where words run together, how far from Whisper's boundary to look for
/// the quietest frame: 60 ms.
const TIGHT_REACH: usize = 6;

/// Levels of 16 kHz mono `samples`, and which of their frames hold sound.
pub fn measure(samples: &[f32]) -> (Levels, Vec<bool>) {
    let stats: Vec<(f32, f32)> = samples.chunks(FRAME).map(frame_stats).collect();
    let db: Vec<f32> = stats.iter().map(|s| s.0).collect();
    let floor = noise_floor(&db);
    let sound = stats
        .iter()
        .enumerate()
        .map(|(i, &(db, zcr))| {
            let threshold = (floor[i / FLOOR_STEP] + ABOVE_NOISE_DB).max(MIN_THRESHOLD_DB);
            db >= threshold || (db >= threshold - FRICATIVE_DB && zcr > FRICATIVE_ZCR)
        })
        .collect();
    (Levels::from_db(db), sound)
}

/// One floor per `FLOOR_STEP` frames.
fn noise_floor(db: &[f32]) -> Vec<f32> {
    (0..db.len().div_ceil(FLOOR_STEP))
        .map(|block| {
            let centre = block * FLOOR_STEP + FLOOR_STEP / 2;
            let around = centre.saturating_sub(FLOOR_SPAN)..(centre + FLOOR_SPAN).min(db.len());
            let mut around = db[around].to_vec();
            let k = ((around.len() as f32 * FLOOR_QUANTILE) as usize).min(around.len() - 1);
            *around.select_nth_unstable_by(k, f32::total_cmp).1
        })
        .collect()
}

/// Move each word's edges onto the audio; `levels` and `sound` come from
/// [`measure`] on the same audio.
pub fn refine(words: &mut [Word], levels: &Levels, sound: &[bool]) {
    if sound.is_empty() || levels.len() != sound.len() {
        return;
    }
    for k in 0..=words.len() {
        junction(words, k, levels, sound);
    }
}

/// The edges between `words[k - 1]` and `words[k]`; either may be missing
/// at the ends.
fn junction(words: &mut [Word], k: usize, levels: &Levels, sound: &[bool]) {
    let n = sound.len();
    let frame = |t: Time| t.0.div_euclid(Levels::FRAME.0).clamp(0, n as i64) as usize;
    let mid = |r: TimeRange| frame(Time(r.start.0 + (r.end.0 - r.start.0) / 2));
    let prev = k.checked_sub(1).map(|i| words[i].range);
    let next = words.get(k).map(|w| w.range);
    // Whisper's end of one word and start of the next, often the same.
    let (gap_lo, gap_hi) = match (prev, next) {
        (Some(p), Some(q)) => (
            frame(p.end).min(frame(q.start)),
            frame(p.end).max(frame(q.start)),
        ),
        (Some(p), None) => (frame(p.end), frame(p.end)),
        (None, Some(q)) => (frame(q.start), frame(q.start)),
        (None, None) => return,
    };
    // Never past the middle of either word.
    let lo = prev
        .map_or(0, |p| mid(p) + 1)
        .max(gap_lo.saturating_sub(REACH));
    let hi = next.map_or(n, mid).min(gap_hi + REACH).min(n);
    if lo + MIN_PAUSE > hi {
        return;
    }
    let pauses = pauses(&sound[lo..hi], lo);
    // A gap this short is not evidence of a pause.
    let whisper_gap = gap_hi - gap_lo > 2 * TIGHT_REACH;
    let found = if whisper_gap {
        // Whisper heard nothing between the words: every pause inside that
        // gap belongs to it, and sound there (a breath, a cough) to neither.
        let inside: Vec<_> = pauses
            .iter()
            .filter(|&&(s, e)| s < gap_hi && e > gap_lo)
            .collect();
        match (inside.first(), inside.last()) {
            (Some(first), Some(last)) => Some((first.0, last.1)),
            // Sound all through the gap (music, noise): keep Whisper's.
            _ => return,
        }
    } else {
        // The pause nearest Whisper's boundary, if it is longer than it
        // is far away.
        let distance =
            |&(s, e): &(usize, usize)| gap_lo.saturating_sub(e).max(s.saturating_sub(gap_hi));
        pauses
            .iter()
            .filter(|p| p.1 - p.0 > distance(p))
            .max_by_key(|p| (p.1 - p.0) - distance(p))
            .copied()
    };
    let pause = match found {
        // The whole pause, past where the search stopped, but never past
        // the far edge of either word.
        Some((mut s, mut e)) => {
            let floor = prev.map_or(0, |p| frame(p.start) + 1);
            let ceil = next.map_or(n, |q| frame(q.end).saturating_sub(1));
            while s > floor && !sound[s - 1] {
                s -= 1;
            }
            while e < ceil && !sound[e] {
                e += 1;
            }
            (s, e)
        }
        // The words run together: they meet at the quietest frame near
        // Whisper's boundary.
        None if prev.is_some() && next.is_some() => {
            let from = lo.max(gap_lo.saturating_sub(TIGHT_REACH));
            let to = hi.min(gap_hi + TIGHT_REACH + 1);
            let q = (from..to)
                .min_by(|&i, &j| levels.db(i).total_cmp(&levels.db(j)))
                .unwrap_or(gap_lo)
                .clamp(lo, hi);
            (q, q)
        }
        None => return,
    };
    let at = |f: usize| Time(f as i64 * Levels::FRAME.0);
    if let Some(i) = k.checked_sub(1) {
        let r = &mut words[i].range;
        r.end = at(pause.0).max(r.start + Levels::FRAME);
    }
    if let Some(w) = words.get_mut(k) {
        let r = &mut w.range;
        r.start = at(pause.1).min(r.end - Levels::FRAME);
    }
}

/// Runs of at least [`MIN_PAUSE`] frames without sound, as frame ranges
/// offset by `offset`.
fn pauses(sound: &[bool], offset: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, &s) in sound.iter().chain([&true]).enumerate() {
        match (start, s) {
            (None, false) => start = Some(i),
            (Some(from), true) => {
                if i - from >= MIN_PAUSE {
                    out.push((offset + from, offset + i));
                }
                start = None;
            }
            _ => {}
        }
    }
    out
}
