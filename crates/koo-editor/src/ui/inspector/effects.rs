//! A picture clip's effect stack: add, enable, reorder (drag a row onto
//! another), fold, reset, remove, and every parameter with keyframes. Rows
//! drag into the chat as `@effect <clip>/<effect>` references. The Video
//! page lists the stack compactly; the Effects page holds the cards.

use super::controls::{FxOwner, FxTarget};
use super::{InspectorPanel, PictureTab, group};
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, DropdownMenu as _, EmptyState, Icon, IconName, PopupMenu, PopupMenuItem, SectionHeader,
    Switch, h_flex, v_flex,
};
use crate::ui::library::shared;
use crate::ui::{FxDrag, RefDrag};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::command::Command;
use koo_core::fx::{self, FxInstance, FxKind, FxRef};
use koo_core::model::Clip;
use koo_core::{ClipId, EffectId, LibraryId};

/// Chat token and label for an effect on a clip.
pub(super) fn effect_mention(clip: ClipId, effect: EffectId, name: &str) -> (String, SharedString) {
    (format!("@effect {clip}/{effect}"), name.to_owned().into())
}

impl InspectorPanel {
    /// The Effects page: a card per effect, then add and presets. Effects
    /// dropped from the library are added.
    pub(super) fn effects_view(
        &mut self,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let clip_id = clip.id;
        let header = SectionHeader::new("effects", "Effects").child(
            self.add_effect_menu(
                clip_id,
                Button::new("add-effect")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .tooltip("Add effect"),
                cx,
            ),
        );
        let cards: Vec<AnyElement> = clip
            .effects
            .iter()
            .enumerate()
            .map(|(i, e)| self.effect_card(clip_id, i, e, window, cx))
            .collect();
        let footer = h_flex()
            .gap(px(4.))
            .px(px(8.))
            .pt(px(6.))
            .child(
                self.add_effect_menu(
                    clip_id,
                    Button::new("add-effect-below")
                        .small()
                        .icon(IconName::Plus)
                        .label("Add effect"),
                    cx,
                ),
            )
            .child(self.presets_menu(clip, cx));
        v_flex()
            .id("effects")
            .pb(px(12.))
            .drag_over::<FxDrag>(|s, d: &FxDrag, _, _| {
                if d.kind == FxKind::Effect {
                    s.bg(t::alpha(t::AC, 0.08))
                } else {
                    s
                }
            })
            .on_drop(cx.listener(move |this, d: &FxDrag, _, cx| {
                if d.kind == FxKind::Effect
                    && let Some(fx) = d.program(&this.state, cx)
                {
                    this.add_effect(clip_id, fx, cx);
                }
            }))
            .child(header)
            .when(cards.is_empty(), |v| {
                v.child(EmptyState::new(IconName::Sparkles, "No effects").compact())
            })
            .child(v_flex().gap(px(6.)).px(px(8.)).children(cards))
            .child(footer)
    }

    /// The Video page's list of the stack: drag to reorder, switch to
    /// enable, click a name to open its card.
    pub(super) fn effects_summary(&mut self, clip: &Clip, cx: &mut Context<Self>) -> Div {
        let clip_id = clip.id;
        let header = self.header("effects-list", "Effects", cx).child(
            self.add_effect_menu(
                clip_id,
                Button::new("add-effect-list")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .tooltip("Add effect"),
                cx,
            ),
        );
        let rows: Vec<AnyElement> = if self.is_open("effects-list") {
            clip.effects
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let name = self.effect_name(clip_id, e, cx);
                    let id = e.id;
                    let enabled = e.enabled;
                    let row = h_flex()
                        .id(SharedString::from(format!("effect-row-{id}")))
                        .h(px(26.))
                        .gap(px(8.))
                        .pl(px(22.))
                        .pr(px(10.))
                        .cursor_grab()
                        .child(Icon::new(IconName::GripVertical).size(px(12.)).color(t::T3))
                        .child(
                            div()
                                .id(SharedString::from(format!("effect-open-{id}")))
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .cursor_pointer()
                                .text_color(if enabled { t::T1 } else { t::T2 })
                                .hover(|s| s.text_color(t::T1))
                                .child(name.clone())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.collapsed_fx.remove(&id);
                                    this.picture_tab = PictureTab::Effects;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Switch::new(SharedString::from(format!("effect-on-{id}")))
                                .checked(enabled)
                                .on_click(cx.listener(move |this, _: &bool, _, cx| {
                                    this.toggle_effect(clip_id, id, cx)
                                })),
                        );
                    self.reorderable(row, clip_id, i, id, &name, cx)
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };
        group(header, rows)
    }

