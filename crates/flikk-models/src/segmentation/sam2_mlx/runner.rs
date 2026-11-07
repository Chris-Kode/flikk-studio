//! The network run on frames: an encoded frame is handed back as its
//! features on the CPU (channels last), so it can be kept between clicks;
//! while tracking, features and memory stay on the device. A tracked frame
//! attends to every prompted frame and the latest tracked ones, as SAM 2
//! does.

use super::config::{GRID, HIDDEN, INPUT, LABEL_BOX, LABEL_OFF, LABEL_ON, LOGITS, MEMORY, WEIGHTS};
use super::net::{Decoded, Features, Memory, Net};
use crate::segmentation::plan::{self, POINTERS};
use crate::segmentation::{Encoded, Prompt, Segment, Segmenter};
use anyhow::{Result, anyhow, bail};
use flikk_core::services::Frame;
use mlx_rs::{Array, ops};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Mutex, MutexGuard};

/// The network, loaded on first use. One frame at a time.
#[derive(Default)]
pub struct Sam2Mlx {
    net: Mutex<Option<Net>>,
}

fn err(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("{e}")
}

impl Sam2Mlx {
    fn net(&self) -> Result<MutexGuard<'_, Option<Net>>> {
        let mut net = self.net.lock().unwrap_or_else(|e| e.into_inner());
        // Its files were deleted: let go of it, and fail as if never there.
        if !super::MODEL.is_downloaded() {
            *net = None;
        }
        if net.is_none() {
            *net = Some(Net::load(&WEIGHTS.locate()?)?);
        }
        Ok(net)
    }
}

/// Sizes of the features as handed back: finer, fine, coarse.
const SHAPES: [[i32; 4]; 3] = [
    [1, LOGITS, LOGITS, 32],
    [1, LOGITS / 2, LOGITS / 2, 64],
    [1, GRID, GRID, HIDDEN],
];

fn upload(frame: &Encoded) -> Result<Features> {
    let features = frame.of(super::ID)?;
    let array = |i: usize| Array::from_slice(&features[i], &SHAPES[i]);
    Ok(Features {
        fine: [array(0), array(1)],
        coarse: array(2),
    })
}

fn host(a: &Array) -> Result<Vec<f32>> {
    a.eval().map_err(err)?;
    Ok(a.as_slice::<f32>().to_vec())
}

/// `prompt` as labelled points in the model's square: a box's corners,
/// then the clicks.
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
    out
}

fn segment(d: &Decoded) -> Result<Segment> {
    Ok(Segment {
        logits: host(&d.logits)?,
        side: LOGITS as usize,
        score: d.score.clamp(0.0, 1.0),
        present: d.object_score > 0.0,
    })
}

impl Segmenter for Sam2Mlx {
    fn encode(&self, picture: &Frame) -> Result<Encoded> {
        let net = self.net()?;
        let net = net.as_ref().expect("loaded");
        let f = net.encode(&plan::square(picture, INPUT as usize)?)?;
        Ok(Encoded {
            model: super::ID,
            features: vec![host(&f.fine[0])?, host(&f.fine[1])?, host(&f.coarse)?],
        })
    }

    fn segment(&self, frame: &Encoded, prompt: &Prompt) -> Result<Segment> {
        let net = self.net()?;
        let net = net.as_ref().expect("loaded");
        let f = upload(frame)?;
        let cond = net.without_memory(&f.coarse)?;
        segment(&net.decode(&f.fine, &cond, &points(prompt))?)
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
        let net = self.net()?;
        let net = net.as_ref().expect("loaded");
        let mut prompted: BTreeMap<usize, Memory> = BTreeMap::new();
        for (i, prompt) in prompts {
            let f = net.encode(&plan::square(&frame(*i)?, INPUT as usize)?)?;
            let cond = net.without_memory(&f.coarse)?;
            let decoded = net.decode(&f.fine, &cond, &points(prompt))?;
            prompted.insert(*i, net.memorize(&f.coarse, &decoded, true)?);
            done(*i, segment(&decoded)?)?;
        }
        let keys: BTreeSet<usize> = prompted.keys().copied().collect();
        let forward = (first + 1..count).collect::<Vec<_>>();
        let backward = (0..first).rev().collect::<Vec<_>>();
        for (order, reverse) in [(forward, false), (backward, true)] {
            let mut tracked: HashMap<usize, Memory> = HashMap::new();
            for i in order {
                if prompted.contains_key(&i) {
                    continue;
                }
                let f = net.encode(&plan::square(&frame(i)?, INPUT as usize)?)?;
                let bank = plan::bank(&keys, |j| tracked.contains_key(&j), i, count, reverse, None);
                let memory_of = |j: &usize| {
                    tracked
                        .get(j)
                        .or_else(|| prompted.get(j))
                        .expect("in the bank")
                };
                let mut tokens = Vec::new();
                let mut positions = Vec::new();
                for (j, row) in &bank.frames {
                    let m = memory_of(j);
                    tokens.push(m.tokens.clone());
                    positions.push(m.positions.add(net.temporal(*row)?).map_err(err)?);
                }
                let distances: Vec<f32> = bank.pointers.iter().map(|(_, d)| *d).collect();
                let split = HIDDEN / MEMORY;
                let pointers = bank.pointers.len() as i32;
                let pointer_tokens: Vec<Array> = bank
                    .pointers
                    .iter()
                    .map(|(j, _)| memory_of(j).pointer.reshape(&[split, MEMORY]).map_err(err))
                    .collect::<Result<_>>()?;
                if !pointer_tokens.is_empty() {
                    let pos = net.pointer_positions(&distances)?;
                    tokens.push(ops::concatenate(&pointer_tokens, 0).map_err(err)?);
                    positions.push(
                        pos.expand_dims(1)
                            .and_then(|p| ops::broadcast_to(&p, &[pointers, split, MEMORY]))
                            .and_then(|p| p.reshape(&[-1, MEMORY]))
                            .map_err(err)?,
                    );
                }
                debug_assert!(pointers as usize <= POINTERS);
                let memory = ops::concatenate(&tokens, 0).map_err(err)?;
                let positions = ops::concatenate(&positions, 0).map_err(err)?;
                let cond = net.attend_memory(&f.coarse, &memory, &positions, pointers * split)?;
                let decoded = net.decode(&f.fine, &cond, &[])?;
                tracked.insert(i, net.memorize(&f.coarse, &decoded, false)?);
                // Only the frames the bank can still reach.
                let reach = plan::MEMORY_FRAMES.max(POINTERS);
                tracked.retain(|j, _| j.abs_diff(i) < reach);
                done(i, segment(&decoded)?)?;
            }
        }
        Ok(())
    }
}
