//! Generate: music or a sound effect from a description, made by the
//! provider set in Settings → Integrations, landing in the Music Generated
//! or Sound Effects Generated bin. Music runs as long as In/Out when they
//! are set.

use super::MediaPoolPanel;
use crate::state::EditorState;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::*;
use koo_core::cloud::{Capability, CloudError, GenerateRequest};
use koo_core::time::Time;

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

    /// Seconds: shortest, longest, and the step between.
    fn lengths(self) -> (f32, f32, f32) {
        match self {
            Kind::Music => (5., 300., 1.),
            Kind::SoundEffect => (0.5, 30., 0.5),
        }
    }
}

/// The form: what it should sound like, how long, and instrumental (music)
/// or looping (sound effects).
pub struct GenerateForm {
    state: Entity<EditorState>,
    kind: Kind,
    prompt: Entity<TextareaState>,
    length: Entity<SliderState>,
    /// Instrumental music, or a sound effect that loops.
    option: bool,
    _subscriptions: Vec<Subscription>,
}

impl GenerateForm {
    fn new(
        state: Entity<EditorState>,
        kind: Kind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (min, max, step) = kind.lengths();
        let in_out = state
            .read(cx)
            .in_out
            .map(|r| r.duration().as_secs_f64() as f32);
        let default = match kind {
            Kind::Music => in_out.unwrap_or(30.),
            Kind::SoundEffect => 5.,
        };
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 6)
                .placeholder(match kind {
                    Kind::Music => "Warm lo-fi beat with soft piano, slow and calm…",
                    Kind::SoundEffect => "Heavy wooden door creaking open in a stone hall…",
                })
        });
        let length = cx.new(|_| {
            SliderState::new()
                .min(min)
                .max(max)
                .step(step)
                .default_value(default.clamp(min, max))
        });
        let _subscriptions = vec![
            cx.subscribe(&prompt, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&length, |_, _, _: &SliderEvent, cx| cx.notify()),
        ];
        Self {
            state,
            kind,
            prompt,
            length,
            option: kind == Kind::Music,
            _subscriptions,
        }
    }

    fn request(&self, cx: &App) -> Option<GenerateRequest> {
        let prompt = self.prompt.read(cx).value().trim().to_owned();
        if prompt.is_empty() {
            return None;
        }
        let length = Some(Time::from_secs_f64(
            self.length.read(cx).value().start() as f64
        ));
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
                looping: self.option,
            },
        })
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(request) = self.request(cx) {
            self.state.update(cx, |s, cx| s.generate(request, cx));
            window.close_dialog(cx);
        }
    }
}

impl Render for GenerateForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let seconds = self.length.read(cx).value().start();
        let muted = cx.theme().muted_foreground;
        let option = match self.kind {
            Kind::Music => "Instrumental",
            Kind::SoundEffect => "Loop",
        };
        v_flex()
            .gap_4()
            .child(Textarea::new(&self.prompt).text_sm())
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex().justify_between().text_sm().child("Length").child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(length_label(seconds)),
                        ),
                    )
                    .child(Slider::new(&self.length)),
            )
            .child(
                h_flex().justify_between().text_sm().child(option).child(
                    Switch::new("generate-option")
                        .small()
                        .checked(self.option)
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.option = *on;
                            cx.notify();
                        })),
                ),
            )
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
        let form = cx.new(|cx| GenerateForm::new(self.state.clone(), kind, window, cx));
        window.open_dialog(cx, move |dialog, _, _| {
            let submit = form.clone();
            dialog
                .title(kind.label())
                .w(px(440.))
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("generate-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("generate-ok")
                                .primary()
                                .label("Generate")
                                .on_click(move |_, window, cx| {
                                    submit.update(cx, |f, cx| f.submit(window, cx))
                                }),
                        ),
                )
        });
    }

    /// A row for each generation running or failed, above the items.
    pub(super) fn generations(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let generating = self.state.read(cx).generating.clone();
        let theme = cx.theme().clone();
        generating
            .into_iter()
            .map(|g| {
                let id = g.id;
                let summary: SharedString = g.request.summary().into();
                let lead = match &g.failed {
                    None => Spinner::new().xsmall().into_any_element(),
                    Some(e) => {
                        let message: SharedString = failure(e).into();
                        div()
                            .id(("generation-failed", id as usize))
                            .child(
                                Icon::new(IconName::CircleAlert)
                                    .small()
                                    .text_color(theme.red),
                            )
                            .tooltip(move |window, cx| {
                                let message = message.clone();
                                Tooltip::element(move |_, _| {
                                    div().max_w(px(320.)).child(message.clone())
                                })
                                .build(window, cx)
                            })
                            .into_any_element()
                    }
                };
                h_flex()
                    .h(px(28.))
                    .px_2()
                    .gap_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(lead)
                    .child(div().flex_1().min_w_0().truncate().child(summary))
                    .child(
                        Button::new(("generation-dismiss", id as usize))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .disabled(g.failed.is_none())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |s, cx| s.dismiss_generation(id, cx))
                            })),
                    )
                    .into_any_element()
            })
            .collect()
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
