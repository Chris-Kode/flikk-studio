//! Everything about the models the editor runs on this machine: one
//! folder per task, and in it one folder per model.
//!
//! - `<task>/mod.rs`: what the task does, as the trait the analysis
//!   pipeline calls; `MODELS`, the models that can do it, the default
//!   first; and `runner`, which builds the runner of one of them.
//! - `<task>/<model>/mod.rs`: the model as the user sees it, its
//!   [`Model`] (one per size, when it comes in several).
//! - `<task>/<model>/config.rs`: its files, where they download from, and
//!   every number it is run with.
//! - `<task>/<model>/runner.rs`: loading and running it, behind the
//!   task's feature.
//!
//! What the models share sits here: the catalog ([`Task`], [`Model`],
//! [`ModelFile`]), the model the user picked for each task
//! ([`Task::pick`]), the audio rates they hear, [`Downloads`], ONNX Runtime
//! (`onnx.rs`) and the BPE tokenizer (`bpe.rs`).
//!
//! Nothing is downloaded unless the user asks ([`Downloads::start`]);
//! until then a runner whose files are missing fails, saying so
//! ([`ModelFile::locate`]). A new model is a folder under its task, its
//! `MODEL` in the task's `MODELS` and an arm in the task's `runner`.

pub mod beats;
#[cfg(any(feature = "aligner", feature = "sounds"))]
mod bpe;
mod download;
pub mod interpolation;
#[cfg(any(
    feature = "aligner",
    feature = "diarizer",
    feature = "sounds",
    feature = "segment",
    feature = "interpolate"
))]
mod onnx;
pub mod segmentation;
pub mod sound_search;
pub mod sound_tagging;
pub mod speakers;
pub mod transcription;
pub mod voice_activity;
pub mod word_timing;

pub use download::{Downloads, Status};

use anyhow::{Result, bail};
use koo_core::paths;
use koo_core::time::Time;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

/// Samples per second of the mono audio the speech models hear: voice
/// activity, transcription, word timing and speakers.
pub const SPEECH_RATE: u32 = 16_000;

/// Samples per second of the mono audio the sound models hear: sound
/// tagging, beats and sound search. 16 kHz would drop everything above
/// 8 kHz (cymbals, glass, sibilance).
pub const SOUND_RATE: u32 = 32_000;

/// How a sound model hears a stream of [`SOUND_RATE`] audio: windows of
/// `len` samples every `step`, run `batch` at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Windows {
    pub len: usize,
    pub step: usize,
    pub batch: usize,
}

impl Windows {
    /// The time a window spans.
    pub fn len_time(&self) -> Time {
        Time::from_secs_f64(self.len as f64 / SOUND_RATE as f64)
    }

    /// The time between the starts of windows.
    pub fn step_time(&self) -> Time {
        Time::from_secs_f64(self.step as f64 / SOUND_RATE as f64)
    }
}

/// What a model is used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Task {
    VoiceActivity,
    Transcription,
    WordTiming,
    Speakers,
    SoundTagging,
    Beats,
    SoundSearch,
    /// Subject masks: not part of analysis.
    Segmentation,
    /// Optical flow slow motion: not part of analysis.
    FrameInterpolation,
}

impl Task {
    /// In the order analysis runs them, then subject masks and optical
    /// flow.
    pub const ALL: [Task; 9] = [
        Task::VoiceActivity,
        Task::Transcription,
        Task::WordTiming,
        Task::Speakers,
        Task::SoundTagging,
        Task::Beats,
        Task::SoundSearch,
        Task::Segmentation,
        Task::FrameInterpolation,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Task::VoiceActivity => "Voice activity",
            Task::Transcription => "Transcription",
            Task::WordTiming => "Word timing",
            Task::Speakers => "Speakers",
            Task::SoundTagging => "Sounds and music",
            Task::Beats => "Beats",
            Task::SoundSearch => "Sound search",
            Task::Segmentation => "Subject masks",
            Task::FrameInterpolation => "Optical flow",
        }
    }

    /// The models that can do this task, the default first.
    pub fn models(self) -> &'static [&'static Model] {
        match self {
            Task::VoiceActivity => voice_activity::MODELS,
            Task::Transcription => transcription::MODELS,
            Task::WordTiming => word_timing::MODELS,
            Task::Speakers => speakers::MODELS,
            Task::SoundTagging => sound_tagging::MODELS,
            Task::Beats => beats::MODELS,
            Task::SoundSearch => sound_search::MODELS,
            Task::Segmentation => segmentation::MODELS,
            Task::FrameInterpolation => interpolation::MODELS,
        }
    }

    /// The model this task runs on: the one the user picked, else the
    /// default.
    pub fn model(self) -> &'static Model {
        let models = self.models();
        let picked = picked_lock();
        models
            .iter()
            .find(|m| picked.contains(&m.id))
            .copied()
            .unwrap_or(models[0])
    }

    /// Run this task on `model`, one of its [`models`](Self::models), from
    /// now on.
    pub fn pick(self, model: &'static Model) {
        let models = self.models();
        let mut picked = picked_lock();
        picked.retain(|id| models.iter().all(|m| m.id != *id));
        if models[1..].iter().any(|m| m.id == model.id) {
            picked.push(model.id);
        }
    }
}

