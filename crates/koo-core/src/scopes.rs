//! Video scope readings from a frame (waveform, parade, vectorscope and
//! histogram in numbers), and shot matching from them. The Color workspace
//! draws the scopes; the agent reads the numbers (`get_scopes`) and asks for
//! matches (`match_grade`).

use crate::model::Grade;
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
    /// HSV saturation.
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
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let s = if max > 0.0 { (max - min) / max } else { 0.0 };
        h[0][y.round().clamp(0.0, 255.0) as usize] += 1;
        h[1][p[0] as usize] += 1;
        h[2][p[1] as usize] += 1;
        h[3][p[2] as usize] += 1;
        h[4][(s * 255.0).round() as usize] += 1;
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
/// reference's: black and white points through lift and gain per channel,
/// midtones through gamma, and saturation. Starts from `current`, so a match
/// refines an existing grade rather than replacing it.
pub fn match_grade(reference: &ScopeReadings, target: &ScopeReadings, current: &Grade) -> Grade {
    let mut g = current.clone();
    let channels = [
        (&reference.red, &target.red),
        (&reference.green, &target.green),
        (&reference.blue, &target.blue),
    ];
    for (i, (r, t)) in channels.iter().enumerate() {
        let t_range = (t.p95 - t.p5).max(0.02);
        let r_range = (r.p95 - r.p5).max(0.02);
        let gain = (r_range / t_range).clamp(0.5, 2.0);
        g.gain[i] = (current.gain[i] * gain).clamp(0.25, 4.0);
        g.lift[i] = (current.lift[i] + (r.p5 - t.p5 * gain)).clamp(-0.5, 0.5);
        // Midtones: gamma that maps the target's median onto the reference's.
        let tm = ((t.median - t.p5) / t_range).clamp(0.02, 0.98);
        let rm = ((r.median - r.p5) / r_range).clamp(0.02, 0.98);
        let gamma = (rm.ln() / tm.ln()).clamp(0.5, 2.0);
        g.gamma_rgb[i] = (current.gamma_rgb[i] / gamma).clamp(0.4, 2.5);
    }
    if target.saturation.mean > 0.01 {
        let s = reference.saturation.mean / target.saturation.mean;
        g.saturation = (current.saturation * s).clamp(0.0, 2.0);
    }
    g
}
