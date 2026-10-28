//! The four steps of the pipeline, and a word's voice read off the
//! voices they find.

use super::config::{
    BATCH, BLOCK, CLASSES, EMBEDDING, FRAME_SPAN, FRAME_STEP, FRAMES, LOCAL, MAX_FITTED,
    MIN_CLUSTER, MIN_FEATURES, REACH, SEGMENTATION, STEP, THRESHOLD, WINDOW,
};
use super::fbank::{self, Fbank};
use crate::SPEECH_RATE;
use crate::onnx;
use crate::speakers::{Diarizer, Labels};
use anyhow::{Result, anyhow, bail};
use koo_core::analysis::Word;
use koo_core::time::Time;
use ort::session::Session;
use ort::value::Tensor;
use std::sync::Mutex;

/// The models, loaded on first use and shared by every job.
#[derive(Default)]
pub struct Pyannote {
    loaded: Mutex<Option<(Session, Session)>>,
}

impl Diarizer for Pyannote {
    fn diarize(&self, audio: &[f32], words: &[Word], progress: &dyn Fn(f32)) -> Result<Labels> {
        if words.is_empty() || audio.is_empty() {
            return Ok((0, vec![None; words.len()]));
        }
        let mut loaded = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        if loaded.is_none() {
            *loaded = Some((
                onnx::session(&SEGMENTATION.locate()?, SEGMENTATION.what)?,
                onnx::session(&EMBEDDING.locate()?, EMBEDDING.what)?,
            ));
        }
        let (segmentation, embedding) = loaded.as_mut().expect("loaded above");

        let starts = window_starts(audio.len());
        let local = segment(segmentation, audio, &starts, &|f| progress(0.3 * f))?;
        let prints = voiceprints(embedding, audio, &starts, &local, &|f| {
            progress(0.3 + 0.65 * f)
        })?;
        let (count, clusters) = cluster(&prints);
        progress(0.95);
        let voices = reconstruct(&starts, &local, &clusters, count);
        progress(1.0);
        Ok((count, words.iter().map(|w| attribute(&voices, w)).collect()))
    }
}

/// First sample of each window: every [`STEP`] while a whole window fits,
/// then one more, padded with silence, for what is left.
fn window_starts(samples: usize) -> Vec<usize> {
    let whole = if samples >= WINDOW {
        1 + (samples - WINDOW) / STEP
    } else {
        0
    };
    let mut starts: Vec<usize> = (0..whole).map(|c| c * STEP).collect();
    if whole == 0 || !(samples - WINDOW).is_multiple_of(STEP) {
        starts.push(whole * STEP);
    }
    starts
}

/// `audio[from..to]`, silence past its end.
fn padded(audio: &[f32], from: usize, to: usize) -> Vec<f32> {
    let mut out = audio[from.min(audio.len())..to.min(audio.len())].to_vec();
    out.resize(to - from, 0.0);
    out
}

/// Which local voices speak in each frame of each window, as bits:
/// [`FRAMES`] per window.
fn segment(
    session: &mut Session,
    audio: &[f32],
    starts: &[usize],
    progress: &dyn Fn(f32),
) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(starts.len() * FRAMES);
    for (n, batch) in starts.chunks(BATCH).enumerate() {
        progress((n * BATCH) as f32 / starts.len() as f32);
        let input: Vec<f32> = batch
            .iter()
            .flat_map(|&s| padded(audio, s, s + WINDOW))
            .collect();
        let input =
            Tensor::from_array(([batch.len(), 1, WINDOW], input)).map_err(|e| anyhow!("{e}"))?;
        let outputs = session
            .run(ort::inputs!["input_values" => input])
            .map_err(|e| anyhow!("speaker segmentation model failed: {e}"))?;
        let (shape, logits) = outputs["logits"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        if shape[..] != [batch.len() as i64, FRAMES as i64, CLASSES.len() as i64] {
            bail!("speaker segmentation model gave {shape:?}");
        }
        out.extend(logits.chunks(CLASSES.len()).map(|scores| {
            let best = (0..scores.len())
                .max_by(|&a, &b| scores[a].total_cmp(&scores[b]))
                .expect("seven classes");
            CLASSES[best]
        }));
    }
    Ok(out)
}

