//! Whisper hearing each stretch of speech on its own, the longest first,
//! and its tokens put together into words.

use super::config::{GPU, MAX_THREADS};
use crate::transcription::{Progress, Transcribed, Transcriber};
use crate::{ModelFile, SPEECH_RATE};
use anyhow::{Context, Result};
use koo_core::analysis::Word;
use koo_core::time::{Time, TimeRange};
use std::cmp::Reverse;
use std::ops::Range;
use std::sync::{Arc, Mutex};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    WhisperTokenId,
};

/// The loaded model, shared by every job. Loading takes a moment, so it
/// happens once on first use.
pub struct Whisper {
    file: &'static ModelFile,
    ctx: Mutex<Option<Arc<WhisperContext>>>,
}

impl Whisper {
    /// Runs the size in `file`, one of [`super::config`]'s.
    pub fn new(file: &'static ModelFile) -> Self {
        Self {
            file,
            ctx: Mutex::default(),
        }
    }

    fn context(&self) -> Result<Arc<WhisperContext>> {
        let mut ctx = self.ctx.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ctx) = &*ctx {
            return Ok(ctx.clone());
        }
        let model = self.file.locate()?;
        let path = model.to_str().context("model path is not valid UTF-8")?;
        // Flash attention is faster on the GPU; it rules out DTW
        // timestamps, which are off anyway.
        let params = WhisperContextParameters {
            flash_attn: GPU,
            ..WhisperContextParameters::default()
        };
        let loaded = WhisperContext::new_with_params(path, params)
            .with_context(|| format!("could not load Whisper model {}", model.display()))?;
        let loaded = Arc::new(loaded);
        *ctx = Some(loaded.clone());
        Ok(loaded)
    }
}

impl Transcriber for Whisper {
    fn transcribe(
        &self,
        audio: &[f32],
        speech: &[Range<usize>],
        language: Option<&str>,
        progress: Progress,
    ) -> Result<Transcribed> {
        // Route whisper.cpp's chatter through `log` instead of stderr.
        whisper_rs::install_logging_hooks();
        let ctx = self.context()?;
        let mut state = ctx
            .create_state()
            .context("could not create Whisper state")?;
        let eot = ctx.token_eot();
        let time = |sample: usize| Time::from_secs_f64(sample as f64 / SPEECH_RATE as f64);
        // The longest stretch first, and the others in its language, so a
        // short one is not taken for another.
        let mut order: Vec<usize> = (0..speech.len()).collect();
        order.sort_by_key(|&i| Reverse(speech[i].len()));
        let mut language = language.map(str::to_string);
        let mut heard = vec![Vec::new(); speech.len()];
        let total = speech.iter().map(|r| r.len()).sum::<usize>() as f32;
        let mut done = 0.0;
        for i in order {
            let r = speech[i].clone();
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_language(Some(language.as_deref().unwrap_or("auto")));
            params.set_token_timestamps(true);
            // Each 30 s window on its own, not after the text of the one
            // before: given that, Whisper repeats it over music, or hears
            // nothing in a window that follows other speech.
            params.set_n_max_text_ctx(0);
            params.set_n_threads(threads());
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            let (progress, share) = (progress.clone(), r.len() as f32 / total);
            params.set_progress_callback_safe(move |p: i32| {
                progress(done + share * p as f32 / 100.0)
            });
            state
                .full(params, &audio[r.clone()])
                .context("Whisper failed")?;
            if language.is_none() {
                language =
                    whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_string);
            }
            heard[i] = words(&state, eot, time(r.start), time(r.end));
            done += share;
        }
        Ok(Transcribed {
            language: language.unwrap_or_else(|| "und".into()),
            words: heard.concat(),
        })
    }

    fn language(&self, tag: &str) -> Option<&'static str> {
        (tag.len() == 2)
            .then(|| whisper_rs::get_lang_id(tag))
            .flatten()
            .and_then(whisper_rs::get_lang_str)
    }
}

/// The words Whisper heard in `state`, moved to `start` and never past
/// `end`. Its notes on what is not speech, "(upbeat music)" or
/// "\[BLANK_AUDIO\]", are left out.
fn words(state: &WhisperState, eot: WhisperTokenId, start: Time, end: Time) -> Vec<Word> {
    let mut words = Vec::new();
    for segment in state.as_iter() {
        let mut heard = Vec::new();
        let mut current: Option<Pending> = None;
        for i in 0..segment.n_tokens() {
            let Some(token) = segment.get_token(i) else {
                continue;
            };
            if token.token_id() >= eot {
                continue; // timestamps, language and control tokens
            }
            let Ok(bytes) = token.to_bytes() else {
                continue;
            };
            let data = token.token_data();
            let starts_word = bytes.first() == Some(&b' ')
                || current
                    .as_ref()
                    .is_some_and(|w| w.ends_cjk() && starts_cjk(bytes));
            match &mut current {
                Some(w) if !starts_word => w.push(bytes, data.t1, data.p),
                _ => {
                    heard.extend(current.take().and_then(Pending::finish));
                    current = Some(Pending::new(bytes, data.t0, data.t1, data.p));
                }
            }
        }
        // Words never span segments, nor do notes.
        heard.extend(current.take().and_then(Pending::finish));
        let mut note = false;
        for mut w in heard {
            note |= w.text.starts_with(['(', '[']);
            let closes = w
                .text
                .trim_end_matches(|c: char| c.is_ascii_punctuation() && !matches!(c, ')' | ']'))
                .ends_with([')', ']']);
            if !note {
                w.range = TimeRange::new(
                    (w.range.start + start).min(end),
                    (w.range.end + start).min(end),
                );
                words.push(w);
            }
            note &= !closes;
        }
    }
    words
}

/// A word being assembled from tokens. Tokens can split a UTF-8 character,
/// so text is collected as bytes.
struct Pending {
    bytes: Vec<u8>,
    t0: i64,
    t1: i64,
    p_sum: f32,
    n: u32,
}

impl Pending {
    fn new(bytes: &[u8], t0: i64, t1: i64, p: f32) -> Self {
        Self {
            bytes: bytes.to_vec(),
            t0,
            t1,
            p_sum: p,
            n: 1,
        }
    }

    fn push(&mut self, bytes: &[u8], t1: i64, p: f32) {
        self.bytes.extend_from_slice(bytes);
        self.t1 = self.t1.max(t1);
        self.p_sum += p;
        self.n += 1;
    }

    fn ends_cjk(&self) -> bool {
        std::str::from_utf8(&self.bytes)
            .ok()
            .and_then(|s| s.chars().next_back())
            .is_some_and(is_cjk)
    }

    fn finish(self) -> Option<Word> {
        let text = String::from_utf8_lossy(&self.bytes).trim().to_string();
        if text.is_empty() {
            return None;
        }
        // whisper.cpp timestamps are in centiseconds.
        let start = Time::from_millis(self.t0.max(0) * 10);
        let end = Time::from_millis(self.t1.max(self.t0 + 1) * 10);
        Some(Word {
            range: TimeRange::new(start, end),
            text,
            confidence: (self.p_sum / self.n as f32).clamp(0.0, 1.0),
            speaker: None,
        })
    }
}

/// Scripts written without spaces get one word per character group.
fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

fn starts_cjk(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.chars().next())
        .is_some_and(is_cjk)
}

fn threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get().min(MAX_THREADS))
        .unwrap_or(4) as i32
}
