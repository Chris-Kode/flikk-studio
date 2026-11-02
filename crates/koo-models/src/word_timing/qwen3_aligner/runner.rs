//! The aligner hearing runs of words up to [`MAX_CHUNK`] long, each
//! against the audio around it, and the times it answers put in order.

use super::config::{
    EARLIER, FILE, INTO_PAUSE, LANGUAGES, MAX_CHUNK, MERGES, PAUSE, SLACK, STEP_MS, TIMESTAMP,
    VOCAB,
};
use crate::SPEECH_RATE;
use crate::bpe::Bpe;
use crate::onnx;
use crate::word_timing::Aligner;
use anyhow::{Result, anyhow, bail};
use koo_core::analysis::{Levels, Word};
use koo_core::time::{Time, TimeRange};
use ort::session::Session;
use ort::value::Tensor;
use std::sync::Mutex;

/// Loaded model, shared by every job. Runs one chunk at a time.
#[derive(Default)]
pub struct Qwen3Aligner {
    loaded: Mutex<Option<(Session, Bpe)>>,
}

impl Aligner for Qwen3Aligner {
    fn supports(&self, language: &str) -> bool {
        LANGUAGES.contains(&language)
    }

    fn align(&self, audio: &[f32], words: &mut [Word], progress: &dyn Fn(f32)) -> Result<()> {
        let mut loaded = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        if loaded.is_none() {
            *loaded = Some((
                onnx::session(&FILE.locate()?, FILE.what)?,
                Bpe::load(&VOCAB.locate()?, &MERGES.locate()?)?,
            ));
        }
        let (session, bpe) = loaded.as_mut().expect("loaded above");

        let end = Time::from_secs_f64(audio.len() as f64 / SPEECH_RATE as f64);
        let chunks = chunks(words, end);
        let mut aligned = vec![None; words.len()];
        for (n, (span, window)) in chunks.iter().enumerate() {
            progress(n as f32 / chunks.len() as f32);
            let mut ids = Vec::new();
            let mut owner = Vec::new();
            for i in span.clone() {
                for unit in units(&words[i].text) {
                    ids.extend(bpe.encode(&unit));
                    ids.extend([TIMESTAMP, TIMESTAMP]);
                    owner.push(i);
                }
            }
            // Features are 10 ms frames: whole ones only, and at least ten.
            let at = |t: Time| ((t.as_secs_f64() * SPEECH_RATE as f64) as usize).min(audio.len());
            let from = at(window.start);
            let frame = SPEECH_RATE as usize / 100;
            let to = from + (at(window.end) - from) / frame * frame;
            if owner.is_empty() || to - from < 10 * frame {
                continue;
            }
            let classes = classes(session, &audio[from..to], &ids)?;
            // Given more words than the audio holds, the model answers past
            // its end; those words stay at the end, not in the next run's
            // audio, where they would come before its words.
            let heard = ((to - from) * 1000 / SPEECH_RATE as usize) as i64;
            let steps: Vec<i64> = ids
                .iter()
                .zip(&classes)
                .filter(|(id, _)| **id == TIMESTAMP)
                .map(|(_, c)| (c * STEP_MS).min(heard))
                .collect();
            if steps.len() != 2 * owner.len() {
                bail!(
                    "alignment model gave {} times for {} words",
                    steps.len(),
                    owner.len()
                );
            }
            let ms = monotonic(&steps);
            for (u, &i) in owner.iter().enumerate() {
                let (s, e) = (
                    window.start + Time::from_millis(ms[2 * u]),
                    window.start + Time::from_millis(ms[2 * u + 1]),
                );
                let range: &mut Option<TimeRange> = &mut aligned[i];
                *range = Some(range.map_or(TimeRange::new(s, e), |r| TimeRange::new(r.start, e)));
            }
        }
        progress(1.0);
        place(words, &aligned, end);
        Ok(())
    }
}

/// The model's answer for each of `ids`, heard against `samples`.
fn classes(session: &mut Session, samples: &[f32], ids: &[i64]) -> Result<Vec<i64>> {
    let samples =
        Tensor::from_array(([samples.len()], samples.to_vec())).map_err(|e| anyhow!("{e}"))?;
    let text = Tensor::from_array(([ids.len()], ids.to_vec())).map_err(|e| anyhow!("{e}"))?;
    let outputs = session
        .run(ort::inputs!["samples" => samples, "text_ids" => text])
        .map_err(|e| anyhow!("alignment model failed: {e}"))?;
    let (_, classes) = outputs["classes"]
        .try_extract_tensor::<i64>()
        .map_err(|e| anyhow!("{e}"))?;
    if classes.len() != ids.len() {
        bail!(
            "alignment model gave {} answers for {} tokens",
            classes.len(),
            ids.len()
        );
    }
    Ok(classes.to_vec())
}

/// What the model places for one word, split as Qwen splits text: its
/// letters, digits and apostrophes, with each Chinese character alone.
fn units(word: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut run = String::new();
    for c in word
        .chars()
        .filter(|&c| c.is_alphabetic() || c.is_numeric() || c == '\'')
    {
        if ideograph(c) {
            out.extend((!run.is_empty()).then(|| std::mem::take(&mut run)));
            out.push(c.to_string());
        } else {
            run.push(c);
        }
    }
    out.extend((!run.is_empty()).then_some(run));
    out
}

fn ideograph(c: char) -> bool {
    matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2CEAF | 0xF900..=0xFAFF)
}

