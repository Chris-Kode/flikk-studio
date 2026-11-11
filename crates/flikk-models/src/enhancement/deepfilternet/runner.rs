//! DeepFilterNet 3 run offline, as the authors' `enhance()` runs it
//! (`DeepFilterNet/df/enhance.py`), with `libDF`'s STFT and features:
//!
//! 1. the input, padded with an FFT of silence at the end, is cut into
//!    frames by the square-root Vorbis window, 960 samples every 480;
//! 2. each frame gives two features: the power of its 32 ERB bands in dB,
//!    less their running mean, over 40; and its 96 lowest bins over the
//!    square root of their running magnitude;
//! 3. the encoder reads the features two frames ahead; its decoders give a
//!    gain per ERB band and five complex taps per low bin;
//! 4. each frame's spectrum takes the band gains above bin 96, and below
//!    it the taps over the frames two before to two after (deep
//!    filtering);
//! 5. overlap-add gives the sound back, 480 samples late, which are
//!    dropped.
//!
//! The graphs run on [`CHUNK`] frames at a time, each stretch started
//! [`WARM_UP`] frames early and those frames thrown away, so memory stays
//! bounded on long recordings. Checked against the authors' PyTorch model
//! on their sample: 20.015 dB SI-SDR here, 20.0149 dB in their tests.

use super::config::{
    ARCHIVE, BINS, CHUNK, CONV_LOOKAHEAD, DF_BINS, DF_LOOKAHEAD, DF_ORDER, ERB_BANDS, ERB_MIN_BINS,
    FFT, HOP, MEAN_NORM_INIT, NORM_ALPHA, UNIT_NORM_INIT, WARM_UP,
};
use crate::enhancement::{Enhancer, Pass, RATE};
use crate::onnx;
use anyhow::{Context as _, Result, anyhow, bail};
use flikk_os::sync::lock;
use ort::session::Session;
use ort::value::TensorRef;
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::io::Read;
use std::sync::{Arc, Mutex};

/// The three graphs, loaded from the archive on first use.
#[derive(Default)]
pub struct DeepFilterNet {
    graphs: Mutex<Option<Graphs>>,
}

struct Graphs {
    encoder: Session,
    erb_decoder: Session,
    df_decoder: Session,
}

impl Graphs {
    fn load() -> Result<Self> {
        let path = ARCHIVE.locate()?;
        let file = std::fs::File::open(&path)
            .with_context(|| format!("could not open {}", path.display()))?;
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
        let (mut encoder, mut erb_decoder, mut df_decoder) = (None, None, None);
        for entry in archive
            .entries()
            .context("the Clean voice model is damaged")?
        {
            let mut entry = entry.context("the Clean voice model is damaged")?;
            let name = entry.path()?.to_string_lossy().into_owned();
            let slot = if name.ends_with("/enc.onnx") {
                &mut encoder
            } else if name.ends_with("/erb_dec.onnx") {
                &mut erb_decoder
            } else if name.ends_with("/df_dec.onnx") {
                &mut df_decoder
            } else {
                continue;
            };
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            *slot = Some(onnx::session_from_memory(&bytes, ARCHIVE.what)?);
        }
        match (encoder, erb_decoder, df_decoder) {
            (Some(encoder), Some(erb_decoder), Some(df_decoder)) => Ok(Self {
                encoder,
                erb_decoder,
                df_decoder,
            }),
            _ => bail!("the Clean voice model is missing a graph: download it again"),
        }
    }

    /// The band gains and deep filter taps of `frames` frames of features
    /// (already shifted [`CONV_LOOKAHEAD`] ahead): `erb` is `frames` ×
    /// [`ERB_BANDS`], `spec` the real parts of `frames` × [`DF_BINS`] then
    /// the imaginary ones.
    fn run(&mut self, frames: usize, erb: &[f32], spec: &[f32]) -> Result<(Vec<f32>, Vec<f32>)> {
        let failed = |e: ort::Error| anyhow!("the Clean voice model failed: {e}");
        let feat_erb =
            TensorRef::from_array_view(([1, 1, frames, ERB_BANDS], erb)).map_err(failed)?;
        let feat_spec =
            TensorRef::from_array_view(([1, 2, frames, DF_BINS], spec)).map_err(failed)?;
        let encoded = self
            .encoder
            .run(ort::inputs!["feat_erb" => feat_erb, "feat_spec" => feat_spec])
            .map_err(failed)?;
        let output = |name: &str| -> Result<TensorRef<'_, f32>> {
            let (shape, values) = encoded[name].try_extract_tensor::<f32>().map_err(failed)?;
            let dims: Vec<usize> = shape.iter().map(|d| *d as usize).collect();
            TensorRef::from_array_view((dims, values)).map_err(failed)
        };
        let gains = self
            .erb_decoder
            .run(ort::inputs![
                "emb" => output("emb")?,
                "e3" => output("e3")?,
                "e2" => output("e2")?,
                "e1" => output("e1")?,
                "e0" => output("e0")?
            ])
            .map_err(failed)?;
        let (_, gains) = gains["m"].try_extract_tensor::<f32>().map_err(failed)?;
        let gains = gains.to_vec();
        let taps = self
            .df_decoder
            .run(ort::inputs!["emb" => output("emb")?, "c0" => output("c0")?])
            .map_err(failed)?;
        let (_, taps) = taps["coefs"].try_extract_tensor::<f32>().map_err(failed)?;
        let taps = taps.to_vec();
        if gains.len() != frames * ERB_BANDS || taps.len() != frames * DF_BINS * DF_ORDER * 2 {
            bail!("the Clean voice model gave an answer of the wrong size");
        }
        if gains.iter().chain(&taps).any(|v| !v.is_finite()) {
            bail!("the Clean voice model gave an answer that is not numbers");
        }
        Ok((gains, taps))
    }
}

