//! Finding where one recording sits in another by their sound.
//!
//! Both are decoded to mono at [`RATE`]. A coarse pass cross-correlates
//! their onset envelopes (spectral flux of log band energies, 100 a
//! second) over every lag at once with an FFT, each lag normalized by the
//! overlap it leaves, so a recording that starts late or stops early still
//! matches, and offsets of hours cost no more than seconds. A fine pass
//! then reads short windows of both spread over the overlap and finds each
//! window's lag to the sample with GCC-PHAT (band-limited, sub-sample by a
//! parabola). A robust line through the windows' offsets gives the offset
//! and, when the clocks drift, the drift.

use super::fft::{Complex, Plan, forward_real, xcorr};
use crate::model::MediaItem;
use crate::services::{MediaService, ServiceError, ServiceResult};
use crate::time::{Time, TimeRange};

/// Sample rate both recordings are heard at.
pub const RATE: u32 = 8_000;
/// Onset envelope values per second.
pub const ENVELOPE_RATE: u32 = 100;
const HOP: usize = (RATE / ENVELOPE_RATE) as usize;
const FRAME: usize = 256;
const BANDS: usize = 16;
/// Longest stretch of a recording that is read.
pub const MAX_LENGTH: f64 = 6.0 * 3600.0;
/// Least overlap a match may leave, in seconds.
const MIN_OVERLAP: f64 = 3.0;
/// Fine windows: length, and the most read.
const WINDOW: f64 = 8.0;
const MAX_WINDOWS: usize = 12;
/// Offsets of fine windows within this of the fitted line agree.
const AGREE: f64 = 0.0015;
/// Below this a match is not used.
pub const MIN_CONFIDENCE: f32 = 0.5;
/// Drift that adds up to less than this over the overlap is left alone.
const MIN_DRIFT_TOTAL: f64 = 0.002;

/// One recording: the audio stream of `item` (as [`MediaService::audio`]
/// takes it) over the source range `range`.
#[derive(Clone, Debug)]
pub struct Source {
    pub item: MediaItem,
    pub stream: u32,
    pub range: TimeRange,
}

impl Source {
    /// The whole of `item`'s `stream`, up to [`MAX_LENGTH`].
    pub fn whole(item: &MediaItem, stream: u32) -> Option<Self> {
        let info = item.info.as_ref()?;
        if info.audio.is_empty() || info.duration <= Time::ZERO {
            return None;
        }
        let end = info.duration.min(Time::from_secs_f64(MAX_LENGTH));
        Some(Self {
            item: item.clone(),
            stream,
            range: TimeRange::new(Time::ZERO, end),
        })
    }
}

/// Where the follower's sound sits against the reference's, in their
/// source times: the follower's time `f` that sounds like the reference's
/// `r` is `offset + r · (1 + drift)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Measured {
    /// Seconds.
    pub offset: f64,
    /// Relative clock difference (1e-4 is 0.01%); 0 when none was found.
    pub drift: f64,
    /// 0..1; under [`MIN_CONFIDENCE`] the match is not used.
    pub confidence: f32,
    /// Where they overlap, in reference source time.
    pub overlap: TimeRange,
    /// Fine windows read, and how many agree with the result.
    pub windows: usize,
    pub agreeing: usize,
}

impl Measured {
    pub fn matched(&self) -> bool {
        self.confidence >= MIN_CONFIDENCE
    }

    /// Follower source time that sounds like reference source time `r`.
    pub fn follower_time(&self, r: f64) -> f64 {
        self.offset + r * (1.0 + self.drift)
    }
}

/// The onset envelope of a recording, from `start` (source time).
#[derive(Clone, Debug)]
pub struct Envelope {
    pub start: Time,
    pub values: Vec<f32>,
}

impl Envelope {
    pub fn seconds(&self) -> f64 {
        self.values.len() as f64 / ENVELOPE_RATE as f64
    }
}

/// Mono samples at [`RATE`] of `source.item` from `from`, `seconds` long
/// (shorter at the end of the media).
pub fn read_mono(
    media: &dyn MediaService,
    source: &Source,
    from: Time,
    seconds: f64,
) -> ServiceResult<Vec<f32>> {
    let mut out = Vec::new();
    let want = (seconds * RATE as f64).round().max(0.0) as usize;
    stream_mono(media, source, from, want, &mut |chunk| {
        out.extend_from_slice(chunk);
    })?;
    Ok(out)
}

