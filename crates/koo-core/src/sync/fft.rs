//! A plain iterative radix-2 FFT, enough for cross-correlating envelopes
//! and short windows of 8 kHz audio.

use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    pub fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    pub fn conj(self) -> Self {
        Self::new(self.re, -self.im)
    }

    pub fn times(self, o: Self) -> Self {
        Self::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }

    pub fn norm(self) -> f64 {
        self.re.hypot(self.im)
    }

    pub fn scale(self, k: f64) -> Self {
        Self::new(self.re * k, self.im * k)
    }
}

/// Tables for transforms of one power-of-two length, made once and reused
/// (the onset envelope runs one per 10 ms of audio).
pub struct Plan {
    n: usize,
    /// e^(-2πik/n) for k < n/2.
    twiddles: Vec<Complex>,
}

impl Plan {
    pub fn new(n: usize) -> Self {
        assert!(n.is_power_of_two());
        let twiddles = (0..n / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / n as f64;
                Complex::new(a.cos(), a.sin())
            })
            .collect();
        Self { n, twiddles }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// In place; `inverse` divides by the length.
    pub fn run(&self, data: &mut [Complex], inverse: bool) {
        let n = self.n;
        assert_eq!(data.len(), n);
        if n < 2 {
            return;
        }
        let bits = n.trailing_zeros();
        for i in 0..n {
            let j = i.reverse_bits() >> (usize::BITS - bits);
            if j > i {
                data.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let stride = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let w = self.twiddles[k * stride];
                    let w = if inverse { w.conj() } else { w };
                    let a = data[start + k];
                    let b = data[start + k + half].times(w);
                    data[start + k] = Complex::new(a.re + b.re, a.im + b.im);
                    data[start + k + half] = Complex::new(a.re - b.re, a.im - b.im);
                }
            }
            len *= 2;
        }
        if inverse {
            let k = 1.0 / n as f64;
            for v in data.iter_mut() {
                *v = v.scale(k);
            }
        }
    }
}

/// In place; `data.len()` must be a power of two. `inverse` divides by the
/// length.
pub fn fft(data: &mut [Complex], inverse: bool) {
    Plan::new(data.len()).run(data, inverse);
}

/// `real` zero-padded to `n` and transformed.
pub fn forward_real(real: &[f32], n: usize) -> Vec<Complex> {
    let mut data = vec![Complex::default(); n];
    for (d, &v) in data.iter_mut().zip(real) {
        d.re = v as f64;
    }
    fft(&mut data, false);
    data
}

/// Cross-correlation of `a` and `b`: `out[lag]` = Σ a[i]·b[i + lag], for
/// lags from `-(a.len() - 1)` to `b.len() - 1`, as a vector indexed by
/// `lag + a.len() - 1`.
pub fn xcorr(a: &[f32], b: &[f32]) -> Vec<f64> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let total = a.len() + b.len() - 1;
    let n = total.next_power_of_two();
    let fa = forward_real(a, n);
    let mut fb = forward_real(b, n);
    for (y, x) in fb.iter_mut().zip(&fa) {
        *y = y.times(x.conj());
    }
    fft(&mut fb, true);
    // fb[k] holds lag k for k ≥ 0 and lag k − n for the wrapped negatives.
    let neg = a.len() - 1;
    (0..total)
        .map(|i| {
            let lag = i as isize - neg as isize;
            let idx = if lag >= 0 {
                lag as usize
            } else {
                (n as isize + lag) as usize
            };
            fb[idx].re
        })
        .collect()
}