impl Enhancer for DeepFilterNet {
    fn start(&self) -> Result<Box<dyn Pass + '_>> {
        let mut graphs = lock(&self.graphs);
        // Its file was deleted: let go of it, and fail as if never there.
        if !super::MODEL.is_downloaded() {
            *graphs = None;
        }
        if graphs.is_none() {
            *graphs = Some(Graphs::load()?);
        }
        drop(graphs);
        Ok(Box::new(Stream::new(&self.graphs)))
    }
}

/// The ERB band each bin belongs to, as bins per band (`libDF`'s
/// `erb_fb`).
fn erb_widths() -> Vec<usize> {
    let to_erb = |hz: f32| 9.265 * (hz / (24.7 * 9.265)).ln_1p();
    let from_erb = |erb: f32| 24.7 * 9.265 * ((erb / 9.265).exp() - 1.0);
    let bin_hz = RATE as f32 / FFT as f32;
    let (low, high) = (to_erb(0.0), to_erb(RATE as f32 / 2.0));
    let step = (high - low) / ERB_BANDS as f32;
    let mut widths = vec![0usize; ERB_BANDS];
    let (mut previous, mut over) = (0i32, 0i32);
    for (i, width) in widths.iter_mut().enumerate() {
        let bin = (from_erb(low + (i + 1) as f32 * step) / bin_hz).round() as i32;
        let mut n = bin - previous - over;
        if n < ERB_MIN_BINS as i32 {
            over = ERB_MIN_BINS as i32 - n;
            n = ERB_MIN_BINS as i32;
        } else {
            over = 0;
        }
        *width = n as usize;
        previous = bin;
    }
    widths[ERB_BANDS - 1] += 1;
    let total: usize = widths.iter().sum();
    widths[ERB_BANDS - 1] -= total.saturating_sub(BINS);
    widths
}

/// `n` values from `from` to `to`, evenly spaced.
fn ramp(from: f32, to: f32, n: usize) -> Vec<f32> {
    let step = (to - from) / (n - 1) as f32;
    (0..n).map(|i| from + i as f32 * step).collect()
}

/// The STFT both ways, with `libDF`'s window and scaling.
struct Stft {
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    /// The last hop of input, for the next frame.
    analysis: Vec<f32>,
    /// The second half of the last frame out, to add to the next.
    synthesis: Vec<f32>,
    time: Vec<f32>,
    scratch: Vec<Complex32>,
}

