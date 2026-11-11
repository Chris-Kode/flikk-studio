//! Real-time processors for [`AudioFx`], on interleaved stereo `f32`.
//!
//! Each processor keeps its own state (filter memory, envelopes, delay
//! lines), so a chain is built per clip or track instance and rebuilt on a
//! seek. Nothing here allocates after construction.
//!
//! The render maps the same effects to FFmpeg filters, and the processors
//! are written to match them:
//!
//! * equalizer, high/low pass: RBJ biquads with FFmpeg's coefficients
//!   (`lowshelf`/`highshelf` at slope 1, `equalizer` with Q, 2-pole
//!   `highpass`/`lowpass` at Q 0.707);
//! * compressor and gate: ports of `acompressor` and `agate` (RMS
//!   detection, linked channels, soft knee);
//! * de-esser: split band, the high band compressed by itself
//!   (`x + (g − 1)·hp(x)`), like the render's `sidechaincompress` graph;
//! * echo: a feedback delay line, which the render expands into `aecho`
//!   taps;
//! * reverb: Freeverb; it is linear, so the render convolves with its
//!   impulse response ([`reverb_impulse`]) and matches exactly;
//! * limiter: a 5 ms look-ahead brick wall (the render uses `alimiter`);
//!
//! Noise reduction is not live: FFmpeg's `afftdn` makes a copy of the
//! source that both play ([`crate::cleanup`]).

use super::{AudioFx, AudioFxKind};

/// One effect instance.
pub trait Processor: Send {
    /// Process interleaved stereo frames in place.
    fn process(&mut self, buf: &mut [f32]);

    /// Frames the output lags the input by (look-ahead).
    fn latency(&self) -> usize {
        0
    }
}

/// The enabled, non-neutral effects of a stack, in order.
pub struct Chain {
    stages: Vec<Box<dyn Processor>>,
}

