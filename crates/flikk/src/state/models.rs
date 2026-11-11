//! Model files and the notices about them: downloads the user starts,
//! the models each task runs on, and what can't be made without them.

use super::EditorState;
use crate::notices::{Level, Notice, NoticeAction};
use crate::persistence;
use gpui_kit::*;

/// Key of the notice listing the models not downloaded.
const MODELS_NOTICE: &str = "models-missing";
/// Key of the notice that subject masks need their model.
const SUBJECTS_NOTICE: &str = "subjects-model";
/// Key of the notice that optical flow needs its model.
const FLOW_NOTICE: &str = "optical-flow-model";
/// Key of the notice that person masks need their model.
const PEOPLE_NOTICE: &str = "people-model";

impl EditorState {
    /// Download `model`: the user asked for it.
    pub fn download_model(&mut self, model: &'static flikk_models::Model, cx: &mut Context<Self>) {
        self.notices.clear(&download_notice(model));
        self.models.start(model);
        cx.notify();
    }

    pub fn cancel_download(&mut self, model: &flikk_models::Model, cx: &mut Context<Self>) {
        self.models.cancel(model);
        cx.notify();
    }

    /// Delete `model`'s files: the user asked. Analysis that needs it waits
    /// until it is downloaded again.
    pub fn delete_model(&mut self, model: &flikk_models::Model, cx: &mut Context<Self>) {
        if let Err(e) = model.delete() {
            self.status = Some(format!("{e:#}"));
        }
        self.poll_models(cx);
        cx.notify();
    }

    /// Run `task` on `model` from now on: the user picked it. Media
    /// analyzed already keeps its results. Once the setup is done, a model
    /// not on disk downloads at once: every model is needed.
    pub fn pick_model(
        &mut self,
        task: flikk_models::Task,
        model: &'static flikk_models::Model,
        cx: &mut Context<Self>,
    ) {
        if task.model().id == model.id {
            return;
        }
        let set_up = !self.projects_wait();
        task.pick(model);
        if set_up && !model.is_downloaded() {
            self.download_model(model, cx);
        }
        persistence::with_store(|s| s.save_model_picks(&flikk_models::picked()));
        self.poll_models(cx);
        self.refresh_model_notices();
        cx.notify();
    }

    /// Media can be analyzed: every analysis task has a model downloaded
    /// or the provider set to transcribe doing it, with its key.
    pub fn models_ready(&self) -> bool {
        let online = self
            .online_transcription()
            .is_none_or(|p| self.provider_status(p).has_key());
        online
            && self
                .required_tasks()
                .iter()
                .all(|t| t.model().is_downloaded())
    }

    /// Analyze `media` (again, replacing its results, when `again`): the
    /// user asked. Waits its turn behind the items queued before it.
    pub fn analyze(&mut self, media: flikk_core::MediaId, again: bool, cx: &mut Context<Self>) {
        let (Some(analysis), Some(item)) =
            (&self.services.analysis, self.bus.project().media(media))
        else {
            return;
        };
        if again {
            analysis.analyze_again(item);
        } else {
            analysis.analyze(item);
        }
        cx.notify();
    }

    /// Whether subject masks can be made; when not (their model is
    /// missing), a notice that leads to Settings → Integrations.
    pub fn subjects_ready(&mut self, cx: &mut Context<Self>) -> bool {
        let ready = match &self.services.subjects {
            Some(s) => s.available(),
            None => Err("Subject masks need the media engine (FFmpeg).".into()),
        };
        let changed = match &ready {
            Ok(()) => self.notices.clear(SUBJECTS_NOTICE),
            Err(e) => {
                // Asked for just now: shown even if closed before.
                self.notices.clear(SUBJECTS_NOTICE);
                self.notices.set(Notice {
                    key: SUBJECTS_NOTICE.into(),
                    level: Level::Warning,
                    title: "Subject masks need their model".into(),
                    items: Vec::new(),
                    detail: Some(e.clone()),
                    action: Some(NoticeAction::OpenModels),
                })
            }
        };
        if changed {
            cx.notify();
        }
        ready.is_ok()
    }

