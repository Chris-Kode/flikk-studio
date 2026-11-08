//! The graphs run on frames: pictures stretched onto the model's square and
//! normalized, prompts as labelled points (a box's corners first), and the
//! memory a tracked frame attends to assembled from the prompted frames and
//! the latest tracked ones. The attention takes a fixed number of each, so
//! a bank with fewer repeats its latest frame and its last pointer.

use super::config::{
    CONSTANTS, DECODER, ENCODER, FINE, GRID, HIDDEN, INPUT, LABEL_BOX, LABEL_OFF, LABEL_ON,
    LABEL_PAD, MEMORY, MEMORY_ATTENTION, MEMORY_ENCODER, POINTER_POSITIONS, PROMPTED_FRAMES,
    TOKENS,
};
use crate::onnx;
use crate::segmentation::plan::{self, MEMORY_FRAMES, POINTERS};
use crate::segmentation::{Encoded, Prompt, Segment, Segmenter};
use anyhow::{Result, anyhow, bail};
use flikk_core::services::Frame;
use flikk_os::sync::lock;
use ort::session::Session;
use ort::value::Tensor;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Mutex, MutexGuard};

/// Where each of the encoder's outputs sits in [`Encoded::features`].
const FEATS0: usize = 0;
const FEATS1: usize = 1;
const FEATS2: usize = 2;
const FEATS2_NO_MEMORY: usize = 3;
const POSITIONS: usize = 4;
const ENCODER_OUTPUTS: [&str; 5] = [
    "feats0",
    "feats1",
    "feats2",
    "feats2_no_mem",
    "vision_pos_embed",
];
/// Side of the decoder's logits, and of its larger copy the memory
/// encoder reads.
const LOGITS: usize = 256;

/// The graphs, loaded on first use; the memory graphs only once something
/// is tracked. One frame at a time.
#[derive(Default)]
pub struct Sam2 {
    graphs: Mutex<Option<Graphs>>,
}

struct Graphs {
    encoder: Session,
    decoder: Session,
    memory: Option<MemoryGraphs>,
    constants: Constants,
}

struct MemoryGraphs {
    encoder: Session,
    attention: Session,
    pointers: Session,
}

struct Constants {
    /// The temporal position of a memory frame by its distance (row
    /// `distance - 1`); the last row is a prompted frame's.
    temporal: Vec<f32>,
}

/// What a decoder run gives besides the [`Segment`]: what the memory
/// encoder reads, and the object's pointer.
struct Decoded {
    segment: Segment,
    high_res: Vec<f32>,
    object_score: f32,
    pointer: Vec<f32>,
}

/// One frame's memory: its tokens, their positions, and the object's
/// pointer.
struct Memory {
    tokens: Vec<f32>,
    positions: Vec<f32>,
    pointer: Vec<f32>,
}

impl Sam2 {
    fn graphs(&self, memory: bool) -> Result<MutexGuard<'_, Option<Graphs>>> {
        let mut graphs = lock(&self.graphs);
        // Its files were deleted: let go of it, and fail as if never there.
        if !super::MODEL.is_downloaded() {
            *graphs = None;
        }
        if graphs.is_none() {
            *graphs = Some(Graphs {
                encoder: onnx::session(&ENCODER.locate()?, ENCODER.what)?,
                decoder: onnx::session(&DECODER.locate()?, DECODER.what)?,
                memory: None,
                constants: Constants::load()?,
            });
        }
        let loaded = graphs.as_mut().expect("loaded above");
        if memory && loaded.memory.is_none() {
            loaded.memory = Some(MemoryGraphs {
                encoder: onnx::session(&MEMORY_ENCODER.locate()?, MEMORY_ENCODER.what)?,
                attention: onnx::session(&MEMORY_ATTENTION.locate()?, MEMORY_ATTENTION.what)?,
                pointers: onnx::session(&POINTER_POSITIONS.locate()?, POINTER_POSITIONS.what)?,
            });
        }
        Ok(graphs)
    }
}

impl Segmenter for Sam2 {
    fn encode(&self, picture: &Frame) -> Result<Encoded> {
        let mut graphs = self.graphs(false)?;
        encode(graphs.as_mut().expect("loaded"), picture)
    }

    fn segment(&self, frame: &Encoded, prompt: &Prompt) -> Result<Segment> {
        let mut graphs = self.graphs(false)?;
        let g = graphs.as_mut().expect("loaded");
        let cond = &frame.of(super::ID)?[FEATS2_NO_MEMORY];
        Ok(decode(g, frame, cond, &points(prompt))?.segment)
    }

