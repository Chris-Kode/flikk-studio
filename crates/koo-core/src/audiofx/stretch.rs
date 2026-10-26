//! Pitch-preserving time stretch (WSOLA) for retimed clips in preview; the
//! render uses FFmpeg's `atempo`, which works the same way.
//!
//! Output is built from 40 ms Hann-windowed grains at 50 % overlap. Grain
//! `k` is read near input position `k · hop · speed`, shifted within
//! ±10 ms to the offset that best continues the previous grain (highest
//! normalized cross-correlation), so waveforms line up and pitch stays put.

use std::collections::VecDeque;

/// Interleaved stereo source: fill the slice, return samples written
/// (0 at the end).
pub type Pull<'a> = dyn FnMut(&mut [f32]) -> usize + 'a;

pub struct Stretch {
    speed: f64,
    /// Grain length in frames.
    n: usize,
    /// Output hop, n / 2.
    hop: usize,
    /// Search radius in frames.
    tol: usize,
    window: Vec<f32>,
    /// Input frames from absolute frame `base` on, interleaved stereo.
    input: Vec<f32>,
    base: u64,
    eof: bool,
    /// Grains placed so far.
    k: u64,
    /// Absolute input frame of the last grain.
    prev: Option<u64>,
    /// Overlap-add accumulator, n frames.
    acc: Vec<f32>,
    ready: VecDeque<f32>,
}

impl Stretch {
    pub fn new(speed: f64, rate: u32) -> Self {
        let rate = rate.max(8000) as f64;
        let n = ((0.04 * rate) as usize / 2 * 2).max(64);
        let window = (0..n)
            .map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()) as f32)
            .collect();
        Self {
            speed: speed.clamp(0.01, 100.0),
            n,
            hop: n / 2,
            tol: (0.01 * rate) as usize,
            window,
            input: Vec::new(),
            base: 0,
            eof: false,
            k: 0,
            prev: None,
            acc: vec![0.0; n * 2],
            ready: VecDeque::new(),
        }
    }

    /// Fill `out` (interleaved stereo) with stretched audio pulled from
    /// `pull`; zeros once the source has ended.
    pub fn read(&mut self, out: &mut [f32], pull: &mut Pull) {
        while self.ready.len() < out.len() {
            self.grain(pull);
        }
        let len = out.len();
        for (o, v) in out.iter_mut().zip(self.ready.drain(..len)) {
            *o = v;
        }
    }

    /// Make sure input covers absolute frames up to `end`.
    fn fill(&mut self, end: u64, pull: &mut Pull) {
        let have = self.base + (self.input.len() / 2) as u64;
        if end <= have {
            return;
        }
        let want = ((end - have) as usize) * 2;
        let start = self.input.len();
        self.input.resize(start + want, 0.0);
        let mut filled = 0;
        while !self.eof && filled < want {
            let n = pull(&mut self.input[start + filled..start + want]);
            if n == 0 {
                self.eof = true;
            }
            filled += n;
        }
    }

    /// Mono sample at absolute frame `i`.
    #[inline]
    fn mono(&self, i: u64) -> f32 {
        let j = ((i - self.base) as usize) * 2;
        self.input[j] + self.input[j + 1]
    }

    fn grain(&mut self, pull: &mut Pull) {
        let (n, hop) = (self.n as u64, self.hop as u64);
        let nominal = (self.k as f64 * hop as f64 * self.speed).round() as u64;
        let lo = nominal.saturating_sub(self.tol as u64).max(self.base);
        let hi = nominal + self.tol as u64;
        self.fill(hi.max(nominal) + n, pull);
        let pos = match self.prev {
            None => nominal.max(self.base),
            Some(prev) => {
                // The grain that best continues the previous one.
                let template = prev + hop;
                let corr = |c: u64, step: usize| {
                    let (mut xy, mut yy) = (0f32, 1e-9f32);
                    for i in (0..hop).step_by(step) {
                        let y = self.mono(c + i);
                        xy += self.mono(template + i) * y;
                        yy += y * y;
                    }
                    xy / yy.sqrt()
                };
                let mut best = (f32::MIN, nominal.clamp(lo, hi));
                for c in (lo..=hi).step_by(2) {
                    let v = corr(c, 4);
                    if v > best.0 {
                        best = (v, c);
                    }
                }
                let around = best.1;
                for c in [around.saturating_sub(1).max(lo), (around + 1).min(hi)] {
                    let v = corr(c, 4);
                    if v > best.0 {
                        best = (v, c);
                    }
                }
                best.1
            }
        };
        let first = self.prev.is_none();
        for i in 0..self.n {
            // No grain before the first: its rising half is flat.
            let w = if first && i < self.hop {
                1.0
            } else {
                self.window[i]
            };
            let j = ((pos - self.base) as usize + i) * 2;
            self.acc[i * 2] += self.input[j] * w;
            self.acc[i * 2 + 1] += self.input[j + 1] * w;
        }
        self.ready.extend(self.acc.drain(..self.hop * 2));
        self.acc.resize(self.n * 2, 0.0);
        self.prev = Some(pos);
        self.k += 1;
        // Drop input no later grain or template can reach.
        let next = ((self.k as f64 * hop as f64 * self.speed).round() as u64)
            .saturating_sub(self.tol as u64);
        let keep = next.min(pos + hop);
        if keep > self.base {
            let drop = ((keep - self.base) as usize).min(self.input.len() / 2);
            self.input.drain(..drop * 2);
            self.base += drop as u64;
        }
    }
}
