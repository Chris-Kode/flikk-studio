//! Video scope readings from a frame (waveform, parade, vectorscope and
//! histogram in numbers), and shot matching from them. The Color workspace
//! draws the scopes; the agent reads the numbers (`get_scopes`) and asks for
//! matches (`match_grade`). Readings use the same color math as
//! [`crate::grade`]: Rec. 709 luma, and a chroma measure that scales exactly
//! with [`Grade::saturation`].

use crate::grade::{self, Curve};
use crate::model::{Curves, Grade};
use crate::services::Frame;
use serde::{Deserialize, Serialize};

/// Distribution of one channel, 0..1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub min: f32,
    pub p5: f32,
    pub mean: f32,
    pub median: f32,
    pub p95: f32,
    pub max: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScopeReadings {
    /// Rec. 709 luma.
    pub luma: Stats,
    pub red: Stats,
    pub green: Stats,
    pub blue: Stats,
    /// Chroma, `max(r, g, b) − min(r, g, b)`: scales linearly with
    /// [`Grade::saturation`].
    pub saturation: Stats,
    /// Fraction of pixels with luma ≤ 1/255 and ≥ 254/255.
    pub crushed: f32,
    pub clipped: f32,
    /// Mean chroma (Cb, Cr), each -0.5..0.5: the vectorscope's centre of
    /// mass. Away from zero means a color cast.
    pub cast: [f32; 2],
    /// 64-bin histograms of luma, red, green, blue, each summing to 1.
    pub histogram: [Vec<f32>; 4],
}

const BINS: usize = 256;

fn stats(hist: &[u32; BINS], n: u64) -> Stats {
    if n == 0 {
        return Stats::default();
    }
    let at = |q: f64| -> f32 {
        let target = (q * n as f64).ceil().max(1.0) as u64;
        let mut acc = 0u64;
        for (i, c) in hist.iter().enumerate() {
            acc += *c as u64;
            if acc >= target {
                return i as f32 / 255.0;
            }
        }
        1.0
    };
    let sum: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, c)| i as f64 * *c as f64)
        .sum();
    Stats {
        min: at(0.0),
        p5: at(0.05),
        mean: (sum / n as f64 / 255.0) as f32,
        median: at(0.5),
        p95: at(0.95),
        max: at(1.0),
    }
}

fn coarse(hist: &[u32; BINS], n: u64) -> Vec<f32> {
    let mut out = vec![0f32; 64];
    for (i, c) in hist.iter().enumerate() {
        out[i / 4] += *c as f32;
    }
    if n > 0 {
        out.iter_mut().for_each(|v| *v /= n as f32);
    }
    out
}

/// Readings over every opaque pixel of `frame` (at most ~250k sampled).
pub fn measure(frame: &Frame) -> ScopeReadings {
    let px = (frame.width as usize) * (frame.height as usize);
    let step = (px / 250_000).max(1);
    let mut h = [[0u32; BINS]; 5];
    let (mut n, mut crushed, mut clipped) = (0u64, 0u64, 0u64);
    let (mut cb, mut cr) = (0f64, 0f64);
    for p in frame.rgba.as_chunks::<4>().0.iter().step_by(step) {
        if p[3] < 8 {
            continue;
        }
        let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
        let [kr, kg, kb] = grade::LUMA;
        let y = kr * r + kg * g + kb * b;
        let chroma = r.max(g).max(b) - r.min(g).min(b);
        h[0][y.round().clamp(0.0, 255.0) as usize] += 1;
        h[1][p[0] as usize] += 1;
        h[2][p[1] as usize] += 1;
        h[3][p[2] as usize] += 1;
        h[4][chroma as usize] += 1;
        crushed += (y <= 1.0) as u64;
        clipped += (y >= 254.0) as u64;
        cb += ((b - y) / 1.8556 / 255.0) as f64;
        cr += ((r - y) / 1.5748 / 255.0) as f64;
        n += 1;
    }
    let frac = |v: u64| if n == 0 { 0.0 } else { v as f32 / n as f32 };
    ScopeReadings {
        luma: stats(&h[0], n),
        red: stats(&h[1], n),
        green: stats(&h[2], n),
        blue: stats(&h[3], n),
        saturation: stats(&h[4], n),
        crushed: frac(crushed),
        clipped: frac(clipped),
        cast: if n == 0 {
            [0.0; 2]
        } else {
            [(cb / n as f64) as f32, (cr / n as f64) as f32]
        },
        histogram: [
            coarse(&h[0], n),
            coarse(&h[1], n),
            coarse(&h[2], n),
            coarse(&h[3], n),
        ],
    }
}