    fn track(
        &self,
        count: usize,
        prompts: &[(usize, Prompt)],
        frame: &mut dyn FnMut(usize) -> Result<Frame>,
        done: &mut dyn FnMut(usize, Segment) -> Result<()>,
    ) -> Result<()> {
        let prompts: Vec<&(usize, Prompt)> = prompts.iter().filter(|(i, _)| *i < count).collect();
        let Some(first) = prompts.iter().map(|(i, _)| *i).min() else {
            bail!("nothing picks the object to follow");
        };
        let mut graphs = self.graphs(true)?;
        let g = graphs.as_mut().expect("loaded");
        // Each prompted frame by itself, as when it was picked.
        let mut prompted: BTreeMap<usize, Memory> = BTreeMap::new();
        for (i, prompt) in prompts {
            let encoded = encode(g, &frame(*i)?)?;
            let decoded = decode(
                g,
                &encoded,
                &encoded.features[FEATS2_NO_MEMORY],
                &points(prompt),
            )?;
            let memory = memorize(g, &encoded, &decoded, true)?;
            prompted.insert(*i, memory);
            done(*i, decoded.segment)?;
        }
        let forward = (first + 1..count).collect::<Vec<_>>();
        let backward = (0..first).rev().collect::<Vec<_>>();
        for (order, reverse) in [(forward, false), (backward, true)] {
            let mut tracked: HashMap<usize, Memory> = HashMap::new();
            for i in order {
                if prompted.contains_key(&i) {
                    continue;
                }
                let encoded = encode(g, &frame(i)?)?;
                let (memory, positions) = bank(g, &prompted, &tracked, i, count, reverse)?;
                let conditioned = attend(g, &encoded, memory, positions)?;
                let decoded = decode(g, &encoded, &conditioned, &[([0.0, 0.0], LABEL_PAD)])?;
                let memory = memorize(g, &encoded, &decoded, false)?;
                tracked.insert(i, memory);
                // Only the frames the bank can still reach.
                let reach = MEMORY_FRAMES.max(POINTERS);
                tracked.retain(|j, _| j.abs_diff(i) < reach);
                done(i, decoded.segment)?;
            }
        }
        Ok(())
    }
}

impl Constants {
    fn load() -> Result<Self> {
        let path = CONSTANTS.locate()?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow!("could not read {}: {e}", path.display()))?;
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| anyhow!("{} is not JSON: {e}", path.display()))?;
        let numbers = |v: &serde_json::Value| -> Vec<f32> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|n| n.as_f64())
                        .map(|n| n as f32)
                        .collect()
                })
                .unwrap_or_default()
        };
        let temporal: Vec<f32> = json["memory_temporal_positional_encoding"]
            .as_array()
            .map(|rows| rows.iter().flat_map(numbers).collect())
            .unwrap_or_default();
        if temporal.len() != MEMORY_FRAMES * MEMORY {
            bail!("{} has no temporal position table", path.display());
        }
        Ok(Self { temporal })
    }

    fn temporal(&self, row: usize) -> &[f32] {
        &self.temporal[row * MEMORY..(row + 1) * MEMORY]
    }
}

fn tensor<T: ort::value::PrimitiveTensorElementType + std::fmt::Debug + Clone + 'static>(
    shape: &[usize],
    data: Vec<T>,
) -> Result<Tensor<T>> {
    Tensor::from_array((shape.to_vec(), data)).map_err(|e| anyhow!("{e}"))
}

fn encode(g: &mut Graphs, picture: &Frame) -> Result<Encoded> {
    let input = tensor(&[1, 3, INPUT, INPUT], plan::square(picture, INPUT)?)?;
    let outputs = g
        .encoder
        .run(ort::inputs!["pixel_values" => input])
        .map_err(|e| anyhow!("subject mask encoder failed: {e}"))?;
    let mut features = Vec::with_capacity(ENCODER_OUTPUTS.len());
    for name in ENCODER_OUTPUTS {
        let (_, v) = outputs[name]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        features.push(v.to_vec());
    }
    let expect = [
        FINE[0].0 * FINE[0].1 * FINE[0].1,
        FINE[1].0 * FINE[1].1 * FINE[1].1,
        HIDDEN * TOKENS,
        HIDDEN * TOKENS,
        HIDDEN * TOKENS,
    ];
    if features.iter().map(Vec::len).ne(expect) {
        bail!("subject mask encoder gave features of unexpected sizes");
    }
    Ok(Encoded {
        model: super::ID,
        features,
    })
}

