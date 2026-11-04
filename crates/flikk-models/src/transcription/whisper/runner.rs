//! Whisper hearing each stretch of speech on its own, the longest first,
//! after a short silence when it fits one window, and its tokens put
//! together into words. A stretch it doubts holds speech is left out, and
//! it never says the same thing over and over.

use super::config::{
    BEAM_SIZE, DOUBT, FADE, GPU, LEAD, MAX_THREADS, MIN_LOGPROB, NO_SPEECH, WINDOW, repeats,
};
use crate::transcription::{Progress, Transcribed, Transcriber};
use crate::{ModelFile, SPEECH_RATE};
use anyhow::{Context, Result};
use flikk_core::analysis::Word;
use flikk_core::time::{Time, TimeRange};
use std::cmp::Reverse;
use std::ffi::{c_int, c_void};
use std::ops::Range;
use std::sync::{Arc, Mutex};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    WhisperSysContext, WhisperSysState, WhisperTokenData, WhisperTokenId,
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
        let vocab = Vocab {
            eot,
            size: ctx.n_vocab(),
        };
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
            let lead = if LEAD + r.len() <= WINDOW { LEAD } else { 0 };
            let heard_audio = padded(&audio[r.clone()], lead);
            let share = r.len() as f32 / total;
            // Only a stretch Whisper hears in one window can be judged by
            // its start; a longer one is that much speech. Without the
            // look, the stretch is heard out as it was before.
            let first = if heard_audio.len() <= WINDOW {
                first_look(&ctx, &mut state, &heard_audio).ok()
            } else {
                None
            };
            if first.is_some_and(|f| f.no_speech > NO_SPEECH) {
                done += share;
                progress(done);
                continue;
            }
            let mut params = FullParams::new(SamplingStrategy::BeamSearch {
                beam_size: BEAM_SIZE,
                patience: -1.0,
            });
            // The language the look heard, taken for the stretches after
            // only if this one is kept: noise was heard as Norwegian.
            let this = language.as_deref().or(first.and_then(|f| f.language));
            params.set_language(Some(this.unwrap_or("auto")));
            params.set_token_timestamps(true);
            // Each 30 s window on its own, not after the text of the one
            // before: given that, Whisper repeats it over music, or hears
            // nothing in a window that follows other speech.
            params.set_n_max_text_ctx(0);
            // SAFETY: `no_loops` reads `vocab`, which outlives `full`, and
            // the tokens and logits whisper.cpp passes it, and keeps none.
            unsafe {
                params.set_filter_logits_callback(Some(no_loops));
                params.set_filter_logits_callback_user_data(&vocab as *const Vocab as *mut c_void);
            }
            params.set_n_threads(threads());
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            let progress = progress.clone();
            params.set_progress_callback_safe(move |p: i32| {
                progress(done + share * p as f32 / 100.0)
            });
            state.full(params, &heard_audio).context("Whisper failed")?;
            done += share;
            // As OpenAI's transcribe does, but without trusting the words
            // on their own: Whisper is sure of the lines it makes up.
            let doubted = first.is_some_and(|f| {
                f.no_speech > DOUBT && mean_logprob(&state, eot).is_none_or(|l| l < MIN_LOGPROB)
            });
            if doubted {
                continue;
            }
            if language.is_none() {
                language =
                    whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_string);
            }
            let at = TimeRange::new(time(r.start), time(r.end));
            heard[i] = words(&state, eot, at.start - time(lead), at);
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

/// What Whisper makes of a window before it says a word.
#[derive(Clone, Copy)]
struct FirstLook {
    /// The odds that it holds no speech.
    no_speech: f32,
    /// The language it hears.
    language: Option<&'static str>,
}