impl Stft {
    fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let half = (FFT / 2) as f64;
        let window = (0..FFT)
            .map(|i| {
                let s = (0.5 * std::f64::consts::PI * (i as f64 + 0.5) / half).sin();
                (0.5 * std::f64::consts::PI * s * s).sin() as f32
            })
            .collect();
        let forward = planner.plan_fft_forward(FFT);
        let inverse = planner.plan_fft_inverse(FFT);
        let scratch =
            vec![Complex32::default(); forward.get_scratch_len().max(inverse.get_scratch_len())];
        Self {
            forward,
            inverse,
            window,
            analysis: vec![0.0; FFT - HOP],
            synthesis: vec![0.0; FFT - HOP],
            time: vec![0.0; FFT],
            scratch,
        }
    }

    /// The spectrum of the frame ending with `hop`.
    fn analyze(&mut self, hop: &[f32]) -> Vec<Complex32> {
        let (old, new) = self.time.split_at_mut(FFT - HOP);
        for ((t, x), w) in old.iter_mut().zip(&self.analysis).zip(&self.window) {
            *t = x * w;
        }
        for ((t, x), w) in new.iter_mut().zip(hop).zip(&self.window[FFT - HOP..]) {
            *t = x * w;
        }
        self.analysis.copy_from_slice(hop);
        let mut spectrum = vec![Complex32::default(); BINS];
        self.forward
            .process_with_scratch(&mut self.time, &mut spectrum, &mut self.scratch)
            .expect("sizes are fixed");
        let norm = 2.0 * HOP as f32 / (FFT * FFT) as f32;
        for x in &mut spectrum {
            *x *= norm;
        }
        spectrum
    }

    /// The next hop of sound from `spectrum`.
    fn synthesize(&mut self, mut spectrum: Vec<Complex32>) -> Vec<f32> {
        // The DC and Nyquist bins of a real signal are real.
        spectrum[0].im = 0.0;
        spectrum[BINS - 1].im = 0.0;
        self.inverse
            .process_with_scratch(&mut spectrum, &mut self.time, &mut self.scratch)
            .expect("sizes are fixed");
        for (t, w) in self.time.iter_mut().zip(&self.window) {
            *t *= w;
        }
        let out: Vec<f32> = self.time[..HOP]
            .iter()
            .zip(&self.synthesis)
            .map(|(t, m)| t + m)
            .collect();
        self.synthesis.copy_from_slice(&self.time[HOP..]);
        out
    }
}

/// One stream in progress. Frames are numbered from the stream's start;
/// `spectra` and the features hold those from `kept` on.
struct Stream<'a> {
    graphs: &'a Mutex<Option<Graphs>>,
    stft: Stft,
    widths: Vec<usize>,
    mean_norm: Vec<f32>,
    unit_norm: Vec<f32>,
    /// Input not yet a whole hop.
    pending: Vec<f32>,
    /// Input samples taken.
    taken: usize,
    kept: usize,
    spectra: Vec<Vec<Complex32>>,
    /// Per frame: ERB features, and the low bins normalized.
    erb: Vec<[f32; ERB_BANDS]>,
    spec: Vec<[Complex32; DF_BINS]>,
    /// Frames enhanced and turned back into sound.
    done: usize,
    /// Output samples still to drop (the STFT's delay), and given.
    skip: usize,
    given: usize,
}

impl<'a> Stream<'a> {
    fn new(graphs: &'a Mutex<Option<Graphs>>) -> Self {
        Self {
            graphs,
            stft: Stft::new(),
            widths: erb_widths(),
            mean_norm: ramp(MEAN_NORM_INIT[0], MEAN_NORM_INIT[1], ERB_BANDS),
            unit_norm: ramp(UNIT_NORM_INIT[0], UNIT_NORM_INIT[1], DF_BINS),
            pending: Vec::new(),
            taken: 0,
            kept: 0,
            spectra: Vec::new(),
            erb: Vec::new(),
            spec: Vec::new(),
            done: 0,
            skip: FFT - HOP,
            given: 0,
        }
    }

    /// Frames analyzed so far.
    fn frames(&self) -> usize {
        self.kept + self.spectra.len()
    }

    /// Analyze every whole hop in `pending`.
    fn analyze(&mut self) {
        let hops = self.pending.len() / HOP;
        for h in 0..hops {
            let spectrum = self.stft.analyze(&self.pending[h * HOP..(h + 1) * HOP]);
            self.features(&spectrum);
            self.spectra.push(spectrum);
        }
        self.pending.drain(..hops * HOP);
    }

    fn features(&mut self, spectrum: &[Complex32]) {
        let mut erb = [0f32; ERB_BANDS];
        let mut bin = 0;
        for ((e, width), mean) in erb.iter_mut().zip(&self.widths).zip(&mut self.mean_norm) {
            let power: f32 = spectrum[bin..bin + width]
                .iter()
                .map(|x| x.norm_sqr())
                .sum::<f32>()
                / *width as f32;
            bin += width;
            let level = (power + 1e-10).log10() * 10.0;
            *mean = level * (1.0 - NORM_ALPHA) + *mean * NORM_ALPHA;
            *e = (level - *mean) / 40.0;
        }
        let mut spec = [Complex32::default(); DF_BINS];
        for ((s, x), unit) in spec.iter_mut().zip(spectrum).zip(&mut self.unit_norm) {
            *unit = x.norm() * (1.0 - NORM_ALPHA) + *unit * NORM_ALPHA;
            *s = *x / unit.sqrt();
        }
        self.erb.push(erb);
        self.spec.push(spec);
    }

