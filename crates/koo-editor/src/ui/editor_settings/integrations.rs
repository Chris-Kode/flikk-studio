//! Settings → Integrations: each provider's API key and what it says of the
//! account, and who does each capability (this computer, nobody, or a
//! provider) on which of its models. Built from `koo_cloud::PROVIDERS`,
//! so a new provider shows up here as it is added there.

use super::{row, section};
use crate::state::{Check, EditorState};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, IndexPath, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_cloud::{KeySource, PROVIDERS, Provider};
use koo_core::cloud::Capability;
use std::collections::HashMap;

type List = SearchableVec<SharedString>;

pub struct IntegrationsTab {
    state: Entity<EditorState>,
    /// A key field per provider, in `PROVIDERS` order, with the
    /// placeholder last given it.
    keys: Vec<(Entity<InputState>, String)>,
    /// Per capability, who does it.
    doers: Vec<(Capability, Entity<SelectState<List>>)>,
    /// Per capability, the models of the provider picked (by its id).
    models: HashMap<Capability, (&'static str, Entity<SelectState<List>>)>,
    _subscriptions: Vec<Subscription>,
}

/// The first entry of a capability's list: what happens with no provider.
fn no_provider(capability: Capability) -> &'static str {
    if capability.runs_locally() {
        "This computer"
    } else {
        "Off"
    }
}

/// Who can do `capability`, in the order its list shows them.
fn doers(capability: Capability) -> Vec<Option<&'static Provider>> {
    std::iter::once(None)
        .chain(koo_cloud::providers_for(capability).map(Some))
        .collect()
}