impl Chain {
    pub fn new(effects: &[AudioFx], rate: u32) -> Self {
        let rate = rate.max(8000) as f64;
        Self {
            stages: effects
                .iter()
                .filter(|fx| fx.is_active())
                .filter_map(|fx| processor(fx, rate))
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for stage in &mut self.stages {
            stage.process(buf);
        }
    }

    /// Frames the output lags the input by. Callers feed the chain this
    /// far ahead and drop as much output, as the render's `alimiter`
    /// does with `latency=1`.
    pub fn latency(&self) -> usize {
        self.stages.iter().map(|s| s.latency()).sum()
    }
}

/// A processor for `fx` at `rate` Hz; `None` for Noise Reduction, which
/// is made into a copy of the source.
pub fn processor(fx: &AudioFx, rate: f64) -> Option<Box<dyn Processor>> {
    let v = |name: &str| fx.value(name);
    Some(match fx.kind {
        AudioFxKind::Equalizer => {
            let mut bands = Vec::new();
            for (shape, gain) in [
                (Shape::LowShelf, v("low_gain_db")),
                (Shape::Peak, v("mid_gain_db")),
                (Shape::HighShelf, v("high_gain_db")),
            ] {
                if gain.abs() < 1e-3 {
                    continue;
                }
                let (freq, q) = match shape {
                    Shape::LowShelf => (v("low_freq"), SHELF_SLOPE),
                    Shape::Peak => (v("mid_freq"), v("mid_q")),
                    _ => (v("high_freq"), SHELF_SLOPE),
                };
                bands.push(Biquad::new(coefs(shape, freq, q, gain, rate)));
            }
            Box::new(Filters(bands))
        }
        AudioFxKind::HighPass => Box::new(Filters(vec![Biquad::new(coefs(
            Shape::HighPass,
            v("freq"),
            PASS_Q,
            0.0,
            rate,
        ))])),
        AudioFxKind::LowPass => Box::new(Filters(vec![Biquad::new(coefs(
            Shape::LowPass,
            v("freq"),
            PASS_Q,
            0.0,
            rate,
        ))])),
        AudioFxKind::Compressor => Box::new(Compressor {
            curve: Curve::new(
                db(v("threshold_db")),
                v("ratio"),
                v("attack_ms"),
                v("release_ms"),
                rate,
            ),
            makeup: db(v("makeup_db")),
        }),
        AudioFxKind::Limiter => Box::new(Limiter::new(db(v("ceiling_db")), v("release_ms"), rate)),
        AudioFxKind::Gate => Box::new(Gate::new(
            db(v("threshold_db")),
            v("attack_ms"),
            v("release_ms"),
            rate,
        )),
        AudioFxKind::DeEsser => Box::new(DeEsser {
            band: Biquad::new(coefs(Shape::HighPass, v("freq"), PASS_Q, 0.0, rate)),
            curve: de_esser_curve(v("amount"), rate),
            high: [0.0; 2],
        }),
        // Made into a copy of the source instead (`crate::cleanup`).
        AudioFxKind::NoiseReduction => return None,
        AudioFxKind::Reverb => Box::new(Reverb::new(v("room"), v("damping"), v("mix"), rate)),
        AudioFxKind::Echo => Box::new(Echo::new(v("delay_ms"), v("feedback"), v("mix"), rate)),
    })
}

/// dB → linear amplitude.
pub fn db(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

// ------------------------------------------------------------------ biquads

/// Q of the 2-pole high and low pass (Butterworth), as FFmpeg's default.
pub const PASS_Q: f64 = 0.707;
/// Shelf slope `S` (FFmpeg `t=s:w=1`): the steepest without overshoot.
pub const SHELF_SLOPE: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    LowPass,
    HighPass,
    Peak,
    LowShelf,
    HighShelf,
}

/// Normalized biquad coefficients (`a0` = 1).
#[derive(Clone, Copy, Debug)]
pub struct Coefs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

/// Highest usable filter frequency at `rate`.
pub fn max_freq(rate: f64) -> f64 {
    rate * 0.45
}

/// RBJ cookbook coefficients. `q` is the Q for pass and peak filters and
/// the slope for shelves; `gain_db` is ignored by pass filters.
pub fn coefs(shape: Shape, freq: f64, q: f64, gain_db: f64, rate: f64) -> Coefs {
    let f = freq.clamp(10.0, max_freq(rate));
    let w0 = 2.0 * std::f64::consts::PI * f / rate;
    let (sn, cs) = w0.sin_cos();
    let a = 10f64.powf(gain_db / 40.0);
    let (b0, b1, b2, a0, a1, a2) = match shape {
        Shape::LowPass | Shape::HighPass => {
            let alpha = sn / (2.0 * q);
            let (b0, b1) = if shape == Shape::LowPass {
                ((1.0 - cs) / 2.0, 1.0 - cs)
            } else {
                ((1.0 + cs) / 2.0, -(1.0 + cs))
            };
            (b0, b1, b0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha)
        }
        Shape::Peak => {
            let alpha = sn / (2.0 * q);
            (
                1.0 + alpha * a,
                -2.0 * cs,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cs,
                1.0 - alpha / a,
            )
        }
        Shape::LowShelf | Shape::HighShelf => {
            let alpha = sn / 2.0 * ((a + 1.0 / a) * (1.0 / q - 1.0) + 2.0).sqrt();
            let k = 2.0 * a.sqrt() * alpha;
            if shape == Shape::LowShelf {
                (
                    a * ((a + 1.0) - (a - 1.0) * cs + k),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                    a * ((a + 1.0) - (a - 1.0) * cs - k),
                    (a + 1.0) + (a - 1.0) * cs + k,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                    (a + 1.0) + (a - 1.0) * cs - k,
                )
            } else {
                (
                    a * ((a + 1.0) + (a - 1.0) * cs + k),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                    a * ((a + 1.0) + (a - 1.0) * cs - k),
                    (a + 1.0) - (a - 1.0) * cs + k,
                    2.0 * ((a - 1.0) - (a + 1.0) * cs),
                    (a + 1.0) - (a - 1.0) * cs - k,
                )
            }
        }
    };
    Coefs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

/// Direct form I, per channel.
#[derive(Clone)]
pub struct Biquad {
    c: Coefs,
    /// x1, x2, y1, y2 per channel.
    s: [[f64; 4]; 2],
}

impl Biquad {
    pub fn new(c: Coefs) -> Self {
        Self {
            c,
            s: [[0.0; 4]; 2],
        }
    }

    #[inline]
    pub fn tick(&mut self, ch: usize, x: f64) -> f64 {
        let c = &self.c;
        let s = &mut self.s[ch];
        let y = c.b0 * x + c.b1 * s[0] + c.b2 * s[1] - c.a1 * s[2] - c.a2 * s[3];
        let y = if y.abs() < 1e-30 { 0.0 } else { y };
        *s = [x, s[0], y, s[2]];
        y
    }
}

struct Filters(Vec<Biquad>);

impl Processor for Filters {
    fn process(&mut self, buf: &mut [f32]) {
        for f in &mut self.0 {
            for frame in buf.as_chunks_mut::<2>().0.iter_mut() {
                frame[0] = f.tick(0, frame[0] as f64) as f32;
                frame[1] = f.tick(1, frame[1] as f64) as f32;
            }
        }
    }
}

// ----------------------------------------------------------------- dynamics

/// FFmpeg's soft knee: `2.82843` (9 dB).
const KNEE: f64 = 2.828_427_12;

/// Envelope coefficient as FFmpeg computes it.
fn ff_coef(ms: f64, rate: f64) -> f64 {
    (1.0 / (ms.max(0.01) * rate / 4000.0)).min(1.0)
}

fn hermite(x: f64, x0: f64, x1: f64, p0: f64, p1: f64, m0: f64, m1: f64) -> f64 {
    let width = x1 - x0;
    let t = (x - x0) / width;
    let (m0, m1) = (m0 * width, m1 * width);
    let (t2, t3) = (t * t, t * t * t);
    let ct0 = p0;
    let ct1 = m0;
    let ct2 = -3.0 * p0 - 2.0 * m0 + 3.0 * p1 - m1;
    let ct3 = 2.0 * p0 + m0 - 2.0 * p1 + m1;
    ct3 * t3 + ct2 * t2 + ct1 * t + ct0
}

/// The `acompressor` gain computer: RMS detection, channels averaged.
pub struct Curve {
    attack: f64,
    release: f64,
    ratio: f64,
    thres: f64,
    knee_start: f64,
    knee_stop: f64,
    compressed_knee_stop: f64,
    adj_knee_start: f64,
    lin_slope: f64,
}

impl Curve {
    pub fn new(threshold: f64, ratio: f64, attack_ms: f64, release_ms: f64, rate: f64) -> Self {
        let threshold = threshold.clamp(1e-6, 1.0);
        let ratio = ratio.max(1.0);
        let lin_knee_start = threshold / KNEE.sqrt();
        let lin_knee_stop = threshold * KNEE.sqrt();
        let thres = threshold.ln();
        let knee_stop = lin_knee_stop.ln();
        Self {
            attack: ff_coef(attack_ms, rate),
            release: ff_coef(release_ms, rate),
            ratio,
            thres,
            knee_start: lin_knee_start.ln(),
            knee_stop,
            compressed_knee_stop: (knee_stop - thres) / ratio + thres,
            adj_knee_start: lin_knee_start * lin_knee_start,
            lin_slope: 0.0,
        }
    }

    /// Gain for the next frame given its detector level (mean of |x|).
    #[inline]
    pub fn gain(&mut self, level: f64) -> f64 {
        let x = level * level;
        let k = if x > self.lin_slope {
            self.attack
        } else {
            self.release
        };
        self.lin_slope += (x - self.lin_slope) * k;
        if self.lin_slope <= 0.0 || self.lin_slope <= self.adj_knee_start {
            return 1.0;
        }
        let slope = self.lin_slope.ln() * 0.5;
        let delta = 1.0 / self.ratio;
        let mut gain = (slope - self.thres) / self.ratio + self.thres;
        if slope < self.knee_stop {
            gain = hermite(
                slope,
                self.knee_start,
                self.knee_stop,
                self.knee_start,
                self.compressed_knee_stop,
                1.0,
                delta,
            );
        }
        (gain - slope).exp()
    }
}

struct Compressor {
    curve: Curve,
    makeup: f64,
}

impl Processor for Compressor {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.as_chunks_mut::<2>().0.iter_mut() {
            let level = (f[0].abs() as f64 + f[1].abs() as f64) / 2.0;
            let g = (self.curve.gain(level) * self.makeup) as f32;
            f[0] *= g;
            f[1] *= g;
        }
    }
}

/// Ducking (music under dialogue) as `sidechaincompress` settings:
/// threshold and ratio for a reduction in dB. It has no range control, so
/// the ratio is picked so dialogue around -20 dBFS (16.5 dB over the
/// threshold) pulls the music down by about `reduction_db`.
pub fn ducking(reduction_db: f64) -> (f64, f64) {
    let reduction = reduction_db.abs();
    let ratio = if reduction >= 16.0 {
        20.0
    } else {
        (1.0 / (1.0 - reduction / 16.5)).clamp(1.0, 20.0)
    };
    (DUCK_THRESHOLD, ratio)
}

/// Linear threshold of ducking's compressor.
pub const DUCK_THRESHOLD: f64 = 0.015;

/// Fixed settings of the gate, shared with the render's `agate`.
pub const GATE_RATIO: f64 = 10.0;
/// Deepest gate attenuation, -60 dB.
pub const GATE_RANGE: f64 = 0.001;

/// Port of `agate` (downward expander, RMS detection).
struct Gate {
    attack: f64,
    release: f64,
    thres: f64,
    knee_start: f64,
    knee_stop: f64,
    lin_knee_stop: f64,
    lin_slope: f64,
}

impl Gate {
    fn new(threshold: f64, attack_ms: f64, release_ms: f64, rate: f64) -> Self {
        let lin = threshold.clamp(1e-6, 1.0).powi(2);
        let (lin_knee_start, lin_knee_stop) = (lin / KNEE.sqrt(), lin * KNEE.sqrt());
        Self {
            attack: ff_coef(attack_ms, rate),
            release: ff_coef(release_ms, rate),
            thres: lin.ln(),
            knee_start: lin_knee_start.ln(),
            knee_stop: lin_knee_stop.ln(),
            lin_knee_stop,
            lin_slope: 0.0,
        }
    }

