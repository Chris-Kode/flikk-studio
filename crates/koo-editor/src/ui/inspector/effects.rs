//! A picture clip's effect stack: add, enable, reorder (drag a row onto
//! another), collapse, remove, and every parameter with keyframes. Rows drag
//! into the chat as `@effect <clip>/<effect>` references.

use super::controls::{FxOwner, FxTarget};
use super::{InspectorPanel, section_with};
use crate::ui::library::{FxGlyph, fx_glyph, shared};
use crate::ui::timeline::glyph::{Glyph, glyph};
use crate::ui::{FxDrag, RefDrag};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
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
    pub(super) fn effects_view(
        &mut self,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let clip_id = clip.id;
        let add = self.add_effect_button(clip_id, cx);
        let mut list = v_flex().gap_2();
        for (i, e) in clip.effects.iter().enumerate() {
            list = list.child(self.effect_card(clip_id, i, e, window, cx));
        }
        let theme = cx.theme();
        let (muted, drop) = (theme.muted_foreground, theme.drop_target);
        let empty = clip.effects.is_empty();
        section_with("Effects", add, cx)
            .id("effects")
            .drag_over::<FxDrag>(move |s, d: &FxDrag, _, _| {
                if d.kind == FxKind::Effect {
                    s.bg(drop)
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
            .rounded_md()
            .when(empty, |d| {
                d.child(div().py_6().flex().justify_center().child(fx_glyph(
                    FxGlyph::Effect,
                    px(16.),
                    muted.opacity(0.6),
                )))
            })
            .child(list)
            .into_any_element()
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
            Pick::Preset(id) => self.state.update(cx, |s, cx| {
                if let Some(preset) = s.library.preset(*id).cloned() {
                    shared::apply_preset_to(s, &preset, &[clip], &[], cx);
                }
            }),
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

    /// "+" with every effect program, built-ins by category, then the
    /// project's and the library's, then the effect presets.
    fn add_effect_button(&self, clip: ClipId, cx: &mut Context<Self>) -> AnyElement {
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
        effects.extend(
            s.library
                .presets
                .iter()
                .filter(|p| !p.effects.is_empty())
                .map(|p| ("Presets".into(), p.name.clone(), Pick::Preset(p.id))),
        );
        let panel = cx.entity();
        Button::new("add-effect")
            .ghost()
            .xsmall()
            .icon(IconName::Plus)
            .tooltip("Add effect")
            .dropdown_menu(move |mut menu, _, _| {
                let mut group: Option<&str> = None;
                for (g, name, pick) in &effects {
                    if group != Some(g.as_str()) {
                        if group.is_some() {
                            menu = menu.item(PopupMenuItem::separator());
                        }
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
                menu
            })
            .into_any_element()
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
        let theme = cx.theme();
        let (muted, fg, border, hover, drop) = (
            theme.muted_foreground,
            theme.foreground,
            theme.border,
            theme.secondary_hover,
            theme.drop_target,
        );
        let id = effect.id;
        let drag = RefDrag {
            refs: vec![effect_mention(clip, id, &name)],
        };
        let header = h_flex()
            .id(SharedString::from(format!("effect-{id}")))
            .h(px(28.))
            .pl_1()
            .pr_0p5()
            .gap_1()
            .rounded_md()
            .cursor_grab()
            .child(fx_glyph(FxGlyph::Grip, px(12.), muted.opacity(0.7)))
            .child(
                div()
                    .id(SharedString::from(format!("collapse-{id}")))
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(muted)
                    .hover(|s| s.text_color(fg))
                    .child(
                        Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .xsmall(),
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
                    .text_sm()
                    .truncate()
                    .when(!enabled, |d| d.text_color(muted))
                    .child(name.clone()),
            )
            .children(self.focus_pick(clip, id, target.as_ref(), cx))
            .child(
                Button::new(SharedString::from(format!("enable-{id}")))
                    .ghost()
                    .xsmall()
                    .tooltip(if enabled { "Disable" } else { "Enable" })
                    .child(glyph(
                        if enabled { Glyph::Eye } else { Glyph::EyeOff },
                        px(13.),
                        muted,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_effect(clip, id, cx);
                    })),
            )
            .child(self.effect_menu(clip, index, effect, &name, cx))
            .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
            .drag_over::<RefDrag>(move |s, d: &RefDrag, _, _| {
                if moves_effect(d, clip).is_some() {
                    s.bg(drop)
                } else {
                    s
                }
            })
            .on_drop(cx.listener(move |this, d: &RefDrag, _, cx| {
                if let Some(moved) = moves_effect(d, clip) {
                    this.move_effect(clip, &moved, index, cx);
                }
            }));
        let params = match (&target, collapsed) {
            (Some(t), false) if !t.asset.params.is_empty() => {
                Some(self.effect_params(t, enabled, window, cx))
            }
            _ => None,
        };
        v_flex()
            .rounded_md()
            .border_1()
            .border_color(border)
            .p_1()
            .hover(|s| s.border_color(hover))
            .child(header)
            .children(params)
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
        let theme = cx.theme();
        let color = if armed {
            theme.primary
        } else {
            theme.muted_foreground
        };
        Some(
            Button::new(SharedString::from(format!("focus-{effect}")))
                .ghost()
                .xsmall()
                .toggled(armed)
                .tooltip("Pick focus")
                .child(glyph(Glyph::Focus, px(13.), color))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |s, cx| {
                        s.depth_pick = if armed { None } else { Some((clip, effect)) };
                        cx.notify();
                    });
                })),
        )
    }

    fn effect_params(
        &mut self,
        target: &FxTarget,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = self.fx_param_rows(target, window, cx);
        v_flex()
            .px_1()
            .pt_1()
            .when(!enabled, |d| d.opacity(0.5))
            .children(rows)
            .into_any_element()
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
    ) -> AnyElement {
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
            .dropdown_menu(move |menu, _, _| {
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
                let (p_reset, p_remove) = (panel.clone(), panel.clone());
                let (p_one, p_all) = (panel.clone(), panel.clone());
                let (s_chat, s_code, s_dup) = (state.clone(), state.clone(), state.clone());
                let (m, f_code, f_dup) = (mention.clone(), fx.clone(), fx.clone());
                menu.item(mv("Move up", index.saturating_sub(1), index > 0))
                    .item(mv("Move down", index + 1, index + 1 < count))
                    .item(PopupMenuItem::separator())
                    .item(PopupMenuItem::new("Reset").on_click(move |_, _, cx| {
                        p_reset.update(cx, |this, cx| {
                            let Some(t) = this.fx_target(&FxOwner::Effect { clip, effect: id }, cx)
                            else {
                                return;
                            };
                            let mut instance = t.instance.clone();
                            instance.params.clear();
                            instance.keyframes.clear();
                            this.run(vec![t.store(instance)], "set_effect", cx);
                        })
                    }))
                    .item(PopupMenuItem::new("Add to chat").on_click(move |_, _, cx| {
                        s_chat.update(cx, |s, cx| s.mention(vec![m.clone()], cx))
                    }))
                    .item(
                        PopupMenuItem::new(if builtin { "View code" } else { "Edit code" })
                            .on_click(move |_, _, cx| {
                                s_code.update(cx, |s, cx| s.edit_fx(Some(f_code.clone()), cx))
                            }),
                    )
                    .when(builtin, |menu| {
                        menu.item(PopupMenuItem::new("Duplicate to edit").on_click(
                            move |_, _, cx| {
                                s_dup.update(cx, |s, cx| {
                                    crate::ui::library::duplicate(s, &f_dup, cx)
                                })
                            },
                        ))
                    })
                    .item(PopupMenuItem::separator())
                    .item(
                        PopupMenuItem::new("Save as Preset").on_click(move |_, _, cx| {
                            p_one.update(cx, |this, cx| this.save_preset(clip, Some(id), cx))
                        }),
                    )
                    .when(count > 1, |menu| {
                        menu.item(PopupMenuItem::new("Save All as Preset").on_click(
                            move |_, _, cx| {
                                p_all.update(cx, |this, cx| this.save_preset(clip, None, cx))
                            },
                        ))
                    })
                    .item(PopupMenuItem::separator())
                    .item(PopupMenuItem::new("Remove").on_click(move |_, _, cx| {
                        p_remove.update(cx, |this, cx| {
                            this.run(
                                vec![Command::RemoveEffect { clip, effect: id }],
                                "remove_effect",
                                cx,
                            )
                        })
                    }))
            })
            .into_any_element()
    }
}

/// An entry of the "+" menu.
#[derive(Clone)]
enum Pick {
    Fx(FxRef),
    Program(LibraryId),
    Preset(LibraryId),
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
