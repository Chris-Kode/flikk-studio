//! Word times from a forced aligner, as WhisperX does: a CTC model (MMS,
//! trained on 1,100+ languages written in Latin letters) hears the audio,
//! and the transcript's letters are fitted to what it heard. Whisper only
//! knows roughly where a word is and sometimes gives it to the wrong sound
//! (or drifts by a second in conversation); the aligner finds where its
//! letters sound. Words with no letters it knows (digits, symbols) keep the
//! times [`crate::align`] measured.

use crate::download::Download;
use crate::ffmpeg::SAMPLE_RATE;
use anyhow::{Context, Result, anyhow, bail};
use koo_core::analysis::Word;
use koo_core::paths;
use koo_core::time::{FLICKS_PER_SECOND, Time, TimeRange};
use ort::session::Session;
use ort::value::Tensor;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// int8 ONNX export of `MahmoudAshraf/mms-300m-1130-forced-aligner`.
pub const MODEL: &str = "mms-300m-1130-forced-aligner-int8.onnx";
const MODEL_URL: &str = "https://huggingface.co/onnx-community/mms-300m-1130-forced-aligner-ONNX/resolve/main/onnx/model_int8.onnx";

/// The model's letters and their output index; 0 is the CTC blank.
const LETTERS: &str = "aienoutsrmkldghybpwcvjzf'qx";
const FIRST_LETTER: u32 = 4;
/// Longest stretch of audio aligned at once. 30 s keeps the model's
/// working memory under 300 MB; shorter chunks cut more of the words
/// Whisper misplaced at their edges.
const MAX_CHUNK: Time = Time(FLICKS_PER_SECOND * 30);
/// The model hears a letter a little after it starts and stops hearing it
/// a little before it ends; edges move out by this much, never past a
/// neighbour. Measured on hand-labelled English and Dutch.
const EARLIER: Time = Time(FLICKS_PER_SECOND * 40 / 1000);
const LATER: Time = Time(FLICKS_PER_SECOND * 20 / 1000);

/// Loaded model, shared by every job. Runs one chunk at a time.
#[derive(Default)]
pub struct Aligner {
    session: Mutex<Option<Session>>,
    download: Download,
}

impl Aligner {
    /// The model path, downloading it first if needed; `start_download`
    /// returns a progress callback for the download.
    pub fn ensure_model(
        &self,
        start_download: impl FnOnce(&str) -> Box<dyn Fn(f32) + Send + Sync>,
    ) -> Result<PathBuf> {
        // `$KOOEDITOR_ALIGNER_MODEL` is a path, never fetched.
        if let Some(path) = std::env::var_os("KOOEDITOR_ALIGNER_MODEL").filter(|v| !v.is_empty()) {
            let path = PathBuf::from(path);
            if !path.is_file() {
                bail!("alignment model not found at {}", path.display());
            }
            return Ok(path);
        }
        let path = paths::models().join(MODEL);
        self.download
            .ensure(&path, MODEL_URL, "alignment model", || {
                start_download(MODEL)
            })?;
        Ok(path)
    }

