//! What each bar of a song sounds like, from mono PCM: its harmony
//! (chroma) and timbre (MFCC, the shape of the spectral envelope) in each
//! quarter of the bar, and its loudness. Also moves a downbeat onto the
//! attack the beat tracker heard it near, so edits keep the hit whole.

/// Sample rate the song is decoded at for analysis.
pub const RATE: u32 = 22_050;
/// STFT frame and hop: 93 ms frames every 23 ms.
const FRAME: usize = 2048;
const HOP: usize = 512;
const MELS: usize = 26;
/// MFCCs kept, after dropping the zeroth (loudness, kept apart).
pub const MFCC: usize = 12;
/// Each bar is described per quarter, so a bar's progression counts as
/// well as its average.
pub const PARTS: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct BarFeatures {
    /// Per quarter, unit length (or zero in silence).
    pub chroma: [[f32; 12]; PARTS],
    pub mfcc: [[f32; MFCC]; PARTS],
    /// RMS level in dBFS.
    pub loudness_db: f32,
}

/// Per STFT frame: chroma power and MFCCs.
struct Frames {
    chroma: Vec<[f32; 12]>,
    mfcc: Vec<[f32; MFCC]>,
}

/// Features of each bar; `bars` are `(start, end)` seconds from the start of
/// `pcm`.
pub fn bars(pcm: &[f32], bars: &[(f64, f64)]) -> Vec<BarFeatures> {
    let frames = frames(pcm);
    let frame_of = |t: f64| -> usize {
        let center = t * RATE as f64 - FRAME as f64 / 2.0;
        (center / HOP as f64).round().max(0.0) as usize
    };
    bars.iter()
        .map(|&(s, e)| {
            let mut chroma = [[0f32; 12]; PARTS];
            let mut mfcc = [[0f32; MFCC]; PARTS];
            for p in 0..PARTS {
                let a = s + (e - s) * p as f64 / PARTS as f64;
                let b = s + (e - s) * (p + 1) as f64 / PARTS as f64;
                let (fa, fb) = (frame_of(a), frame_of(b).max(frame_of(a) + 1));
                let fb = fb.min(frames.chroma.len());
                let n = fb.saturating_sub(fa);
                if n == 0 {
                    continue;
                }
                for f in fa..fb {
                    for (acc, v) in chroma[p].iter_mut().zip(&frames.chroma[f]) {
                        *acc += v;
                    }
                    for (acc, v) in mfcc[p].iter_mut().zip(&frames.mfcc[f]) {
                        *acc += v / n as f32;
                    }
                }
                // Magnitude rather than power, so one loud note does not
                // drown the chord.
                for v in &mut chroma[p] {
                    *v = v.sqrt();
                }
                let norm = chroma[p].iter().map(|v| v * v).sum::<f32>().sqrt();
                if norm > 1e-9 {
                    for v in &mut chroma[p] {
                        *v /= norm;
                    }
                }
            }
            let (i, j) = (
                ((s * RATE as f64) as usize).min(pcm.len()),
                ((e * RATE as f64) as usize).min(pcm.len()),
            );
            let rms = if j > i {
                (pcm[i..j].iter().map(|x| (x * x) as f64).sum::<f64>() / (j - i) as f64).sqrt()
            } else {
                0.0
            };
            BarFeatures {
                chroma,
                mfcc,
                loudness_db: (20.0 * rms.max(1e-6).log10()) as f32,
            }
        })
        .collect()
}