/// Times that run backwards fixed as Qwen does: the longest run that never
/// goes back stands; one or two strays take the nearer neighbour's time,
/// longer stretches are spread evenly between their neighbours.
fn monotonic(ms: &[i64]) -> Vec<i64> {
    let n = ms.len();
    if n == 0 {
        return Vec::new();
    }
    let mut len = vec![1usize; n];
    let mut parent = vec![None; n];
    for i in 1..n {
        for j in 0..i {
            if ms[j] <= ms[i] && len[j] + 1 > len[i] {
                len[i] = len[j] + 1;
                parent[i] = Some(j);
            }
        }
    }
    let longest = *len.iter().max().expect("not empty");
    let mut normal = vec![false; n];
    let mut at = len.iter().position(|&l| l == longest);
    while let Some(i) = at {
        normal[i] = true;
        at = parent[i];
    }
    let mut out = ms.to_vec();
    let mut i = 0;
    while i < n {
        if normal[i] {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < n && !normal[j] {
            j += 1;
        }
        let left = (0..i).rev().find(|&k| normal[k]).map(|k| out[k]);
        let right = (j..n).find(|&k| normal[k]).map(|k| out[k]);
        for k in i..j {
            out[k] = match (left, right) {
                (Some(l), Some(r)) if j - i > 2 => {
                    let step = (r - l) as f64 / (j - i + 1) as f64;
                    (l as f64 + step * (k - i + 1) as f64) as i64
                }
                (Some(l), Some(r)) => {
                    if k + 1 - i <= j - k {
                        l
                    } else {
                        r
                    }
                }
                (l, r) => l.or(r).unwrap_or(ms[k]),
            };
        }
        i = j;
    }
    out
}

/// Runs of words aligned together, and the audio each is looked for in.
/// Runs split at Whisper's longest pause so none is longer than
/// [`MAX_CHUNK`]; a run's audio reaches [`SLACK`] past its words but never
/// past the middle of the pause to the next run.
fn chunks(words: &[Word], end: Time) -> Vec<(std::ops::Range<usize>, TimeRange)> {
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    let mut from = 0;
    for i in 0..words.len() {
        while i > from && words[i].range.end - words[from].range.start > MAX_CHUNK {
            let split = (from..i)
                .max_by_key(|&j| words[j + 1].range.start - words[j].range.end)
                .expect("from < i");
            runs.push(from..split + 1);
            from = split + 1;
        }
    }
    if from < words.len() {
        runs.push(from..words.len());
    }
    let meet = |a: &Word, b: &Word| Time(a.range.end.0 + (b.range.start.0 - a.range.end.0) / 2);
    (0..runs.len())
        .map(|k| {
            let (first, last) = (&words[runs[k].start], &words[runs[k].end - 1]);
            let lo = k
                .checked_sub(1)
                .map_or(Time::ZERO, |p| meet(&words[runs[p].end - 1], first));
            let hi = runs.get(k + 1).map_or(end, |n| meet(last, &words[n.start]));
            let start = lo.max(first.range.start - SLACK).max(Time::ZERO);
            let stop = hi
                .min(last.range.end + SLACK)
                .min(start + MAX_CHUNK + SLACK + SLACK)
                .min(end);
            (runs[k].clone(), TimeRange::new(start, stop))
        })
        .filter(|(_, w)| w.start < w.end)
        .collect()
}

/// Give each aligned word its range, its start [`EARLIER`] without
/// reaching the word before and its edges next to a pause [`INTO_PAUSE`]
/// into it; the others stay between their aligned neighbours, so words
/// stay in order. `end` is where the audio ends.
fn place(words: &mut [Word], aligned: &[Option<TimeRange>], end: Time) {
    for i in 0..words.len() {
        let Some(r) = aligned[i] else {
            // Inside the room the aligned words around it leave.
            let lo = aligned[..i].iter().rev().find_map(|a| *a).map(|a| a.end);
            let hi = aligned[i + 1..].iter().find_map(|a| *a).map(|a| a.start);
            let before = i.checked_sub(1).map(|p| words[p].range.end);
            let lo = lo.max(before).unwrap_or(Time::ZERO);
            let hi = hi.unwrap_or(Time(i64::MAX)).max(lo);
            let w = &mut words[i].range;
            let start = w.start.clamp(lo, hi);
            *w = TimeRange::new(start, w.end.clamp(start, hi));
            continue;
        };
        // After the word before, which may have been made a frame longer.
        let lo = i.checked_sub(1).map_or(Time::ZERO, |p| words[p].range.end);
        let w = &mut words[i].range;
        w.start = (r.start - EARLIER).max(lo);
        // Both edges in one 80 ms step: still a word, inside the audio.
        w.end = r.end.max(w.start + Levels::FRAME).min(end).max(w.start);
    }
    // The room before each word, and after the last.
    let room: Vec<Time> = (0..=words.len())
        .map(|k| {
            let lo = k.checked_sub(1).map_or(Time::ZERO, |p| words[p].range.end);
            words.get(k).map_or(end, |w| w.range.start) - lo
        })
        .collect();
    let into = |room: Time| {
        if room >= PAUSE {
            INTO_PAUSE.min(Time(room.0 / 2))
        } else {
            Time::ZERO
        }
    };
    for (i, w) in words.iter_mut().enumerate() {
        if aligned[i].is_some() {
            w.range.start = w.range.start - into(room[i]);
            w.range.end = w.range.end + into(room[i + 1]);
        }
    }
}