/// Read up to `want` mono samples from `from`, a chunk at a time. Times
/// before the media's start read as silence.
fn stream_mono(
    media: &dyn MediaService,
    source: &Source,
    from: Time,
    want: usize,
    sink: &mut dyn FnMut(&[f32]),
) -> ServiceResult<()> {
    let mut left = want;
    if from < Time::ZERO {
        let pad = ((-from).as_secs_f64() * RATE as f64).round() as usize;
        let pad = pad.min(left);
        let zeros = vec![0.0; pad];
        sink(&zeros);
        left -= pad;
    }
    if left == 0 {
        return Ok(());
    }
    let mut pcm = media.audio(&source.item, source.stream, from.max(Time::ZERO), RATE)?;
    let mut buf = vec![0f32; RATE as usize * 2];
    let mut mono = Vec::with_capacity(RATE as usize);
    while left > 0 {
        let n = pcm.read(&mut buf)?;
        if n == 0 {
            break;
        }
        mono.clear();
        mono.extend(
            buf[..n]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|s| 0.5 * (s[0] + s[1]))
                .take(left),
        );
        left -= mono.len();
        sink(&mono);
    }
    Ok(())
}

/// The onset envelope of `source` over its range.
pub fn envelope(media: &dyn MediaService, source: &Source) -> ServiceResult<Envelope> {
    let want = (source.range.duration().as_secs_f64() * RATE as f64).round() as usize;
    let mut onsets = Onsets::new();
    stream_mono(media, source, source.range.start, want, &mut |chunk| {
        onsets.push(chunk)
    })?;
    let values = onsets.finish();
    if values.len() < (MIN_OVERLAP * ENVELOPE_RATE as f64) as usize {
        return Err(ServiceError(format!(
            "{} has too little sound to sync",
            source.item.path.display()
        )));
    }
    Ok(Envelope {
        start: source.range.start,
        values,
    })
}

/// Spectral flux of log band energies, one value per [`HOP`] samples.
struct Onsets {
    plan: Plan,
    window: Vec<f64>,
    /// FFT bin range of each band.
    bands: Vec<(usize, usize)>,
    /// Samples not yet past; frames start at `next`.
    pending: Vec<f32>,
    next: usize,
    last: [f64; BANDS],
    started: bool,
    out: Vec<f32>,
    buf: Vec<Complex>,
}

impl Onsets {
    fn new() -> Self {
        let window = (0..FRAME)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / FRAME as f64).cos())
            .collect();
        // Log-spaced from 100 Hz to just under Nyquist; every band at
        // least one bin wide.
        let bin = |hz: f64| (hz / RATE as f64 * FRAME as f64).round() as usize;
        let (lo, hi) = (100.0f64, 3_800.0f64);
        let mut bands = Vec::with_capacity(BANDS);
        let mut prev = bin(lo).max(1);
        for b in 1..=BANDS {
            let edge = lo * (hi / lo).powf(b as f64 / BANDS as f64);
            let end = bin(edge).max(prev + 1).min(FRAME / 2);
            bands.push((prev, end));
            prev = end;
        }
        Self {
            plan: Plan::new(FRAME),
            window,
            bands,
            pending: Vec::new(),
            next: 0,
            last: [0.0; BANDS],
            started: false,
            out: Vec::new(),
            buf: vec![Complex::default(); FRAME],
        }
    }

    fn push(&mut self, samples: &[f32]) {
        self.pending.extend_from_slice(samples);
        while self.next + FRAME <= self.pending.len() {
            self.frame(self.next);
            self.next += HOP;
        }
        self.pending.drain(..self.next);
        self.next = 0;
    }

    fn frame(&mut self, at: usize) {
        let samples = &self.pending[at..at + FRAME];
        for (i, b) in self.buf.iter_mut().enumerate() {
            *b = Complex::new(samples[i] as f64 * self.window[i], 0.0);
        }
        self.plan.run(&mut self.buf, false);
        let mut flux = 0.0;
        for (b, &(lo, hi)) in self.bands.iter().enumerate() {
            let e: f64 = self.buf[lo..hi]
                .iter()
                .map(|c| c.re * c.re + c.im * c.im)
                .sum::<f64>()
                / (hi - lo) as f64;
            // A floor well under any recording's noise keeps digital
            // silence from making onsets of its own.
            let l = (e + 1e-7).ln();
            if self.started {
                flux += (l - self.last[b]).max(0.0);
            }
            self.last[b] = l;
        }
        self.started = true;
        self.out.push(flux as f32);
    }

    /// The envelope with its local mean (over a second) taken away, and
    /// its peaks limited, so one loud bang can't outweigh the rest.
    fn finish(self) -> Vec<f32> {
        let x = self.out;
        let n = x.len();
        if n == 0 {
            return x;
        }
        let half = ENVELOPE_RATE as usize / 2;
        let mut prefix = vec![0.0f64; n + 1];
        for i in 0..n {
            prefix[i + 1] = prefix[i] + x[i] as f64;
        }
        let mut y: Vec<f32> = (0..n)
            .map(|i| {
                let a = i.saturating_sub(half);
                let b = (i + half + 1).min(n);
                let mean = (prefix[b] - prefix[a]) / (b - a) as f64;
                (x[i] as f64 - mean).max(0.0) as f32
            })
            .collect();
        let mut sorted: Vec<f32> = y.iter().copied().filter(|v| *v > 0.0).collect();
        if !sorted.is_empty() {
            let k = (sorted.len() * 99 / 100).min(sorted.len() - 1);
            let cap = *sorted.select_nth_unstable_by(k, f32::total_cmp).1;
            for v in &mut y {
                *v = v.min(cap);
            }
        }
        // Zero mean, so silence neither helps nor hurts a lag.
        let mean = y.iter().map(|v| *v as f64).sum::<f64>() / n as f64;
        for v in &mut y {
            *v -= mean as f32;
        }
        y
    }
}