    /// Drag `row` to move its effect, or into the chat; drop another
    /// effect of the clip on it to move that one here.
    fn reorderable(
        &self,
        row: Stateful<Div>,
        clip: ClipId,
        index: usize,
        effect: EffectId,
        name: &str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let drag = RefDrag {
            refs: vec![effect_mention(clip, effect, name)],
        };
        row.on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
            .drag_over::<RefDrag>(move |s, d: &RefDrag, _, _| {
                if moves_effect(d, clip).is_some() {
                    s.bg(t::alpha(t::AC, 0.12))
                } else {
                    s
                }
            })
            .on_drop(cx.listener(move |this, d: &RefDrag, _, cx| {
                if let Some(moved) = moves_effect(d, clip) {
                    this.move_effect(clip, &moved, index, cx);
                }
            }))
    }

    fn effect_name(&self, clip: ClipId, effect: &FxInstance, cx: &App) -> String {
        self.fx_target(
            &FxOwner::Effect {
                clip,
                effect: effect.id,
            },
            cx,
        )
        .map(|t| t.asset.name)
        .unwrap_or_else(|| effect.fx.to_string())
    }

    fn pick_effect(&mut self, clip: ClipId, pick: &Pick, cx: &mut Context<Self>) {
        match pick {
            Pick::Fx(r) => self.add_effect(clip, r.clone(), cx),
            Pick::Program(id) => {
                let r = self
                    .state
                    .update(cx, |s, cx| shared::use_program(s, *id, cx));
                if let Some(r) = r {
                    self.add_effect(clip, r, cx);
                }
            }
        }
    }

