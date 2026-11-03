//! Playing a clip a hair faster or slower by resampling, as a recorder
//! whose clock runs fast does, rather than by time-stretching: for the
//! tiny speed changes that correct clock drift (at most
//! [`VARISPEED_LIMIT`], under two cents of pitch). Output frame `n` is the
//! input at `n · speed`, cubic-interpolated, so every sample lands where
//! the timeline says. The preview mixer and the render (which renders such
//! a clip's sound through this before mixing) share it, so they match to
//! the sample.

/// Largest speed change played by resampling; larger ones time-stretch.
pub const VARISPEED_LIMIT: f64 = 1e-3;

/// Whether a clip at `speed` plays through [`Varispeed`].
pub fn is_varispeed(speed: f64) -> bool {
    let d = (speed - 1.0).abs();
    d > 1e-9 && d <= VARISPEED_LIMIT
}

/// Interleaved stereo source: fill the slice, return samples written
/// (0 at the end).
pub type Pull<'a> = dyn FnMut(&mut [f32]) -> usize + 'a;

pub struct Varispeed {
    speed: f64,
    /// Input frames from absolute frame `base` on, interleaved stereo.
    input: Vec<f32>,
    base: u64,
    eof: bool,
    /// Output frames made so far.
    made: u64,
}

impl Varispeed {
    pub fn new(speed: f64) -> Self {
        Self {
            speed,
            input: Vec::new(),
            base: 0,
            eof: false,
            made: 0,
        }
    }

    /// Fill `out` (interleaved stereo) with resampled audio pulled from
    /// `pull`; zeros once the source has ended.
    pub fn read(&mut self, out: &mut [f32], pull: &mut Pull) {
        let frames = out.len() / 2;
        // Input needed: up to two frames past the last position.
        let last = ((self.made + frames as u64) as f64 * self.speed).floor() as u64 + 3;
        self.fill(last, pull);
        for f in 0..frames {
            let pos = (self.made + f as u64) as f64 * self.speed;
            let i = pos.floor() as u64;
            let t = (pos - i as f64) as f32;
            for ch in 0..2 {
                let s = |k: i64| self.sample(k, ch);
                let i = i as i64;
                let (p0, p1, p2, p3) = (s(i - 1), s(i), s(i + 1), s(i + 2));
                // Catmull-Rom.
                let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
                let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
                let c = -0.5 * p0 + 0.5 * p2;
                out[f * 2 + ch] = ((a * t + b) * t + c) * t + p1;
            }
        }
        self.made += frames as u64;
        // Keep a frame before the next position for the interpolation.
        let next = (self.made as f64 * self.speed).floor() as u64;
        let keep_from = next.saturating_sub(1).max(self.base);
        let drop = ((keep_from - self.base) as usize * 2).min(self.input.len());
        self.input.drain(..drop);
        self.base += (drop / 2) as u64;
    }

    /// Input sample `k` of channel `ch`; the first frame before the start,
    /// silence past the end.
    fn sample(&self, k: i64, ch: usize) -> f32 {
        let k = k.max(0) as u64;
        if k < self.base {
            return self.input.get(ch).copied().unwrap_or(0.0);
        }
        self.input
            .get(((k - self.base) as usize) * 2 + ch)
            .copied()
            .unwrap_or(0.0)
    }

    /// Make sure input covers absolute frames below `end`.
    fn fill(&mut self, end: u64, pull: &mut Pull) {
        let have = self.base + (self.input.len() / 2) as u64;
        if end <= have || self.eof {
            return;
        }
        let want = ((end - have) as usize) * 2;
        let start = self.input.len();
        self.input.resize(start + want, 0.0);
        let mut filled = 0;
        while filled < want {
            let n = pull(&mut self.input[start + filled..start + want]);
            if n == 0 {
                self.eof = true;
                break;
            }
            filled += n;
        }
        // Whole frames only; the rest reads as silence.
        self.input.truncate(start + filled / 2 * 2);
    }
}