/// The coarse lag: follower envelope time minus reference envelope time,
/// in seconds from each envelope's start, with its normalized score and
/// how far the best lag stands above the others.
#[derive(Clone, Copy, Debug)]
pub struct Coarse {
    pub lag: f64,
    pub score: f64,
    /// Best score outside a second around the peak.
    pub second: f64,
}

/// Every lag of `follower` against `reference` that leaves enough overlap,
/// scored by the overlap's normalized correlation.
pub fn coarse(reference: &Envelope, follower: &Envelope) -> Option<Coarse> {
    let (x, y) = (&reference.values, &follower.values);
    let (n, m) = (x.len(), y.len());
    let c = xcorr(x, y);
    let prefix = |v: &[f32]| {
        let mut p = vec![0.0f64; v.len() + 1];
        for (i, s) in v.iter().enumerate() {
            p[i + 1] = p[i] + (*s as f64) * (*s as f64);
        }
        p
    };
    let (px, py) = (prefix(x), prefix(y));
    let shorter = n.min(m) as f64 / ENVELOPE_RATE as f64;
    let min_overlap =
        ((MIN_OVERLAP.max(shorter * 0.3)).min(shorter) * ENVELOPE_RATE as f64).max(1.0) as usize;
    let mut scores = vec![f64::NEG_INFINITY; c.len()];
    for (i, &v) in c.iter().enumerate() {
        let lag = i as isize - (n as isize - 1);
        // Reference frames i0..i1 overlap follower frames i0+lag..i1+lag.
        let i0 = (-lag).max(0) as usize;
        let i1 = (m as isize - lag).min(n as isize).max(0) as usize;
        if i1 <= i0 || i1 - i0 < min_overlap {
            continue;
        }
        let ex = px[i1] - px[i0];
        let ey = py[(i1 as isize + lag) as usize] - py[(i0 as isize + lag) as usize];
        if ex <= 0.0 || ey <= 0.0 {
            continue;
        }
        scores[i] = v / (ex * ey).sqrt();
    }
    let (best, &score) = scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    if !score.is_finite() {
        return None;
    }
    let guard = ENVELOPE_RATE as usize;
    let second = scores
        .iter()
        .enumerate()
        .filter(|(i, _)| i.abs_diff(best) > guard)
        .map(|(_, s)| *s)
        .fold(f64::NEG_INFINITY, f64::max)
        .max(0.0);
    let frac = parabola(&scores, best);
    let lag = (best as f64 + frac - (n as f64 - 1.0)) / ENVELOPE_RATE as f64;
    Some(Coarse { lag, score, second })
}

/// Sub-sample position of the peak at `i`, from its neighbours.
fn parabola(v: &[f64], i: usize) -> f64 {
    if i == 0 || i + 1 >= v.len() {
        return 0.0;
    }
    let (a, b, c) = (v[i - 1], v[i], v[i + 1]);
    if !a.is_finite() || !c.is_finite() {
        return 0.0;
    }
    let d = a - 2.0 * b + c;
    if d.abs() < 1e-12 {
        0.0
    } else {
        (0.5 * (a - c) / d).clamp(-0.5, 0.5)
    }
}

