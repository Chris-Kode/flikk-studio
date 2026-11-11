//! First-run setup: every on-device model is needed, so the whole set is
//! downloaded once, from the welcome page, before a project opens. The set
//! is each task's picked model ([`flikk_models::Task::ALL`]); after an
//! update that adds a task, only its files are missing.

use super::EditorState;
use flikk_models::{Model, Status, Task};
use gpui_kit::*;

/// Where the set of models stands, as the welcome page shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum SetupStatus {
    /// Every model is on disk.
    Complete,
    /// Nothing is fetching; these bytes are still to come.
    Missing { bytes: u64 },
    /// 0..1 of the bytes missing when the download started.
    Downloading { fraction: f32 },
    /// A model's download failed, and why; the others may be on disk.
    Failed { message: String },
}

/// The set's download as the editor follows it.
#[derive(Default)]
pub struct Setup {
    /// Bytes missing when the set's download started: what its progress
    /// counts against.
    baseline: u64,
    /// `FLIKK_DEMO_SETUP`: the status shown instead of the real one.
    seeded: Option<SetupStatus>,
}

impl Setup {
    /// The setup the editor starts with: `FLIKK_DEMO_SETUP` seeds one.
    pub fn new() -> Self {
        Self {
            baseline: 0,
            seeded: crate::demo::setup(),
        }
    }
}

impl EditorState {
    /// The models the editor needs, one per task: the one picked for it.
    pub fn required_models(&self) -> Vec<&'static Model> {
        Task::ALL.iter().map(|task| task.model()).collect()
    }

    /// Where the set stands. Reads the files' metadata, which is cheap.
    pub fn setup_status(&self) -> SetupStatus {
        if let Some(seeded) = &self.setup.seeded {
            return seeded.clone();
        }
        let models = self.required_models();
        let statuses: Vec<Status> = models.iter().map(|m| self.models.status(m)).collect();
        if statuses.iter().all(|s| *s == Status::Downloaded) {
            return SetupStatus::Complete;
        }
        let remaining: u64 = models.iter().map(|m| m.missing_bytes()).sum();
        if statuses.iter().any(|s| matches!(s, Status::Downloading(_))) {
            // Downloads started from Settings count too.
            let baseline = self.setup.baseline.max(remaining).max(1);
            let fraction = 1.0 - remaining as f32 / baseline as f32;
            return SetupStatus::Downloading { fraction };
        }
        match statuses.into_iter().find_map(|s| match s {
            Status::Failed(message) => Some(message),
            _ => None,
        }) {
            Some(message) => SetupStatus::Failed { message },
            None => SetupStatus::Missing { bytes: remaining },
        }
    }

    /// Projects wait on the welcome page while a model is missing. The
    /// demo, on engines of its own, opens without them.
    pub fn projects_wait(&self) -> bool {
        match &self.setup.seeded {
            Some(seeded) => *seeded != SetupStatus::Complete,
            None => !crate::demo::enabled() && self.setup_status() != SetupStatus::Complete,
        }
    }

    /// Download every model of the set that is missing, at once: the user
    /// asked, from the welcome page. A failed one starts again; a stopped
    /// one goes on from where it stopped.
    pub fn download_setup(&mut self, cx: &mut Context<Self>) {
        let missing: Vec<&'static Model> = self
            .required_models()
            .into_iter()
            .filter(|m| !matches!(self.models.status(m), Status::Downloaded))
            .collect();
        if !self.models.busy() {
            self.setup.baseline = missing.iter().map(|m| m.missing_bytes()).sum();
        }
        for model in missing {
            self.download_model(model, cx);
        }
        cx.notify();
    }

    /// Stop every download of the set; what they fetched stays for the
    /// next one.
    pub fn cancel_setup(&mut self, cx: &mut Context<Self>) {
        for model in self.required_models() {
            self.models.cancel(model);
        }
        cx.notify();
    }
}
