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
pub mod depth;
mod download;
pub mod interpolation;
pub mod matting;
#[cfg(any(
    feature = "aligner",
    feature = "diarizer",
    feature = "sounds",
    feature = "segment",
    feature = "interpolate",
    feature = "matting",
    feature = "depth"
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
use flikk_core::paths;
use flikk_core::time::Time;
use flikk_os::sync::lock;
use std::path::PathBuf;
use std::sync::Mutex;

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
    /// Person masks: not part of analysis.
    Matting,
    /// Depth maps for depth effects and masks: not part of analysis.
    Depth,
}

impl Task {
    /// In the order analysis runs them, then subject masks, optical flow,
    /// person masks and depth.
    pub const ALL: [Task; 11] = [
        Task::VoiceActivity,
        Task::Transcription,
        Task::WordTiming,
        Task::Speakers,
        Task::SoundTagging,
        Task::Beats,
        Task::SoundSearch,
        Task::Segmentation,
        Task::FrameInterpolation,
        Task::Matting,
        Task::Depth,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Task::VoiceActivity => "Silence detection",
            Task::Transcription => "Transcription",
            Task::WordTiming => "Word timing",
            Task::Speakers => "Speaker labels",
            Task::SoundTagging => "Sound labels",
            Task::Beats => "Beat detection",
            Task::SoundSearch => "Sound search",
            Task::Segmentation => "Subject masks",
            Task::FrameInterpolation => "Motion smoothing",
            Task::Matting => "Person masks",
            Task::Depth => "Depth maps",
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
            Task::Matting => matting::MODELS,
            Task::Depth => depth::MODELS,
        }
    }

    /// The model this task runs on: the one the user picked, else the
    /// default.
    pub fn model(self) -> &'static Model {
        let models = self.models();
        let picked = lock(&PICKED);
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
        let mut picked = lock(&PICKED);
        picked.retain(|id| models.iter().all(|m| m.id != *id));
        if models[1..].iter().any(|m| m.id == model.id) {
            picked.push(model.id);
        }
    }
}

/// The models the user picked over their task's default, by id.
static PICKED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// The ids of the models picked over their task's default, to save.
pub fn picked() -> Vec<&'static str> {
    lock(&PICKED).clone()
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
    /// Every file is on disk and whole ([`ModelFile::is_present`]).
    pub fn is_downloaded(&self) -> bool {
        self.files.iter().all(ModelFile::is_present)
    }

    /// Bytes still to fetch: of the files not on disk yet, or not whole,
    /// less what a download stopped part way through already holds.
    pub fn missing_bytes(&self) -> u64 {
        self.files
            .iter()
            .filter(|f| !f.is_present())
            .map(|f| f.size.saturating_sub(f.fetched()))
            .sum()
    }

    /// Whether a download stopped part way through left any of its files
    /// behind, to go on from.
    pub fn partly_downloaded(&self) -> bool {
        self.files.iter().any(|f| f.fetched() > 0)
    }

    /// Delete its files, and what a download left of them, from
    /// `paths::models()`. Copies an environment variable points at, and
    /// files another model also uses, stay.
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
            for path in [file.path(), download::part_path(&file.path())] {
                match std::fs::remove_file(&path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                        bail!("could not delete {}: {e}", path.display())
                    }
                    _ => {}
                }
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
    /// Bytes, as published at `url`: exact, since a file of any other
    /// length is taken for a cut or damaged download. 0 when unknown, and
    /// then any length passes.
    pub size: u64,
    /// SHA-256 of the file at `url`, lower-case hex: the host's own for
    /// the pinned commit (the LFS object id), or measured on bytes checked
    /// against the host. A download is checked against it once, before it
    /// takes the file's name; a mismatch deletes it. Empty when unknown.
    pub sha256: &'static str,
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

    /// Bytes a download stopped part way through left in its part file,
    /// to go on from. 0 for a copy the environment points at.
    fn fetched(&self) -> u64 {
        if self.overridden().is_some() {
            return 0;
        }
        std::fs::metadata(download::part_path(&self.path())).map_or(0, |m| m.len())
    }

    /// On disk and whole. Cheap: no more than the file's metadata is read.
    pub fn is_present(&self) -> bool {
        matches!(self.on_disk().1, OnDisk::Whole)
    }

    /// The file on disk, or why it isn't there.
    pub fn locate(&self) -> Result<PathBuf> {
        let (path, on_disk) = self.on_disk();
        match on_disk {
            OnDisk::Whole => Ok(path),
            OnDisk::Missing => match self.env.filter(|_| self.overridden().is_some()) {
                Some(env) => bail!("{} not found at {} (from {env})", self.what, path.display()),
                None => bail!("{} is not downloaded", self.what),
            },
            OnDisk::WrongSize(len) => bail!(
                "{} is damaged or incomplete ({len} of {} bytes): download it again",
                self.what,
                self.size
            ),
        }
    }

    /// Where the file is read from, and what is there. A file in
    /// `paths::models()` is whole when it is as long as published (a cut
    /// or damaged download is not); a copy [`Self::env`] points at may be
    /// any size, as may a file whose [`Self::size`] is 0.
    fn on_disk(&self) -> (PathBuf, OnDisk) {
        let overridden = self.overridden();
        let any_size = overridden.is_some() || self.size == 0;
        let path = overridden.unwrap_or_else(|| paths::models().join(self.name));
        let on_disk = match std::fs::metadata(&path) {
            Ok(m) if m.is_file() && (any_size || m.len() == self.size) => OnDisk::Whole,
            Ok(m) if m.is_file() => OnDisk::WrongSize(m.len()),
            _ => OnDisk::Missing,
        };
        (path, on_disk)
    }
}

/// What [`ModelFile::on_disk`] found.
enum OnDisk {
    Whole,
    Missing,
    /// A file this many bytes long, not the size published.
    WrongSize(u64),
}
