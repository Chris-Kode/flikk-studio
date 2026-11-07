//! Settings → Integrations: each provider's API key and what it says of the
//! account, who does each capability (this computer, nobody, or a
//! provider) on which of its models, and the models this computer runs
//! ([`on_device`](super::on_device)). Built from `flikk_cloud::PROVIDERS`,
//! so a new provider shows up here as it is added there.

use super::on_device::{self, THIS_MACHINE, model_status, transcription_sizes};
use super::{group_label, page, row};
use crate::state::{Check, EditorState};
use crate::ui::kit::{
    Badge, Button, Icon, IconName, Input, ScrollableElement as _, Segmented, Select, SelectEvent,
    SelectState, Spinner, Switch, Tooltip, h_flex, theme as t, v_flex,
};
use flikk_cloud::{KeySource, PROVIDERS, Provider};
use flikk_core::cloud::Capability;
use flikk_models::Task as ModelTask;
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
use std::time::Duration;

/// How often a running download's progress is redrawn.
const PROGRESS_POLL: Duration = Duration::from_millis(200);

/// The label column of the page's rows.
const LABEL_W: f32 = 140.;
/// The actions after a key: Replace and remove, or Save, end on one edge.
const ACTIONS_W: f32 = 96.;

pub struct IntegrationsTab {
    state: Entity<EditorState>,
    /// A key field per provider, in `PROVIDERS` order, with the
    /// placeholder last given it.
    keys: Vec<(Entity<InputState>, String)>,
    /// Per provider, whether its stored key is being replaced.
    replacing: Vec<bool>,
    /// Per capability, who does it.
    doers: Vec<(Capability, Entity<SelectState<SharedString>>)>,
    /// Per capability, the models of the provider picked (by its id).
    models: HashMap<Capability, (&'static str, Entity<SelectState<SharedString>>)>,
    /// A model download was running at the last poll.
    busy: bool,
    _subscriptions: Vec<Subscription>,
    _poll: Task<()>,
}

/// The first entry of a capability's list: what happens with no provider.
fn no_provider(capability: Capability) -> &'static str {
    if capability.runs_locally() {
        THIS_MACHINE
    } else {
        "Off"
    }
}

/// Who can do `capability`, in the order its list shows them.
fn doers(capability: Capability) -> Vec<Option<&'static Provider>> {
    std::iter::once(None)
        .chain(flikk_cloud::providers_for(capability).map(Some))
        .collect()
}