    /// Enhance frames `done..done + len`, of `total` frames when the input
    /// has ended (frames past it are silence), and turn them into sound.
    fn enhance(&mut self, len: usize, total: Option<usize>) -> Result<Vec<f32>> {
        let start = self.done;
        let from = start.saturating_sub(WARM_UP);
        let frames = start + len - from;
        // The features each frame reads: two frames ahead, zero past the end.
        let mut erb = Vec::with_capacity(frames * ERB_BANDS);
        let mut spec = vec![0f32; frames * DF_BINS * 2];
        let (re, im) = spec.split_at_mut(frames * DF_BINS);
        for (i, t) in (from..start + len).enumerate() {
            let at = (t + CONV_LOOKAHEAD)
                .checked_sub(self.kept)
                .filter(|_| total.is_none_or(|n| t + CONV_LOOKAHEAD < n));
            match at.and_then(|k| Some((self.erb.get(k)?, self.spec.get(k)?))) {
                Some((e, s)) => {
                    erb.extend_from_slice(e);
                    for (f, x) in s.iter().enumerate() {
                        re[i * DF_BINS + f] = x.re;
                        im[i * DF_BINS + f] = x.im;
                    }
                }
                None => erb.extend_from_slice(&[0.0; ERB_BANDS]),
            }
        }
        let (gains, taps) = {
            let mut graphs = lock(self.graphs);
            let graphs = graphs
                .as_mut()
                .ok_or_else(|| anyhow!("the Clean voice model is not loaded"))?;
            graphs.run(frames, &erb, &spec)?
        };
        let mut out = Vec::with_capacity(len * HOP);
        for t in start..start + len {
            let i = t - from;
            let spectrum = &self.spectra[t - self.kept];
            let mut enhanced = Vec::with_capacity(BINS);
            enhanced.resize(DF_BINS, Complex32::default());
            for (f, e) in enhanced.iter_mut().enumerate() {
                for k in 0..DF_ORDER {
                    let Some(source) = (t + k).checked_sub(DF_ORDER - 1 - DF_LOOKAHEAD) else {
                        continue;
                    };
                    let Some(x) = source
                        .checked_sub(self.kept)
                        .and_then(|s| self.spectra.get(s))
                    else {
                        continue;
                    };
                    let tap = ((i * DF_BINS + f) * DF_ORDER + k) * 2;
                    *e += x[f] * Complex32::new(taps[tap], taps[tap + 1]);
                }
            }
            let mut bin = 0;
            for (band, width) in self.widths.iter().enumerate() {
                let gain = gains[i * ERB_BANDS + band];
                for x in &spectrum[bin..bin + width] {
                    if bin >= DF_BINS {
                        enhanced.push(*x * gain);
                    }
                    bin += 1;
                }
            }
            out.extend(self.stft.synthesize(enhanced));
        }
        self.done += len;
        self.forget();
        let skip = self.skip.min(out.len());
        self.skip -= skip;
        out.drain(..skip);
        Ok(out)
    }

    /// Let go of the frames no stretch reads again: spectra two frames
    /// back for the filter, features as far back as a warm-up reaches.
    fn forget(&mut self) {
        let keep_from = self
            .done
            .saturating_sub(WARM_UP)
            .min(self.done.saturating_sub(DF_ORDER));
        let drop = keep_from.saturating_sub(self.kept);
        self.spectra.drain(..drop);
        self.erb.drain(..drop);
        self.spec.drain(..drop);
        self.kept += drop;
    }

    /// Hand out at most what the input was long, less what was given.
    fn give(&mut self, mut out: Vec<f32>, limit: usize) -> Vec<f32> {
        out.truncate(limit.saturating_sub(self.given));
        self.given += out.len();
        out
    }
}

impl Pass for Stream<'_> {
    fn push(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        self.pending.extend_from_slice(samples);
        self.taken += samples.len();
        self.analyze();
        let mut out = Vec::new();
        // A stretch also needs the frames its last ones look ahead to.
        let ahead = CONV_LOOKAHEAD.max(DF_LOOKAHEAD);
        while self.frames() >= self.done + CHUNK + ahead {
            out.extend(self.enhance(CHUNK, None)?);
        }
        let limit = self.taken;
        Ok(self.give(out, limit))
    }

    fn finish(mut self: Box<Self>) -> Result<Vec<f32>> {
        // A whole FFT of silence flushes the last frames, then up to a hop.
        let tail = FFT + (HOP - (self.pending.len() + FFT) % HOP) % HOP;
        self.pending.extend(std::iter::repeat_n(0.0, tail));
        self.analyze();
        let total = self.frames();
        let mut out = Vec::new();
        while self.done < total {
            let len = CHUNK.min(total - self.done);
            out.extend(self.enhance(len, Some(total))?);
        }
        let limit = self.taken;
        Ok(self.give(out, limit))
    }
}
