//! Kaldi log-mel filterbank features, as WeSpeaker's speaker embeddings
//! are trained on: 25 ms Hamming frames every 10 ms, no dither, DC
//! removed, pre-emphasis 0.97, a 512-point power spectrum and 80 mel bins
//! from 20 Hz to 8 kHz. Matches `torchaudio.compliance.kaldi.fbank`.

use crate::SPEECH_RATE;

/// Mel bins per frame.
pub const BINS: usize = 80;
/// Samples per frame.
const LENGTH: usize = SPEECH_RATE as usize * 25 / 1000;
/// Samples between frame starts.
pub const SHIFT: usize = SPEECH_RATE as usize / 100;
const FFT: usize = 512;
const PREEMPHASIS: f32 = 0.97;
const LOW_HZ: f32 = 20.0;

/// The window, the mel filters and the FFT's tables, made once.
pub struct Fbank {
    window: Vec<f32>,
    /// For each mel bin, its first FFT bin and weights from there.
    filters: Vec<(usize, Vec<f32>)>,
    twiddles: Vec<(f32, f32)>,
    reversed: Vec<usize>,
}

impl Default for Fbank {
    fn default() -> Self {
        let window = (0..LENGTH)
            .map(|i| {
                let phase = 2.0 * std::f64::consts::PI * i as f64 / (LENGTH - 1) as f64;
                (0.54 - 0.46 * phase.cos()) as f32
            })
            .collect();
        let mel = |hz: f32| 1127.0 * (1.0 + hz / 700.0).ln();
        let (low, high) = (mel(LOW_HZ), mel(SPEECH_RATE as f32 / 2.0));
        let delta = (high - low) / (BINS + 1) as f32;
        let bin_hz = SPEECH_RATE as f32 / FFT as f32;
        let filters = (0..BINS)
            .map(|b| {
                let (left, center, right) = (
                    low + b as f32 * delta,
                    low + (b + 1) as f32 * delta,
                    low + (b + 2) as f32 * delta,
                );
                let weights: Vec<(usize, f32)> = (0..FFT / 2)
                    .filter_map(|i| {
                        let m = mel(bin_hz * i as f32);
                        (m > left && m < right).then(|| {
                            let w = if m <= center {
                                (m - left) / (center - left)
                            } else {
                                (right - m) / (right - center)
                            };
                            (i, w)
                        })
                    })
                    .collect();
                let first = weights.first().map_or(0, |w| w.0);
                (first, weights.into_iter().map(|w| w.1).collect())
            })
            .collect();
        let twiddles = (0..FFT / 2)
            .map(|k| {
                let a = -2.0 * std::f64::consts::PI * k as f64 / FFT as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        let bits = FFT.trailing_zeros();
        let reversed = (0..FFT)
            .map(|i| i.reverse_bits() >> (usize::BITS - bits))
            .collect();
        Self {
            window,
            filters,
            twiddles,
            reversed,
        }
    }
}

impl Fbank {
    /// Frames that fit whole in `samples` samples.
    pub fn frames(samples: usize) -> usize {
        if samples < LENGTH {
            0
        } else {
            1 + (samples - LENGTH) / SHIFT
        }
    }

    /// [`BINS`] log energies per frame of 16 kHz mono `samples` (−1..1),
    /// frame after frame.
    pub fn compute(&self, samples: &[f32]) -> Vec<f32> {
        let frames = Self::frames(samples.len());
        let mut out = Vec::with_capacity(frames * BINS);
        let mut frame = [0f32; LENGTH];
        let mut re = vec![0f32; FFT];
        let mut im = vec![0f32; FFT];
        let mut power = vec![0f32; FFT / 2 + 1];
        for f in 0..frames {
            // Kaldi works on 16-bit sample values.
            for (x, s) in frame.iter_mut().zip(&samples[f * SHIFT..]) {
                *x = s * 32768.0;
            }
            let mean = frame.iter().sum::<f32>() / LENGTH as f32;
            frame.iter_mut().for_each(|x| *x -= mean);
            for i in (1..LENGTH).rev() {
                frame[i] -= PREEMPHASIS * frame[i - 1];
            }
            frame[0] -= PREEMPHASIS * frame[0];
            re.fill(0.0);
            im.fill(0.0);
            for (i, (x, w)) in frame.iter().zip(&self.window).enumerate() {
                re[self.reversed[i]] = x * w;
            }
            self.fft(&mut re, &mut im);
            for (k, p) in power.iter_mut().enumerate() {
                *p = re[k] * re[k] + im[k] * im[k];
            }
            out.extend(self.filters.iter().map(|(first, weights)| {
                let energy: f32 = weights
                    .iter()
                    .zip(&power[*first..])
                    .map(|(w, p)| w * p)
                    .sum();
                energy.max(f32::EPSILON).ln()
            }));
        }
        out
    }

    /// In-place radix-2 FFT of input already in bit-reversed order.
    fn fft(&self, re: &mut [f32], im: &mut [f32]) {
        let mut size = 2;
        while size <= FFT {
            let half = size / 2;
            let stride = FFT / size;
            for start in (0..FFT).step_by(size) {
                for k in 0..half {
                    let (c, s) = self.twiddles[k * stride];
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * c - im[b] * s;
                    let ti = re[b] * s + im[b] * c;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            size *= 2;
        }
    }
}