    /// When optical flow is picked without its model: a notice that leads
    /// to Settings → Integrations (slowed frames are blended meanwhile).
    pub fn optical_flow_picked(&mut self, cx: &mut Context<Self>) {
        let Some(Err(e)) = self.services.interpolation.as_ref().map(|s| s.available()) else {
            return;
        };
        self.notices.clear(FLOW_NOTICE);
        self.notices.set(Notice {
            key: FLOW_NOTICE.into(),
            level: Level::Warning,
            title: "Motion smoothing needs its model".into(),
            items: Vec::new(),
            detail: Some(e),
            action: Some(NoticeAction::OpenModels),
        });
        cx.notify();
    }

    /// Whether person masks can be made; when not (their model is
    /// missing), a notice that leads to Settings → Integrations.
    pub fn people_ready(&mut self, cx: &mut Context<Self>) -> bool {
        let ready = match &self.services.subjects {
            Some(s) => s.people_available(),
            None => Err("Person masks need the media engine (FFmpeg).".into()),
        };
        let changed = match &ready {
            Ok(()) => self.notices.clear(PEOPLE_NOTICE),
            Err(e) => {
                self.notices.clear(PEOPLE_NOTICE);
                self.notices.set(Notice {
                    key: PEOPLE_NOTICE.into(),
                    level: Level::Warning,
                    title: "Person masks need their model".into(),
                    items: Vec::new(),
                    detail: Some(e.clone()),
                    action: Some(NoticeAction::OpenModels),
                })
            }
        };
        if changed {
            cx.notify();
        }
        ready.is_ok()
    }

    /// Mattes were made, frames made between, or camera paths measured:
    /// draw again.
    pub(super) fn poll_subjects(&mut self, cx: &mut Context<Self>) {
        let version = self.services.subjects.as_ref().map_or(0, |s| s.version())
            + self
                .services
                .interpolation
                .as_ref()
                .map_or(0, |s| s.version())
            + self.services.stabilizer.as_ref().map_or(0, |s| s.version())
            + self.services.depth.as_ref().map_or(0, |d| d.version());
        if version != self.subjects_version {
            self.subjects_version = version;
            cx.notify();
        }
    }

    pub fn dismiss_notice(&mut self, key: &str, cx: &mut Context<Self>) {
        self.notices.dismiss(key);
        cx.notify();
    }

    /// When a download ends or a model file appears or goes, bring the
    /// notices up to date.
    pub(super) fn poll_models(&mut self, cx: &mut Context<Self>) {
        let seen = (
            self.model_tasks
                .iter()
                .map(|t| t.model().is_downloaded())
                .collect::<Vec<_>>(),
            self.models.finished(),
        );
        if seen == self.models_seen {
            return;
        }
        self.models_seen = seen;
        self.analyze_sounds(cx);
        if self.refresh_model_notices() {
            cx.notify();
        }
    }

    /// One notice listing the models analysis needs that are not
    /// downloaded, one for each download that failed, and one for a
    /// provider without its key. Whether any changed.
    pub(super) fn refresh_model_notices(&mut self) -> bool {
        let mut changed = self.refresh_key_notice();
        let mut missing = Vec::new();
        let required = self.required_tasks();
        for task in &self.model_tasks {
            let model = task.model();
            let key = download_notice(model);
            match self.models.status(model) {
                flikk_models::Status::Downloaded => {
                    changed |= self.notices.clear(&key);
                    continue;
                }
                flikk_models::Status::Failed(message) => {
                    changed |= self.notices.set(Notice {
                        key,
                        level: Level::Error,
                        title: format!("Could not download {}", task.label().to_lowercase()),
                        items: Vec::new(),
                        detail: Some(message),
                        action: Some(NoticeAction::OpenModels),
                    });
                }
                flikk_models::Status::Missing | flikk_models::Status::Downloading(_) => {}
            }
            if required.contains(task) {
                missing.push(task.label().to_owned());
            }
        }
        changed |= if missing.is_empty() {
            self.notices.clear(MODELS_NOTICE)
        } else {
            self.notices.set(Notice {
                key: MODELS_NOTICE.into(),
                level: Level::Warning,
                title: "Some models are not downloaded".into(),
                items: missing,
                detail: Some("Features that use them may not be available.".into()),
                action: Some(NoticeAction::OpenModels),
            })
        };
        changed
    }
}

fn download_notice(model: &flikk_models::Model) -> String {
    format!("download-failed:{}", model.id)
}
