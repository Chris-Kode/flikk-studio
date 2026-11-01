//! Voiceover: a script read by one of the provider's voices, landing in the
//! Voiceover Generated bin. Delivery is written into the script: bracketed
//! tags and pauses from the menus go in at the cursor, ellipses and capitals
//! by hand. Regenerating a voiceover opens this form with its script.

use super::{MediaPoolPanel, listen_button};
use crate::state::EditorState;
use crate::ui::timeline::glyph::{Glyph, glyph};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::searchable_list::SearchableListItem;
use gpui_kit::component::select::{SearchableVec, Select, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::cloud::{
    self, Capability, CloudResult, GenerateRequest, Reading, SCRIPT_CHARS, Voice,
};
use koo_core::model::MediaItem;
use std::collections::HashMap;

const EMOTIONS: [&str; 11] = [
    "[happy]",
    "[excited]",
    "[sad]",
    "[angry]",
    "[annoyed]",
    "[surprised]",
    "[curious]",
    "[thoughtful]",
    "[sarcastic]",
    "[crying]",
    "[whispers]",
];
const REACTIONS: [&str; 8] = [
    "[laughs]",
    "[chuckles]",
    "[sighs]",
    "[exhales sharply]",
    "[inhales deeply]",
    "[clears throat]",
    "[gulps]",
    "[woo]",
];
const PAUSES: [&str; 3] = ["...", "[short pause]", "[long pause]"];

/// A voice in the picker; typing searches its name and traits.
#[derive(Clone)]
struct VoiceRow {
    id: String,
    title: SharedString,
}

impl VoiceRow {
    fn new(voice: &Voice) -> Self {
        let title = match voice.traits.as_slice() {
            [] => voice.name.clone(),
            traits => format!("{} · {}", voice.name, traits.join(", ")),
        };
        Self {
            id: voice.id.clone(),
            title: title.into(),
        }
    }
}

impl SearchableListItem for VoiceRow {
    type Value = String;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &String {
        &self.id
    }
}

/// A take to make again: its script and who read it.
#[derive(Clone)]
pub struct Take {
    pub script: String,
    pub reading: Reading,
}

pub struct VoiceoverForm {
    state: Entity<EditorState>,
    voices: Vec<Voice>,
    picker: Entity<SelectState<SearchableVec<VoiceRow>>>,
    /// The voice to pick once they are in.
    wanted: Option<String>,
    loading: bool,
    /// Samples heard so far, by voice id.
    samples: HashMap<String, MediaItem>,
    /// Voice whose sample is on its way.
    fetching: Option<String>,
    error: Option<SharedString>,
    script: Entity<TextareaState>,
    stability: Entity<SliderState>,
    similarity: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl VoiceoverForm {
    fn new(
        state: Entity<EditorState>,
        take: Option<Take>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (script_text, reading) =
            take.map_or((String::new(), None), |t| (t.script, Some(t.reading)));
        let wanted = reading
            .as_ref()
            .map(|r| r.voice.clone())
            .or_else(|| state.read(cx).last_voice());
        let script = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(6, 12)
                .placeholder("[excited] Welcome back... today is the BIG one.")
                .default_value(script_text)
        });
        let picker = cx.new(|cx| {
            SelectState::new(SearchableVec::new(Vec::new()), None, window, cx).searchable(true)
        });
        let slider = |value: f32, cx: &mut Context<Self>| {
            cx.new(|_| {
                SliderState::new()
                    .min(0.)
                    .max(1.)
                    .step(0.05)
                    .default_value(value)
            })
        };
        let stability = slider(
            reading.as_ref().map_or(Reading::STABILITY, |r| r.stability),
            cx,
        );
        let similarity = slider(
            reading
                .as_ref()
                .map_or(Reading::SIMILARITY, |r| r.similarity),
            cx,
        );
        let _subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.observe(&picker, |_, _, cx| cx.notify()),
            cx.subscribe(&script, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&stability, |_, _, _: &SliderEvent, cx| cx.notify()),
            cx.subscribe(&similarity, |_, _, _: &SliderEvent, cx| cx.notify()),
        ];
        let generation = state.read(cx).services.generation.clone();
        let load = cx.background_executor().spawn(async move {
            match generation {
                Some(generation) => generation.voices(),
                None => Ok(Vec::new()),
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = load.await;
            let _ = this.update_in(cx, |this, window, cx| this.loaded(result, window, cx));
        })
        .detach();
        Self {
            state,
            voices: Vec::new(),
            picker,
            wanted,
            loading: true,
            samples: HashMap::new(),
            fetching: None,
            error: None,
            script,
            stability,
            similarity,
            _subscriptions,
        }
    }

    fn loaded(
        &mut self,
        result: CloudResult<Vec<Voice>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.loading = false;
        match result {
            Ok(voices) => {
                let rows: Vec<VoiceRow> = voices.iter().map(VoiceRow::new).collect();
                let wanted = voices
                    .iter()
                    .position(|v| Some(&v.id) == self.wanted.as_ref());
                let selected = wanted.or((!voices.is_empty()).then_some(0));
                self.picker.update(cx, |picker, cx| {
                    picker.set_items(SearchableVec::new(rows), window, cx);
                    picker.set_selected_index(selected.map(IndexPath::new), window, cx);
                });
                self.voices = voices;
            }
            Err(e) => self.error = Some(e.to_string().into()),
        }
        cx.notify();
    }

    fn voice(&self, cx: &App) -> Option<&Voice> {
        let id = self.picker.read(cx).selected_value()?;
        self.voices.iter().find(|v| v.id == *id)
    }

    fn text(&self, cx: &App) -> String {
        self.script.read(cx).value().trim().to_owned()
    }

    fn slider_value(slider: &Entity<SliderState>, cx: &App) -> f32 {
        slider.read(cx).value().start()
    }

    fn request(&self, cx: &App) -> Option<GenerateRequest> {
        let voice = self.voice(cx)?;
        let text = self.text(cx);
        if text.is_empty() || cloud::check_script(&text).is_err() {
            return None;
        }
        Some(GenerateRequest::Voiceover {
            text,
            reading: Reading {
                voice: voice.id.clone(),
                voice_name: voice.name.clone(),
                stability: Self::slider_value(&self.stability, cx),
                similarity: Self::slider_value(&self.similarity, cx),
            },
        })
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(request) = self.request(cx) else {
            return;
        };
        self.state.update(cx, |s, cx| {
            if let Some(reading) = request.reading() {
                s.remember_voice(&reading.voice);
            }
            s.generate(request, cx);
        });
        window.close_dialog(cx);
    }

    /// Fetch the sample of `voice`, then play it.
    fn fetch_sample(&mut self, voice: Voice, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let (generation, media) = (
            state.services.generation.clone(),
            state.services.media.clone(),
        );
        let Some(generation) = generation else {
            return;
        };
        let id = voice.id.clone();
        self.fetching = Some(id.clone());
        self.error = None;
        let fetch = cx.background_executor().spawn(async move {
            let path = generation.voice_sample(&voice)?;
            Ok::<_, cloud::CloudError>(cloud::sample_item(path, media.as_deref()))
        });
        cx.spawn(async move |this, cx| {
            let result = fetch.await;
            let _ = this.update(cx, |this, cx| {
                this.fetching = None;
                match result {
                    Ok(item) => {
                        this.state.update(cx, |s, cx| s.toggle_source(&item, cx));
                        this.samples.insert(id, item);
                    }
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Put `tag` in at the cursor, set off from the word before it.
    fn insert_tag(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.script.update(cx, |script, cx| {
            let value = script.value();
            let before = value.get(..script.cursor()).unwrap_or(&value);
            let word = before.chars().last().is_some_and(|c| !c.is_whitespace());
            let lead = if word && tag.starts_with('[') {
                " "
            } else {
                ""
            };
            script.insert(format!("{lead}{tag} "), window, cx);
            script.focus(window, cx);
        });
        cx.notify();
    }

    fn sample_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let voice = self.voice(cx).cloned();
        let id = voice.as_ref().map(|v| &v.id);
        if id.is_some() && id == self.fetching.as_ref() {
            return div()
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new().xsmall())
                .into_any_element();
        }
        if let Some(item) = id.and_then(|id| self.samples.get(id)) {
            return listen_button("voice-sample", &self.state, item, cx).into_any_element();
        }
        Button::new("voice-sample")
            .ghost()
            .xsmall()
            .tooltip("Play")
            .disabled(!voice.as_ref().is_some_and(|v| v.sample.is_some()))
            .child(glyph(Glyph::Play, px(13.), cx.theme().foreground))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(voice) = voice.clone() {
                    this.fetch_sample(voice, cx);
                }
            }))
            .into_any_element()
    }

    /// A menu of what can go in the script.
    fn tag_menu(
        &self,
        name: &'static str,
        tags: &'static [&'static str],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let form = cx.entity();
        Button::new(name)
            .ghost()
            .xsmall()
            .label(name)
            .dropdown_menu(move |mut menu, _, _| {
                for &tag in tags {
                    let form = form.clone();
                    menu = menu.item(PopupMenuItem::new(tag).on_click(move |_, window, cx| {
                        form.update(cx, |this, cx| this.insert_tag(tag, window, cx))
                    }));
                }
                menu
            })
    }

    fn slider_row(name: &'static str, slider: &Entity<SliderState>, cx: &App) -> impl IntoElement {
        let percent = format!("{:.0}%", Self::slider_value(slider, cx) * 100.);
        v_flex()
            .gap_1()
            .child(
                h_flex().justify_between().text_sm().child(name).child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(percent),
                ),
            )
            .child(Slider::new(slider))
    }
}