/// One fine window: the follower's offset at reference time `at`, and how
/// clearly its peak stood out.
#[derive(Clone, Copy, Debug)]
struct Fine {
    at: f64,
    offset: f64,
    peak: f64,
}

/// GCC-PHAT lag of `follower` against `reference` (sample `i` of the
/// reference sounding at sample `i + lag` of the follower), searched over
/// `lags`; the lag in samples and its peak over the mean of the search.
fn gcc_phat(reference: &[f32], follower: &[f32], lags: (isize, isize)) -> Option<(f64, f64)> {
    let total = reference.len() + follower.len();
    let n = total.next_power_of_two();
    let plan = Plan::new(n);
    let fr = forward_real(reference, n);
    let mut ff = forward_real(follower, n);
    let lo = (150.0 / RATE as f64 * n as f64) as usize;
    let hi = (3_800.0 / RATE as f64 * n as f64) as usize;
    for (k, (y, x)) in ff.iter_mut().zip(&fr).enumerate() {
        let bin = k.min(n - k);
        if bin < lo || bin > hi {
            *y = Complex::default();
            continue;
        }
        let p = y.times(x.conj());
        let mag = p.norm();
        // PHAT weighting a little short of full whitening holds up better
        // in noise.
        *y = if mag > 1e-20 {
            p.scale(1.0 / mag.powf(0.8))
        } else {
            Complex::default()
        };
    }
    plan.run(&mut ff, true);
    let at = |lag: isize| {
        let idx = if lag >= 0 {
            lag as usize
        } else {
            (n as isize + lag) as usize
        };
        ff[idx].re
    };
    let (lo, hi) = lags;
    if hi < lo {
        return None;
    }
    let values: Vec<f64> = (lo..=hi).map(at).collect();
    let (best, &peak) = values
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    let mean = values.iter().map(|v| v.abs()).sum::<f64>() / values.len() as f64;
    if mean <= 0.0 || peak <= 0.0 {
        return None;
    }
    let frac = parabola(&values, best);
    Some((lo as f64 + best as f64 + frac, peak / mean))
}

/// Fine windows over the overlap, each read from both recordings with
/// `margin` seconds of slack on the follower's side.
fn fine(
    media: &dyn MediaService,
    reference: &Source,
    follower: &Source,
    coarse: f64,
    overlap: (f64, f64),
) -> ServiceResult<Vec<Fine>> {
    let length = overlap.1 - overlap.0;
    let window = WINDOW.min(length * 0.8).max(1.0);
    // Clock drift moves the ends away from the coarse lag (measured over
    // the whole overlap) by up to 0.1% of half the overlap.
    let margin = 0.25 + length * 5e-4;
    let count = ((length / 30.0).round() as usize).clamp(3, MAX_WINDOWS);
    let first = overlap.0 + window / 2.0;
    let last = overlap.1 - window / 2.0;
    let mut out = Vec::new();
    for k in 0..count {
        let at = if count == 1 || last <= first {
            (overlap.0 + overlap.1) / 2.0
        } else {
            first + (last - first) * k as f64 / (count - 1) as f64
        };
        let r0 = at - window / 2.0;
        let f0 = r0 + coarse - margin;
        let r = read_mono(media, reference, Time::from_secs_f64(r0), window)?;
        let f = read_mono(
            media,
            follower,
            Time::from_secs_f64(f0),
            window + 2.0 * margin,
        )?;
        if r.len() < RATE as usize / 2 || f.len() < RATE as usize / 2 {
            continue;
        }
        let slack = (2.0 * margin * RATE as f64) as isize;
        let Some((lag, peak)) = gcc_phat(&r, &f, (0, slack)) else {
            continue;
        };
        let offset = f0 + lag / RATE as f64 - r0;
        out.push(Fine { at, offset, peak });
    }
    Ok(out)
}