/// The models the user picked over their task's default, by id.
static PICKED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn picked_lock() -> MutexGuard<'static, Vec<&'static str>> {
    PICKED.lock().unwrap_or_else(|e| e.into_inner())
}

/// The ids of the models picked over their task's default, to save.
pub fn picked() -> Vec<&'static str> {
    picked_lock().clone()
}

/// Pick the model with each of `ids` again, as [`picked`] gave them. Ids
/// no task has any more are skipped.
pub fn restore_picks(ids: &[String]) {
    for task in Task::ALL {
        let saved = task
            .models()
            .iter()
            .find(|m| ids.iter().any(|id| id == m.id));
        if let Some(model) = saved {
            task.pick(model);
        }
    }
}

/// One model as the user sees it: a name, and the files it is made of.
#[derive(Debug)]
pub struct Model {
    /// Stable key for downloads and settings: "whisper-small-q8".
    pub id: &'static str,
    pub name: &'static str,
    pub files: &'static [ModelFile],
}

impl Model {
    /// Every file is on disk.
    pub fn is_downloaded(&self) -> bool {
        self.files.iter().all(ModelFile::is_present)
    }

    /// Bytes of the files not on disk yet.
    pub fn missing_bytes(&self) -> u64 {
        self.files
            .iter()
            .filter(|f| !f.is_present())
            .map(|f| f.size)
            .sum()
    }

    /// Delete its files from `paths::models()`. Copies an environment
    /// variable points at, and files another model also uses, stay.
    pub fn delete(&self) -> Result<()> {
        let shared = |file: &ModelFile| {
            Task::ALL
                .iter()
                .flat_map(|t| t.models())
                .any(|m| m.id != self.id && m.files.iter().any(|f| f.name == file.name))
        };
        for file in self.files {
            if file.overridden().is_some() || shared(file) {
                continue;
            }
            match std::fs::remove_file(file.path()) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    bail!("could not delete {}: {e}", file.path().display())
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// One file of a model, kept in `paths::models()` under its name.
#[derive(Debug)]
pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    /// Points at a copy instead, never fetched: a path, or a file name in
    /// `paths::models()`.
    pub env: Option<&'static str>,
    /// What errors call it: "sound tagger".
    pub what: &'static str,
    /// Bytes, as published.
    pub size: u64,
}

impl ModelFile {
    /// Where the file is read from.
    pub fn path(&self) -> PathBuf {
        self.overridden()
            .unwrap_or_else(|| paths::models().join(self.name))
    }

    /// The copy [`Self::env`] points at, when it is set.
    fn overridden(&self) -> Option<PathBuf> {
        let value = std::env::var_os(self.env?).filter(|v| !v.is_empty())?;
        let path = PathBuf::from(value);
        Some(if path.components().count() == 1 {
            paths::models().join(path)
        } else {
            path
        })
    }

    pub fn is_present(&self) -> bool {
        self.path().is_file()
    }

    /// The file on disk, or why it isn't there.
    pub fn locate(&self) -> Result<PathBuf> {
        let path = self.path();
        if path.is_file() {
            return Ok(path);
        }
        match self.env.filter(|_| self.overridden().is_some()) {
            Some(env) => bail!("{} not found at {} (from {env})", self.what, path.display()),
            None => bail!("{} is not downloaded", self.what),
        }
    }
}