    fn gain(&mut self, level: f64) -> f64 {
        let x = level * level;
        let k = if x > self.lin_slope {
            self.attack
        } else {
            self.release
        };
        self.lin_slope += (x - self.lin_slope) * k;
        if self.lin_slope >= self.lin_knee_stop {
            return 1.0;
        }
        if self.lin_slope <= 0.0 {
            return GATE_RANGE;
        }
        let slope = self.lin_slope.ln();
        let mut gain = (slope - self.thres) * GATE_RATIO + self.thres;
        if slope > self.knee_start {
            gain = hermite(
                slope,
                self.knee_start,
                self.knee_stop,
                (self.knee_start - self.thres) * GATE_RATIO + self.thres,
                self.knee_stop,
                GATE_RATIO,
                1.0,
            );
        }
        (gain - slope).exp().max(GATE_RANGE)
    }
}

impl Processor for Gate {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.as_chunks_mut::<2>().0.iter_mut() {
            let level = (f[0].abs() as f64 + f[1].abs() as f64) / 2.0;
            let g = self.gain(level) as f32;
            f[0] *= g;
            f[1] *= g;
        }
    }
}

/// De-esser band compressor: threshold, ratio, attack and release for
/// `amount` (0..1). The render uses the same numbers.
pub fn de_esser_settings(amount: f64) -> (f64, f64, f64, f64) {
    let amount = amount.clamp(0.0, 1.0);
    (db(-30.0), 1.0 + 19.0 * amount, 1.0, 60.0)
}