/// A unit voiceprint for each window's voice that speaks long enough, at
/// `window * LOCAL + voice`.
fn voiceprints(
    session: &mut Session,
    audio: &[f32],
    starts: &[usize],
    local: &[u8],
    progress: &dyn Fn(f32),
) -> Result<Vec<Option<Vec<f32>>>> {
    let features = Fbank::default();
    let per_window = Fbank::frames(WINDOW);
    let mut prints = vec![None; starts.len() * LOCAL];
    for block in (0..starts.len()).step_by(BLOCK) {
        let windows = block..(block + BLOCK).min(starts.len());
        progress(block as f32 / starts.len() as f32);
        let first = starts[block];
        let rows = features.compute(&padded(audio, first, starts[windows.end - 1] + WINDOW));
        for c in windows {
            let at = (starts[c] - first) / fbank::SHIFT * fbank::BINS;
            let mut window = rows[at..at + per_window * fbank::BINS].to_vec();
            // WeSpeaker takes features less their mean over the window.
            for b in 0..fbank::BINS {
                let mean =
                    window.iter().skip(b).step_by(fbank::BINS).sum::<f32>() / per_window as f32;
                window
                    .iter_mut()
                    .skip(b)
                    .step_by(fbank::BINS)
                    .for_each(|x| *x -= mean);
            }
            let frames = &local[c * FRAMES..(c + 1) * FRAMES];
            for voice in 0..LOCAL {
                let bit = 1u8 << voice;
                if frames.iter().all(|f| f & bit == 0) {
                    continue;
                }
                // Where it speaks alone, unless that is next to nothing.
                let alone = frames.iter().filter(|&&f| f == bit).count() > 2;
                let keep = |f: u8| if alone { f == bit } else { f & bit != 0 };
                let mut picked = Vec::new();
                for j in 0..per_window {
                    if keep(frames[j * FRAMES / per_window]) {
                        picked.extend_from_slice(&window[j * fbank::BINS..(j + 1) * fbank::BINS]);
                    }
                }
                let n = picked.len() / fbank::BINS;
                if n < MIN_FEATURES {
                    continue;
                }
                let input = Tensor::from_array(([1, n, fbank::BINS], picked))
                    .map_err(|e| anyhow!("{e}"))?;
                let outputs = session
                    .run(ort::inputs!["feats" => input])
                    .map_err(|e| anyhow!("speaker embedding model failed: {e}"))?;
                let (_, print) = outputs["embs"]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| anyhow!("{e}"))?;
                prints[c * LOCAL + voice] = unit(print);
            }
        }
    }
    Ok(prints)
}

/// `v` scaled to length 1; `None` when it has no direction.
fn unit(v: &[f32]) -> Option<Vec<f32>> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    (norm.is_finite() && norm > 0.0).then(|| v.iter().map(|x| x / norm).collect())
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Number of voices, and the voice of each voiceprint.
fn cluster(prints: &[Option<Vec<f32>>]) -> (usize, Vec<Option<usize>>) {
    let valid: Vec<usize> = (0..prints.len()).filter(|&i| prints[i].is_some()).collect();
    if valid.is_empty() {
        return (0, vec![None; prints.len()]);
    }
    let fitted: Vec<usize> = if valid.len() > MAX_FITTED {
        (0..MAX_FITTED)
            .map(|k| valid[k * valid.len() / MAX_FITTED])
            .collect()
    } else {
        valid
    };
    let vectors: Vec<&[f32]> = fitted
        .iter()
        .map(|&i| prints[i].as_deref().expect("valid"))
        .collect();
    let groups = centroid_linkage(&vectors);
    let centroids = fold_small(&vectors, groups);
    let labels = prints
        .iter()
        .map(|p| {
            let p = p.as_deref()?;
            (0..centroids.len())
                .max_by(|&a, &b| cosine(p, &centroids[a]).total_cmp(&cosine(p, &centroids[b])))
        })
        .collect();
    (centroids.len(), labels)
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let norm = (dot(a, a) * dot(b, b)).sqrt();
    if norm > 0.0 { dot(a, b) / norm } else { -1.0 }
}

