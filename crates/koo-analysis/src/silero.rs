//! Where someone talks, from Silero VAD v6.2 run by whisper.cpp. Unlike
//! the energy VAD in `vad.rs`, which stands in for it without the model, it
//! does not take music for speech. What it hears in an item gives both the
//! tight speech ranges of `Analysis::speech`, for silences and cuts, and
//! the looser stretches Whisper hears.

use crate::download::{Download, ModelFile, StartDownload};
use crate::ffmpeg::SAMPLE_RATE;
use crate::vad;
use anyhow::{Context, Result};
use koo_core::time::{Time, TimeRange};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use whisper_rs::{WhisperVadContext, WhisperVadContextParams};

/// Silero VAD as ggml, which whisper.cpp runs.
const MODEL: ModelFile = ModelFile {
    name: "ggml-silero-v6.2.0.bin",
    url: "https://huggingface.co/ggml-org/whisper-vad/resolve/9ffd54a1e1ee413ddf265af9913beaf518d1639b/ggml-silero-v6.2.0.bin",
    env: "KOOEDITOR_VAD_MODEL",
    what: "voice activity model",
};
/// Silero hears 16 kHz audio in windows of this many samples.
const WINDOW: usize = 512;
/// Music holds Silero down for seconds after it ends, missing the first
/// words said, so it also hears the audio in 4 s blocks, each after the
/// second before it. A block that starts during a sentence doubts the rest
/// of it, so a window is speech when either hearing finds it.
const BLOCK: usize = 125 * WINDOW;
const WARM_UP: usize = 32 * WINDOW;
/// Speech starts where Silero is sure of it and lasts until it doubts it.
const THRESHOLD: f32 = 0.5;
const DOUBT: f32 = THRESHOLD - 0.15;
/// faster-whisper's VAD filter for the stretches Whisper hears: they end
/// once Silero has doubted the speech for 2 s, and take 400 ms either side.
const MIN_SILENCE: usize = 2 * SAMPLE_RATE as usize;
const SPEECH_PAD: usize = SAMPLE_RATE as usize * 400 / 1000;

/// The loaded model, shared by every job.
#[derive(Default)]
pub struct Silero {
    /// Hears one item at a time.
    vad: Mutex<Option<WhisperVadContext>>,
    download: Download,
}

impl Silero {
    /// The model's file, downloading it first if needed.
    pub fn ensure_model(&self, start_download: StartDownload) -> Result<PathBuf> {
        MODEL.ensure(&self.download, start_download)
    }

    /// Silero's chance of speech through 16 kHz mono `audio`.
    pub fn hear(&self, model: &Path, audio: &[f32], progress: &dyn Fn(f32)) -> Result<Heard> {
        // Route whisper.cpp's chatter through `log` instead of stderr.
        whisper_rs::install_logging_hooks();
        let mut vad = self.vad.lock().unwrap_or_else(|e| e.into_inner());
        if vad.is_none() {
            let path = model.to_str().context("model path is not valid UTF-8")?;
            // Too small a model to gain from threads: one runs three times
            // faster than four.
            let mut params = WhisperVadContextParams::default();
            params.set_n_threads(1);
            let loaded = WhisperVadContext::new(path, params).with_context(|| {
                format!("could not load voice activity model {}", model.display())
            })?;
            *vad = Some(loaded);
        }
        let vad = vad.as_mut().expect("loaded above");
        if audio.is_empty() {
            return Ok(Heard {
                probs: Vec::new(),
                len: 0,
            });
        }
        vad.detect_speech(audio)
            .context("voice activity detection failed")?;
        let mut probs = vad.probabilities().to_vec();
        progress(0.5);
        for from in (0..audio.len()).step_by(BLOCK) {
            let warm = from.saturating_sub(WARM_UP);
            vad.detect_speech(&audio[warm..(from + BLOCK).min(audio.len())])
                .context("voice activity detection failed")?;
            let block = &vad.probabilities()[(from - warm) / WINDOW..];
            for (p, &q) in probs[from / WINDOW..].iter_mut().zip(block) {
                *p = p.max(q);
            }
            progress(0.5 + 0.5 * (from + BLOCK).min(audio.len()) as f32 / audio.len() as f32);
        }
        Ok(Heard {
            probs,
            len: audio.len(),
        })
    }
}

/// Silero's chance of speech in each window of an item's audio.
pub struct Heard {
    probs: Vec<f32>,
    /// Samples of audio.
    len: usize,
}

impl Heard {
    /// Speech ranges as tight as the energy VAD's, joined and dropped as
    /// its are. Silero doubts speech a window or two after it ends, so the
    /// ranges need no padding after.
    pub fn speech(&self) -> Vec<TimeRange> {
        let mut talking = false;
        let voiced: Vec<bool> = self
            .probs
            .iter()
            .map(|&p| {
                talking = p >= THRESHOLD || (talking && p >= DOUBT);
                talking
            })
            .collect();
        vad::ranges(&voiced, WINDOW, self.len, Time::ZERO)
    }

    /// The stretches of speech Whisper hears, in samples. They are at least
    /// [`MIN_SILENCE`] apart, so their padding never meets.
    pub fn stretches(&self) -> Vec<Range<usize>> {
        let len = self.len;
        let mut out = Vec::new();
        // Where the current stretch started, and where Silero began to
        // doubt it.
        let mut start = None;
        let mut doubt = None;
        for (i, &p) in self.probs.iter().enumerate() {
            let at = i * WINDOW;
            if p >= THRESHOLD {
                start.get_or_insert(at);
                doubt = None;
            } else if p < DOUBT
                && let Some(s) = start
            {
                let d = *doubt.get_or_insert(at);
                if at - d >= MIN_SILENCE {
                    out.push(s..d);
                    (start, doubt) = (None, None);
                }
            }
        }
        out.extend(start.map(|s| s..len));
        out.into_iter()
            .map(|r| r.start.saturating_sub(SPEECH_PAD)..(r.end + SPEECH_PAD).min(len))
            .collect()
    }
}
