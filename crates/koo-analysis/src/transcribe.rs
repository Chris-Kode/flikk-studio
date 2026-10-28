//! Word-level transcription with whisper.cpp, and the ggml models it needs.
//!
//! Whisper given music or a long silence makes words up there, often a
//! verbatim repeat of what was said before, and its timestamps come loose
//! around them. So, as faster-whisper's VAD filter does, Silero finds the
//! speech first and Whisper hears only that, each stretch on its own: one
//! that starts with music Silero took for speech would throw the times of
//! the speech after it.

use crate::download::{Download, ModelFile, StartDownload};
use crate::ffmpeg::SAMPLE_RATE;
use anyhow::{Context, Result, bail};
use koo_core::analysis::Word;
use koo_core::paths;
use koo_core::time::{Time, TimeRange};
use std::cmp::Reverse;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    WhisperTokenId, WhisperVadContext, WhisperVadContextParams,
};

pub const DEFAULT_MODEL: &str = "ggml-base.bin";
const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";

/// Silero VAD as ggml, which whisper.cpp runs.
const VAD: ModelFile = ModelFile {
    name: "ggml-silero-v6.2.0.bin",
    url: "https://huggingface.co/ggml-org/whisper-vad/resolve/9ffd54a1e1ee413ddf265af9913beaf518d1639b/ggml-silero-v6.2.0.bin",
    env: "KOOEDITOR_VAD_MODEL",
    what: "voice activity model",
};
/// Silero hears 16 kHz audio in windows of this many samples.
const WINDOW: usize = 512;
/// Music holds Silero down for seconds after it ends, missing the first
/// words said, so it hears the audio in 4 s blocks, each after the second
/// before it, instead of in one go.
const BLOCK: usize = 125 * WINDOW;
const WARM_UP: usize = 32 * WINDOW;
/// faster-whisper's VAD filter: speech starts where Silero is sure of it,
/// ends once Silero has doubted it for 2 s, and takes 400 ms either side.
const THRESHOLD: f32 = 0.5;
const DOUBT: f32 = THRESHOLD - 0.15;
const MIN_SILENCE: usize = 2 * SAMPLE_RATE as usize;
const SPEECH_PAD: usize = SAMPLE_RATE as usize * 400 / 1000;

/// The models on disk.
pub struct Files {
    model: PathBuf,
    vad: PathBuf,
}

/// Recognized words and the detected (or given) language.
pub struct Output {
    pub language: String,
    pub words: Vec<Word>,
}

/// Loaded models, shared by every job. Loading takes a moment, so it
/// happens once on first use.
#[derive(Default)]
pub struct Whisper {
    ctx: Mutex<Option<Arc<WhisperContext>>>,
    /// Silero; hears one item at a time.
    vad: Mutex<Option<WhisperVadContext>>,
    downloads: [Download; 2],
}

impl Whisper {
    /// The models' files, downloading them first if needed;
    /// `start_download` returns a progress callback for each download.
    pub fn ensure_model(&self, start_download: StartDownload) -> Result<Files> {
        let (model, name) = model_location()?;
        if !model.is_file() {
            let Some(name) = name else {
                bail!("Whisper model not found at {}", model.display());
            };
            let url = format!("{MODEL_URL}{name}");
            self.downloads[0].ensure(&model, &url, "Whisper model", || start_download(&name))?;
        }
        let vad = VAD.ensure(&self.downloads[1], start_download)?;
        Ok(Files { model, vad })
    }

    fn context(&self, model: &Path) -> Result<Arc<WhisperContext>> {
        let mut ctx = self.ctx.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ctx) = &*ctx {
            return Ok(ctx.clone());
        }
        let path = model.to_str().context("model path is not valid UTF-8")?;
        let loaded = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .with_context(|| format!("could not load Whisper model {}", model.display()))?;
        let loaded = Arc::new(loaded);
        *ctx = Some(loaded.clone());
        Ok(loaded)
    }

    /// Where Silero hears speech in 16 kHz mono `audio`, in samples.
    fn speech(&self, model: &Path, audio: &[f32]) -> Result<Vec<Range<usize>>> {
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
        let mut probs = Vec::with_capacity(audio.len().div_ceil(WINDOW));
        for from in (0..audio.len()).step_by(BLOCK) {
            let warm = from.saturating_sub(WARM_UP);
            vad.detect_speech(&audio[warm..(from + BLOCK).min(audio.len())])
                .context("voice activity detection failed")?;
            probs.extend_from_slice(&vad.probabilities()[(from - warm) / WINDOW..]);
        }
        Ok(stretches(&probs, audio.len()))
    }

    /// Transcribe 16 kHz mono `audio`. `language` is an ISO 639-1 hint;
    /// `None` detects it.
    pub fn transcribe(
        &self,
        files: &Files,
        audio: &[f32],
        language: Option<&str>,
        progress: impl Fn(f32) + Send + 'static,
    ) -> Result<Output> {
        // Route whisper.cpp's chatter through `log` instead of stderr.
        whisper_rs::install_logging_hooks();
        let speech = self.speech(&files.vad, audio)?;
        if speech.is_empty() {
            return Ok(Output {
                language: language.unwrap_or("und").to_string(),
                words: Vec::new(),
            });
        }
        let ctx = self.context(&files.model)?;
        let mut state = ctx
            .create_state()
            .context("could not create Whisper state")?;
        let eot = ctx.token_eot();
        let time = |sample: usize| Time::from_secs_f64(sample as f64 / SAMPLE_RATE as f64);
        // The longest stretch first, and the others in its language, so a
        // short one is not taken for another.
        let mut order: Vec<usize> = (0..speech.len()).collect();
        order.sort_by_key(|&i| Reverse(speech[i].len()));
        let mut language = language.map(str::to_string);
        let mut heard = vec![Vec::new(); speech.len()];
        let total = speech.iter().map(|r| r.len()).sum::<usize>() as f32;
        let progress = Arc::new(progress);
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
        Ok(Output {
            language: language.unwrap_or_else(|| "und".into()),
            words: heard.concat(),
        })
    }
}

/// Stretches of speech, in samples of audio `len` long, from Silero's
/// chance of speech in each window. They are at least [`MIN_SILENCE`]
/// apart, so their padding never meets.
fn stretches(probs: &[f32], len: usize) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    // Where the current stretch started, and where Silero began to doubt it.
    let mut start = None;
    let mut doubt = None;
    for (i, &p) in probs.iter().enumerate() {
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

/// The words Whisper heard in `state`, moved to `start` and never past
/// `end`. Its notes on what is not speech, "(upbeat music)" or
/// "[BLANK_AUDIO]", are left out.
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
        .map(|n| n.get().min(8))
        .unwrap_or(4) as i32
}

/// `$KOOEDITOR_WHISPER_MODEL` (a path, or a ggml file name to fetch into
/// `paths::models()`), else `models()/ggml-base.bin`. The name is set when
/// the file may be downloaded.
fn model_location() -> Result<(PathBuf, Option<String>)> {
    match std::env::var("KOOEDITOR_WHISPER_MODEL") {
        Ok(v) if !v.is_empty() => {
            let path = PathBuf::from(&v);
            let bare = path.components().count() == 1;
            if bare && v.starts_with("ggml-") && v.ends_with(".bin") {
                Ok((paths::models().join(&v), Some(v)))
            } else {
                Ok((path, None))
            }
        }
        _ => Ok((
            paths::models().join(DEFAULT_MODEL),
            Some(DEFAULT_MODEL.to_string()),
        )),
    }
}
