//! Word times from a forced aligner: Qwen3-ForcedAligner-0.6B
//! (Apache-2.0) hears a stretch of audio with the words said in it and
//! gives each word a start and an end, in 80 ms steps. Whisper only knows
//! roughly where a word is, sometimes puts it on the wrong sound and
//! drifts by a second in conversation; the aligner does not. It knows 11
//! languages ([`supports`]); elsewhere, and for words with no letters or
//! digits, the edges [`crate::align`] measured stay.

use crate::bpe::Bpe;
use crate::download::Download;
use crate::ffmpeg::SAMPLE_RATE;
use crate::onnx;
use anyhow::{Result, anyhow, bail};
use koo_core::analysis::{Levels, Word};
use koo_core::paths;
use koo_core::time::{FLICKS_PER_SECOND, Time, TimeRange};
use ort::session::Session;
use ort::value::Tensor;
use std::path::PathBuf;
use std::sync::Mutex;

/// The model as ONNX, with its Whisper-style features built in (16 kHz
/// samples and token ids in, a class per token out), weights quantized to
/// 4 bits.
pub const MODEL: &str = "qwen3-forced-aligner-0.6b-q4.onnx";
/// Where [`MODEL`] is downloaded from. Qwen publishes no ONNX export; how
/// this one is made and hosted is in `docs/TECHNICAL_DESIGN.md`.
const MODEL_URL: Option<&str> = Some(
    "https://huggingface.co/Chris-Kode/qwen3-forced-aligner-0.6b-onnx/resolve/c1386c6a094f575f2067cf82632c30238c1b1f9d/qwen3-forced-aligner-0.6b-q4.onnx",
);
/// Qwen's tokenizer, from the model's own repository.
const TOKENIZER: [(&str, &str); 2] = [
    (
        "qwen3-forced-aligner-vocab.json",
        "https://huggingface.co/Qwen/Qwen3-ForcedAligner-0.6B/resolve/c7cbfc2048c462b0d63a45797104fc9db3ad62b7/vocab.json",
    ),
    (
        "qwen3-forced-aligner-merges.txt",
        "https://huggingface.co/Qwen/Qwen3-ForcedAligner-0.6B/resolve/c7cbfc2048c462b0d63a45797104fc9db3ad62b7/merges.txt",
    ),
];

/// Follows each unit twice; the model answers with its start and its end.
const TIMESTAMP: i64 = 151705;
/// One step of those answers.
const STEP_MS: i64 = 80;
/// Longest run of words aligned at once.
const MAX_CHUNK: Time = Time(FLICKS_PER_SECOND * 30);
/// Audio either side of a run, so a word Whisper misplaced is still in it.
const SLACK: Time = Time(FLICKS_PER_SECOND * 5);
/// The model's starts come a little late; they move this much earlier,
/// never into the word before. Measured on hand-labelled English.
const EARLIER: Time = Time(FLICKS_PER_SECOND * 20 / 1000);

/// Whether the model knows `language` (Whisper's code).
pub fn supports(language: &str) -> bool {
    matches!(
        language,
        "en" | "es" | "fr" | "de" | "it" | "pt" | "ru" | "zh" | "yue" | "ja" | "ko"
    )
}

/// The model and its tokenizer on disk.
pub struct Files {
    model: PathBuf,
    vocab: PathBuf,
    merges: PathBuf,
}

/// Loaded model, shared by every job. Runs one chunk at a time.
#[derive(Default)]
pub struct Aligner {
    loaded: Mutex<Option<(Session, Bpe)>>,
    downloads: [Download; 3],
}

impl Aligner {
    /// The model's files, downloading them first if needed;
    /// `start_download` returns a progress callback for each download.
    pub fn ensure_model(
        &self,
        start_download: impl Fn(&str) -> Box<dyn Fn(f32) + Send + Sync>,
    ) -> Result<Files> {
        // `$KOOEDITOR_ALIGNER_MODEL` is a path, never fetched.
        let model = match std::env::var_os("KOOEDITOR_ALIGNER_MODEL").filter(|v| !v.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => {
                let path = paths::models().join(MODEL);
                if let Some(url) = MODEL_URL {
                    self.downloads[0]
                        .ensure(&path, url, "alignment model", || start_download(MODEL))?;
                }
                path
            }
        };
        if !model.is_file() {
            bail!(
                "alignment model not found at {} (set KOOEDITOR_ALIGNER_MODEL)",
                model.display()
            );
        }
        let mut tokenizer = Vec::new();
        for ((name, url), download) in TOKENIZER.iter().zip(&self.downloads[1..]) {
            let path = paths::models().join(name);
            download.ensure(&path, url, "alignment tokenizer", || start_download(name))?;
            tokenizer.push(path);
        }
        let [vocab, merges] = <[PathBuf; 2]>::try_from(tokenizer).expect("two files");
        Ok(Files {
            model,
            vocab,
            merges,
        })
    }

    /// Move `words` onto where the model hears them in 16 kHz mono
    /// `audio`. Their times only need to be roughly right; they decide
    /// which stretch of audio each word is looked for in.
    pub fn align(
        &self,
        files: &Files,
        audio: &[f32],
        words: &mut [Word],
        progress: impl Fn(f32),
    ) -> Result<()> {
        let mut loaded = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        if loaded.is_none() {
            *loaded = Some((
                onnx::session(&files.model, "alignment model")?,
                Bpe::load(&files.vocab, &files.merges)?,
            ));
        }
        let (session, bpe) = loaded.as_mut().expect("loaded above");

        let end = Time::from_secs_f64(audio.len() as f64 / SAMPLE_RATE as f64);
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
            let at = |t: Time| ((t.as_secs_f64() * SAMPLE_RATE as f64) as usize).min(audio.len());
            let from = at(window.start);
            let frame = SAMPLE_RATE as usize / 100;
            let to = from + (at(window.end) - from) / frame * frame;
            if owner.is_empty() || to - from < 10 * frame {
                continue;
            }
            let classes = classes(session, &audio[from..to], &ids)?;
            let steps: Vec<i64> = ids
                .iter()
                .zip(&classes)
                .filter(|(id, _)| **id == TIMESTAMP)
                .map(|(_, c)| c * STEP_MS)
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
        place(words, &aligned);
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
/// reaching the word before; the others stay between their aligned
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
        // Both edges in one 80 ms step: still a word.
        w.range.end = r.end.max(w.range.start + Levels::FRAME);
    }
}
