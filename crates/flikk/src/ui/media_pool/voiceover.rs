//! Voiceover: a script read by one of the provider's voices, landing in the
//! Voiceover Generated bin. Delivery is written into the script: bracketed
//! tags and pauses from the menus go in at the cursor, ellipses and capitals
//! by hand. Regenerating a voiceover opens this form with its script. The
//! user's own voices come first; "More voices" opens the Voices dialog to
//! find, design or clone another, which this form then reads with. A voice
//! in the library starts on the stability and similarity it last read with.

use super::{MediaPoolPanel, form, listen_button, play_button};
use crate::state::EditorState;
use crate::state::voices::Listening;
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, DialogExt as _, DropdownMenu as _, IconName, PopupMenuItem, Select, SelectEvent,
    SelectItem, SelectState, Spinner, Textarea, h_flex, v_flex,
};
use flikk_core::cloud::{
    self, Capability, CloudResult, GenerateRequest, Reading, SCRIPT_CHARS, Voice,
};
use flikk_core::model::MediaItem;
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::base::slider::{SliderEvent, SliderState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};

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

impl SelectItem for VoiceRow {
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
    picker: Entity<SelectState<VoiceRow>>,
    /// The voice to pick once they are in.
    wanted: Option<String>,
    /// A voice the user picked while they loaded, to keep then.
    picked: Option<Voice>,
    loading: bool,
    /// Samples heard so far, by voice id.
    samples: HashMap<String, MediaItem>,
    /// Voices whose sample is on its way.
    fetching: HashSet<String>,
    /// The voice last asked to hear, and what the player was doing then.
    wanted_sample: Option<(String, Listening)>,
    /// Why the voices didn't load.
    load_error: Option<SharedString>,
    /// Why the last sample didn't come.
    sample_error: Option<SharedString>,
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
                .placeholder("[warm] The studio opened in 1974, two rooms and one tape machine.")
                .default_value(script_text)
        });
        let picker = cx.new(|cx| SelectState::new(Vec::new(), None, window, cx).searchable(true));
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
            cx.subscribe_in(
                &picker,
                window,
                |this, _, ev: &SelectEvent<VoiceRow>, window, cx| {
                    let SelectEvent::Confirm(id) = ev;
                    if this.loading {
                        this.picked = this.voices.iter().find(|v| v.id == *id).cloned();
                    }
                    this.apply_saved_reading(&id.clone(), window, cx);
                },
            ),
            cx.subscribe_in(&script, window, |_, script, _: &InputEvent, window, cx| {
                form::mark_tags(script, form::Tags::Bracketed, window, cx);
                cx.notify();
            }),
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
            picked: None,
            loading: true,
            samples: HashMap::new(),
            fetching: HashSet::new(),
            wanted_sample: None,
            load_error: None,
            sample_error: None,
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
        let picked = self.picked.take();
        match result {
            Ok(mut voices) => {
                voices.sort_by_key(|v| !v.mine());
                // Keep a voice picked meanwhile: one missing from the list
                // still reads if it is the user's own (newer than the
                // list) or shared (added to the account when it reads).
                let picked = picked
                    .filter(|p| voices.iter().any(|v| v.id == p.id) || p.mine() || p.shared());
                if let Some(p) = &picked
                    && !voices.iter().any(|v| v.id == p.id)
                {
                    voices.insert(0, p.clone());
                }
                self.voices = voices;
                let wanted = picked.map(|p| p.id).or(self.wanted.clone());
                self.show_voices(wanted, window, cx);
            }
            Err(e) => self.load_error = Some(e.to_string().into()),
        }
        cx.notify();
    }

    /// Fill the picker, with `wanted` (else the first) picked.
    fn show_voices(&mut self, wanted: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let rows: Vec<VoiceRow> = self.voices.iter().map(VoiceRow::new).collect();
        let found = self
            .voices
            .iter()
            .position(|v| Some(&v.id) == wanted.as_ref());
        let selected = found.or((!self.voices.is_empty()).then_some(0));
        self.picker.update(cx, |picker, cx| {
            picker.set_items(rows, window, cx);
            picker.set_selected_index(selected, window, cx);
        });
    }

    /// Read with `voice`, just found or made in the Voices dialog.
    fn use_voice(&mut self, voice: &Voice, window: &mut Window, cx: &mut Context<Self>) {
        if !self.voices.iter().any(|v| v.id == voice.id) {
            self.voices.insert(0, voice.clone());
        }
        if self.loading {
            self.picked = Some(voice.clone());
        }
        self.load_error = None;
        self.show_voices(Some(voice.id.clone()), window, cx);
        self.apply_saved_reading(&voice.id, window, cx);
        cx.notify();
    }

    /// Start on how library voice `voice` last read, when it is one.
    fn apply_saved_reading(&mut self, voice: &str, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let Some(provider) = state.voice_provider() else {
            return;
        };
        let Some(saved) = state.library.voice_of(&provider, voice) else {
            return;
        };
        let (stability, similarity) = (saved.stability, saved.similarity);
        self.stability
            .update(cx, |s, cx| s.set_value(stability, window, cx));
        self.similarity
            .update(cx, |s, cx| s.set_value(similarity, window, cx));
        cx.notify();
    }

    /// Open the Voices dialog over this form; the voice kept there is
    /// picked here.
    fn more_voices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = cx.entity().downgrade();
        let on_kept: crate::ui::voices::OnKept = std::rc::Rc::new(move |voice, window, cx| {
            let _ = form.update(cx, |f, cx| f.use_voice(voice, window, cx));
        });
        crate::ui::voices::open(
            self.state.clone(),
            crate::ui::voices::Start::Find,
            Some(on_kept),
            window,
            cx,
        );
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
            context: None,
        })
    }

    /// Start it; `true` once it is on its way.
    fn submit(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(request) = self.request(cx) else {
            return false;
        };
        self.state.update(cx, |s, cx| {
            if let Some(reading) = request.reading() {
                s.remember_voice(&reading.voice);
                s.keep_reading(reading, cx);
            }
            s.generate(request, cx);
        });
        true
    }

    /// Fetch the sample of `voice`, then play it if it is still the one
    /// last asked to hear and nothing was played or stopped meanwhile.
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
        self.wanted_sample = Some((id.clone(), state.listening()));
        // On its way already: it plays when it comes, as now asked.
        if !self.fetching.insert(id.clone()) {
            cx.notify();
            return;
        }
        self.sample_error = None;
        let fetch = cx.background_executor().spawn(async move {
            let path = generation.voice_sample(&voice)?;
            Ok::<_, cloud::CloudError>(cloud::sample_item(path, media.as_deref()))
        });
        cx.spawn(async move |this, cx| {
            let result = fetch.await;
            let _ = this.update(cx, |this, cx| {
                this.fetching.remove(&id);
                let then = this
                    .wanted_sample
                    .take_if(|(last, _)| *last == id)
                    .map(|(_, then)| then);
                match result {
                    Ok(item) => {
                        if let Some(then) = then {
                            this.state.update(cx, |s, cx| {
                                if s.still_listening(&then) {
                                    s.toggle_source(&item, cx);
                                }
                            });
                        }
                        this.samples.insert(id, item);
                    }
                    Err(e) => this.sample_error = Some(e.to_string().into()),
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
        if id.is_some_and(|id| self.fetching.contains(id)) {
            return div()
                .flex()
                .flex_none()
                .size(px(22.))
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .into_any_element();
        }
        if let Some(item) = id.and_then(|id| self.samples.get(id)) {
            return listen_button("voice-sample", &self.state, item, cx).into_any_element();
        }
        let heard = voice.as_ref().is_some_and(|v| v.sample.is_some());
        play_button("voice-sample", false)
            .disabled(!heard)
            .tooltip("Play")
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(voice) = voice.clone().filter(|v| v.sample.is_some()) {
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
            .small()
            .label(name)
            .trailing_icon(IconName::ChevronDown)
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
        let percent = format!("{:.0} %", Self::slider_value(slider, cx) * 100.);
        form::row(name, form::slider(slider, percent))
    }
}

impl Render for VoiceoverForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.text(cx);
        let chars = text.chars().count();
        let fixed = cloud::without_breaks(&text);
        let sample = self.sample_button(cx);
        v_flex()
            .gap(px(10.))
            .child(form::row(
                "Voice",
                h_flex()
                    .gap(px(6.))
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
                    .child(sample)
                    .child(
                        Button::new("more-voices")
                            .ghost()
                            .icon(IconName::Plus)
                            .tooltip("More voices")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.more_voices(window, cx)),
                            ),
                    ),
            ))
            .when_some(self.load_error.clone(), |el, error| {
                el.child(form::problem(error))
            })
            .when_some(self.sample_error.clone(), |el, error| {
                el.child(form::problem(error))
            })
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(
                        Textarea::new(&self.script)
                            .counter(chars, SCRIPT_CHARS)
                            .token(form::tag_token),
                    )
                    .child(
                        // Pulled left so the labels line up with the script.
                        h_flex()
                            .gap(px(2.))
                            .ml(px(-8.))
                            .child(self.tag_menu("Emotion", &EMOTIONS, cx))
                            .child(self.tag_menu("Reaction", &REACTIONS, cx))
                            .child(self.tag_menu("Pause", &PAUSES, cx)),
                    ),
            )
            .when_some(fixed, |el, fixed| {
                let script = self.script.clone();
                el.child(
                    h_flex()
                        .gap(px(8.))
                        .text_color(t::ERR)
                        .child(div().flex_1().child("<break> tags aren't read"))
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
        open(self.state.clone(), take, window, cx);
    }
}

/// The voiceover form, with `take`'s script and voice when given: to make
/// one again, or to read with a voice from the library.
pub fn open(state: Entity<EditorState>, take: Option<Take>, window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) {
        return;
    }
    let form = cx.new(|cx| VoiceoverForm::new(state, take, window, cx));
    window.open_dialog(cx, move |dialog, _, cx| {
        let submit = form.clone();
        let ready = form.read(cx).request(cx).is_some();
        dialog
            .title(Capability::Voiceover.label())
            .w(px(520.))
            .child(form.clone())
            .confirm("Generate", move |_, cx| {
                submit.update(cx, |f, cx| f.submit(cx))
            })
            .confirm_disabled(!ready)
    });
}
