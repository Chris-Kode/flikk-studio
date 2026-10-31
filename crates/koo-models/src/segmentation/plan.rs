//! What the segmentation runners share: pictures stretched onto the
//! model's square, and the memory a tracked frame attends to.

use anyhow::{Result, bail};
use koo_core::services::Frame;
use std::collections::BTreeSet;

/// Frames of memory a tracked frame attends to besides the prompted ones
/// (each at its distance), and the temporal positions there are: the last
/// is a prompted frame's.
pub const MEMORY_FRAMES: usize = 7;
/// Object pointers it attends to at most.
pub const POINTERS: usize = 16;
/// The normalization SAM 2 was trained with (ImageNet's).
pub const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
pub const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// The memory frame `i` attends to.
pub struct Bank {
    /// Frame, and the row of its temporal position: `distance - 1`, or
    /// `MEMORY_FRAMES - 1` for a prompted frame.
    pub frames: Vec<(usize, usize)>,
    /// Frame, and its distance as the pointers' positions read it.
    pub pointers: Vec<(usize, f32)>,
}

/// The memory frame `i` attends to, tracking forward (or `reverse`) through
/// `count` frames, as SAM 2 assembles it: the prompted frames (all, or with
/// `prompted_at_most` the nearest before and after when there are more),
/// the tracked frames up to `MEMORY_FRAMES - 1` back (a prompted frame left
/// out counts as one), then the pointers of the prompted frames passed and
/// of the tracked frames up to `POINTERS - 1` back, at most `POINTERS`.
pub fn bank(
    prompted: &BTreeSet<usize>,
    tracked: impl Fn(usize) -> bool,
    i: usize,
    count: usize,
    reverse: bool,
    prompted_at_most: Option<usize>,
) -> Bank {
    let back = |d: usize| -> Option<usize> {
        if reverse {
            i.checked_add(d).filter(|j| *j < count)
        } else {
            i.checked_sub(d)
        }
    };
    let chosen: Vec<usize> = match prompted_at_most {
        Some(most) if prompted.len() > most => {
            let before = prompted.range(..i).next_back().copied();
            let after = prompted.range(i..).next().copied();
            before.into_iter().chain(after).collect()
        }
        _ => prompted.iter().copied().collect(),
    };
    let mut frames: Vec<(usize, usize)> = chosen.iter().map(|k| (*k, MEMORY_FRAMES - 1)).collect();
    for d in (1..MEMORY_FRAMES).rev() {
        let Some(j) = back(d) else { continue };
        if tracked(j) || (prompted.contains(&j) && !chosen.contains(&j)) {
            frames.push((j, d - 1));
        }
    }

    let most = count.clamp(1, POINTERS);
    let scale = (most - 1).max(1) as f32;
    let mut pointers: Vec<(usize, f32)> = prompted
        .iter()
        .filter(|k| if reverse { **k >= i } else { **k <= i })
        .map(|k| (*k, k.abs_diff(i) as f32 / scale))
        .collect();
    for d in 1..most {
        let Some(j) = back(d) else { break };
        if tracked(j) {
            pointers.push((j, d as f32 / scale));
        }
    }
    pointers.truncate(POINTERS);
    Bank { frames, pointers }
}

/// `picture` stretched onto a `side` square and normalized, channels first.
/// Resampled with a triangle filter at least a source pixel wide (PIL's
/// bilinear, which SAM 2's own preprocessing uses), so a large picture is
/// averaged down rather than skipped through.
pub fn square(picture: &Frame, side: usize) -> Result<Vec<f32>> {
    let (w, h) = (picture.width as usize, picture.height as usize);
    if w == 0 || h == 0 || picture.rgba.len() < w * h * 4 {
        bail!("the picture to segment is empty");
    }
    let (cols, rows) = (taps(w, side), taps(h, side));
    let mut across = vec![0f32; h * side * 3];
    for y in 0..h {
        let src = &picture.rgba[y * w * 4..(y + 1) * w * 4];
        let dst = &mut across[y * side * 3..(y + 1) * side * 3];
        for (x, (start, weights)) in cols.iter().enumerate() {
            let mut acc = [0f32; 3];
            for (k, wt) in weights.iter().enumerate() {
                let p = &src[(start + k) * 4..(start + k) * 4 + 3];
                for ch in 0..3 {
                    acc[ch] += p[ch] as f32 * wt;
                }
            }
            dst[x * 3..x * 3 + 3].copy_from_slice(&acc);
        }
    }
    let plane = side * side;
    let mut out = vec![0f32; 3 * plane];
    let mut row = vec![0f32; side * 3];
    for (y, (start, weights)) in rows.iter().enumerate() {
        row.fill(0.0);
        for (k, wt) in weights.iter().enumerate() {
            let src = &across[(start + k) * side * 3..(start + k + 1) * side * 3];
            for (a, s) in row.iter_mut().zip(src) {
                *a += s * wt;
            }
        }
        for x in 0..side {
            for ch in 0..3 {
                let v = row[x * 3 + ch] / 255.0;
                out[ch * plane + y * side + x] = (v - MEAN[ch]) / STD[ch];
            }
        }
    }
    Ok(out)
}

/// For each of `dst` samples across `src` pixels: the first source pixel
/// it reads and the weights from there on, summing to one.
fn taps(src: usize, dst: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = src as f32 / dst as f32;
    let support = scale.max(1.0);
    (0..dst)
        .map(|i| {
            let center = (i as f32 + 0.5) * scale;
            let lo = (center - support).floor().max(0.0) as usize;
            let hi = ((center + support).ceil() as usize).clamp(lo + 1, src);
            let mut weights: Vec<f32> = (lo..hi)
                .map(|j| (1.0 - ((j as f32 + 0.5 - center) / support).abs()).max(0.0))
                .collect();
            let sum: f32 = weights.iter().sum();
            if sum > 0.0 {
                weights.iter_mut().for_each(|w| *w /= sum);
            } else {
                weights = vec![0.0; hi - lo];
                let nearest = (center.floor() as usize).clamp(lo, hi - 1) - lo;
                weights[nearest] = 1.0;
            }
            (lo, weights)
        })
        .collect()
}