impl IntegrationsTab {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![cx.observe(&state, |_, _, cx| cx.notify())];
        let keys: Vec<_> = PROVIDERS
            .iter()
            .enumerate()
            .map(|(ix, _)| {
                let input = cx.new(|cx| InputState::new(window, cx).masked(true));
                subscriptions.push(cx.subscribe_in(
                    &input,
                    window,
                    move |this, input, event: &InputEvent, window, cx| match event {
                        InputEvent::PressEnter { .. } => this.save_key(ix, window, cx),
                        InputEvent::Change => cx.notify(),
                        // Leaving the field empty keeps the stored key.
                        InputEvent::Blur if input.read(cx).value().trim().is_empty() => {
                            this.replacing[ix] = false;
                            cx.notify();
                        }
                        _ => {}
                    },
                ));
                (input, String::new())
            })
            .collect();
        let settings = state.read(cx).cloud.settings();
        let doers: Vec<_> = Capability::ALL
            .iter()
            .map(|&capability| {
                let options = doers(capability);
                let names: Vec<SharedString> = options
                    .iter()
                    .map(|p| p.map_or(no_provider(capability), |p| p.name).into())
                    .collect();
                let picked = settings.provider(capability);
                let selected = options
                    .iter()
                    .position(|p| p.map(|p| p.id) == picked)
                    .unwrap_or(0);
                let select = cx.new(|cx| SelectState::new(names, Some(selected), window, cx));
                subscriptions.push(cx.subscribe(
                    &select,
                    move |this, _, SelectEvent::Confirm(name): &SelectEvent<SharedString>, cx| {
                        let provider = doers(capability)
                            .into_iter()
                            .find(|p| {
                                p.map_or(no_provider(capability), |p| p.name) == name.as_ref()
                            })
                            .flatten();
                        this.state
                            .update(cx, |s, cx| s.set_provider(capability, provider, cx));
                    },
                ));
                (capability, select)
            })
            .collect();
        let _poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PROGRESS_POLL).await;
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            state,
            replacing: vec![false; keys.len()],
            keys,
            doers,
            models: HashMap::new(),
            busy: false,
            _subscriptions: subscriptions,
            _poll,
        }
    }

    /// Redraw while a model downloads, and once more after it ends.
    fn poll(&mut self, cx: &mut Context<Self>) {
        let busy = self.state.read(cx).models.busy();
        if busy || self.busy {
            cx.notify();
        }
        self.busy = busy;
    }

    /// Transcription on this computer: its size, and where its model
    /// stands.
    fn render_transcription_size(&self, cx: &App) -> impl IntoElement {
        let model = ModelTask::Transcription.model();
        let selected = transcription_sizes()
            .position(|(_, m)| m.id == model.id)
            .unwrap_or(0);
        let state = self.state.clone();
        let status = self.state.read(cx).models.status(model);
        h_flex()
            .gap(px(10.))
            .child(
                Segmented::new("transcription-size")
                    .field()
                    .items(transcription_sizes().map(|(label, _)| label))
                    .selected(selected)
                    .on_click(move |ix, _, cx| {
                        if let Some((_, model)) = transcription_sizes().nth(*ix) {
                            state.update(cx, |s, cx| {
                                s.pick_model(ModelTask::Transcription, model, cx)
                            });
                        }
                    }),
            )
            .child(model_status(
                &self.state,
                ModelTask::Transcription,
                model,
                status,
                cx,
            ))
    }

    /// The page is shown again: drop a replacement left half typed and
    /// check the keys anew.
    pub fn reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (ix, replacing) in self.replacing.iter_mut().enumerate() {
            if std::mem::take(replacing) {
                self.keys[ix]
                    .0
                    .update(cx, |i, cx| i.set_value("", window, cx));
            }
        }
        self.state.update(cx, |s, cx| s.refresh_accounts(cx));
    }

    fn save_key(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.keys[ix].0.clone();
        let key = input.read(cx).value().trim().to_owned();
        if key.is_empty() {
            return;
        }
        input.update(cx, |i, cx| i.set_value("", window, cx));
        self.replacing[ix] = false;
        self.state
            .update(cx, |s, cx| s.set_api_key(PROVIDERS[ix], key, cx));
    }

    fn replace_key(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.replacing[ix] = true;
        self.keys[ix].0.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    fn cancel_replace(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.replacing[ix] = false;
        self.keys[ix]
            .0
            .update(cx, |i, cx| i.set_value("", window, cx));
        cx.notify();
    }

    /// Keep each list on what is picked now (a first key turns generation
    /// on), and the model lists on the provider picked.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (ix, &provider) in PROVIDERS.iter().enumerate() {
            let status = self.state.read(cx).provider_status(provider);
            // A key read after the page opened is checked as it shows, once:
            // renders before the deferred check runs queue it again.
            if status.has_key() && status.check == Check::Unchecked {
                let state = self.state.clone();
                cx.defer(move |cx| {
                    state.update(cx, |s, cx| {
                        if s.provider_status(provider).check == Check::Unchecked {
                            s.check_account(provider, cx)
                        }
                    })
                });
            }
            if status.key != Some(KeySource::Stored) {
                self.replacing[ix] = false;
            }
            let placeholder = if self.replacing[ix] {
                "New API key"
            } else {
                "API key"
            };
            let (input, shown) = &mut self.keys[ix];
            if shown != placeholder {
                input.update(cx, |i, cx| i.set_placeholder(placeholder, window, cx));
                *shown = placeholder.to_owned();
            }
        }
        let settings = self.state.read(cx).cloud.settings();
        for (capability, select) in &self.doers {
            let picked = settings.provider(*capability);
            let ix = doers(*capability)
                .iter()
                .position(|p| p.map(|p| p.id) == picked)
                .unwrap_or(0);
            if select.read(cx).selected_index() != Some(ix) {
                select.update(cx, |s, cx| s.set_selected_index(Some(ix), window, cx));
            }
        }
        for &capability in &Capability::ALL {
            let choice = settings.choice(capability);
            let Some(provider) = choice.provider.as_deref().and_then(flikk_cloud::provider) else {
                self.models.remove(&capability);
                continue;
            };
            let Some(offer) = provider.offer(capability) else {
                continue;
            };
            let current = provider
                .model(capability, choice.model.as_deref())
                .map_or(0, |m| {
                    offer.models.iter().position(|o| o.id == m.id).unwrap_or(0)
                });
            if let Some((id, select)) = self.models.get(&capability)
                && *id == provider.id
            {
                if select.read(cx).selected_index() != Some(current) {
                    select.update(cx, |s, cx| s.set_selected_index(Some(current), window, cx));
                }
                continue;
            }
            let names: Vec<SharedString> = offer.models.iter().map(|m| m.name.into()).collect();
            let many = names.len() > 8;
            let select =
                cx.new(|cx| SelectState::new(names, Some(current), window, cx).searchable(many));
            self._subscriptions.push(cx.subscribe(
                &select,
                move |this, _, SelectEvent::Confirm(name): &SelectEvent<SharedString>, cx| {
                    if let Some(model) = offer.models.iter().find(|m| m.name == name.as_ref()) {
                        this.state
                            .update(cx, |s, cx| s.set_cloud_model(capability, model.id, cx));
                    }
                },
            ));
            self.models.insert(capability, (provider.id, select));
        }
    }

    /// One line per provider: its name, then what is known of its key or
    /// the field to type one, then its actions.
    fn render_provider(
        &self,
        ix: usize,
        provider: &'static Provider,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let status = self.state.read(cx).provider_status(provider);
        let input = self.keys[ix].0.clone();
        let typed = !input.read(cx).value().trim().is_empty();
        let stored = status.key == Some(KeySource::Stored);
        let from_env = status.key == Some(KeySource::Environment);
        let replacing = self.replacing[ix] && stored;
        let editing = replacing || (!status.has_key() && status.check != Check::Checking);
        let (failure, retry) = match &status.check {
            Check::Failed { message, retry } => (Some(message.clone()), *retry),
            _ => (None, false),
        };
        let keys_url = provider.keys_url;

        let name = h_flex()
            .w(px(LABEL_W))
            .flex_none()
            .h(px(26.))
            .gap(px(2.))
            .font_weight(FontWeight::MEDIUM)
            .child(provider.name)
            .when(editing || (failure.is_some() && !retry), |el| {
                el.child(
                    Button::new(("integration-keys", ix))
                        .ghost()
                        .xsmall()
                        .icon(IconName::ExternalLink)
                        .tooltip("Get an API key")
                        .on_click(move |_, _, cx| cx.open_url(keys_url)),
                )
            });

        let middle = if editing {
            // A key the credential store would not keep.
            let unsaved = failure.clone().filter(|_| !status.has_key());
            div()
                .child(
                    Input::new(&input)
                        .mask_toggle()
                        .when_some(unsaved, |i, message| {
                            i.suffix(
                                div()
                                    .id(("integration-unsaved", ix))
                                    .child(
                                        Icon::new(IconName::CircleAlert)
                                            .size(px(13.))
                                            .color(t::ERR),
                                    )
                                    .tooltip(Tooltip::text(message)),
                            )
                        }),
                )
                .into_any_element()
        } else {
            match &status.check {
                Check::Unchecked => div().into_any_element(),
                Check::Checking => h_flex()
                    .gap(px(6.))
                    .child(Spinner::new().size(px(12.)))
                    .child("Checking…")
                    .into_any_element(),
                Check::Valid(account) => {
                    let mut parts = Vec::new();
                    if let Some(plan) = &account.plan {
                        parts.push(plan_label(plan));
                    }
                    if let Some(left) = account.credits_left() {
                        parts.push(format!("{} credits left", thousands(left)));
                    }
                    if let Some(balance) = &account.balance {
                        parts.push(format!("{balance} left"));
                    }
                    let summary = if parts.is_empty() {
                        "Connected".to_owned()
                    } else {
                        parts.join(" · ")
                    };
                    h_flex()
                        .id(("integration-status", ix))
                        .min_w_0()
                        .gap(px(6.))
                        .cursor_pointer()
                        .child(Icon::new(IconName::CircleCheck).size(px(13.)).color(t::OK))
                        .child(div().truncate().child(summary))
                        .tooltip(Tooltip::text("Check again"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |s, cx| s.check_account(provider, cx))
                        }))
                        .into_any_element()
                }
                Check::Failed { message, retry } => {
                    let shown = if *retry {
                        "Couldn't check the key".to_owned()
                    } else {
                        message.clone()
                    };
                    h_flex()
                        .id(("integration-status", ix))
                        .min_w_0()
                        .gap(px(6.))
                        .text_color(t::T1)
                        .child(
                            Icon::new(IconName::CircleAlert)
                                .size(px(13.))
                                .color(if *retry { t::WARN } else { t::ERR }),
                        )
                        .child(div().truncate().child(shown))
                        .tooltip(Tooltip::text(message.clone()))
                        .into_any_element()
                }
            }
        };

        let actions = if editing {
            h_flex()
                .gap(px(4.))
                .when(replacing, |el| {
                    el.child(
                        Button::new(("integration-cancel", ix))
                            .ghost()
                            .icon(IconName::X)
                            .tooltip("Keep the current key")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.cancel_replace(ix, window, cx)
                            })),
                    )
                })
                .child(
                    Button::new(("integration-save", ix))
                        .label("Save")
                        .when(!replacing, |b| b.w(px(ACTIONS_W)))
                        .disabled(!typed)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.save_key(ix, window, cx)),
                        ),
                )
                .into_any_element()
        } else if from_env {
            h_flex()
                .gap(px(6.))
                .when(retry, |el| el.child(self.retry_button(ix, provider, cx)))
                .child(Badge::new(provider.key_env).mono())
                .into_any_element()
        } else if retry {
            self.retry_button(ix, provider, cx).into_any_element()
        } else {
            let rejected = failure.is_some();
            h_flex()
                .gap(px(4.))
                .when(stored, |el| {
                    el.child(
                        Button::new(("integration-replace", ix))
                            .label("Replace")
                            .when(!rejected, |b| b.ghost())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.replace_key(ix, window, cx)
                            })),
                    )
                    .child(
                        Button::new(("integration-remove", ix))
                            .ghost()
                            .icon(IconName::Trash)
                            .tooltip("Remove key")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state
                                    .update(cx, |s, cx| s.remove_api_key(provider, cx))
                            })),
                    )
                })
                .into_any_element()
        };

        h_flex()
            .flex_none()
            .h(px(46.))
            .gap(px(12.))
            .border_b_1()
            .border_color(t::LN)
            .child(name)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .text_color(t::T2)
                    .child(div().flex_1().min_w_0().child(middle)),
            )
            .child(
                h_flex()
                    .flex_none()
                    .min_w(px(ACTIONS_W))
                    .justify_end()
                    .child(actions),
            )
    }

    fn retry_button(
        &self,
        ix: usize,
        provider: &'static Provider,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(("integration-retry", ix))
            .label("Try again")
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.update(cx, |s, cx| s.check_account(provider, cx))
            }))
    }
}