/// Where `follower` sounds like `reference`.
pub fn measure(
    media: &dyn MediaService,
    reference: &Source,
    reference_envelope: &Envelope,
    follower: &Source,
    follower_envelope: &Envelope,
) -> ServiceResult<Measured> {
    let Some(c) = coarse(reference_envelope, follower_envelope) else {
        return Err(ServiceError("they don't overlap enough to sync".into()));
    };
    // Follower source time = reference source time + coarse.
    let coarse_offset =
        follower_envelope.start.as_secs_f64() + c.lag - reference_envelope.start.as_secs_f64();
    let (r0, r1) = (
        reference.range.start.as_secs_f64(),
        reference.range.end.as_secs_f64(),
    );
    let (f0, f1) = (
        follower.range.start.as_secs_f64(),
        follower.range.end.as_secs_f64(),
    );
    let overlap = (r0.max(f0 - coarse_offset), r1.min(f1 - coarse_offset));
    if overlap.1 - overlap.0 < MIN_OVERLAP {
        return Err(ServiceError("they don't overlap enough to sync".into()));
    }
    let windows = fine(media, reference, follower, coarse_offset, overlap)?;
    let (offset, drift, agreeing) = fit(&windows, overlap.1 - overlap.0, coarse_offset);
    let confidence = confidence(&c, windows.len(), agreeing);
    let at0 = |r: f64| Time::from_secs_f64(r);
    Ok(Measured {
        offset,
        drift,
        confidence,
        overlap: TimeRange::new(at0(overlap.0), at0(overlap.1)),
        windows: windows.len(),
        agreeing,
    })
}

/// A clear fine peak: well above the GCC-PHAT floor.
const CLEAR_PEAK: f64 = 8.0;

/// Line through the windows' offsets that most of them agree with:
/// Theil–Sen, then least squares on the windows within [`AGREE`]. The
/// offset at reference time zero, the drift (0 unless it adds up to
/// [`MIN_DRIFT_TOTAL`] and enough windows show it), and how many agree.
fn fit(windows: &[Fine], length: f64, coarse: f64) -> (f64, f64, usize) {
    let good: Vec<&Fine> = windows.iter().filter(|w| w.peak >= CLEAR_PEAK).collect();
    if good.is_empty() {
        return (coarse, 0.0, 0);
    }
    let median = |mut v: Vec<f64>| -> f64 {
        v.sort_by(f64::total_cmp);
        let n = v.len();
        if n % 2 == 1 {
            v[n / 2]
        } else {
            0.5 * (v[n / 2 - 1] + v[n / 2])
        }
    };
    // Flat first: most recordings don't drift.
    let flat = median(good.iter().map(|w| w.offset).collect());
    let flat_agree = good
        .iter()
        .filter(|w| (w.offset - flat).abs() <= AGREE)
        .count();
    let mut slopes = Vec::new();
    for (i, a) in good.iter().enumerate() {
        for b in &good[i + 1..] {
            if (b.at - a.at).abs() > 1.0 {
                slopes.push((b.offset - a.offset) / (b.at - a.at));
            }
        }
    }
    if good.len() >= 4 && !slopes.is_empty() {
        let slope = median(slopes);
        let intercept = median(good.iter().map(|w| w.offset - slope * w.at).collect());
        let inliers: Vec<&&Fine> = good
            .iter()
            .filter(|w| (w.offset - (intercept + slope * w.at)).abs() <= AGREE)
            .collect();
        if inliers.len() >= 4 && inliers.len() > flat_agree {
            let n = inliers.len() as f64;
            let mt = inliers.iter().map(|w| w.at).sum::<f64>() / n;
            let mo = inliers.iter().map(|w| w.offset).sum::<f64>() / n;
            let stt: f64 = inliers.iter().map(|w| (w.at - mt).powi(2)).sum();
            let sto: f64 = inliers.iter().map(|w| (w.at - mt) * (w.offset - mo)).sum();
            let slope = if stt > 0.0 { sto / stt } else { 0.0 };
            if (slope * length).abs() >= MIN_DRIFT_TOTAL {
                // offset(r) = (mo - slope·mt) + slope·r, and the follower
                // time is r + offset(r) = intercept + r·(1 + slope).
                return (mo - slope * mt, slope, inliers.len());
            }
        }
    }
    let agreeing: Vec<f64> = good
        .iter()
        .filter(|w| (w.offset - flat).abs() <= AGREE)
        .map(|w| w.offset)
        .collect();
    let mean = agreeing.iter().sum::<f64>() / agreeing.len().max(1) as f64;
    (mean, 0.0, flat_agree)
}

fn confidence(c: &Coarse, windows: usize, agreeing: usize) -> f32 {
    // How far the coarse peak stands above the best elsewhere.
    let prominence = if c.score > 0.0 {
        ((c.score - c.second) / c.score).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let agreement = if windows == 0 {
        0.0
    } else {
        agreeing as f64 / windows as f64
    };
    // Fine windows that agree to the millisecond can't happen by chance;
    // a clear coarse peak alone gets half way.
    let fine_part = if agreeing >= 2 { agreement } else { 0.0 };
    (0.35 * prominence.min(0.5) / 0.5 + 0.65 * fine_part).clamp(0.0, 1.0) as f32
}
