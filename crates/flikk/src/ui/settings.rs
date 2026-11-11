//! Settings → Agent (PRD "Agent configuration"), also opened from the chat
//! panel's gear. Every change goes straight to `EditorState::set_settings`.

use crate::state::EditorState;
use crate::ui::editor_settings::{page, row, section_label};
use crate::ui::kit::{
    Button, ScrollableElement as _, Segmented, Select, SelectEvent, SelectState, Slider, Tooltip,
    h_flex, theme as t, v_flex,
};
use flikk_agent::tools::image;
use flikk_core::agent::{AgentSettings, Backend};
use flikk_core::bus::{Permission, Permissions};
use flikk_core::command::ToolCategory;
use gpui_kit::base::slider::{SliderEvent, SliderState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const BACKENDS: [Backend; 2] = [Backend::Claude, Backend::Codex];

/// `None` follows the source language.
const LANGUAGES: [(&str, Option<&str>); 10] = [
    ("Same as source", None),
    ("English", Some("en")),
    ("Spanish", Some("es")),
    ("French", Some("fr")),
    ("German", Some("de")),
    ("Portuguese", Some("pt")),
    ("Italian", Some("it")),
    ("Japanese", Some("ja")),
    ("Korean", Some("ko")),
    ("Chinese", Some("zh")),
];

/// The label column of the page's rows.
const LABEL_W: f32 = 150.;

pub struct SettingsPanel {
    state: Entity<EditorState>,
    frames: Entity<SliderState>,
    tile: Entity<SliderState>,
    language: Entity<SelectState<SharedString>>,
    /// The paid ChatGPT plan Codex is signed in with, which image
    /// generation needs: looked up when the panel opens and when the switch
    /// is clicked.
    chatgpt: Option<String>,
    /// The settings the fields were last filled from: a change made
    /// elsewhere since goes into them, so a stale field cannot write the
    /// old value back.
    shown: AgentSettings,
    _subscriptions: Vec<Subscription>,
}

impl SettingsPanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = state.read(cx).settings.clone();
        let frames = cx.new(|_| {
            SliderState::new()
                .max(30.)
                .min(4.)
                .step(1.)
                .default_value(settings.vision.frames_per_collage as f32)
        });
        let tile = cx.new(|_| {
            SliderState::new()
                .max(640.)
                .min(160.)
                .step(32.)
                .default_value(settings.vision.tile_width as f32)
        });
        let selected = LANGUAGES
            .iter()
            .position(|(_, code)| *code == settings.language.as_deref())
            .unwrap_or(0);
        let language = cx.new(|cx| {
            SelectState::new(
                LANGUAGES
                    .iter()
                    .map(|(name, _)| SharedString::from(*name))
                    .collect(),
                Some(selected),
                window,
                cx,
            )
        });
        let _subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.subscribe(&frames, |this, _, event: &SliderEvent, cx| match event {
                SliderEvent::Release(v) => this.update(cx, |s| {
                    s.vision.frames_per_collage = v.start().round() as u32
                }),
                SliderEvent::Change(_) => cx.notify(),
            }),
            cx.subscribe(&tile, |this, _, event: &SliderEvent, cx| match event {
                SliderEvent::Release(v) => {
                    this.update(cx, |s| s.vision.tile_width = v.start().round() as u32)
                }
                SliderEvent::Change(_) => cx.notify(),
            }),
            cx.subscribe(
                &language,
                |this, _, SelectEvent::Confirm(name): &SelectEvent<SharedString>, cx| {
                    let code = LANGUAGES
                        .iter()
                        .find(|(n, _)| *n == name.as_ref())
                        .and_then(|(_, code)| code.map(str::to_owned));
                    this.update(cx, |s| s.language = code);
                },
            ),
        ];
        Self {
            state,
            frames,
            tile,
            language,
            chatgpt: image::subscription(),
            shown: settings,
            _subscriptions,
        }
    }

    /// Put changes made elsewhere into the sliders and language.
    fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.state.read(cx).settings.clone();
        if settings == self.shown {
            return;
        }
        let old = std::mem::replace(&mut self.shown, settings.clone());
        for (slider, new, old) in [
            (
                &self.frames,
                settings.vision.frames_per_collage,
                old.vision.frames_per_collage,
            ),
            (
                &self.tile,
                settings.vision.tile_width,
                old.vision.tile_width,
            ),
        ] {
            if new != old {
                slider.update(cx, |s, cx| s.set_value(new as f32, window, cx));
            }
        }
        if settings.language != old.language {
            let ix = LANGUAGES
                .iter()
                .position(|(_, code)| *code == settings.language.as_deref())
                .unwrap_or(0);
            self.language
                .update(cx, |s, cx| s.set_selected_index(Some(ix), window, cx));
        }
    }

    fn update(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut AgentSettings)) {
        self.state.update(cx, |s, cx| {
            let mut settings = s.settings.clone();
            f(&mut settings);
            if settings != s.settings {
                s.set_settings(settings, cx);
            }
        });
    }

    /// The backend; its model and thinking level are picked in the chat.
    fn render_backend(&self, settings: &AgentSettings, cx: &mut Context<Self>) -> Div {
        let backend_ix = BACKENDS
            .iter()
            .position(|b| *b == settings.backend)
            .unwrap_or(0);
        v_flex().child(section_label("Backend")).child(
            row("Runs on", LABEL_W).child(
                Segmented::new("backend")
                    .items(BACKENDS.iter().map(|b| b.label()))
                    .selected(backend_ix)
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        let backend = BACKENDS[*ix];
                        this.update(cx, |s| s.backend = backend);
                    })),
            ),
        )
    }

    fn render_permissions(&self, settings: &AgentSettings, cx: &mut Context<Self>) -> Div {
        // By effect: settings saved before a category existed lack it.
        let defaults = Permissions::default();
        let differs = ToolCategory::ALL
            .iter()
            .any(|&c| settings.permissions.get(c) != defaults.get(c));
        let perm_row = |label: &'static str, fixed: bool, control: AnyElement| {
            h_flex()
                .flex_none()
                .h(px(32.))
                .justify_between()
                .border_b_1()
                .border_color(t::LN)
                .child(div().when(fixed, |el| el.text_color(t::T3)).child(label))
                .child(control)
        };
        let rows = ToolCategory::ALL.iter().map(|&category| {
            let effective = settings.permissions.get(category);
            let choices = Permissions::choices(category);
            let fixed = choices.is_empty();
            let control = if fixed {
                div()
                    .text_size(px(11.))
                    .text_color(t::T3)
                    .child(match effective {
                        Permission::Allowed => "Always",
                        Permission::NeedsApproval => "Ask each time",
                        Permission::Off => "Never",
                        Permission::Propose => "Propose",
                    })
                    .into_any_element()
            } else {
                let label = |permission: &Permission| match permission {
                    Permission::Off => "Off",
                    Permission::NeedsApproval => "Ask",
                    Permission::Allowed => "Allow",
                    Permission::Propose => "Propose",
                };
                Segmented::new(("perm", category as usize))
                    .compact()
                    .items(choices.iter().map(label))
                    .selected(choices.iter().position(|p| *p == effective).unwrap_or(1))
                    .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                        let permission = choices[*ix];
                        this.update(cx, |s| s.permissions.set(category, permission));
                    }))
                    .into_any_element()
            };
            perm_row(category.label(), fixed, control)
        });
        let web = Segmented::new("perm-web")
            .compact()
            .items(["Off", "On"])
            .selected(settings.web as usize)
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                let on = *ix == 1;
                this.update(cx, |s| s.web = on);
            }));
        // On only with a paid ChatGPT plan to make the pictures with; the
        // provider set in Integrations makes them instead when there is one.
        let linked = self.chatgpt.is_some();
        let provider = self
            .state
            .read(cx)
            .image_provider()
            .map(|(p, m)| format!("Set in Integrations: {} · {}", p.name, m.name));
        let overridden = provider.is_some();
        let images = Segmented::new("perm-images")
            .compact()
            .items(["Off", "On"])
            .disabled_item(0, overridden)
            .disabled_item(1, overridden || !linked)
            .selected((settings.image_generation && linked) as usize)
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                let on = *ix == 1;
                this.chatgpt = image::subscription();
                if on && this.chatgpt.is_none() {
                    cx.notify();
                    return;
                }
                this.update(cx, |s| s.image_generation = on);
            }));
        let tip = provider.or_else(|| {
            (!linked).then(|| "Needs a paid ChatGPT plan signed in with Codex".to_owned())
        });
        let images = div()
            .id("perm-images-tip")
            .when_some(tip, |d, tip| d.tooltip(Tooltip::text(tip)))
            .child(images);
        v_flex()
            .child(section_label("Permissions").when(differs, |el| {
                el.child(
                    Button::new("perm-reset")
                        .ghost()
                        .xsmall()
                        .label("Reset")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.update(cx, |s| s.permissions = Permissions::default())
                        })),
                )
            }))
            .children(rows)
            .child(perm_row("Web", false, web.into_any_element()))
            .child(perm_row(
                "Image generation",
                !linked || overridden,
                images.into_any_element(),
            ))
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_fields(window, cx);
        let settings = self.state.read(cx).settings.clone();
        let value = |state: &Entity<SliderState>| {
            (state.read(cx).value().start().round() as u32).to_string()
        };
        let (frames, tile) = (value(&self.frames), value(&self.tile));
        page("Agent", None)
            .child(self.render_backend(&settings, cx))
            .child(self.render_permissions(&settings, cx).mt(px(20.)))
            .child(
                v_flex()
                    .mt(px(20.))
                    .gap(px(12.))
                    .child(section_label("Vision").mb_0())
                    .child(
                        Slider::new(&self.frames)
                            .label("Frames per collage")
                            .readout(frames),
                    )
                    .child(Slider::new(&self.tile).label("Tile width").readout(tile)),
            )
            .child(
                v_flex()
                    .mt(px(20.))
                    .child(section_label("Language"))
                    .child(div().w(px(220.)).child(Select::new(&self.language))),
            )
            .overflow_y_scrollbar()
    }
}