impl Render for IntegrationsTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(window, cx);
        let providers: Vec<_> = PROVIDERS
            .iter()
            .enumerate()
            .map(|(ix, &p)| self.render_provider(ix, p, cx).into_any_element())
            .collect();
        let settings = self.state.read(cx).cloud.settings();
        let sound = self.state.read(cx).video_sound();
        let rows: Vec<_> = self
            .doers
            .iter()
            .map(|(capability, select)| {
                let capability = *capability;
                let picked = settings.provider(capability).is_some();
                let model = self.models.get(&capability).map(|(_, m)| m.clone());
                row(capability.label(), LABEL_W)
                    .h(px(36.))
                    .child(div().w(px(160.)).flex_none().child(Select::new(select)))
                    .children(model.map(|m| {
                        let single = self
                            .models
                            .get(&capability)
                            .and_then(|(id, _)| flikk_cloud::provider(id))
                            .and_then(|p| p.offer(capability))
                            .is_some_and(|o| o.models.len() < 2);
                        div()
                            .w(px(180.))
                            .flex_none()
                            .child(Select::new(&m).disabled(single))
                    }))
                    .when(capability.runs_locally() && !picked, |el| {
                        el.child(self.render_transcription_size(cx))
                    })
                    .when(capability == Capability::Video && picked, |el| {
                        let state = self.state.clone();
                        el.child(
                            Switch::new("video-sound")
                                .checked(sound)
                                .label("With sound")
                                .on_click(move |on: &bool, _, cx| {
                                    state.update(cx, |s, cx| s.set_video_sound(*on, cx))
                                }),
                        )
                    })
                    .into_any_element()
            })
            .collect();
        page("Integrations", None)
            .children(providers)
            .child(
                v_flex()
                    .mt(px(20.))
                    .child(group_label("Use for"))
                    .children(rows),
            )
            .child(on_device::section(&self.state, cx))
            .overflow_y_scrollbar()
    }
}

/// "creator" → "Creator"; "growing_business" → "Growing business";
/// "scale_2024_08_10" → "Scale".
fn plan_label(plan: &str) -> String {
    let words = plan
        .split('_')
        .take_while(|w| !w.starts_with(|c: char| c.is_ascii_digit()))
        .collect::<Vec<_>>()
        .join(" ");
    let mut chars = words.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// 12345 → "12,345".
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