fn frames(pcm: &[f32]) -> Frames {
    let window: Vec<f32> = (0..FRAME)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FRAME as f32).cos())
        .collect();
    let bin_hz = RATE as f32 / FRAME as f32;
    // Pitch class of each bin between A1 and about D8.
    let pitch: Vec<Option<usize>> = (0..FRAME / 2 + 1)
        .map(|b| {
            let f = b as f32 * bin_hz;
            (55.0..5000.0).contains(&f).then(|| {
                let midi = 69.0 + 12.0 * (f / 440.0).log2();
                (midi.round() as i64).rem_euclid(12) as usize
            })
        })
        .collect();
    let mel = mel_bank(bin_hz);
    let dct = dct_matrix();
    let count = if pcm.len() >= FRAME {
        (pcm.len() - FRAME) / HOP + 1
    } else {
        0
    };
    let mut out = Frames {
        chroma: Vec::with_capacity(count),
        mfcc: Vec::with_capacity(count),
    };
    let mut re = vec![0f32; FRAME];
    let mut im = vec![0f32; FRAME];
    let mut power = vec![0f32; FRAME / 2 + 1];
    for k in 0..count {
        let at = k * HOP;
        for i in 0..FRAME {
            re[i] = pcm[at + i] * window[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        for b in 0..power.len() {
            power[b] = re[b] * re[b] + im[b] * im[b];
        }
        let mut chroma = [0f32; 12];
        for (b, p) in pitch.iter().enumerate() {
            if let Some(p) = p {
                chroma[*p] += power[b];
            }
        }
        let mut log_mel = [0f32; MELS];
        for (m, band) in mel.iter().enumerate() {
            let e: f32 = band.iter().map(|&(b, w)| power[b] * w).sum();
            log_mel[m] = (e + 1e-10).ln();
        }
        let mut mfcc = [0f32; MFCC];
        for (c, row) in dct.iter().enumerate() {
            mfcc[c] = row.iter().zip(&log_mel).map(|(a, b)| a * b).sum();
        }
        out.chroma.push(chroma);
        out.mfcc.push(mfcc);
    }
    out
}

/// Triangular mel bands from 30 Hz to 8 kHz, as (bin, weight) lists.
fn mel_bank(bin_hz: f32) -> Vec<Vec<(usize, f32)>> {
    let to_mel = |f: f32| 2595.0 * (1.0 + f / 700.0).log10();
    let from_mel = |m: f32| 700.0 * (10f32.powf(m / 2595.0) - 1.0);
    let (lo, hi) = (to_mel(30.0), to_mel(8000.0));
    let edges: Vec<f32> = (0..MELS + 2)
        .map(|i| from_mel(lo + (hi - lo) * i as f32 / (MELS + 1) as f32))
        .collect();
    (0..MELS)
        .map(|m| {
            let (a, c, b) = (edges[m], edges[m + 1], edges[m + 2]);
            (0..FRAME / 2 + 1)
                .filter_map(|bin| {
                    let f = bin as f32 * bin_hz;
                    let w = if f > a && f <= c {
                        (f - a) / (c - a)
                    } else if f > c && f < b {
                        (b - f) / (b - c)
                    } else {
                        0.0
                    };
                    (w > 0.0).then_some((bin, w))
                })
                .collect()
        })
        .collect()
}

/// DCT-II rows 1..=MFCC over the mel bands.
fn dct_matrix() -> Vec<[f32; MELS]> {
    (1..=MFCC)
        .map(|c| {
            let mut row = [0f32; MELS];
            for (m, v) in row.iter_mut().enumerate() {
                *v = (std::f32::consts::PI * c as f32 * (m as f32 + 0.5) / MELS as f32).cos()
                    * (2.0 / MELS as f32).sqrt();
            }
            row
        })
        .collect()
}

/// In-place radix-2 FFT; `re.len()` is a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * std::f64::consts::PI / len as f64;
        let (wr, wi) = (angle.cos() as f32, angle.sin() as f32);
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1f32, 0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let next = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = next;
            }
        }
        len <<= 1;
    }
}

/// The attack nearest a downbeat at `t` seconds: where the high-passed
/// level rises most within 40 ms either side, a few milliseconds early so
/// the cut sits just before it. `t` itself when nothing rises by 6 dB.
pub fn attack(pcm: &[f32], t: f64) -> f64 {
    const BLOCK: usize = 55; // 2.5 ms
    const REACH: f64 = 0.040;
    const BEFORE: usize = 4; // blocks the rise is measured against
    let rate = RATE as f64;
    let first = ((t - REACH) * rate) as i64 - (BEFORE * BLOCK) as i64;
    if first < 1 {
        return t;
    }
    let first = first as usize;
    let last = (((t + REACH) * rate) as usize + BLOCK).min(pcm.len());
    if last <= first + BLOCK * (BEFORE + 1) {
        return t;
    }
    let blocks: Vec<f32> = (first..last)
        .step_by(BLOCK)
        .map(|s| {
            (s..(s + BLOCK).min(last))
                .map(|i| {
                    let d = pcm[i] - pcm[i - 1];
                    d * d
                })
                .sum::<f32>()
                / BLOCK as f32
        })
        .collect();
    let mut best = (0f32, None);
    for b in BEFORE..blocks.len() {
        let before = blocks[b - BEFORE..b].iter().sum::<f32>() / BEFORE as f32;
        let rise = blocks[b] / (before + 1e-9);
        if rise > best.0 {
            best = (rise, Some(b));
        }
    }
    match best {
        (rise, Some(b)) if rise >= 4.0 => {
            let at = (first + b * BLOCK) as f64 / rate - 0.003;
            if (at - t).abs() <= REACH + 0.005 {
                at
            } else {
                t
            }
        }
        _ => t,
    }
}