/// `prompt` as the decoder's labelled points in its square's pixels: a
/// box's corners, then the clicks; a padding point when there is nothing.
fn points(prompt: &Prompt) -> Vec<([f32; 2], i32)> {
    let s = INPUT as f32;
    let mut out = Vec::new();
    if let Some([u0, v0, u1, v1]) = prompt.rect {
        out.push(([u0.min(u1) * s, v0.min(v1) * s], LABEL_BOX[0]));
        out.push(([u0.max(u1) * s, v0.max(v1) * s], LABEL_BOX[1]));
    }
    for (p, on) in &prompt.points {
        out.push(([p[0] * s, p[1] * s], if *on { LABEL_ON } else { LABEL_OFF }));
    }
    if out.is_empty() {
        out.push(([0.0, 0.0], LABEL_PAD));
    }
    out
}

/// The object in `frame` whose coarsest features are `cond` (the frame's
/// own, or conditioned on memory), as `points` pick it.
fn decode(
    g: &mut Graphs,
    frame: &Encoded,
    cond: &[f32],
    points: &[([f32; 2], i32)],
) -> Result<Decoded> {
    let n = points.len();
    let coords: Vec<f32> = points.iter().flat_map(|(p, _)| *p).collect();
    let labels: Vec<i32> = points.iter().map(|(_, l)| *l).collect();
    let inputs = ort::inputs![
        "feats0" => tensor(&[1, FINE[0].0, FINE[0].1, FINE[0].1], frame.features[FEATS0].clone())?,
        "feats1" => tensor(&[1, FINE[1].0, FINE[1].1, FINE[1].1], frame.features[FEATS1].clone())?,
        "feats2_cond" => tensor(&[1, HIDDEN, GRID, GRID], cond.to_vec())?,
        "input_points" => tensor(&[1, 1, n, 2], coords)?,
        "input_labels" => tensor(&[1, 1, n], labels)?,
    ];
    let outputs = g
        .decoder
        .run(inputs)
        .map_err(|e| anyhow!("subject mask decoder failed: {e}"))?;
    let get = |name: &str| -> Result<Vec<f32>> {
        let (_, v) = outputs[name]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        Ok(v.to_vec())
    };
    let logits = get("low_res_mask")?;
    let high_res = get("high_res_mask")?;
    let score = get("iou")?.first().copied().unwrap_or(0.0);
    let object_score = get("object_score_logits")?.first().copied().unwrap_or(0.0);
    let pointer = get("object_pointer")?;
    if logits.len() != LOGITS * LOGITS || high_res.len() != INPUT * INPUT || pointer.len() != HIDDEN
    {
        bail!("subject mask decoder gave outputs of unexpected sizes");
    }
    Ok(Decoded {
        segment: Segment {
            logits,
            side: LOGITS,
            score: score.clamp(0.0, 1.0),
            present: object_score > 0.0,
        },
        high_res,
        object_score,
        pointer,
    })
}

/// The memory of a decoded frame; a prompted frame's mask is made binary.
fn memorize(g: &mut Graphs, frame: &Encoded, d: &Decoded, prompted: bool) -> Result<Memory> {
    let m = g.memory.as_mut().expect("memory graphs loaded");
    let inputs = ort::inputs![
        "feats2" => tensor(&[1, HIDDEN, GRID, GRID], frame.features[FEATS2].clone())?,
        "high_res_mask" => tensor(&[1, 1, INPUT, INPUT], d.high_res.clone())?,
        "object_score_logits" => tensor(&[1, 1], vec![d.object_score])?,
        "binarize" => Tensor::from_array(((), vec![if prompted { 1.0f32 } else { 0.0 }]))
            .map_err(|e| anyhow!("{e}"))?,
    ];
    let outputs = m
        .encoder
        .run(inputs)
        .map_err(|e| anyhow!("subject tracking memory failed: {e}"))?;
    let get = |name: &str| -> Result<Vec<f32>> {
        let (_, v) = outputs[name]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        Ok(v.to_vec())
    };
    let (tokens, positions) = (get("memory_tokens")?, get("memory_pos")?);
    if tokens.len() != TOKENS * MEMORY || positions.len() != TOKENS * MEMORY {
        bail!("subject tracking memory gave outputs of unexpected sizes");
    }
    Ok(Memory {
        tokens,
        positions,
        pointer: d.pointer.clone(),
    })
}