fn de_esser_curve(amount: f64, rate: f64) -> Curve {
    let (threshold, ratio, attack, release) = de_esser_settings(amount);
    Curve::new(threshold, ratio, attack, release, rate)
}

/// `x + (g − 1)·hp(x)`: only the band above the frequency is turned down.
struct DeEsser {
    band: Biquad,
    curve: Curve,
    high: [f64; 2],
}

impl Processor for DeEsser {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.as_chunks_mut::<2>().0.iter_mut() {
            self.high = [
                self.band.tick(0, f[0] as f64),
                self.band.tick(1, f[1] as f64),
            ];
            let level = (self.high[0].abs() + self.high[1].abs()) / 2.0;
            let g = self.curve.gain(level) - 1.0;
            f[0] += (g * self.high[0]) as f32;
            f[1] += (g * self.high[1]) as f32;
        }
    }
}

/// Look-ahead of the limiter, in ms (the render's `alimiter` attack).
pub const LIMITER_LOOKAHEAD_MS: f64 = 5.0;

/// Brick-wall limiter: the gain needed by every sample in the look-ahead
/// window (a running minimum), smoothed by a box filter of the same length
/// so it never overshoots, then released exponentially.
struct Limiter {
    ceiling: f64,
    release: f64,
    window: usize,
    /// Delay line of input frames.
    delay: Vec<[f32; 2]>,
    /// Required gain per position, for the running minimum.
    need: Vec<f64>,
    /// Running minima, for the box filter.
    mins: Vec<f64>,
    pos: usize,
    sum: f64,
    gain: f64,
    /// Monotonic deque of (index, need) for the sliding minimum.
    deque: std::collections::VecDeque<(u64, f64)>,
    n: u64,
}

