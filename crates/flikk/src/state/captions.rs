//! Caption styles: the presets of the selected style profile, which of
//! them new captions use, the unsaved style the viewer shows while a
//! control is dragged, and when the captions' letterer must be made again
//! for the styles shown.

use super::EditorState;
use crate::persistence;
use flikk_core::TrackId;
use flikk_core::command::Command;
use flikk_core::memory::{CaptionStyle, Presets};
use flikk_core::model::{Project, Track, TrackKind};
use flikk_core::services::LetteringService;
use flikk_deliver::Letterer;
use gpui_kit::*;
use std::borrow::Cow;
use std::sync::Arc;

/// The presets in use and the caption styles lettered.
#[derive(Default)]
pub(super) struct CaptionStyles {
    /// Caption styles, loudness and ducking defaults of the selected style
    /// profile.
    presets: Presets,
    /// Counts reloads of `presets`, so an edit begun before one can tell.
    presets_version: u64,
    /// Index into `presets.caption_styles` used for new captions.
    active: usize,
    /// Unsaved caption style the viewer shows while a control is dragged,
    /// with its index.
    caption_live: Option<(usize, CaptionStyle)>,
    /// The caption styles the letterer letters in.
    lettering_styles: Vec<CaptionStyle>,
    /// Counts new letterers, so the viewer knows to redraw.
    lettering_version: u64,
}

impl CaptionStyles {
    /// Take `presets`, loaded for another style profile or project: the
    /// version moves on and the active style is the one `project`'s first
    /// caption track names, else the first. `lettering` is made again for
    /// them when they differ.
    pub(super) fn load(
        &mut self,
        presets: Presets,
        project: &Project,
        lettering: &mut Option<Arc<dyn LetteringService>>,
    ) {
        self.presets = presets;
        self.presets_version += 1;
        self.active = 0;
        self.follow(project);
        self.refresh_lettering(lettering);
    }

    /// Change the presets with `f`, and `lettering` with them. Whether they
    /// changed.
    pub(super) fn edit(
        &mut self,
        f: impl FnOnce(&mut Presets),
        lettering: &mut Option<Arc<dyn LetteringService>>,
    ) -> bool {
        let before = self.presets.clone();
        f(&mut self.presets);
        self.refresh_lettering(lettering);
        self.presets != before
    }

    /// Make preset `index` the active style. The command that saves its
    /// name on `project`'s first caption track, when that names another.
    pub(super) fn pick(&mut self, index: usize, project: &Project) -> Option<Command> {
        self.active = index;
        let name = self
            .presets
            .caption_styles
            .get(index)
            .map(|s| s.name.clone());
        match first_caption_track(project) {
            Some(t) if name.is_some() && t.caption_style != name => {
                Some(Command::SetCaptionStyle {
                    track: t.id,
                    style: name,
                })
            }
            _ => None,
        }
    }

    /// Make the style `project`'s first caption track names the active
    /// one, when there is such a preset.
    pub(super) fn follow(&mut self, project: &Project) {
        let saved = first_caption_track(project).and_then(|t| t.caption_style.as_deref());
        if let Some(i) = saved.and_then(|name| self.position(name)) {
            self.active = i;
        }
    }

    /// Caption styles as the viewer shows them: with the one being dragged.
    pub(super) fn shown(&self) -> Cow<'_, [CaptionStyle]> {
        let styles = &self.presets.caption_styles;
        match &self.caption_live {
            Some((i, live)) if *i < styles.len() => {
                let mut styles = styles.clone();
                styles[*i] = live.clone();
                Cow::Owned(styles)
            }
            _ => Cow::Borrowed(styles),
        }
    }

    /// Show style `index` as `style` without saving it, lettered by
    /// `lettering`.
    pub(super) fn show_live(
        &mut self,
        index: usize,
        style: CaptionStyle,
        lettering: &mut Option<Arc<dyn LetteringService>>,
    ) {
        self.caption_live = Some((index, style));
        self.refresh_lettering(lettering);
    }

    /// Stop showing an unsaved style, lettering the saved ones again.
    /// Whether one was shown.
    pub(super) fn hide_live(&mut self, lettering: &mut Option<Arc<dyn LetteringService>>) -> bool {
        if self.caption_live.take().is_none() {
            return false;
        }
        self.refresh_lettering(lettering);
        true
    }

    /// Forget an unsaved style, leaving the letterer to the caller (the
    /// presets are about to be replaced or edited).
    pub(super) fn clear_live(&mut self) {
        self.caption_live = None;
    }

    /// The preset index caption `track` of `project` is styled with: its
    /// named preset, else the first.
    pub(super) fn of_track(&self, project: &Project, track: TrackId) -> usize {
        project
            .timeline
            .track(track)
            .and_then(|t| t.caption_style.as_deref())
            .and_then(|name| self.position(name))
            .unwrap_or(0)
    }

    /// Letter captions in the styles the viewer shows, when they changed:
    /// `lettering` is made again only then (or when there is none yet).
    pub(super) fn refresh_lettering(&mut self, lettering: &mut Option<Arc<dyn LetteringService>>) {
        let styles = self.shown().into_owned();
        if lettering.is_some() && styles == self.lettering_styles {
            return;
        }
        *lettering = Some(Arc::new(Letterer::new(styles.clone())));
        self.lettering_styles = styles;
        self.lettering_version += 1;
    }

    fn position(&self, name: &str) -> Option<usize> {
        self.presets
            .caption_styles
            .iter()
            .position(|s| s.name == name)
    }
}