/// A grade for the target shot that brings its readings toward the
/// reference's. `target` reads the target graded with `current` (what the
/// scopes show for it) and `reference` the reference with its own grade.
///
/// Curves are the last stage of the grade, so the correction is exact to
/// compose: each channel gets a monotone curve taking the target's 5th
/// percentile, median and 95th percentile onto the reference's (contrast
/// limited to ×0.5..×2), folded with `current`'s curves. Other fields of
/// `current` are kept.
pub fn match_grade(reference: &ScopeReadings, target: &ScopeReadings, current: &Grade) -> Grade {
    let channels = [
        (&reference.red, &target.red),
        (&reference.green, &target.green),
        (&reference.blue, &target.blue),
    ];
    let maps: Vec<Option<Curve>> = channels
        .iter()
        .map(|(r, t)| Curve::new(&mapping(r, t)))
        .collect();
    let mut g = current.clone();
    let old = &current.curves;
    let fold = |points: &[[f32; 2]], i: usize| -> Vec<[f32; 2]> {
        let chan = Curve::new(points);
        let master = Curve::new(&old.master);
        if chan.is_none() && master.is_none() {
            return mapping(channels[i].0, channels[i].1);
        }
        (0..=8)
            .map(|k| {
                let x = k as f32 / 8.0;
                let v = chan.as_ref().map_or(x, |c| c.eval(x));
                let v = master.as_ref().map_or(v, |c| c.eval(v));
                [x, maps[i].as_ref().map_or(v, |m| m.eval(v))]
            })
            .collect()
    };
    g.curves = Curves {
        master: Vec::new(),
        red: fold(&old.red, 0),
        green: fold(&old.green, 1),
        blue: fold(&old.blue, 2),
    };
    g
}

/// Control points taking `t`'s distribution onto `r`'s, end points included.
fn mapping(r: &Stats, t: &Stats) -> Vec<[f32; 2]> {
    let t_range = (t.p95 - t.p5).max(0.02);
    let r_range = (r.p95 - r.p5).max(0.02);
    let a = (r_range / t_range).clamp(0.5, 2.0);
    let y = |x: f32| (r.p5 + (x - t.p5) * a).clamp(0.0, 1.0);
    let mut p = vec![[0.0, y(0.0)], [t.p5, r.p5.clamp(0.0, 1.0)]];
    // Midtones: where the reference's median sits in its range.
    let tm = (t.median - t.p5) / t_range;
    let rm = ((r.median - r.p5) / r_range).clamp(0.0, 1.0);
    if (0.1..=0.9).contains(&tm) && t.p95 - t.p5 >= 0.05 {
        p.push([t.median, (r.p5 + rm * t_range * a).clamp(0.0, 1.0)]);
    }
    p.push([t.p95, y(t.p95)]);
    p.push([1.0, y(1.0)]);
    p.dedup_by(|b, a| b[0] - a[0] < 0.01);
    // Keep the curve rising.
    for k in 1..p.len() {
        p[k][1] = p[k][1].max(p[k - 1][1]);
    }
    p
}

/// Match `target` (graded with `current`) to `reference` (graded with
/// `reference_grade`): both frames are graded here, then measured.
pub fn match_frames(
    reference: &Frame,
    reference_grade: &Grade,
    target: &Frame,
    current: &Grade,
) -> Grade {
    let r = measure(&grade::graded(reference, &[reference_grade]));
    let t = measure(&grade::graded(target, &[current]));
    match_grade(&r, &t, current)
}