impl Render for VoiceoverForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let text = self.text(cx);
        let chars = text.chars().count();
        let fixed = cloud::without_breaks(&text);
        let count = format!("{chars} / {SCRIPT_CHARS}");
        let sample = self.sample_button(cx);
        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div().flex_1().min_w_0().child(
                            Select::new(&self.picker)
                                .placeholder(if self.loading {
                                    "Loading voices…"
                                } else {
                                    "Voice"
                                })
                                .disabled(self.voices.is_empty()),
                        ),
                    )
                    .child(sample),
            )
            .when_some(self.error.clone(), |el, error| {
                el.child(div().text_xs().text_color(theme.red).child(error))
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(Textarea::new(&self.script).text_sm())
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(self.tag_menu("Emotion", &EMOTIONS, cx))
                                    .child(self.tag_menu("Reaction", &REACTIONS, cx))
                                    .child(self.tag_menu("Pause", &PAUSES, cx)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if chars > SCRIPT_CHARS {
                                        theme.red
                                    } else {
                                        theme.muted_foreground
                                    })
                                    .child(count),
                            ),
                    ),
            )
            .when_some(fixed, |el, fixed| {
                let script = self.script.clone();
                el.child(
                    h_flex()
                        .justify_between()
                        .text_xs()
                        .text_color(theme.red)
                        .child("<break> tags aren't read")
                        .child(
                            Button::new("use-pauses")
                                .xsmall()
                                .label("Use pauses")
                                .on_click(move |_, window, cx| {
                                    let fixed = fixed.clone();
                                    script.update(cx, |s, cx| s.set_value(fixed, window, cx));
                                }),
                        ),
                )
            })
            .child(Self::slider_row("Stability", &self.stability, cx))
            .child(Self::slider_row("Similarity", &self.similarity, cx))
    }
}

impl MediaPoolPanel {
    /// Whether a provider is ready to make voiceovers.
    pub(super) fn can_voiceover(&self, cx: &App) -> bool {
        let generation = self.state.read(cx).services.generation.as_ref();
        generation.is_some_and(|g| g.provider_for(Capability::Voiceover).is_some())
    }

    /// The form, with `take`'s script and voice when it makes one again.
    pub(super) fn open_voiceover(
        &mut self,
        take: Option<Take>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let form = cx.new(|cx| VoiceoverForm::new(self.state.clone(), take, window, cx));
        window.open_dialog(cx, move |dialog, _, cx| {
            let submit = form.clone();
            let ready = form.read(cx).request(cx).is_some();
            dialog
                .title(Capability::Voiceover.label())
                .w(px(520.))
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("voiceover-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("voiceover-ok")
                                .primary()
                                .label("Generate")
                                .disabled(!ready)
                                .on_click(move |_, window, cx| {
                                    submit.update(cx, |f, cx| f.submit(window, cx))
                                }),
                        ),
                )
        });
    }
}
