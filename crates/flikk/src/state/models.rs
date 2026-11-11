//! Model files: downloads the user starts, the models each task runs on,
//! and analysis asked for. Every model is on disk before a project opens
//! (see `setup.rs`), so nothing here warns about a missing one.

use super::EditorState;
use crate::persistence;
use gpui_kit::*;

impl EditorState {
    /// Download `model`: the user asked for it.
    pub fn download_model(&mut self, model: &'static flikk_models::Model, cx: &mut Context<Self>) {
        self.setup_download_started(model);
        self.models.start(model);
        cx.notify();
    }

    /// Stop `model`'s download; what it fetched stays for the next one. A
    /// switch to it is given up: the model in use stays.
    pub fn cancel_download(&mut self, model: &flikk_models::Model, cx: &mut Context<Self>) {
        self.models.cancel(model);
        if self.switching.is_some_and(|(_, to)| to.id == model.id) {
            self.switching = None;
        }
        cx.notify();
    }

    /// Delete `model`'s files: the user asked. Only the transcription size
    /// not picked is offered.
    pub fn delete_model(&mut self, model: &flikk_models::Model, cx: &mut Context<Self>) {
        if let Err(e) = model.delete() {
            self.status = Some(format!("{e:#}"));
        }
        self.poll_models(cx);
        cx.notify();
    }

    /// Run `task` on `model`: the user picked it. Media analyzed already
    /// keeps its results. A model on disk, or any during the setup, takes
    /// over at once. Otherwise it downloads, and the model in use stays
    /// until it is on disk ([`Self::picked_model`] shows the pick): every
    /// model is needed, and analysis on a missing one would leave its step
    /// out. Picking again gives up a switch under way, and its download.
    pub fn pick_model(
        &mut self,
        task: flikk_models::Task,
        model: &'static flikk_models::Model,
        cx: &mut Context<Self>,
    ) {
        if let Some((_, to)) = self.switching.take_if(|(t, _)| *t == task)
            && to.id != model.id
        {
            self.models.cancel(to);
        }
        if task.model().id != model.id {
            if model.is_downloaded() || self.projects_wait() {
                self.use_model(task, model);
            } else {
                self.switching = Some((task, model));
                self.download_model(model, cx);
            }
        }
        self.poll_models(cx);
        cx.notify();
    }

    /// The model the user picked for `task`: the one downloading to take
    /// over, else the one in use.
    pub fn picked_model(&self, task: flikk_models::Task) -> &'static flikk_models::Model {
        match self.switching {
            Some((t, to)) if t == task => to,
            _ => task.model(),
        }
    }

    /// Run `task` on `model` from now on, and remember it.
    fn use_model(&mut self, task: flikk_models::Task, model: &'static flikk_models::Model) {
        task.pick(model);
        persistence::with_store(|s| s.save_model_picks(&flikk_models::picked()));
    }

    /// Media can be analyzed: unless a provider is picked to transcribe
    /// and has no key, which has a notice of its own. The models are on
    /// disk before a project opens.
    pub fn analysis_ready(&self) -> bool {
        self.online_transcription()
            .is_none_or(|p| self.provider_status(p).has_key())
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

    /// When a download ends or a model file appears or goes: the library's
    /// sounds may be heard now, and the pages showing models redraw.
    pub(super) fn poll_models(&mut self, cx: &mut Context<Self>) {
        let seen = (
            self.required_models()
                .iter()
                .map(|m| m.is_downloaded())
                .collect::<Vec<_>>(),
            self.models.finished(),
        );
        if seen == self.models_seen {
            return;
        }
        self.models_seen = seen;
        if let Some((task, to)) = self.switching.take_if(|(_, to)| to.is_downloaded()) {
            self.use_model(task, to);
        }
        self.analyze_sounds(cx);
        cx.notify();
    }
}
