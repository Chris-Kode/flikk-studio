//! Generate: music or a sound effect from a description, each in a dialog
//! of its own, made by the provider set in Settings → Integrations and
//! landing in the Music Generated or Sound Effects Generated bin. The form
//! offers what the model picked can make: its lengths, and looping where it
//! loops. Music runs as long as In/Out when they are set. Each generation
//! running (any kind) shows in the pool until it lands.

use super::{MediaPoolPanel, form, thumb_badge};
use crate::state::{EditorState, Generating};
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, DialogExt as _, Icon, IconName, Progress, Spinner, Switch, Textarea, Tooltip, h_flex,
    v_flex,
};
use flikk_core::cloud::{AudioLengths, AudioModel, Capability, CloudError, GenerateRequest, Stage};
use flikk_core::time::Time;
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::base::slider::{SliderEvent, SliderState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Music,
    SoundEffect,
}

impl Kind {
    pub fn capability(self) -> Capability {
        match self {
            Kind::Music => Capability::Music,
            Kind::SoundEffect => Capability::SoundEffects,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Music => "Music",
            Kind::SoundEffect => "Sound effect",
        }
    }

    pub fn icon(self) -> IconName {
        match self {
            Kind::Music => IconName::Music,
            Kind::SoundEffect => IconName::AudioWaveform,
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Kind::Music => "Warm lo-fi beat with soft piano, slow and calm…",
            Kind::SoundEffect => "Heavy wooden door creaking open in a stone hall…",
        }
    }

    /// What to offer when the provider says nothing of its model: the
    /// lengths most models make.
    fn any_model(self) -> AudioModel {
        let (shortest, longest, step) = match self {
            Kind::Music => (5., 300., 1.),
            Kind::SoundEffect => (0.5, 30., 0.5),
        };
        AudioModel {
            name: self.label(),
            lengths: Some(AudioLengths {
                shortest,
                longest,
                step,
            }),
            looping: self == Kind::SoundEffect,
        }
    }

    /// The length it starts at, before the model's range fits it.
    fn default_length(self) -> f64 {
        match self {
            Kind::Music => 30.,
            Kind::SoundEffect => 5.,
        }
    }
}

/// How long it runs, on a slider over the model's lengths.
struct LengthSlider {
    slider: Entity<SliderState>,
    _changed: Subscription,
}

/// The form: what it should sound like, how long, and instrumental (music)
/// or looping (sound effects). Each kind has a dialog of its own.
pub struct GenerateForm {
    state: Entity<EditorState>,
    kind: Kind,
    prompt: Entity<TextareaState>,
    /// `None` when the model picks the length itself.
    length: Option<LengthSlider>,
    /// Whether the model loops sound effects; music is always offered
    /// without vocals.
    can_loop: bool,
    /// Instrumental music, or a sound effect that loops.
    option: bool,
    _prompt: Subscription,
}

impl GenerateForm {
    fn new(
        state: Entity<EditorState>,
        kind: Kind,
        model: AudioModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 6)
                .placeholder(kind.placeholder())
        });
        let length = model.lengths.map(|lengths| {
            let slider = Self::slider(&state, kind, lengths, cx);
            LengthSlider {
                _changed: cx.subscribe(&slider, |_, _, _: &SliderEvent, cx| cx.notify()),
                slider,
            }
        });
        Self {
            _prompt: cx.subscribe(&prompt, |_, _, _: &InputEvent, cx| cx.notify()),
            state,
            kind,
            prompt,
            length,
            can_loop: model.looping,
            option: kind == Kind::Music,
        }
    }

    /// The length slider for `kind` over `lengths`: music starts as long
    /// as In/Out.
    fn slider(
        state: &Entity<EditorState>,
        kind: Kind,
        lengths: AudioLengths,
        cx: &mut App,
    ) -> Entity<SliderState> {
        let in_out = state.read(cx).in_out.map(|r| r.duration().as_secs_f64());
        let default = match kind {
            Kind::Music => in_out.unwrap_or(kind.default_length()),
            Kind::SoundEffect => kind.default_length(),
        };
        cx.new(|_| {
            SliderState::new()
                .min(lengths.shortest as f32)
                .max(lengths.longest as f32)
                .step(lengths.step as f32)
                .default_value(lengths.fit(default) as f32)
        })
    }

    fn request(&self, cx: &App) -> Option<GenerateRequest> {
        let prompt = self.prompt.read(cx).value().trim().to_owned();
        if prompt.is_empty() {
            return None;
        }
        let length = self
            .length
            .as_ref()
            .map(|l| Time::from_secs_f64(l.slider.read(cx).value().start() as f64));
        Some(match self.kind {
            Kind::Music => GenerateRequest::Music {
                prompt,
                length,
                instrumental: self.option,
                sections: Vec::new(),
            },
            Kind::SoundEffect => GenerateRequest::SoundEffect {
                prompt,
                length,
                looping: self.can_loop && self.option,
            },
        })
    }

    /// Start it; `true` once it is on its way.
    fn submit(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(request) = self.request(cx) else {
            return false;
        };
        self.state.update(cx, |s, cx| s.generate(request, cx));
        true
    }
}