/// Agglomerative clustering of unit `vectors` with centroid linkage,
/// merging until the nearest centroids are [`THRESHOLD`] apart. Returns
/// each vector's group.
fn centroid_linkage(vectors: &[&[f32]]) -> Vec<usize> {
    let n = vectors.len();
    // Squared distances between groups, updated as Lance and Williams do.
    let mut d = vec![0f32; n * n];
    for i in 0..n {
        for j in i + 1..n {
            let v = (2.0 - 2.0 * dot(vectors[i], vectors[j])).max(0.0);
            d[i * n + j] = v;
            d[j * n + i] = v;
        }
    }
    let mut size = vec![1usize; n];
    let mut alive = vec![true; n];
    let mut group: Vec<usize> = (0..n).collect();
    let nearest = |d: &[f32], alive: &[bool], i: usize| {
        (0..n)
            .filter(|&j| j != i && alive[j])
            .map(|j| (j, d[i * n + j]))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    };
    let mut nn: Vec<Option<(usize, f32)>> = (0..n).map(|i| nearest(&d, &alive, i)).collect();
    let limit = THRESHOLD * THRESHOLD;
    while let Some((i, (j, dist))) = (0..n)
        .filter(|&i| alive[i])
        .filter_map(|i| nn[i].map(|x| (i, x)))
        .min_by(|a, b| a.1.1.total_cmp(&b.1.1))
    {
        if dist > limit {
            break;
        }
        let (si, sj) = (size[i] as f32, size[j] as f32);
        let s = si + sj;
        for k in (0..n).filter(|&k| alive[k] && k != i && k != j) {
            let v =
                ((si * d[k * n + i] + sj * d[k * n + j]) / s - si * sj * dist / (s * s)).max(0.0);
            d[k * n + i] = v;
            d[i * n + k] = v;
        }
        alive[j] = false;
        size[i] += size[j];
        group.iter_mut().filter(|g| **g == j).for_each(|g| *g = i);
        nn[j] = None;
        nn[i] = nearest(&d, &alive, i);
        for k in (0..n).filter(|&k| alive[k] && k != i) {
            nn[k] = match nn[k] {
                Some((m, _)) if m == i || m == j => nearest(&d, &alive, k),
                Some((_, best)) if d[k * n + i] < best => Some((i, d[k * n + i])),
                other => other,
            };
        }
    }
    group
}

/// Centroids of the groups with at least [`MIN_CLUSTER`] vectors (fewer
/// for few vectors), after each smaller group joins the large one whose
/// centroid is nearest its own. With no large group, one for all.
fn fold_small(vectors: &[&[f32]], group: Vec<usize>) -> Vec<Vec<f32>> {
    let min = MIN_CLUSTER.min(((vectors.len() as f32 * 0.1).round() as usize).max(1));
    let mut ids: Vec<usize> = group.clone();
    ids.sort_unstable();
    ids.dedup();
    let centroid = |members: &[usize]| -> Vec<f32> {
        let mut c = vec![0f32; vectors[0].len()];
        for &m in members {
            c.iter_mut().zip(vectors[m]).for_each(|(c, v)| *c += v);
        }
        c.iter_mut().for_each(|c| *c /= members.len() as f32);
        c
    };
    let members =
        |id: usize| -> Vec<usize> { (0..vectors.len()).filter(|&m| group[m] == id).collect() };
    let (large, small): (Vec<Vec<usize>>, Vec<Vec<usize>>) = ids
        .iter()
        .map(|&id| members(id))
        .partition(|m| m.len() >= min);
    if large.is_empty() {
        let all: Vec<usize> = (0..vectors.len()).collect();
        return vec![centroid(&all)];
    }
    let mut large = large;
    let centroids: Vec<Vec<f32>> = large.iter().map(|m| centroid(m)).collect();
    for s in small {
        let c = centroid(&s);
        let to = (0..centroids.len())
            .max_by(|&a, &b| cosine(&c, &centroids[a]).total_cmp(&cosine(&c, &centroids[b])))
            .expect("a large group");
        large[to].extend(s);
    }
    large.iter().map(|m| centroid(m)).collect()
}

