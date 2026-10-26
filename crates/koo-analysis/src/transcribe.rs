//! Word-level transcription with whisper.cpp, and the ggml model it needs.

use anyhow::{Context, Result, bail};
use koo_core::analysis::Word;
use koo_core::paths;
use koo_core::time::{Time, TimeRange};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub const DEFAULT_MODEL: &str = "ggml-base.bin";
const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";

/// Recognized words and the detected (or given) language.
pub struct Output {
    pub language: String,
    pub words: Vec<Word>,
}

/// Loaded model, shared by every job. Loading takes a moment, so it happens
/// once on first use.
#[derive(Default)]
pub struct Whisper {
    ctx: Mutex<Option<Arc<WhisperContext>>>,
    /// The last failed download, so a queue of items doesn't retry it each.
    download: Mutex<Option<(Instant, String)>>,
}

/// How long a failed download is reported before trying again.
const RETRY_AFTER: Duration = Duration::from_secs(120);

impl Whisper {
    /// The model path, downloading it first if needed; `start_download`
    /// returns a progress callback for the download.
    pub fn ensure_model(
        &self,
        start_download: impl FnOnce(&str) -> Box<dyn Fn(f32) + Send + Sync>,
    ) -> Result<PathBuf> {
        let (path, name) = model_location()?;
        if path.is_file() {
            return Ok(path);
        }
        let Some(name) = name else {
            bail!("Whisper model not found at {}", path.display());
        };
        // One download at a time; later callers find the file in place.
        let mut failed = self.download.lock().unwrap_or_else(|e| e.into_inner());
        if path.is_file() {
            return Ok(path);
        }
        if let Some((at, message)) = &*failed
            && at.elapsed() < RETRY_AFTER
        {
            bail!("{message}");
        }
        match download(&name, &path, &*start_download(&name)) {
            Ok(()) => Ok(path),
            Err(e) => {
                *failed = Some((Instant::now(), format!("{e:#}")));
                Err(e)
            }
        }
    }

    fn context(&self, model: &Path) -> Result<Arc<WhisperContext>> {
        let mut ctx = self.ctx.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ctx) = &*ctx {
            return Ok(ctx.clone());
        }
        // Route whisper.cpp's chatter through `log` instead of stderr.
        whisper_rs::install_logging_hooks();
        let path = model.to_str().context("model path is not valid UTF-8")?;
        let loaded = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .with_context(|| format!("could not load Whisper model {}", model.display()))?;
        let loaded = Arc::new(loaded);
        *ctx = Some(loaded.clone());
        Ok(loaded)
    }

    /// Transcribe 16 kHz mono `audio`. `language` is an ISO 639-1 hint;
    /// `None` detects it.
    pub fn transcribe(
        &self,
        model: &Path,
        audio: &[f32],
        language: Option<&str>,
        progress: impl Fn(f32) + Send + 'static,
    ) -> Result<Output> {
        let ctx = self.context(model)?;
        let mut state = ctx
            .create_state()
            .context("could not create Whisper state")?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(language.unwrap_or("auto")));
        params.set_token_timestamps(true);
        params.set_n_threads(threads());
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_progress_callback_safe(move |p: i32| progress(p as f32 / 100.0));
        state.full(params, audio).context("Whisper failed")?;

        let language = whisper_rs::get_lang_str(state.full_lang_id_from_state())
            .or(language)
            .unwrap_or("und")
            .to_string();
        let eot = ctx.token_eot();
        let mut words = Vec::new();
        let mut current: Option<Pending> = None;
        for segment in state.as_iter() {
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
                        words.extend(current.take().and_then(Pending::finish));
                        current = Some(Pending::new(bytes, data.t0, data.t1, data.p));
                    }
                }
            }
            // Words never span segments.
            words.extend(current.take().and_then(Pending::finish));
        }
        Ok(Output { language, words })
    }
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

/// Fetch `name` from the whisper.cpp Hugging Face repository into `dest`,
/// via a temporary file so a cut download never looks complete.
fn download(name: &str, dest: &Path, progress: &(dyn Fn(f32) + Send + Sync)) -> Result<()> {
    use ureq::tls::{RootCerts, TlsConfig};
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_body(None)
        .build()
        .into();
    let url = format!("{MODEL_URL}{name}");
    let mut response = agent
        .get(&url)
        .call()
        .with_context(|| format!("could not download Whisper model from {url}"))?;
    let total = response.body().content_length();
    let mut body = response.body_mut().as_reader();

    let part = dest.with_extension("bin.part");
    let mut file = std::fs::File::create(&part)
        .with_context(|| format!("could not write {}", part.display()))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        let n = body
            .read(&mut buf)
            .context("Whisper model download interrupted")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        if let Some(total) = total.filter(|t| *t > 0) {
            progress(done as f32 / total as f32);
        }
    }
    file.sync_all()?;
    drop(file);
    if total.is_some_and(|t| t != done) {
        let _ = std::fs::remove_file(&part);
        bail!("Whisper model download incomplete ({done} bytes)");
    }
    std::fs::rename(&part, dest)?;
    Ok(())
}
