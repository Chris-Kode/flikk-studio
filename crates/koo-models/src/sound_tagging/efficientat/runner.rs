//! The tagger run on batches of windows, each followed by silence, and
//! its scores read into sounds and music.

use super::config::{
    BATCH, CLASSES, FILE, INPUT, MUSIC_GAP, MUSIC_MIN, MUSIC_OFF, MUSIC_ON, SOUND_GAP, SOUND_OFF,
    SOUND_ON, STEP, STEP_TIME, WINDOW, WINDOW_TIME,
};
use super::groups::{GROUPS, Kind};
use crate::sound_tagging::{Scores, Tagger};
use crate::{SOUND_RATE, Windows, onnx};
use anyhow::{Result, anyhow, bail};
use koo_core::analysis::{Levels, Music, Sound};
use koo_core::time::{Time, TimeRange};
use ort::session::Session;
use ort::value::Tensor;
use std::sync::Mutex;

/// The model, loaded on first use and shared by every job; it runs one
/// batch at a time. Its scores for a window are the highest probability
/// among the classes of each label in `groups.rs`, in that order.
#[derive(Default)]
pub struct EfficientAt {
    session: Mutex<Option<Session>>,
}

impl Tagger for EfficientAt {
    fn windows(&self) -> Windows {
        Windows {
            len: WINDOW,
            step: STEP,
            batch: BATCH,
        }
    }

