//! Silero's chance of speech in each window, heard twice (see
//! [`BLOCK`]), read into tight speech ranges and the looser stretches
//! Whisper hears.

use super::config::{
    BLOCK, DOUBT, FILE, MIN_SILENCE, SPEECH_PAD, THREADS, THRESHOLD, WARM_UP, WINDOW,
};
use crate::voice_activity::{Speech, VoiceActivity, ranges};
use anyhow::{Context, Result};
use koo_core::time::{Time, TimeRange};
use std::ops::Range;
use std::sync::Mutex;
use whisper_rs::{WhisperVadContext, WhisperVadContextParams};

/// The loaded model, shared by every job.
#[derive(Default)]
pub struct Silero {
    /// Hears one item at a time.
    vad: Mutex<Option<WhisperVadContext>>,
}

impl VoiceActivity for Silero {
    fn hear(&self, audio: &[f32], progress: &dyn Fn(f32)) -> Result<Speech> {
        let heard = self.probabilities(audio, progress)?;
        Ok(Speech {
            ranges: heard.speech(),
            stretches: heard.stretches(),
        })
    }
}

impl Silero {
    /// Silero's chance of speech through 16 kHz mono `audio`.
    fn probabilities(&self, audio: &[f32], progress: &dyn Fn(f32)) -> Result<Heard> {
        // Route whisper.cpp's chatter through `log` instead of stderr.
        whisper_rs::install_logging_hooks();
        let mut vad = self.vad.lock().unwrap_or_else(|e| e.into_inner());
        if vad.is_none() {
            let model = FILE.locate()?;
            let path = model.to_str().context("model path is not valid UTF-8")?;
            let mut params = WhisperVadContextParams::default();
            params.set_n_threads(THREADS);
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
struct Heard {
    probs: Vec<f32>,
    /// Samples of audio.
    len: usize,
}

impl Heard {
    /// Speech ranges as tight as the energy VAD's, joined and dropped as
    /// its are. Silero doubts speech a window or two after it ends, so the
    /// ranges need no padding after.
    fn speech(&self) -> Vec<TimeRange> {
        let mut talking = false;
        let voiced: Vec<bool> = self
            .probs
            .iter()
            .map(|&p| {
                talking = p >= THRESHOLD || (talking && p >= DOUBT);
                talking
            })
            .collect();
        ranges(&voiced, WINDOW, self.len, Time::ZERO)
    }

    /// The stretches of speech Whisper hears, in samples. They are at least
    /// [`MIN_SILENCE`] apart, so their padding never meets.
    fn stretches(&self) -> Vec<Range<usize>> {
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