impl IntegrationsTab {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![cx.observe(&state, |_, _, cx| cx.notify())];
        let keys: Vec<_> = PROVIDERS
            .iter()
            .map(|&provider| {
                let input = cx.new(|cx| InputState::new(window, cx).masked(true));
                subscriptions.push(cx.subscribe_in(
                    &input,
                    window,
                    move |this, input, event: &InputEvent, window, cx| {
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            this.save_key(provider, input, window, cx);
                        } else if matches!(event, InputEvent::Change) {
                            cx.notify();
                        }
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
                let select = cx.new(|cx| {
                    SelectState::new(
                        SearchableVec::new(names),
                        Some(IndexPath::new(selected)),
                        window,
                        cx,
                    )
                });
                subscriptions.push(cx.subscribe(
                    &select,
                    move |this, _, event: &SelectEvent<List>, cx| {
                        if let SelectEvent::Confirm(Some(name)) = event {
                            let provider = doers(capability)
                                .into_iter()
                                .find(|p| {
                                    p.map_or(no_provider(capability), |p| p.name) == name.as_ref()
                                })
                                .flatten();
                            this.state
                                .update(cx, |s, cx| s.set_provider(capability, provider, cx));
                        }
                    },
                ));
                (capability, select)
            })
            .collect();
        Self {
            state,
            keys,
            doers,
            models: HashMap::new(),
            _subscriptions: subscriptions,
        }
    }

    fn save_key(
        &mut self,
        provider: &'static Provider,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = input.read(cx).value().trim().to_owned();
        if key.is_empty() {
            return;
        }
        input.update(cx, |i, cx| i.set_value("", window, cx));
        self.state
            .update(cx, |s, cx| s.set_api_key(provider, key, cx));
    }

    /// Keep each list on what is picked now (a first key turns generation
    /// on), and the model lists on the provider picked.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for ((input, shown), &provider) in self.keys.iter_mut().zip(PROVIDERS) {
            let status = self.state.read(cx).provider_status(provider);
            // A key read after the page opened is checked as it shows.
            if status.has_key() && status.check == Check::Unchecked {
                let state = self.state.clone();
                cx.defer(move |cx| state.update(cx, |s, cx| s.check_account(provider, cx)));
            }
            let placeholder = match status.key {
                Some(KeySource::Environment) => format!("Set by {}", provider.key_env),
                Some(KeySource::Stored) => "••••••••••••••••".to_owned(),
                _ => "API key".to_owned(),
            };
            if *shown != placeholder {
                input.update(cx, |i, cx| {
                    i.set_placeholder(placeholder.clone(), window, cx)
                });
                *shown = placeholder;
            }
        }
        let settings = self.state.read(cx).cloud.settings();
        for (capability, select) in &self.doers {
            let picked = settings.provider(*capability);
            let ix = doers(*capability)
                .iter()
                .position(|p| p.map(|p| p.id) == picked)
                .unwrap_or(0);
            if select.read(cx).selected_index(cx) != Some(IndexPath::new(ix)) {
                select.update(cx, |s, cx| {
                    s.set_selected_index(Some(IndexPath::new(ix)), window, cx)
                });
            }
        }
        for &capability in &Capability::ALL {
            let choice = settings.choice(capability);
            let Some(provider) = choice.provider.as_deref().and_then(koo_cloud::provider) else {
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
                if select.read(cx).selected_index(cx) != Some(IndexPath::new(current)) {
                    select.update(cx, |s, cx| {
                        s.set_selected_index(Some(IndexPath::new(current)), window, cx)
                    });
                }
                continue;
            }
            let names: Vec<SharedString> = offer.models.iter().map(|m| m.name.into()).collect();
            let select = cx.new(|cx| {
                SelectState::new(
                    SearchableVec::new(names),
                    Some(IndexPath::new(current)),
                    window,
                    cx,
                )
            });
            self._subscriptions.push(cx.subscribe(
                &select,
                move |this, _, event: &SelectEvent<List>, cx| {
                    if let SelectEvent::Confirm(Some(name)) = event
                        && let Some(model) = offer.models.iter().find(|m| m.name == name.as_ref())
                    {
                        this.state
                            .update(cx, |s, cx| s.set_cloud_model(capability, model.id, cx));
                    }
                },
            ));
            self.models.insert(capability, (provider.id, select));
        }
    }

    fn render_provider(
        &self,
        ix: usize,
        provider: &'static Provider,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let status = self.state.read(cx).provider_status(provider);
        let theme = cx.theme().clone();
        let input = self.keys[ix].0.clone();
        let typed = !input.read(cx).value().trim().is_empty();
        let from_env = status.key == Some(KeySource::Environment);
        let state_line: Option<AnyElement> = match &status.check {
            Check::Unchecked => None,
            Check::Checking => Some(
                h_flex()
                    .gap_2()
                    .child(Spinner::new().xsmall())
                    .into_any_element(),
            ),
            Check::Valid(account) => {
                let mut parts = Vec::new();
                if let Some(plan) = &account.plan {
                    parts.push(plan_label(plan));
                }
                if let Some(left) = account.credits_left() {
                    parts.push(format!("{} credits left", thousands(left)));
                }
                Some(
                    h_flex()
                        .gap_2()
                        .child(
                            Icon::new(IconName::CircleCheck)
                                .small()
                                .text_color(theme.green),
                        )
                        .when(!parts.is_empty(), |el| el.child(parts.join(" · ")))
                        .into_any_element(),
                )
            }
            Check::Failed(message) => Some(
                h_flex()
                    .gap_2()
                    .min_w_0()
                    .child(
                        Icon::new(IconName::CircleAlert)
                            .small()
                            .text_color(theme.red),
                    )
                    .child(div().truncate().child(message.clone()))
                    .into_any_element(),
            ),
        };
        let keys_url = provider.keys_url;
        v_flex()
            .gap_2()
            .pb_3()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .justify_between()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child(provider.name)
                    .child(
                        Button::new(("integration-keys", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::ExternalLink)
                            .tooltip("Get an API key")
                            .on_click(move |_, _, cx| cx.open_url(keys_url)),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&input).small().mask_toggle().disabled(from_env)),
                    )
                    .child(
                        Button::new(("integration-save", ix))
                            .small()
                            .outline()
                            .label("Save")
                            .disabled(!typed || from_env)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let input = this.keys[ix].0.clone();
                                this.save_key(provider, &input, window, cx)
                            })),
                    )
                    .when(status.key == Some(KeySource::Stored), |el| {
                        el.child(
                            Button::new(("integration-remove", ix))
                                .small()
                                .ghost()
                                .icon(IconName::Delete)
                                .tooltip("Remove key")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.state
                                        .update(cx, |s, cx| s.remove_api_key(provider, cx))
                                })),
                        )
                    }),
            )
            .children(state_line.map(|line| {
                h_flex()
                    .justify_between()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(line)
                    .when(status.has_key(), |el| {
                        el.child(
                            Button::new(("integration-check", ix))
                                .ghost()
                                .xsmall()
                                .icon(IconName::RefreshCw)
                                .tooltip("Check again")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.state.update(cx, |s, cx| s.check_account(provider, cx))
                                })),
                        )
                    })
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
        let rows: Vec<_> = self
            .doers
            .iter()
            .map(|(capability, select)| {
                let model = self.models.get(capability).map(|(_, m)| m.clone());
                row(capability.label(), cx)
                    .child(
                        div()
                            .w(px(170.))
                            .flex_none()
                            .child(Select::new(select).small()),
                    )
                    .children(model.map(|m| {
                        let single = self
                            .models
                            .get(capability)
                            .and_then(|(id, _)| koo_cloud::provider(id))
                            .and_then(|p| p.offer(*capability))
                            .is_some_and(|o| o.models.len() < 2);
                        div()
                            .w(px(190.))
                            .flex_none()
                            .child(Select::new(&m).small().disabled(single))
                    }))
                    .into_any_element()
            })
            .collect();
        v_flex()
            .id("integrations")
            .size_full()
            .gap_6()
            .pb_6()
            .overflow_y_scroll()
            .child(v_flex().gap_4().children(providers))
            .child(section("Use for", cx).child(v_flex().children(rows)))
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
