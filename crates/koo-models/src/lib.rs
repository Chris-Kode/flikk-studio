//! The models the editor runs on this machine: the task each one serves,
//! the files it needs in `paths::models()`, whether they are there, and
//! fetching them. Nothing is downloaded unless the user asks
//! ([`Downloads::start`]); until then a task whose model is missing is left
//! out, and [`ModelFile::locate`] says so.
//!
//! Each [`Task`] lists the models that can do it ([`Task::models`]) and
//! runs on one of them ([`Task::model`]). Today every task has one model;
//! more are added to its list, and the user picks among them, later.

mod catalog;
mod download;

pub use catalog::{files, models};
pub use download::{Downloads, Status};

use anyhow::{Result, bail};
use koo_core::paths;
use std::path::PathBuf;

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
}

impl Task {
    /// In the order analysis runs them.
    pub const ALL: [Task; 7] = [
        Task::VoiceActivity,
        Task::Transcription,
        Task::WordTiming,
        Task::Speakers,
        Task::SoundTagging,
        Task::Beats,
        Task::SoundSearch,
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
        }
    }

    /// The models that can do this task, the default first.
    pub fn models(self) -> &'static [&'static Model] {
        match self {
            Task::VoiceActivity => &[&models::SILERO_VAD],
            Task::Transcription => &[&models::WHISPER_BASE],
            Task::WordTiming => &[&models::QWEN3_ALIGNER],
            Task::Speakers => &[&models::PYANNOTE],
            Task::SoundTagging => &[&models::EFFICIENTAT],
            Task::Beats => &[&models::BEAT_THIS],
            Task::SoundSearch => &[&models::CLAP],
        }
    }

    /// The model this task runs on: the default, until the user can pick
    /// another.
    pub fn model(self) -> &'static Model {
        self.models()[0]
    }
}

/// One model as the user sees it: a name, and the files it is made of.
#[derive(Debug)]
pub struct Model {
    /// Stable key for downloads and settings: "whisper-base".
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