/// Whisper's first look at `audio`, at most a window long: the next token
/// after the start of a transcript, as language detection reads it, whose
/// odds of "no speech" are the ones OpenAI's transcribe goes by.
fn first_look(ctx: &WhisperContext, state: &mut WhisperState, audio: &[f32]) -> Result<FirstLook> {
    let threads = threads() as usize;
    state
        .pcm_to_mel(audio, threads)
        .context("Whisper could not hear the audio")?;
    let (id, _) = state
        .lang_detect(0, threads)
        .context("Whisper could not hear the audio")?;
    let logits = state.get_logits().context("Whisper gave no logits")?;
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let sum: f32 = logits.iter().map(|l| (l - max).exp()).sum();
    let no_speech = logits
        .get(ctx.token_nosp() as usize)
        .map_or(0.0, |l| (l - max).exp() / sum);
    Ok(FirstLook {
        no_speech,
        language: whisper_rs::get_lang_str(id),
    })
}

/// The mean log-probability of the text tokens Whisper chose in `state`;
/// `None` when it chose none.
fn mean_logprob(state: &WhisperState, eot: WhisperTokenId) -> Option<f32> {
    let (mut sum, mut n) = (0.0, 0);
    for segment in state.as_iter() {
        for token in (0..segment.n_tokens()).filter_map(|i| segment.get_token(i)) {
            if token.token_id() < eot {
                sum += token.token_data().plog;
                n += 1;
            }
        }
    }
    (n > 0).then(|| sum / n as f32)
}

/// What [`no_loops`] needs to know of the model's tokens.
struct Vocab {
    /// The end of text; tokens from it on are not text.
    eot: WhisperTokenId,
    size: c_int,
}

/// whisper.cpp's logits filter, for each beam before each token: the
/// tokens that would say a run of text once more than [`repeats`] allows
/// are ruled out.
unsafe extern "C" fn no_loops(
    _: *mut WhisperSysContext,
    _: *mut WhisperSysState,
    tokens: *const WhisperTokenData,
    n_tokens: c_int,
    logits: *mut f32,
    vocab: *mut c_void,
) {
    let Ok(n) = usize::try_from(n_tokens) else {
        return;
    };
    if tokens.is_null() || logits.is_null() || vocab.is_null() || n < 2 {
        return;
    }
    // SAFETY: `vocab` is the `Vocab` the transcription set, and whisper.cpp
    // passes the beam's `n_tokens` tokens and a logit for each token of
    // the vocabulary, all alive for the call.
    let (vocab, tokens) = unsafe {
        (
            &*(vocab as *const Vocab),
            std::slice::from_raw_parts(tokens, n),
        )
    };
    // Timestamps come between the copies of a looped line, each a new one.
    let text: Vec<WhisperTokenId> = tokens
        .iter()
        .map(|t| t.id)
        .filter(|&id| id < vocab.eot)
        .collect();
    for token in looping(&text) {
        if (0..vocab.size).contains(&token) {
            // SAFETY: inside the vocabulary, so inside `logits`.
            unsafe { *logits.add(token as usize) = f32::NEG_INFINITY };
        }
    }
}

/// The tokens that would start one copy too many of a run that `text` ends
/// with.
fn looping(text: &[WhisperTokenId]) -> impl Iterator<Item = WhisperTokenId> + '_ {
    let n = text.len();
    (1..=n / 2).filter_map(move |run| {
        let said = repeats(run);
        let copies = said * run;
        (copies <= n && (n - copies + run..n).all(|i| text[i] == text[i - run]))
            .then(|| text[n - run])
    })
}

/// `speech` after `lead` samples of silence, faded in from it.
fn padded(speech: &[f32], lead: usize) -> Vec<f32> {
    let mut out = vec![0.0; lead];
    out.extend_from_slice(speech);
    if lead > 0 {
        for (k, s) in out[lead..].iter_mut().take(FADE).enumerate() {
            *s *= k as f32 / FADE as f32;
        }
    }
    out
}

/// The words Whisper heard in `state`, its times from `offset` and kept
/// inside `stretch`, the speech it was given. Its notes on what is not
/// speech, "(upbeat music)" or "\[BLANK_AUDIO\]", are left out.
fn words(state: &WhisperState, eot: WhisperTokenId, offset: Time, stretch: TimeRange) -> Vec<Word> {
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
                let inside = |t: Time| (t + offset).clamp(stretch.start, stretch.end);
                w.range = TimeRange::new(inside(w.range.start), inside(w.range.end));
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