impl Limiter {
    fn new(ceiling: f64, release_ms: f64, rate: f64) -> Self {
        let window = ((LIMITER_LOOKAHEAD_MS / 1000.0 * rate) as usize).max(1);
        Self {
            ceiling: ceiling.clamp(1e-3, 1.0),
            release: (-1.0 / (release_ms.max(1.0) / 1000.0 * rate)).exp(),
            window,
            delay: vec![[0.0; 2]; window],
            need: vec![1.0; window],
            mins: vec![1.0; window],
            pos: 0,
            sum: window as f64,
            gain: 1.0,
            deque: std::collections::VecDeque::with_capacity(window + 1),
            n: 0,
        }
    }
}

impl Processor for Limiter {
    fn latency(&self) -> usize {
        self.window
    }

    fn process(&mut self, buf: &mut [f32]) {
        let w = self.window as u64;
        for f in buf.as_chunks_mut::<2>().0.iter_mut() {
            let peak = f[0].abs().max(f[1].abs()) as f64;
            let need = if peak > self.ceiling {
                self.ceiling / peak
            } else {
                1.0
            };
            self.need[self.pos] = need;
            while self.deque.back().is_some_and(|&(_, v)| v >= need) {
                self.deque.pop_back();
            }
            self.deque.push_back((self.n, need));
            while self.deque.front().is_some_and(|&(i, _)| i + w <= self.n) {
                self.deque.pop_front();
            }
            let min = self.deque.front().map_or(1.0, |&(_, v)| v);
            self.sum += min - self.mins[self.pos];
            self.mins[self.pos] = min;
            let smooth = (self.sum / w as f64).min(1.0);
            self.gain = if smooth < self.gain {
                smooth
            } else {
                smooth + (self.gain - smooth) * self.release
            };
            let out = self.delay[self.pos];
            self.delay[self.pos] = [f[0], f[1]];
            let g = self.gain as f32;
            f[0] = out[0] * g;
            f[1] = out[1] * g;
            self.pos = (self.pos + 1) % self.window;
            self.n += 1;
            if self.n.is_multiple_of(65536) {
                // Drop accumulated rounding.
                self.sum = self.mins.iter().sum();
            }
        }
    }
}

// ------------------------------------------------------------------- space

/// Longest echo delay line, in seconds.
const MAX_ECHO_S: f64 = 2.0;

/// `x + mix · Σₖ feedbackᵏ⁻¹ · x[n − k·delay]`.
struct Echo {
    line: Vec<[f32; 2]>,
    pos: usize,
    feedback: f32,
    mix: f32,
}

impl Echo {
    fn new(delay_ms: f64, feedback: f64, mix: f64, rate: f64) -> Self {
        let n = ((delay_ms / 1000.0).clamp(0.001, MAX_ECHO_S) * rate).round() as usize;
        Self {
            line: vec![[0.0; 2]; n.max(1)],
            pos: 0,
            feedback: feedback.clamp(0.0, 0.95) as f32,
            mix: mix.clamp(0.0, 1.0) as f32,
        }
    }
}

impl Processor for Echo {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.as_chunks_mut::<2>().0.iter_mut() {
            let wet = self.line[self.pos];
            self.line[self.pos] = [f[0] + self.feedback * wet[0], f[1] + self.feedback * wet[1]];
            f[0] += self.mix * wet[0];
            f[1] += self.mix * wet[1];
            self.pos = (self.pos + 1) % self.line.len();
        }
    }
}

/// Echo taps for the render: (delay in ms, level), the expansion of the
/// feedback line down to -60 dB.
pub fn echo_taps(delay_ms: f64, feedback: f64, mix: f64) -> Vec<(f64, f64)> {
    let delay = (delay_ms / 1000.0).clamp(0.001, MAX_ECHO_S) * 1000.0;
    let fb = feedback.clamp(0.0, 0.95);
    let mut level = mix.clamp(0.0, 1.0);
    let mut out = Vec::new();
    for k in 1..=48 {
        if level < 1e-3 {
            break;
        }
        out.push((delay * k as f64, level));
        level *= fb;
    }
    out
}

// Freeverb (Jezar at Dreampoint, public domain).
const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
const SPREAD: usize = 23;
const FIXED_GAIN: f64 = 0.015;
const SCALE_WET: f64 = 3.0;

struct Comb {
    buf: Vec<f64>,
    pos: usize,
    store: f64,
}

impl Comb {
    #[inline]
    fn tick(&mut self, x: f64, feedback: f64, damp: f64) -> f64 {
        let out = self.buf[self.pos];
        self.store = out * (1.0 - damp) + self.store * damp;
        self.buf[self.pos] = x + self.store * feedback;
        self.pos = (self.pos + 1) % self.buf.len();
        out
    }
}

