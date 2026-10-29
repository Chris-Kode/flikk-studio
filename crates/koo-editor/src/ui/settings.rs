//! Agent settings (PRD "Agent configuration"), opened from the chat panel.
//! Every change goes straight to `EditorState::set_settings`.

use crate::state::EditorState;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, IndexPath, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::agent::{AgentSettings, Backend};
use koo_core::bus::{AgentRole, Permission, Permissions};
use koo_core::command::ToolCategory;

const BACKENDS: [Backend; 2] = [Backend::Claude, Backend::Codex];
const ROLES: [(AgentRole, &str); 2] = [
    (AgentRole::Editor, "Editor"),
    (AgentRole::Builder, "Builder"),
];

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

type LanguageList = SearchableVec<SharedString>;

pub struct SettingsPanel {
    state: Entity<EditorState>,
    instructions: Entity<TextareaState>,
    frames: Entity<SliderState>,
    tile: Entity<SliderState>,
    language: Entity<SelectState<LanguageList>>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsPanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = state.read(cx).settings.clone();
        let instructions = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 10)
                .placeholder("Always keep the intro under 20 seconds…")
                .default_value(settings.instructions.clone())
        });
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
                SearchableVec::new(
                    LANGUAGES
                        .iter()
                        .map(|(name, _)| SharedString::from(*name))
                        .collect::<Vec<_>>(),
                ),
                Some(IndexPath::new(selected)),
                window,
                cx,
            )
        });
        let _subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.subscribe(&instructions, |this, input, event, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = input.read(cx).value().to_string();
                    this.update(cx, |s| s.instructions = text);
                }
            }),
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
                |this, _, SelectEvent::Confirm(value): &SelectEvent<LanguageList>, cx| {
                    let code = value.as_ref().and_then(|v| {
                        LANGUAGES
                            .iter()
                            .find(|(name, _)| *name == v.as_ref())
                            .and_then(|(_, code)| code.map(str::to_owned))
                    });
                    this.update(cx, |s| s.language = code);
                },
            ),
        ];
        Self {
            state,
            instructions,
            frames,
            tile,
            language,
            _subscriptions,
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

    fn section(&self, title: &'static str, cx: &App) -> Div {
        v_flex().gap_2().child(
            div()
                .text_xs()
                .font_medium()
                .text_color(cx.theme().muted_foreground)
                .child(title),
        )
    }

    fn render_permissions(&self, settings: &AgentSettings, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let rows = ToolCategory::ALL.iter().map(|&category| {
            let effective = settings.permissions.get(category);
            let fixed = category.is_read()
                || matches!(
                    category,
                    ToolCategory::ProjectMemory | ToolCategory::Deliver | ToolCategory::Media
                );
            let control = if fixed {
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(match effective {
                        Permission::Allowed => "Always",
                        Permission::NeedsApproval => "Ask each time",
                        Permission::Off => "Never",
                        Permission::Propose => "Propose",
                    })
                    .into_any_element()
            } else {
                let on = effective == Permission::Propose;
                TabBar::new(("perm", category as usize))
                    .segmented()
                    .xsmall()
                    .selected_index(on as usize)
                    .child(Tab::new().label("Off"))
                    .child(Tab::new().label("Propose"))
                    .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                        let permission = if *ix == 1 {
                            Permission::Propose
                        } else {
                            Permission::Off
                        };
                        this.update(cx, |s| s.permissions.set(category, permission));
                    }))
                    .into_any_element()
            };
            h_flex()
                .h(px(30.))
                .justify_between()
                .text_sm()
                .child(
                    div()
                        .when(fixed, |el| el.text_color(theme.muted_foreground))
                        .child(category.label()),
                )
                .child(control)
        });
        let web = TabBar::new("perm-web")
            .segmented()
            .xsmall()
            .selected_index(settings.web as usize)
            .child(Tab::new().label("Off"))
            .child(Tab::new().label("On"))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                let on = *ix == 1;
                this.update(cx, |s| s.web = on);
            }));
        v_flex().children(rows).child(
            h_flex()
                .h(px(30.))
                .justify_between()
                .text_sm()
                .child("Web")
                .child(web),
        )
    }

    fn slider_row(
        &self,
        label: &'static str,
        state: &Entity<SliderState>,
        cx: &App,
    ) -> impl IntoElement {
        let value = state.read(cx).value().start().round() as u32;
        v_flex()
            .gap_1()
            .child(
                h_flex().justify_between().text_sm().child(label).child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(value.to_string()),
                ),
            )
            .child(Slider::new(state))
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = self.state.read(cx).settings.clone();
        let backend_ix = BACKENDS
            .iter()
            .position(|b| *b == settings.backend)
            .unwrap_or(0);
        let role_ix = ROLES
            .iter()
            .position(|(r, _)| *r == settings.role)
            .unwrap_or(0);
        let differs = settings.permissions != Permissions::default();
        v_flex()
            .id("agent-settings")
            .size_full()
            .gap_6()
            .pb_6()
            .overflow_y_scroll()
            .child(
                self.section("Backend", cx).child(
                    TabBar::new("backend")
                        .segmented()
                        .small()
                        .selected_index(backend_ix)
                        .children(BACKENDS.iter().map(|b| Tab::new().label(b.label())))
                        .on_click(cx.listener(|this, ix: &usize, _, cx| {
                            let backend = BACKENDS[*ix];
                            this.update(cx, |s| s.backend = backend);
                        })),
                ),
            )
            .child(
                self.section("Role", cx).child(
                    TabBar::new("role")
                        .segmented()
                        .small()
                        .selected_index(role_ix)
                        .children(ROLES.iter().map(|(_, label)| Tab::new().label(*label)))
                        .on_click(cx.listener(|this, ix: &usize, _, cx| {
                            let role = ROLES[*ix].0;
                            this.update(cx, |s| s.role = role);
                        })),
                ),
            )
            .child(
                self.section("Permissions", cx)
                    .child(self.render_permissions(&settings, cx))
                    .when(differs, |el| {
                        el.child(
                            h_flex().justify_end().child(
                                Button::new("perm-reset")
                                    .label("Reset")
                                    .xsmall()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.update(cx, |s| s.permissions = Permissions::default())
                                    })),
                            ),
                        )
                    }),
            )
            .child(
                self.section("Instructions", cx)
                    .child(Textarea::new(&self.instructions).text_sm()),
            )
            .child(
                self.section("Vision", cx)
                    .gap_4()
                    .child(self.slider_row("Frames per collage", &self.frames, cx))
                    .child(self.slider_row("Tile width", &self.tile, cx)),
            )
            .child(
                self.section("Language", cx)
                    .child(Select::new(&self.language).small()),
            )
    }
}