/// Which voices speak in each frame of the item: the windows' voices under
/// their clusters' names, averaged over the windows that hear each frame,
/// keeping as many voices as the windows hear at once there.
fn reconstruct(
    starts: &[usize],
    local: &[u8],
    clusters: &[Option<usize>],
    voices: usize,
) -> Voices {
    let frame_of = |sample: usize| (sample as f64 / FRAME_STEP as f64).round() as usize;
    let total = frame_of(starts[starts.len() - 1] + WINDOW) + 1;
    let mut heard = vec![0f32; total];
    let mut windows = vec![0u16; total];
    let mut sum = vec![0f32; total * voices];
    let mut seen = vec![0u16; total * voices];
    for (c, &start) in starts.iter().enumerate() {
        let first = frame_of(start);
        let frames = &local[c * FRAMES..(c + 1) * FRAMES];
        for (j, f) in frames.iter().enumerate() {
            heard[first + j] += f.count_ones() as f32;
            windows[first + j] += 1;
        }
        for k in 0..voices {
            let mine: u8 = (0..LOCAL)
                .filter(|&v| clusters[c * LOCAL + v] == Some(k))
                .map(|v| 1u8 << v)
                .sum();
            if mine == 0 {
                continue;
            }
            for (j, f) in frames.iter().enumerate() {
                let at = (first + j) * voices + k;
                sum[at] += if f & mine != 0 { 1.0 } else { 0.0 };
                seen[at] += 1;
            }
        }
    }
    let mut active = vec![0f32; total * voices];
    let mut order: Vec<usize> = (0..voices).collect();
    for t in 0..total {
        let at_once = if windows[t] > 0 {
            (heard[t] / windows[t] as f32).round() as usize
        } else {
            0
        };
        let level = |k: usize| {
            let i = t * voices + k;
            if seen[i] > 0 {
                sum[i] / seen[i] as f32
            } else {
                0.0
            }
        };
        order.sort_by(|&a, &b| level(b).total_cmp(&level(a)));
        for &k in order.iter().take(at_once) {
            // A voice no window put here does not speak here.
            active[t * voices + k] = level(k);
        }
    }
    Voices {
        count: voices,
        frames: total,
        active,
    }
}

/// Voices speaking in each segmentation frame of the item, each with the
/// share of the windows hearing the frame that heard it there (0 when it
/// does not speak).
struct Voices {
    count: usize,
    frames: usize,
    active: Vec<f32>,
}

impl Voices {
    /// Frames whose middle falls in `from..to`.
    fn frames_in(&self, from: Time, to: Time) -> std::ops::Range<usize> {
        let middle = FRAME_SPAN as f64 / 2.0;
        let index = |t: Time| {
            let sample = t.as_secs_f64().max(0.0) * SPEECH_RATE as f64;
            (((sample - middle) / FRAME_STEP as f64).ceil().max(0.0) as usize).min(self.frames)
        };
        index(from)..index(to)
    }

    fn votes(&self, frames: std::ops::Range<usize>) -> Vec<f32> {
        let mut votes = vec![0.0; self.count];
        for t in frames {
            for (k, v) in votes.iter_mut().enumerate() {
                *v += self.active[t * self.count + k];
            }
        }
        votes
    }
}

/// The voice that speaks through most of `word`, and its share of the
/// speaking there. Where two speak at once, the windows' agreement on
/// each decides.
fn attribute(voices: &Voices, word: &Word) -> Option<(usize, f32)> {
    if voices.count == 0 {
        return None;
    }
    let (start, end) = (word.range.start, word.range.end);
    let mut frames = voices.frames_in(start, end);
    if frames.is_empty() {
        // Shorter than a frame: the one nearest its middle.
        let at = voices.frames_in(start, Time((start.0 + end.0) / 2)).end;
        frames = at.min(voices.frames - 1)..(at + 1).min(voices.frames);
    }
    let mut votes = voices.votes(frames);
    if votes.iter().all(|&v| v == 0.0) {
        votes = voices.votes(voices.frames_in(start - REACH, end + REACH));
    }
    let all: f32 = votes.iter().sum();
    let best = (0..votes.len()).max_by(|&a, &b| votes[a].total_cmp(&votes[b]))?;
    (all > 0.0).then(|| (best, votes[best] / all))
}