impl Render for GenerateForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let option = match self.kind {
            Kind::Music => Some("Instrumental"),
            Kind::SoundEffect => self.can_loop.then_some("Loop"),
        };
        v_flex()
            .gap(px(10.))
            .child(Textarea::new(&self.prompt))
            .when_some(self.length.as_ref(), |el, length| {
                let seconds = length.slider.read(cx).value().start();
                el.child(form::row(
                    "Length",
                    form::slider(&length.slider, length_label(seconds)),
                ))
            })
            .when_some(option, |el, option| {
                el.child(form::row(
                    option,
                    Switch::new("generate-option")
                        .checked(self.option)
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.option = *on;
                            cx.notify();
                        })),
                ))
            })
    }
}

impl MediaPoolPanel {
    /// The capabilities a provider is ready for, in the order the menu
    /// lists them.
    pub(super) fn can_generate(&self, cx: &App) -> Vec<Kind> {
        let state = self.state.read(cx);
        let Some(generation) = &state.services.generation else {
            return Vec::new();
        };
        [Kind::Music, Kind::SoundEffect]
            .into_iter()
            .filter(|k| generation.provider_for(k.capability()).is_some())
            .collect()
    }

    pub(super) fn open_generate(
        &mut self,
        kind: Kind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let model = self
            .state
            .read(cx)
            .services
            .generation
            .as_ref()
            .and_then(|g| g.audio_model(kind.capability()))
            .unwrap_or_else(|| kind.any_model());
        let form = cx.new(|cx| GenerateForm::new(self.state.clone(), kind, model, window, cx));
        window.open_dialog(cx, move |dialog, _, cx| {
            let submit = form.clone();
            let ready = form.read(cx).request(cx).is_some();
            dialog
                .title(kind.label())
                .w(px(440.))
                .child(form.clone())
                .confirm("Generate", move |_, cx| {
                    submit.update(cx, |f, cx| f.submit(cx))
                })
                .confirm_disabled(!ready)
        });
    }
}

impl MediaPoolPanel {
    /// A generation on its way, as a tile among the items.
    pub(super) fn generation_tile(&self, g: &Generating, cx: &mut Context<Self>) -> AnyElement {
        let running = g.failed.is_none();
        let id = g.id as usize;
        let group = SharedString::from(format!("generation-{id}"));
        let center = div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .when(running, |d| d.child(progress(g).w(relative(0.6))))
            .when(!running, |d| {
                d.child(Icon::new(IconName::CircleAlert).size(px(16.)).color(t::ERR))
            });
        let thumb = div()
            .relative()
            .w_full()
            .aspect_ratio(16. / 9.)
            .rounded(t::RADIUS_SM)
            .overflow_hidden()
            .bg(t::R1)
            .child(center)
            .child(
                thumb_badge()
                    .text_color(t::T2)
                    .child(Icon::new(badge_icon(g.request.capability())).size(px(9.)))
                    .child(g.request.capability().label()),
            )
            .child(
                div()
                    .absolute()
                    .top(px(3.))
                    .right(px(3.))
                    .opacity(0.)
                    .group_hover(group.clone(), |s| s.opacity(1.))
                    .child(self.generation_action(g, cx)),
            );
        div()
            .id(ElementId::Name(group.clone()))
            .group(group)
            .flex()
            .flex_col()
            .min_w_0()
            .text_size(px(11.))
            .child(thumb)
            .child(
                div()
                    .mt(px(4.))
                    .truncate()
                    .text_color(t::T2)
                    .child(g.request.summary()),
            )
            .child(self.generation_meta(g))
            .into_any_element()
    }