    /// Move `words` onto the letters the model hears in 16 kHz mono
    /// `audio`. Their times only need to be roughly right; they decide
    /// which stretch of audio each word is looked for in.
    pub fn align(
        &self,
        model: &Path,
        audio: &[f32],
        words: &mut [Word],
        progress: impl Fn(f32),
    ) -> Result<()> {
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if session.is_none() {
            let env = ort::init()
                .build()
                .map_err(|e| anyhow!("could not start ONNX Runtime: {e}"))?;
            let loaded = Session::builder(&env)
                .map_err(|e| anyhow!("{e}"))?
                .with_intra_threads(threads())
                .map_err(|e| anyhow!("{e}"))?
                // Idle threads would spin and slow a Whisper job running
                // next to this one.
                .with_intra_op_spinning(false)
                .map_err(|e| anyhow!("{e}"))?
                // Repacked weights would double the model in memory, for
                // no speed gain here.
                .with_prepacking(false)
                .map_err(|e| anyhow!("{e}"))?
                .commit_from_file(model)
                .map_err(|e| anyhow!("could not load alignment model {}: {e}", model.display()))?;
            *session = Some(loaded);
        }
        let session = session.as_mut().expect("loaded above");

        let end = Time::from_secs_f64(audio.len() as f64 / SAMPLE_RATE as f64);
        let chunks = chunks(words, end);
        let mut aligned = vec![None; words.len()];
        for (n, (span, window)) in chunks.iter().enumerate() {
            progress(n as f32 / chunks.len() as f32);
            let at = |t: Time| ((t.as_secs_f64() * SAMPLE_RATE as f64) as usize).min(audio.len());
            let samples = &audio[at(window.start)..at(window.end)];
            let mut letters = Vec::new();
            let mut owner = Vec::new();
            for i in span.clone() {
                for letter in letters_of(&words[i].text) {
                    letters.push(letter);
                    owner.push(i);
                }
            }
            // Nothing to align, or shorter than one frame of the model.
            if letters.is_empty() || samples.len() < 400 {
                continue;
            }
            let frames = emissions(session, samples)?;
            let per_frame = window.duration().as_secs_f64() / frames.len() as f64;
            // Words that cannot fit keep their times.
            if let Some(spans) = viterbi(&frames, &letters) {
                let time = |f: usize| window.start + Time::from_secs_f64(f as f64 * per_frame);
                for (k, &(first, last)) in spans.iter().enumerate() {
                    let range: &mut Option<TimeRange> = &mut aligned[owner[k]];
                    let (s, e) = (time(first), time(last + 1));
                    *range =
                        Some(range.map_or(TimeRange::new(s, e), |r| TimeRange::new(r.start, e)));
                }
            }
        }
        progress(1.0);
        place(words, &aligned);
        Ok(())
    }
}

/// Log probabilities per 20 ms frame for `samples`, normalized as the
/// model was trained.
fn emissions(session: &mut Session, samples: &[f32]) -> Result<Vec<[f32; 32]>> {
    let n = samples.len() as f32;
    let mean = samples.iter().sum::<f32>() / n;
    let var = samples.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / n;
    let scale = 1.0 / (var + 1e-7).sqrt();
    let input: Vec<f32> = samples.iter().map(|s| (s - mean) * scale).collect();
    let input = Tensor::from_array(([1usize, input.len()], input)).map_err(|e| anyhow!("{e}"))?;
    let outputs = session
        .run(ort::inputs![input])
        .map_err(|e| anyhow!("alignment model failed: {e}"))?;
    let (shape, logits) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow!("{e}"))?;
    let classes = *shape.last().context("alignment model gave no output")? as usize;
    if classes > 32 || classes < FIRST_LETTER as usize + LETTERS.len() {
        bail!("alignment model has {classes} outputs");
    }
    Ok(logits
        .chunks_exact(classes)
        .map(|frame| {
            let max = frame.iter().copied().fold(f32::MIN, f32::max);
            let sum = frame.iter().map(|l| (l - max).exp()).sum::<f32>().ln() + max;
            let mut out = [f32::NEG_INFINITY; 32];
            for (o, l) in out.iter_mut().zip(frame) {
                *o = l - sum;
            }
            out
        })
        .collect())
}

/// The model's output index for each letter of `text` it knows, after
/// writing it in Latin letters.
fn letters_of(text: &str) -> Vec<u32> {
    any_ascii::any_ascii(text)
        .chars()
        .filter_map(|c| LETTERS.find(c.to_ascii_lowercase()))
        .map(|i| FIRST_LETTER + i as u32)
        .collect()
}

/// Runs of words aligned together, and the audio each is looked for in.
/// Runs split at Whisper's longest pause so none is longer than
/// [`MAX_CHUNK`]; neighbouring windows meet in the middle of that pause.
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
            let start = k.checked_sub(1).map_or(Time::ZERO, |p| {
                meet(&words[runs[p].end - 1], &words[runs[k].start])
            });
            let stop = runs
                .get(k + 1)
                .map_or(end, |n| meet(&words[runs[k].end - 1], &words[n.start]));
            (
                runs[k].clone(),
                TimeRange::new(start.max(Time::ZERO), stop.min(end)),
            )
        })
        .filter(|(_, w)| w.start < w.end)
        .collect()
}