/// The memory frame `i` attends to (`plan::bank`, the nearest two prompted
/// frames at most) and the positions of its tokens, padded to the fixed
/// sizes the attention takes by repeating the latest frame and the last
/// pointer, each pointer split into tokens.
fn bank(
    g: &mut Graphs,
    prompted: &BTreeMap<usize, Memory>,
    tracked: &HashMap<usize, Memory>,
    i: usize,
    count: usize,
    reverse: bool,
) -> Result<(Vec<f32>, Vec<f32>)> {
    let keys: BTreeSet<usize> = prompted.keys().copied().collect();
    let bank = plan::bank(
        &keys,
        |j| tracked.contains_key(&j),
        i,
        count,
        reverse,
        Some(PROMPTED_FRAMES),
    );
    let memory_of = |j: &usize| {
        tracked
            .get(j)
            .or_else(|| prompted.get(j))
            .expect("in the bank")
    };
    let mut frames: Vec<(&Memory, usize)> = bank
        .frames
        .iter()
        .map(|(j, row)| (memory_of(j), *row))
        .collect();
    // The latest are last: when the bank is full, the farthest go.
    let extra = frames.len().saturating_sub(MEMORY_FRAMES);
    let chosen = frames
        .iter()
        .filter(|(_, row)| *row == MEMORY_FRAMES - 1)
        .count();
    frames.drain(chosen..chosen + extra);
    while frames.len() < MEMORY_FRAMES {
        let last = *frames.last().expect("a prompted frame");
        frames.push(last);
    }
    let pointers: Vec<(&[f32], f32)> = bank
        .pointers
        .iter()
        .map(|(j, d)| (memory_of(j).pointer.as_slice(), *d))
        .collect();
    let distances: Vec<f32> = pointers.iter().map(|(_, d)| *d).collect();
    let n = distances.len();
    let m = g.memory.as_mut().expect("memory graphs loaded");
    let outputs = m
        .pointers
        .run(ort::inputs!["normalized_diffs" => tensor(&[n], distances)?])
        .map_err(|e| anyhow!("subject tracking pointers failed: {e}"))?;
    let (_, pointer_positions) = outputs["pointer_pos"]
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow!("{e}"))?;
    if pointer_positions.len() != n * MEMORY {
        bail!("subject tracking pointers gave outputs of unexpected sizes");
    }

    let split = HIDDEN / MEMORY;
    let len = MEMORY_FRAMES * TOKENS * MEMORY + POINTERS * split * MEMORY;
    let (mut memory, mut positions) = (Vec::with_capacity(len), Vec::with_capacity(len));
    for (m, row) in &frames {
        memory.extend_from_slice(&m.tokens);
        let temporal = g.constants.temporal(*row);
        positions.extend(
            m.positions
                .as_chunks::<MEMORY>()
                .0
                .iter()
                .flat_map(|p| p.iter().zip(temporal).map(|(a, b)| a + b)),
        );
    }
    for k in 0..POINTERS {
        let k = k.min(n - 1);
        memory.extend_from_slice(pointers[k].0);
        let position = &pointer_positions[k * MEMORY..(k + 1) * MEMORY];
        for _ in 0..split {
            positions.extend_from_slice(position);
        }
    }
    Ok((memory, positions))
}

/// `frame`'s coarsest features conditioned on `memory`.
fn attend(
    g: &mut Graphs,
    frame: &Encoded,
    memory: Vec<f32>,
    positions: Vec<f32>,
) -> Result<Vec<f32>> {
    let m = g.memory.as_mut().expect("memory graphs loaded");
    let len = memory.len() / MEMORY;
    let inputs = ort::inputs![
        "current_vision_features" => tensor(&[TOKENS, 1, HIDDEN], tokens(&frame.features[FEATS2], HIDDEN))?,
        "current_vision_position_embeddings" => tensor(&[TOKENS, 1, HIDDEN], tokens(&frame.features[POSITIONS], HIDDEN))?,
        "memory" => tensor(&[len, 1, MEMORY], memory)?,
        "memory_pos" => tensor(&[len, 1, MEMORY], positions)?,
    ];
    let outputs = m
        .attention
        .run(inputs)
        .map_err(|e| anyhow!("subject tracking attention failed: {e}"))?;
    let (_, v) = outputs["conditioned_feats"]
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow!("{e}"))?;
    if v.len() != HIDDEN * TOKENS {
        bail!("subject tracking attention gave outputs of unexpected sizes");
    }
    Ok(v.to_vec())
}

/// A channels-first map of the coarsest grid as one token per cell.
fn tokens(map: &[f32], channels: usize) -> Vec<f32> {
    let mut out = vec![0.0; map.len()];
    for c in 0..channels {
        for (t, v) in map[c * TOKENS..(c + 1) * TOKENS].iter().enumerate() {
            out[t * channels + c] = *v;
        }
    }
    out
}