    /// A generation on its way, as a row of the list.
    pub(super) fn generation_row(&self, g: &Generating, cx: &mut Context<Self>) -> AnyElement {
        let running = g.failed.is_none();
        h_flex()
            .h(px(30.))
            .gap(px(8.))
            .px(px(8.))
            .text_size(px(12.))
            .child(
                h_flex()
                    .w(px(36.))
                    .flex_none()
                    .justify_center()
                    .child(if running {
                        Spinner::new().size(px(12.)).into_any_element()
                    } else {
                        Icon::new(IconName::CircleAlert)
                            .size(px(13.))
                            .color(t::ERR)
                            .into_any_element()
                    }),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(8.))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(t::T2)
                            .child(g.request.summary()),
                    )
                    .when(running, |d| d.child(progress(g).w(px(60.)).flex_none())),
            )
            .child(self.generation_meta(g).flex_none())
            .child(self.generation_action(g, cx))
            .into_any_element()
    }

    /// Where a generation is, or why it failed (in full on hover).
    fn generation_meta(&self, g: &Generating) -> Stateful<Div> {
        let line = div()
            .id(("generation-meta", g.id as usize))
            .text_size(px(10.))
            .text_color(t::T3)
            .truncate();
        match &g.failed {
            Some(e) => {
                let message: SharedString = failure(e).into();
                line.text_color(t::ERR)
                    .child("Failed")
                    .tooltip(Tooltip::text(message))
            }
            None => line.child(stage_label(g.task.stage())),
        }
    }

    /// Stop a running generation a provider follows, or dismiss a failed
    /// one; one request can't be stopped.
    fn generation_action(&self, g: &Generating, cx: &mut Context<Self>) -> Button {
        let id = g.id;
        let running = g.failed.is_none();
        // A stopped one keeps its Stop button, held down, until it ends.
        if running && (g.task.can_stop() || g.stopping()) {
            Button::new(("generation-stop", id as usize))
                .ghost()
                .xsmall()
                .icon(IconName::Square)
                .tooltip("Stop")
                .disabled(g.task.cancelled())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.stop_generation(id, cx))
                }))
        } else {
            Button::new(("generation-dismiss", id as usize))
                .ghost()
                .xsmall()
                .icon(IconName::X)
                .tooltip("Dismiss")
                .disabled(running)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.dismiss_generation(id, cx))
                }))
        }
    }
}

/// How far a generation has come down, or a sweep until it says.
fn progress(g: &Generating) -> Progress {
    match g.task.stage() {
        Stage::Downloading(f) => Progress::new().value(f),
        _ => Progress::new().indeterminate(),
    }
}

/// Where a running generation is: its place in the queue, how much has
/// come down, or that its provider can't be reached just now.
fn stage_label(stage: Stage) -> String {
    match stage {
        Stage::Queued(Some(ahead)) if ahead > 0 => format!("Queued · {ahead} ahead"),
        Stage::Queued(_) => "Queued".into(),
        Stage::Downloading(f) => format!("Downloading · {:.0} %", f * 100.),
        Stage::Starting | Stage::Running => "Generating".into(),
        Stage::Reconnecting => "Reconnecting".into(),
    }
}

/// "5.5 s", "45 s", "2:30".
fn length_label(seconds: f32) -> String {
    if seconds >= 60. {
        let s = seconds.round() as u32;
        format!("{}:{:02}", s / 60, s % 60)
    } else if seconds.fract() == 0. {
        format!("{seconds:.0} s")
    } else {
        format!("{seconds:.1} s")
    }
}

/// Why it failed, with the provider's rewrite of a refused prompt.
fn failure(e: &CloudError) -> String {
    match e {
        CloudError::BadPrompt {
            message,
            suggestion: Some(s),
        } => format!("{message}\n\n{s}"),
        e => e.to_string(),
    }
}

/// The icon a running generation's tile wears for what it makes.
fn badge_icon(capability: Capability) -> IconName {
    match capability {
        Capability::Image => IconName::Image,
        Capability::Video | Capability::Upscale => IconName::Video,
        _ => IconName::AudioLines,
    }
}