/// The most likely CTC path through `frames` that spells `letters`: the
/// first and last frame of each letter, or `None` when there are more
/// letters than the audio can hold.
fn viterbi(frames: &[[f32; 32]], letters: &[u32]) -> Option<Vec<(usize, usize)>> {
    const BLANK: usize = 0;
    let t_len = frames.len();
    // Blank, letter, blank, letter, …, blank.
    let states: Vec<usize> = (0..2 * letters.len() + 1)
        .map(|s| {
            if s % 2 == 0 {
                BLANK
            } else {
                letters[s / 2] as usize
            }
        })
        .collect();
    let repeats = letters.windows(2).filter(|w| w[0] == w[1]).count();
    if letters.is_empty() || letters.len() + repeats > t_len {
        return None;
    }
    // A letter can follow the one before it directly unless they are the
    // same, which needs a blank between.
    let skip = |s: usize| s % 2 == 1 && s >= 2 && states[s] != states[s - 2];
    let n = states.len();
    let mut score = vec![f32::NEG_INFINITY; n];
    score[0] = frames[0][states[0]];
    score[1] = frames[0][states[1]];
    let mut back = vec![0u8; t_len * n];
    let mut next = vec![f32::NEG_INFINITY; n];
    for t in 1..t_len {
        for s in 0..n {
            let mut best = (score[s], 0u8);
            if s >= 1 && score[s - 1] > best.0 {
                best = (score[s - 1], 1);
            }
            if skip(s) && score[s - 2] > best.0 {
                best = (score[s - 2], 2);
            }
            next[s] = best.0 + frames[t][states[s]];
            back[t * n + s] = best.1;
        }
        std::mem::swap(&mut score, &mut next);
    }
    let mut s = if score[n - 1] >= score[n - 2] {
        n - 1
    } else {
        n - 2
    };
    if score[s] == f32::NEG_INFINITY {
        return None;
    }
    let mut spans = vec![(usize::MAX, 0); letters.len()];
    for t in (0..t_len).rev() {
        if s % 2 == 1 {
            let span = &mut spans[s / 2];
            if span.0 == usize::MAX {
                span.1 = t;
            }
            span.0 = t;
        }
        s -= back[t * n + s] as usize;
    }
    Some(spans)
}

/// Give each aligned word its range, widened by [`EARLIER`] and [`LATER`]
/// without reaching a neighbour; the others stay between their aligned
/// neighbours.
fn place(words: &mut [Word], aligned: &[Option<TimeRange>]) {
    let heard: Vec<TimeRange> = words
        .iter()
        .zip(aligned)
        .map(|(w, a)| a.unwrap_or(w.range))
        .collect();
    for (i, w) in words.iter_mut().enumerate() {
        let Some(r) = aligned[i] else {
            // Between the aligned words around it, where they leave room.
            let lo = aligned[..i].iter().rev().find_map(|a| *a).map(|a| a.end);
            let hi = aligned[i + 1..].iter().find_map(|a| *a).map(|a| a.start);
            let lo = lo.unwrap_or(Time::ZERO);
            let hi = hi.unwrap_or(Time(i64::MAX));
            let clamped = TimeRange::new(w.range.start.max(lo), w.range.end.min(hi));
            if clamped.start < clamped.end {
                w.range = clamped;
            }
            continue;
        };
        let lo = i
            .checked_sub(1)
            .map_or(Time::ZERO, |p| heard[p].end.min(r.start));
        w.range.start = (r.start - EARLIER).max(lo);
        w.range.end = r.end;
    }
    for i in 0..words.len() {
        if aligned[i].is_none() {
            continue;
        }
        let hi = words
            .get(i + 1)
            .map_or(Time(i64::MAX), |n| n.range.start.max(words[i].range.end));
        words[i].range.end = (words[i].range.end + LATER).min(hi);
    }
}

fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().min(8))
        .unwrap_or(4)
}