struct Allpass {
    buf: Vec<f64>,
    pos: usize,
}

impl Allpass {
    #[inline]
    fn tick(&mut self, x: f64) -> f64 {
        let b = self.buf[self.pos];
        self.buf[self.pos] = x + b * 0.5;
        self.pos = (self.pos + 1) % self.buf.len();
        b - x
    }
}

/// Wet (reverb) path only: input is the mono sum of both channels.
struct Freeverb {
    combs: [Vec<Comb>; 2],
    allpasses: [Vec<Allpass>; 2],
    feedback: f64,
    damp: f64,
}

impl Freeverb {
    fn new(room: f64, damping: f64, rate: f64) -> Self {
        let scale = rate / 44_100.0;
        let len =
            |n: usize, side: usize| ((n + side * SPREAD) as f64 * scale).round().max(1.0) as usize;
        let combs = [0, 1].map(|side| {
            COMBS
                .iter()
                .map(|&n| Comb {
                    buf: vec![0.0; len(n, side)],
                    pos: 0,
                    store: 0.0,
                })
                .collect()
        });
        let allpasses = [0, 1].map(|side| {
            ALLPASSES
                .iter()
                .map(|&n| Allpass {
                    buf: vec![0.0; len(n, side)],
                    pos: 0,
                })
                .collect()
        });
        Self {
            combs,
            allpasses,
            feedback: room.clamp(0.0, 1.0) * 0.28 + 0.7,
            damp: damping.clamp(0.0, 1.0) * 0.4,
        }
    }

    /// One frame: input sum (L + R) → wet (L, R), before the wet gain.
    #[inline]
    fn tick(&mut self, sum: f64) -> [f64; 2] {
        let x = sum * FIXED_GAIN;
        let mut out = [0.0; 2];
        for (side, o) in out.iter_mut().enumerate() {
            let mut acc = 0.0;
            for c in &mut self.combs[side] {
                acc += c.tick(x, self.feedback, self.damp);
            }
            for a in &mut self.allpasses[side] {
                acc = a.tick(acc);
            }
            *o = acc;
        }
        out
    }
}

/// Dry and wet gains of the reverb for `mix`.
pub fn reverb_gains(mix: f64) -> (f64, f64) {
    let mix = mix.clamp(0.0, 1.0);
    (1.0 - mix, mix * SCALE_WET)
}

struct Reverb {
    verb: Freeverb,
    dry: f32,
    wet: f64,
}

impl Reverb {
    fn new(room: f64, damping: f64, mix: f64, rate: f64) -> Self {
        let (dry, wet) = reverb_gains(mix);
        Self {
            verb: Freeverb::new(room, damping, rate),
            dry: dry as f32,
            wet,
        }
    }
}

impl Processor for Reverb {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.as_chunks_mut::<2>().0.iter_mut() {
            let w = self.verb.tick(f[0] as f64 + f[1] as f64);
            f[0] = f[0] * self.dry + (w[0] * self.wet) as f32;
            f[1] = f[1] * self.dry + (w[1] * self.wet) as f32;
        }
    }
}

/// Impulse response of the reverb's wet path (wet gain included) at `rate`,
/// per output channel, for an impulse on the input sum; cut where the tail
/// falls 70 dB below its peak, at most 8 s. Convolving `L + R` with it
/// reproduces the processor exactly.
pub fn reverb_impulse(room: f64, damping: f64, mix: f64, rate: u32) -> [Vec<f32>; 2] {
    let rate = rate.max(8000) as f64;
    let (_, wet) = reverb_gains(mix);
    let mut verb = Freeverb::new(room, damping, rate);
    let max = (8.0 * rate) as usize;
    let block = (0.05 * rate) as usize;
    let mut out: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
    let mut peak = 0f64;
    for n in 0..max {
        let w = verb.tick(if n == 0 { 1.0 } else { 0.0 });
        out[0].push((w[0] * wet) as f32);
        out[1].push((w[1] * wet) as f32);
        peak = peak.max(w[0].abs()).max(w[1].abs());
        // Stop after a whole block below the cut-off.
        if n > block && n % block == 0 {
            let tail = out[0][n - block..]
                .iter()
                .chain(&out[1][n - block..])
                .fold(0f32, |m, v| m.max(v.abs())) as f64;
            if tail < peak * wet * db(-70.0) {
                break;
            }
        }
    }
    out
}
