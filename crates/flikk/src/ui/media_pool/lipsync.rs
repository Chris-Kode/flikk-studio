//! Lip sync: a video or picture in the pool made to say one of the pool's
//! sounds by the provider set for it in Settings → Integrations, landing
//! in the Video Lip-synced bin. Voiceovers are offered first; the form says
//! why the picked model can't take a face or a voice. With "Replace clips"
//! on, a video's clips on picture tracks show the lip-synced one once it
//! lands.

use super::MediaPoolPanel;
use super::form;
use super::video::{Choice, Picker, picked, picker};
use crate::state::EditorState;
use crate::ui::format::{clock_rounded, item_name};
use crate::ui::kit::theme as t;
use crate::ui::kit::{DialogExt as _, Select, SelectEvent, Switch, h_flex, v_flex};
use flikk_core::MediaId;
use flikk_core::cloud::{
    Capability, FaceKind, GenerateRequest, LipSyncFace, LipSyncModel, LipSyncVoice,
};
use flikk_core::model::{MediaItem, MediaKind};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub struct LipSyncForm {
    state: Entity<EditorState>,
    media: MediaId,
    face: LipSyncFace,
    model: LipSyncModel,
    /// The pool's sounds, voiceovers first; `None` when it has none.
    voice: Option<Picker>,
    /// The face is a video a clip shows on a picture track.
    used: bool,
    /// Those clips show the lip-synced video once it lands.
    replace: bool,
    _subscriptions: Vec<Subscription>,
}

impl LipSyncForm {
    fn new(
        state: Entity<EditorState>,
        model: LipSyncModel,
        media: MediaId,
        face: LipSyncFace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project = state.read(cx).bus.project();
        let choices: Vec<Choice> = voices_first(&project.media)
            .map(|m| Choice::new(m.id.0.to_string(), item_name(m)))
            .collect();
        let used = face.kind == FaceKind::Video
            && project.timeline.picture_clips_of(media).next().is_some();
        let voice = (!choices.is_empty()).then(|| picker(choices, 0, window, cx));
        let _subscriptions = voice
            .iter()
            .map(|v| cx.subscribe(v, |_, _, _: &SelectEvent<Choice>, cx| cx.notify()))
            .collect();
        Self {
            state,
            media,
            face,
            model,
            voice,
            used,
            replace: used,
            _subscriptions,
        }
    }

    /// The sound picked, as a voice.
    fn voice(&self, cx: &App) -> Option<LipSyncVoice> {
        let key = picked(self.voice.as_ref()?, cx);
        let project = self.state.read(cx).bus.project();
        let item = project.media.iter().find(|m| m.id.0.to_string() == key)?;
        LipSyncVoice::of(item)
    }

    fn request(&self, cx: &App) -> Option<GenerateRequest> {
        Some(GenerateRequest::LipSync {
            face: self.face.clone(),
            voice: self.voice(cx)?,
        })
    }

    /// Why the model can't make it, when it can't.
    fn problem(&self, cx: &App) -> Option<String> {
        let Some(request) = self.request(cx) else {
            return Some("The pool has no sound; import one or make a voiceover".into());
        };
        self.model.check(&request).err().map(|e| e.to_string())
    }

    fn ready(&self, cx: &App) -> bool {
        self.problem(cx).is_none()
    }

    /// Start it; `true` once it is on its way.
    fn submit(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.ready(cx) {
            return false;
        }
        let Some(request) = self.request(cx) else {
            return false;
        };
        let (media, replace) = (self.media, self.used && self.replace);
        self.state
            .update(cx, |s, cx| s.make_version(media, request, replace, cx));
        true
    }

    /// "1920 × 1080 · 0:12" for a video, the size alone for a picture.
    fn face_meta(&self) -> String {
        let size = (self.face.width > 0 && self.face.height > 0)
            .then(|| format!("{} × {}", self.face.width, self.face.height));
        let length = self.face.length.map(clock_rounded);
        [size, length]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// The pool's sounds: its voiceovers, newest first, then the rest in pool
/// order.
fn voices_first(media: &[MediaItem]) -> impl Iterator<Item = &MediaItem> {
    let sounds = move || media.iter().filter(|m| m.kind == MediaKind::Audio);
    let voiceover = |m: &&MediaItem| (m.generated.as_deref()).is_some_and(|g| g.script().is_some());
    let voiceovers = sounds().filter(voiceover).rev();
    voiceovers.chain(sounds().filter(move |m| !voiceover(m)))
}

impl Render for LipSyncForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let face_label = match self.face.kind {
            FaceKind::Video => "Video",
            FaceKind::Picture => "Picture",
        };
        let voice_length = self
            .voice(cx)
            .and_then(|v| v.length)
            .map(clock_rounded)
            .unwrap_or_default();
        v_flex()
            .gap(px(10.))
            .child(form::row(
                face_label,
                h_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(self.face.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(t::mono())
                            .text_color(t::T2)
                            .child(self.face_meta()),
                    ),
            ))
            .when_some(self.voice.as_ref(), |el, voice| {
                el.child(form::row(
                    "Voice",
                    h_flex()
                        .gap(px(8.))
                        .child(div().flex_1().min_w_0().child(Select::new(voice)))
                        .child(
                            h_flex()
                                .w(px(40.))
                                .flex_none()
                                .justify_end()
                                .font_family(t::mono())
                                .text_color(t::T2)
                                .child(voice_length),
                        ),
                ))
            })
            .when(self.used, |el| {
                el.child(form::row(
                    "Timeline",
                    Switch::new("lipsync-replace")
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
    /// What the model picked for lip sync takes, when a provider is ready
    /// for it.
    pub(super) fn lipsync_model(&self, cx: &App) -> Option<LipSyncModel> {
        let generation = self.state.read(cx).services.generation.clone()?;
        generation.provider_for(Capability::LipSync)?;
        generation.lipsync_model()
    }

    /// The form for pool video or picture `media`.
    pub(super) fn open_lipsync(
        &mut self,
        model: LipSyncModel,
        media: MediaId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let item = self.state.read(cx).bus.project().media(media);
        let Some(face) = item.and_then(LipSyncFace::of) else {
            return;
        };
        let state = self.state.clone();
        let form = cx.new(|cx| LipSyncForm::new(state, model, media, face, window, cx));
        window.open_dialog(cx, move |dialog, _, cx| {
            let submit = form.clone();
            let ready = form.read(cx).ready(cx);
            dialog
                .title(Capability::LipSync.label())
                .w(px(440.))
                .child(form.clone())
                .confirm("Lip-sync", move |_, cx| {
                    submit.update(cx, |f, cx| f.submit(cx))
                })
                .confirm_disabled(!ready)
        });
    }
}