/// The first caption track, whose style new captions use.
fn first_caption_track(project: &Project) -> Option<&Track> {
    project
        .timeline
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Caption)
}

impl EditorState {
    /// Caption styles, loudness and ducking defaults of the selected style
    /// profile; edit through [`Self::edit_presets`] so they are saved.
    pub fn presets(&self) -> &Presets {
        &self.captions.presets
    }

    /// Counts reloads of the presets, so an edit begun before one can tell.
    pub fn presets_version(&self) -> u64 {
        self.captions.presets_version
    }

    /// Index into the presets' caption styles used for new captions.
    pub fn caption_style(&self) -> usize {
        self.captions.active
    }

    /// Counts changes to `services.lettering`, so the viewer knows to redraw.
    pub fn lettering_version(&self) -> u64 {
        self.captions.lettering_version
    }

    /// The selected style profile's presets, or the defaults without one.
    pub(super) fn load_presets(&mut self) {
        let mut presets = self
            .settings
            .style_profile
            .and_then(|id| persistence::with_store(|s| s.profile_header(id)).flatten())
            .map(|profile| profile.presets)
            .unwrap_or_default();
        if presets.caption_styles.is_empty() {
            presets.caption_styles = Presets::default().caption_styles;
        }
        self.clear_live();
        self.captions
            .load(presets, self.bus.project(), &mut self.services.lettering);
    }

    /// Change the presets and save them to the selected style profile,
    /// creating a "Default" profile if there is none yet.
    pub fn edit_presets(&mut self, f: impl FnOnce(&mut Presets), cx: &mut Context<Self>) {
        if !self.captions.edit(f, &mut self.services.lettering) {
            return;
        }
        let profile = self
            .settings
            .style_profile
            .filter(|id| {
                persistence::with_store(|s| s.profile_header(*id))
                    .flatten()
                    .is_some()
            })
            .or_else(|| persistence::with_store(|s| s.create_profile("Default")).map(|p| p.id));
        if let Some(id) = profile {
            persistence::with_store(|s| s.set_presets(id, &self.captions.presets));
            if self.settings.style_profile != Some(id) {
                let mut settings = self.settings.clone();
                settings.style_profile = Some(id);
                // The profile already holds these presets.
                persistence::save_settings(&settings);
                self.settings = settings;
                self.load_agent_memory();
            }
        }
        if let Some(notice) = persistence::take_notice() {
            self.status = Some(notice);
        }
        cx.notify();
    }

    /// Make preset `index` the active caption style and save its name on
    /// the first caption track, so the choice travels with the project.
    pub fn pick_caption_style(&mut self, index: usize, cx: &mut Context<Self>) {
        match self.captions.pick(index, self.bus.project()) {
            Some(command) => {
                self.execute(command, cx);
            }
            None => cx.notify(),
        }
    }

    /// Follow the caption track's saved style, e.g. after a project loads.
    pub fn sync_caption_style(&mut self) {
        self.captions.follow(self.bus.project());
    }

    /// Caption styles as the viewer shows them: with the one being dragged.
    pub fn caption_styles(&self) -> Cow<'_, [CaptionStyle]> {
        self.captions.shown()
    }

    /// Show caption style `index` as `style` without saving it.
    pub fn show_caption_style(
        &mut self,
        index: usize,
        style: CaptionStyle,
        cx: &mut Context<Self>,
    ) {
        self.captions
            .show_live(index, style, &mut self.services.lettering);
        cx.notify();
    }

    /// Stop showing an unsaved caption style.
    pub fn hide_caption_style(&mut self, cx: &mut Context<Self>) {
        if self.captions.hide_live(&mut self.services.lettering) {
            cx.notify();
        }
    }

    /// Change caption style `index` and save it.
    pub fn edit_caption_style(
        &mut self,
        index: usize,
        f: impl FnOnce(&mut CaptionStyle),
        cx: &mut Context<Self>,
    ) {
        self.captions.clear_live();
        self.edit_presets(
            |p| {
                if let Some(style) = p.caption_styles.get_mut(index) {
                    f(style);
                }
            },
            cx,
        );
        self.captions
            .refresh_lettering(&mut self.services.lettering);
        cx.notify();
    }

    /// The preset index caption `track` is styled with: its named preset,
    /// else the first.
    pub fn track_caption_style(&self, track: TrackId) -> usize {
        self.captions.of_track(self.bus.project(), track)
    }

    /// Style caption `track` with preset `index`. The first caption track's
    /// style is also the one new captions use.
    pub fn set_track_caption_style(
        &mut self,
        track: TrackId,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        if first_caption_track(self.bus.project()).map(|t| t.id) == Some(track) {
            self.pick_caption_style(index, cx);
        } else if let Some(style) = self.captions.presets.caption_styles.get(index) {
            let style = Some(style.name.clone());
            self.execute(Command::SetCaptionStyle { track, style }, cx);
        }
    }

    /// The caption style new captions use.
    pub fn active_caption_style(&self) -> CaptionStyle {
        self.captions
            .presets
            .caption_styles
            .get(self.captions.active)
            .cloned()
            .unwrap_or_default()
    }
}