    /// Save effects of `clip`'s stack as a library preset.
    fn save_preset(&mut self, clip: ClipId, only: Option<EffectId>, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let Some((_, c)) = s.bus.project().timeline.clip(clip) else {
                return;
            };
            let effects = c
                .effects
                .iter()
                .filter(|e| only.is_none_or(|id| e.id == id))
                .cloned()
                .collect();
            let preset = shared::effects_preset(s, effects);
            shared::save_preset(s, preset, cx);
        });
    }

    fn add_effect(&mut self, clip: ClipId, fx: FxRef, cx: &mut Context<Self>) {
        self.run(
            vec![Command::AddEffect {
                clips: vec![clip],
                effect: FxInstance::new(fx),
                index: None,
            }],
            "add_effect",
            cx,
        );
    }

    /// `button` opening every effect program: built-ins by category, then
    /// the project's and the library's.
    fn add_effect_menu(
        &self,
        clip: ClipId,
        button: Button,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let mut effects: Vec<(String, String, Pick)> = fx::library(self.project(cx))
            .filter(|a| a.kind == FxKind::Effect)
            .map(|a| {
                let group = match a.id {
                    FxRef::Builtin(_) => a.category.clone(),
                    FxRef::Project(_) => "Project".into(),
                };
                (group, a.name.clone(), Pick::Fx(a.id.clone()))
            })
            .collect();
        let s = self.state.read(cx);
        effects.extend(
            shared::programs_to_add(s.bus.project(), &s.library, FxKind::Effect)
                .into_iter()
                .map(|(id, name)| ("Library".into(), name, Pick::Program(id))),
        );
        let panel = cx.entity();
        button.dropdown_menu(move |mut menu, _, _| {
            let mut group: Option<&str> = None;
            for (g, name, pick) in &effects {
                if group != Some(g.as_str()) {
                    menu = menu.separator();
                    if !g.is_empty() {
                        menu = menu.item(PopupMenuItem::label(g.clone()));
                    }
                    group = Some(g);
                }
                let (panel, pick) = (panel.clone(), pick.clone());
                menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                    panel.update(cx, |this, cx| this.pick_effect(clip, &pick, cx))
                }));
            }
            menu.max_h(px(420.))
        })
    }

    /// Effect presets from the library to apply, and saving the stack as
    /// one.
    fn presets_menu(&self, clip: &Clip, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let presets: Vec<(LibraryId, String)> = self
            .state
            .read(cx)
            .library
            .presets
            .iter()
            .filter(|p| !p.effects.is_empty())
            .map(|p| (p.id, p.name.clone()))
            .collect();
        let (panel, id, any) = (cx.entity(), clip.id, !clip.effects.is_empty());
        Button::new("effect-presets")
            .ghost()
            .small()
            .label("Presets")
            .trailing_icon(IconName::ChevronDown)
            .dropdown_menu(move |mut menu, _, _| {
                for (preset, name) in &presets {
                    let (panel, preset) = (panel.clone(), *preset);
                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                        panel.update(cx, |this, cx| {
                            this.state.update(cx, |s, cx| {
                                if let Some(p) = s.library.preset(preset).cloned() {
                                    shared::apply_preset_to(s, &p, &[id], &[], cx);
                                }
                            })
                        })
                    }));
                }
                let panel = panel.clone();
                menu.separator().item(
                    PopupMenuItem::new("Save All as Preset")
                        .disabled(!any)
                        .on_click(move |_, _, cx| {
                            panel.update(cx, |this, cx| this.save_preset(id, None, cx))
                        }),
                )
            })
    }

    fn effect_card(
        &mut self,
        clip: ClipId,
        index: usize,
        effect: &FxInstance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = FxOwner::Effect {
            clip,
            effect: effect.id,
        };
        let target = self.fx_target(&owner, cx);
        let name = target
            .as_ref()
            .map(|t| t.asset.name.clone())
            .unwrap_or_else(|| effect.fx.to_string());
        let collapsed = self.collapsed_fx.contains(&effect.id);
        let enabled = effect.enabled;
        let id = effect.id;
        let header = h_flex()
            .id(SharedString::from(format!("effect-{id}")))
            .h(px(30.))
            .pl(px(8.))
            .pr(px(4.))
            .gap(px(6.))
            .cursor_grab()
            .child(Icon::new(IconName::GripVertical).size(px(12.)).color(t::T3))
            .child(
                div()
                    .id(SharedString::from(format!("collapse-{id}")))
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(t::T2)
                    .hover(|s| s.text_color(t::T1))
                    .child(
                        Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .size(px(12.)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed_fx.remove(&id) {
                            this.collapsed_fx.insert(id);
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if enabled { t::T1 } else { t::T3 })
                    .child(name.clone()),
            )
            .children(self.focus_pick(clip, id, target.as_ref(), cx))
            .child(
                Switch::new(SharedString::from(format!("enable-{id}")))
                    .checked(enabled)
                    .on_click(cx.listener(move |this, _: &bool, _, cx| {
                        this.toggle_effect(clip, id, cx);
                    })),
            )
            .child(self.effect_menu(clip, index, effect, &name, cx));
        let header = self.reorderable(header, clip, index, id, &name, cx);
        let body = match (&target, collapsed) {
            (Some(t), false) => Some(self.effect_body(t, enabled, window, cx)),
            _ => None,
        };
        v_flex()
            .rounded(px(6.))
            .border_1()
            .border_color(t::LN)
            .bg(t::alpha(t::R1, 0.5))
            .child(header)
            .children(body)
            .into_any_element()
    }

    /// For an effect with a depth focus: pick it with the next click in the
    /// viewer.
    fn focus_pick(
        &self,
        clip: ClipId,
        effect: EffectId,
        target: Option<&FxTarget>,
        cx: &mut Context<Self>,
    ) -> Option<Button> {
        let asset = &target?.asset;
        if !koo_core::depth::needs_depth(asset) || asset.param(fx::depth::FOCUS).is_none() {
            return None;
        }
        let armed = self.state.read(cx).depth_pick == Some((clip, effect));
        Some(
            Button::new(SharedString::from(format!("focus-{effect}")))
                .ghost()
                .xsmall()
                .icon(IconName::Focus)
                .selected(armed)
                .accent(armed)
                .tooltip("Pick focus")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |s, cx| {
                        s.depth_pick = if armed { None } else { Some((clip, effect)) };
                        cx.notify();
                    });
                })),
        )
    }

    /// Parameters, then the code and a reset.
    fn effect_body(
        &mut self,
        target: &FxTarget,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let rows = self.fx_param_rows(target, window, cx);
        let fx = target.asset.id.clone();
        let builtin = matches!(fx, FxRef::Builtin(_));
        let owner = target.owner.clone();
        let default = target.instance.params.is_empty() && target.instance.keyframes.is_empty();
        let state = self.state.clone();
        let id = target.instance.id;
        v_flex()
            .pb(px(4.))
            .child(v_flex().when(!enabled, |d| d.opacity(0.5)).children(rows))
            .child(
                h_flex()
                    .gap(px(2.))
                    .pl(px(16.))
                    .pt(px(2.))
                    .child(
                        Button::new(SharedString::from(format!("code-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Code)
                            .label(if builtin { "View code" } else { "Edit code" })
                            .on_click(move |_, _, cx| {
                                state.update(cx, |s, cx| s.edit_fx(Some(fx.clone()), cx))
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("reset-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::RotateCcw)
                            .label("Reset")
                            .disabled(default)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let Some(t) = this.fx_target(&owner, cx) else {
                                    return;
                                };
                                let mut instance = t.instance.clone();
                                instance.params.clear();
                                instance.keyframes.clear();
                                this.run(vec![t.store(instance)], "set_effect", cx);
                            })),
                    ),
            )
    }

    fn toggle_effect(&mut self, clip: ClipId, effect: EffectId, cx: &mut Context<Self>) {
        let Some(t) = self.fx_target(&FxOwner::Effect { clip, effect }, cx) else {
            return;
        };
        let mut instance = t.instance.clone();
        instance.enabled = !instance.enabled;
        self.run(vec![t.store(instance)], "set_effect", cx);
    }

    /// Move the effect whose short id is `moved` to `index`.
    fn move_effect(&mut self, clip: ClipId, moved: &str, index: usize, cx: &mut Context<Self>) {
        let effect = self
            .project(cx)
            .timeline
            .clip(clip)
            .and_then(|(_, c)| c.effects.iter().find(|e| e.id.to_string() == moved))
            .map(|e| e.id);
        if let Some(effect) = effect {
            self.run(
                vec![Command::MoveEffect {
                    clip,
                    effect,
                    index,
                }],
                "move_effect",
                cx,
            );
        }
    }

    fn effect_menu(
        &self,
        clip: ClipId,
        index: usize,
        effect: &FxInstance,
        name: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let (panel, state) = (cx.entity(), self.state.clone());
        let id = effect.id;
        let count = self
            .project(cx)
            .timeline
            .clip(clip)
            .map_or(0, |(_, c)| c.effects.len());
        let fx = effect.fx.clone();
        let mention = effect_mention(clip, id, name);
        let builtin = matches!(fx, FxRef::Builtin(_));
        Button::new(SharedString::from(format!("effect-menu-{id}")))
            .ghost()
            .xsmall()
            .icon(IconName::Ellipsis)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu: PopupMenu, _, _| {
                let mv = |label: &'static str, to: usize, enabled: bool| {
                    let panel = panel.clone();
                    PopupMenuItem::new(label)
                        .disabled(!enabled)
                        .on_click(move |_, _, cx| {
                            panel.update(cx, |this, cx| {
                                this.run(
                                    vec![Command::MoveEffect {
                                        clip,
                                        effect: id,
                                        index: to,
                                    }],
                                    "move_effect",
                                    cx,
                                )
                            })
                        })
                };
                let (p_one, p_remove) = (panel.clone(), panel.clone());
                let (s_chat, s_dup) = (state.clone(), state.clone());
                let (m, f_dup) = (mention.clone(), fx.clone());
                let mut menu = menu
                    .item(mv("Move Up", index.saturating_sub(1), index > 0))
                    .item(mv("Move Down", index + 1, index + 1 < count))
                    .separator()
                    .item(
                        PopupMenuItem::new("Add to Chat")
                            .icon(IconName::MessageSquarePlus)
                            .on_click(move |_, _, cx| {
                                s_chat.update(cx, |s, cx| s.mention(vec![m.clone()], cx))
                            }),
                    );
                if builtin {
                    menu = menu.item(PopupMenuItem::new("Duplicate to Edit").on_click(
                        move |_, _, cx| {
                            s_dup.update(cx, |s, cx| crate::ui::library::duplicate(s, &f_dup, cx))
                        },
                    ));
                }
                menu.item(
                    PopupMenuItem::new("Save as Preset").on_click(move |_, _, cx| {
                        p_one.update(cx, |this, cx| this.save_preset(clip, Some(id), cx))
                    }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Remove")
                        .icon(IconName::Trash)
                        .destructive()
                        .on_click(move |_, _, cx| {
                            p_remove.update(cx, |this, cx| {
                                this.run(
                                    vec![Command::RemoveEffect { clip, effect: id }],
                                    "remove_effect",
                                    cx,
                                )
                            })
                        }),
                )
            })
    }
}

/// An entry of the add menu.
#[derive(Clone)]
enum Pick {
    Fx(FxRef),
    Program(LibraryId),
}

/// The short id of an effect of `clip` that `drag` carries, if any.
fn moves_effect(drag: &RefDrag, clip: ClipId) -> Option<String> {
    let [(token, _)] = drag.refs.as_slice() else {
        return None;
    };
    let rest = token.strip_prefix("@effect ")?;
    let (c, e) = rest.split_once('/')?;
    (c == clip.to_string()).then(|| e.to_owned())
}
