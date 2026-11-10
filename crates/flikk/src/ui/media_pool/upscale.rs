//! Upscale: a video in the pool made larger by the provider set for it in
//! Settings → Integrations, landing in the Video Upscaled bin with the
//! source's sound. The resolutions offered are those the picked model can
//! make this video at; with "Replace clips" on, the video's clips on
//! picture tracks show the larger one once it lands.

use super::MediaPoolPanel;
use super::form;
use super::video::{Choice, Picker, picked, picker};
use crate::state::EditorState;
use crate::ui::kit::theme as t;
use crate::ui::kit::{DialogExt as _, Select, SelectEvent, Switch, h_flex, v_flex};
use flikk_core::MediaId;
use flikk_core::cloud::{Capability, GenerateRequest, UpscaleModel, UpscaleSource};
use flikk_core::model::{ClipContent, TrackKind};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub struct UpscaleForm {
    state: Entity<EditorState>,
    media: MediaId,
    source: UpscaleSource,
    model: UpscaleModel,
    /// The resolutions the model can make this video at; none when it
    /// can't make it larger.
    resolution: Option<Picker>,
    /// A clip shows the video on a picture track.
    used: bool,
    /// Those clips show the upscaled video once it lands.
    replace: bool,
    _subscriptions: Vec<Subscription>,
}

impl UpscaleForm {
    fn new(
        state: Entity<EditorState>,
        model: UpscaleModel,
        media: MediaId,
        source: UpscaleSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let targets = model.targets_for(&source);
        let start = model
            .default_target(&source)
            .and_then(|d| targets.iter().position(|t| *t == d))
            .unwrap_or(0);
        let resolution = (!targets.is_empty()).then(|| {
            let choices = targets
                .iter()
                .map(|t| Choice::new(*t, t.replace('k', "K")))
                .collect();
            picker(choices, start, window, cx)
        });
        let _subscriptions = resolution
            .iter()
            .map(|r| cx.subscribe(r, |_, _, _: &SelectEvent<Choice>, cx| cx.notify()))
            .collect();
        let used = state
            .read(cx)
            .bus
            .project()
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind != TrackKind::Audio)
            .flat_map(|t| &t.clips)
            .any(|c| matches!(&c.content, ClipContent::Media { media: m, .. } if *m == media));
        Self {
            state,
            media,
            source,
            model,
            resolution,
            used,
            replace: used,
            _subscriptions,
        }
    }

    /// The request: the resolution picked, or the model's default when it
    /// offers none for this video (so the problem says why).
    fn request(&self, cx: &App) -> GenerateRequest {
        let resolution = self
            .resolution
            .as_ref()
            .map(|r| picked(r, cx))
            .filter(|r| !r.is_empty());
        GenerateRequest::Upscale {
            source: self.source.clone(),
            resolution,
        }
    }

    /// Why the model can't make it larger, when it can't.
    fn problem(&self, cx: &App) -> Option<String> {
        self.model
            .check(&self.request(cx))
            .err()
            .map(|e| e.to_string())
    }

    fn ready(&self, cx: &App) -> bool {
        self.resolution.is_some() && self.problem(cx).is_none()
    }

    /// Start it; `true` once it is on its way.
    fn submit(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.ready(cx) {
            return false;
        }
        let request = self.request(cx);
        let (media, replace) = (self.media, self.used && self.replace);
        self.state
            .update(cx, |s, cx| s.upscale(media, request, replace, cx));
        true
    }
}

impl Render for UpscaleForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let size = format!("{} × {}", self.source.width, self.source.height);
        v_flex()
            .gap(px(10.))
            .child(form::row(
                "Video",
                h_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(self.source.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(t::mono())
                            .text_color(t::T2)
                            .child(size),
                    ),
            ))
            .when_some(self.resolution.as_ref(), |el, r| {
                el.child(form::row("Resolution", Select::new(r)))
            })
            .when(self.used, |el| {
                el.child(form::row(
                    "Timeline",
                    Switch::new("upscale-replace")
                        .checked(self.replace)
                        .label("Replace clips")
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.replace = *on;
                            cx.notify();
                        })),
                ))
            })
            .when_some(self.problem(cx), |el, problem| {
                el.child(form::problem(problem))
            })
    }
}

impl MediaPoolPanel {
    /// What the model picked for upscaling can do, when a provider is
    /// ready for it.
    pub(super) fn upscale_model(&self, cx: &App) -> Option<UpscaleModel> {
        let generation = self.state.read(cx).services.generation.clone()?;
        generation.provider_for(Capability::Upscale)?;
        generation.upscale_model()
    }

    /// The form for pool video `media`, when its size is known.
    pub(super) fn open_upscale(
        &mut self,
        model: UpscaleModel,
        media: MediaId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let item = self.state.read(cx).bus.project().media(media);
        let Some(source) = item.and_then(UpscaleSource::of) else {
            return;
        };
        let state = self.state.clone();
        let form = cx.new(|cx| UpscaleForm::new(state, model, media, source, window, cx));
        window.open_dialog(cx, move |dialog, _, cx| {
            let submit = form.clone();
            let ready = form.read(cx).ready(cx);
            dialog
                .title(Capability::Upscale.label())
                .w(px(440.))
                .child(form.clone())
                .confirm("Upscale", move |_, cx| {
                    submit.update(cx, |f, cx| f.submit(cx))
                })
                .confirm_disabled(!ready)
        });
    }
}