    fn score(&self, windows: &[Vec<f32>]) -> Result<Vec<Scores>> {
        let n = windows.len();
        if n == 0 {
            return Ok(Vec::new());
        }
        let mut flat = vec![0.0; n * INPUT];
        for (w, slot) in windows.iter().zip(flat.chunks_mut(INPUT)) {
            let w = &w[..w.len().min(WINDOW)];
            slot[..w.len()].copy_from_slice(w);
        }
        let input = Tensor::from_array(([n, INPUT], flat)).map_err(|e| anyhow!("{e}"))?;
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if session.is_none() {
            *session = Some(onnx::session(&FILE.locate()?, FILE.what)?);
        }
        let outputs = session
            .as_mut()
            .expect("loaded above")
            .run(ort::inputs!["samples" => input])
            .map_err(|e| anyhow!("sound tagger failed: {e}"))?;
        let (_, probs) = outputs["probabilities"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        if probs.len() != n * CLASSES {
            bail!("sound tagger gave {} numbers for {n} windows", probs.len());
        }
        Ok(probs
            .chunks(CLASSES)
            .map(|p| {
                GROUPS
                    .iter()
                    .map(|g| g.classes.iter().map(|&c| p[c as usize]).fold(0.0, f32::max))
                    .collect()
            })
            .collect())
    }

    fn read(
        &self,
        scores: &[Scores],
        end: Time,
        levels: Option<&Levels>,
    ) -> (Vec<Sound>, Vec<Music>) {
        read(scores, end, levels)
    }
}

/// Source time where window `k` starts.
fn start(k: usize) -> Time {
    Time::from_secs_f64((k * STEP) as f64 / SOUND_RATE as f64)
}

/// Runs of windows where `score` rises to `on` and stays above `off`,
/// joined across gaps of up to `gap` windows: (first, last, peak).
fn runs(
    scores: &[Scores],
    group: usize,
    on: f32,
    off: f32,
    gap: usize,
) -> Vec<(usize, usize, f32)> {
    let mut out: Vec<(usize, usize, f32)> = Vec::new();
    let mut open: Option<(usize, f32)> = None;
    let mut push = |first: usize, last: usize, peak: f32| {
        if peak < on {
            return;
        }
        match out.last_mut() {
            Some(prev) if first <= prev.1 + gap + 1 => {
                prev.1 = last;
                prev.2 = prev.2.max(peak);
            }
            _ => out.push((first, last, peak)),
        }
    };
    for (k, s) in scores.iter().enumerate() {
        let p = s[group];
        match open {
            Some((first, peak)) if p >= off => open = Some((first, peak.max(p))),
            Some((first, peak)) => {
                push(first, k - 1, peak);
                open = (p >= off).then_some((k, p));
            }
            None if p >= off => open = Some((k, p)),
            None => {}
        }
    }
    if let Some((first, peak)) = open {
        push(first, scores.len() - 1, peak);
    }
    out
}

/// Where a run of windows `first..=last` puts its sound: from half a step
/// before the end of the first window to half a step into the last,
/// reaching the edges of the audio where the run does.
fn heard(first: usize, last: usize, windows: usize, end: Time) -> TimeRange {
    let half = Time(STEP_TIME.0 / 2);
    let a = if first == 0 {
        Time::ZERO
    } else {
        start(first) + WINDOW_TIME - half
    };
    let b = if last + 1 == windows {
        end
    } else {
        start(last) + half
    };
    if b > a {
        return TimeRange::new(a.min(end), b.min(end));
    }
    // Shorter than the windows can tell: about the middle of the overlap.
    let mid = Time((a.0 + b.0) / 2);
    TimeRange::new(
        (mid - Time(STEP_TIME.0 / 2)).max(Time::ZERO).min(end),
        (mid + Time(STEP_TIME.0 / 2)).min(end),
    )
}

/// The sounds and music in `scores` (one entry per window, windows
/// [`STEP`] apart) of audio `end` long; short sounds are moved onto the
/// loudest stretch of `levels` around them.
fn read(scores: &[Scores], end: Time, levels: Option<&Levels>) -> (Vec<Sound>, Vec<Music>) {
    let n = scores.len();
    let music_group = GROUPS
        .iter()
        .position(|g| g.kind == Kind::Music)
        .expect("music group");
    let music: Vec<Music> = runs(scores, music_group, MUSIC_ON, MUSIC_OFF, MUSIC_GAP)
        .into_iter()
        .map(|(first, last, _)| (first, last, heard(first, last, n, end)))
        .filter(|(_, _, range)| range.duration() >= MUSIC_MIN)
        .map(|(first, last, range)| Music {
            range,
            tags: tags(&scores[first..=last]),
            beats: Vec::new(),
            downbeats: Vec::new(),
        })
        .collect();

    let mut sounds = Vec::new();
    for (i, group) in GROUPS.iter().enumerate() {
        if !matches!(group.kind, Kind::Sound | Kind::Musical) {
            continue;
        }
        let mut found: Vec<Sound> = Vec::new();
        for (first, last, peak) in runs(scores, i, SOUND_ON, SOUND_OFF, SOUND_GAP) {
            let mut range = heard(first, last, n, end);
            if range.duration() <= STEP_TIME
                && let Some(levels) = levels
            {
                let reach = TimeRange::new(start(first), (start(last) + WINDOW_TIME).min(end));
                range = loudest(levels, range, reach).unwrap_or(range);
            }
            let in_music = |r: &TimeRange| {
                music.iter().any(|m| {
                    m.range
                        .intersect(r)
                        .is_some_and(|x| x.duration().0 * 2 > r.duration().0)
                })
            };
            if group.kind == Kind::Musical && in_music(&range) {
                continue;
            }
            match found.last_mut() {
                Some(prev) if range.start - prev.range.end <= STEP_TIME => {
                    prev.range.end = prev.range.end.max(range.end);
                    prev.confidence = prev.confidence.max(peak);
                }
                _ => found.push(Sound {
                    range,
                    label: group.label.to_string(),
                    confidence: peak,
                }),
            }
        }
        sounds.extend(found);
    }
    sounds.sort_by_key(|s| (s.range.start, s.range.end));
    (sounds, music)
}

/// The loudest stretch of `levels` around `near`, within `reach`: from
/// its peak out to where the level falls 20 dB below it (and 6 dB above
/// the floor around it), bridging dips of up to 50 ms.
fn loudest(levels: &Levels, near: TimeRange, reach: TimeRange) -> Option<TimeRange> {
    let frame = |t: Time| (t.0.max(0) / Levels::FRAME.0) as usize;
    let len = levels.len();
    let lo = frame(reach.start.max(near.start - STEP_TIME)).min(len);
    let hi = frame(reach.end.min(near.end + STEP_TIME)).min(len);
    if hi <= lo {
        return None;
    }
    let peak = (lo..hi).max_by(|&a, &b| levels.db(a).total_cmp(&levels.db(b)))?;
    let around = frame(near.start).saturating_sub(300)..(frame(near.end) + 300).min(len);
    let mut db: Vec<f32> = around.map(|i| levels.db(i)).collect();
    if db.is_empty() {
        return None;
    }
    let k = db.len() / 10;
    let floor = *db.select_nth_unstable_by(k, f32::total_cmp).1;
    let threshold = (levels.db(peak) - 20.0).max(floor + 6.0);
    if levels.db(peak) < threshold {
        return None;
    }
    let edge = |step: isize| {
        let (mut at, mut quiet) = (peak as isize, 0);
        loop {
            let next = at + step;
            if next < lo as isize || next >= hi as isize {
                return at;
            }
            if levels.db(next as usize) >= threshold {
                quiet = 0;
            } else {
                quiet += 1;
                if quiet > 5 {
                    return at;
                }
            }
            at = next;
        }
    };
    let (a, b) = (edge(-1) as usize, edge(1) as usize);
    let at = |i: usize| Time(i as i64 * Levels::FRAME.0);
    // Back off a dip the walk ended on.
    let (mut a, mut b) = (a, b);
    while a < peak && levels.db(a) < threshold {
        a += 1;
    }
    while b > peak && levels.db(b) < threshold {
        b -= 1;
    }
    Some(TimeRange::new(at(a), at(b + 1)))
}

/// What a stretch of music sounds like: its likeliest genres (two at
/// most), mood, vocals and lead instruments (two at most).
fn tags(scores: &[Scores]) -> Vec<String> {
    let mean = |i: usize| scores.iter().map(|s| s[i]).sum::<f32>() / scores.len().max(1) as f32;
    let pick = |kind: Kind, min: f32, most: usize| {
        let mut found: Vec<(f32, &'static str)> = GROUPS
            .iter()
            .enumerate()
            .filter(|(_, g)| g.kind == kind)
            .map(|(i, g)| (mean(i), g.label))
            .filter(|(m, _)| *m >= min)
            .collect();
        found.sort_by(|a, b| b.0.total_cmp(&a.0));
        found.truncate(most);
        found.into_iter().map(|(_, l)| l.to_string())
    };
    pick(Kind::Genre, 0.15, 2)
        .chain(pick(Kind::Mood, 0.1, 1))
        .chain(pick(Kind::Vocals, 0.25, 1))
        .chain(pick(Kind::Instrument, 0.25, 2))
        .collect()
}
