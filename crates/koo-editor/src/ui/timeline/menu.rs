//! Right-click menus on clips, transitions, lanes and the ruler, and the
//! small Change Speed / Rename popups they open.

use super::{MenuTarget, TimelinePanel, refs, transition_of};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::ClipId;
use koo_core::command::Command;
use koo_core::model::ClipColor;

#[derive(Clone, Copy, PartialEq)]
pub enum PopupKind {
    Speed { ripple: bool },
    Rename,
}

pub struct Popup {
    clip: ClipId,
    kind: PopupKind,
    input: Entity<InputState>,
    /// Window position it opens at.
    at: Point<Pixels>,
    _enter: Subscription,
}

impl TimelinePanel {
    pub(super) fn open_popup(
        &mut self,
        kind: PopupKind,
        clip: ClipId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = {
            let project = self.state.read(cx).bus.project();
            let Some((_, c)) = project.timeline.clip(clip) else {
                return;
            };
            match kind {
                PopupKind::Speed { .. } => format!("{}", (c.speed * 100.).round()),
                PopupKind::Rename => c.display_name(project),
            }
        };
        let input = cx.new(|cx| {
            let state = InputState::new(window, cx).default_value(value);
            match kind {
                PopupKind::Speed { .. } => state,
                PopupKind::Rename => state.placeholder("Name"),
            }
        });
        let _enter = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.apply_popup(window, cx);
            }
        });
        input.update(cx, |i, cx| i.focus(window, cx));
        self.popup = Some(Popup {
            clip,
            kind,
            input,
            at: self.menu_at,
            _enter,
        });
        cx.notify();
    }

    fn close_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    fn apply_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = &self.popup else {
            return;
        };
        let text = popup.input.read(cx).value().to_string();
        let command = match popup.kind {
            PopupKind::Speed { ripple } => {
                let Ok(percent) = text.trim().trim_end_matches('%').trim().parse::<f64>() else {
                    return;
                };
                if !(percent.is_finite() && percent > 0.) {
                    return;
                }
                Command::SetSpeed {
                    clip: popup.clip,
                    speed: percent / 100.,
                    ripple,
                }
            }
            PopupKind::Rename => Command::RenameClip {
                clip: popup.clip,
                name: Some(text),
            },
        };
        self.state.update(cx, |s, cx| {
            s.edit(command, cx);
        });
        self.close_popup(window, cx);
    }

    pub(super) fn render_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let popup = self.popup.as_ref()?;
        let theme = cx.theme();
        let speed = match popup.kind {
            PopupKind::Speed { ripple } => Some(ripple),
            PopupKind::Rename => None,
        };
        let card = v_flex()
            .id("clip-popup")
            .w(px(if speed.is_some() { 180. } else { 220. }))
            .p_2()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_popup(window, cx)))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    this.close_popup(window, cx);
                }
            }))
            .child(
                Input::new(&popup.input)
                    .small()
                    .when(speed.is_some(), |i| i.suffix("%")),
            )
            .when_some(speed, |el, ripple| {
                el.child(
                    h_flex()
                        .justify_between()
                        .child(
                            Switch::new("speed-ripple")
                                .small()
                                .label("Ripple")
                                .checked(ripple)
                                .on_click(cx.listener(|this, on: &bool, _, cx| {
                                    if let Some(p) = &mut this.popup {
                                        p.kind = PopupKind::Speed { ripple: *on };
                                        cx.notify();
                                    }
                                })),
                        )
                        .child(
                            Button::new("speed-apply")
                                .primary()
                                .xsmall()
                                .label("Apply")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.apply_popup(window, cx)),
                                ),
                        ),
                )
            });
        Some(
            deferred(
                anchored()
                    .position(popup.at)
                    .snap_to_window_with_margin(px(8.))
                    .child(card),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

/// What the menu shows, read from the panel before it is built.
enum Content {
    Clip {
        id: ClipId,
        locked: bool,
        enabled: bool,
        media: bool,
        selected: Vec<ClipId>,
        /// The selection can be synced by sound; `true` when it holds a
        /// recorder and a camera.
        sync: Option<bool>,
    },
    Transition((ClipId, koo_core::command::Edge), refs::Ref),
    Lane {
        at: koo_core::Time,
        can_paste: bool,
        in_out: Option<koo_core::TimeRange>,
    },
    Ruler(refs::Ref, Option<koo_core::TimeRange>),
}

fn content(panel: &Entity<TimelinePanel>, cx: &App) -> Option<Content> {
    let this = panel.read(cx);
    let state = this.state.read(cx);
    let project = state.bus.project();
    let tl = &project.timeline;
    Some(match this.menu_target? {
        MenuTarget::Clip(id) => {
            let (track, clip) = tl.clip(id)?;
            Content::Clip {
                id,
                locked: track.locked,
                enabled: clip.enabled,
                media: clip.media().is_some(),
                selected: state.selection.clips.clone(),
                sync: (!state.sync_recordings(&state.selection.clips).is_empty())
                    .then(|| state.sync_has_recorder(&state.selection.clips)),
            }
        }
        MenuTarget::Transition(owner) => {
            transition_of(project, owner)?;
            Content::Transition(owner, refs::transition(owner.0, owner.1, project))
        }
        MenuTarget::Lane(at) => Content::Lane {
            at,
            can_paste: !this.clipboard.is_empty(),
            in_out: state.in_out,
        },
        MenuTarget::Ruler(at, marker) => {
            let first = match marker.and_then(|m| tl.markers.iter().find(|x| x.id == m)) {
                Some(m) => refs::marker(m),
                None => refs::time(at),
            };
            Content::Ruler(first, state.in_out)
        }
    })
}

/// The context menu for whatever the last right click hit.
pub fn build(
    panel: &Entity<TimelinePanel>,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(content) = content(panel, cx) else {
        return menu;
    };
    match content {
        Content::Clip {
            id,
            locked,
            enabled,
            media,
            selected,
            sync,
        } => {
            let open = |kind: PopupKind| {
                let panel = panel.clone();
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    panel.update(cx, |this, cx| this.open_popup(kind, id, window, cx))
                }
            };
            let colors = panel.clone();
            let toggle = panel.clone();
            let mention = panel.clone();
            let menu = match sync {
                Some(recorder) => sync_items(menu, panel, selected.clone(), recorder),
                None => menu,
            };
            menu.item(
                PopupMenuItem::new("Change Speed…")
                    .disabled(locked || !media)
                    .on_click(open(PopupKind::Speed { ripple: false })),
            )
            .submenu("Clip Color", window, cx, move |menu, _, _| {
                color_items(menu, &colors)
            })
            .item(
                PopupMenuItem::new("Rename…")
                    .disabled(locked)
                    .on_click(open(PopupKind::Rename)),
            )
            .item(
                PopupMenuItem::new(if enabled { "Disable" } else { "Enable" })
                    .disabled(locked)
                    .on_click(move |_, _, cx| {
                        let clips = selected.clone();
                        toggle.update(cx, |this, cx| this.set_enabled(clips, !enabled, cx))
                    }),
            )
            .separator()
            .item(PopupMenuItem::new("Add to chat").on_click(move |_, _, cx| {
                mention.update(cx, |this, cx| this.mention_selection(cx))
            }))
        }
        Content::Transition(owner, reference) => {
            let mention = panel.clone();
            let remove = panel.clone();
            menu.item(PopupMenuItem::new("Delete").on_click(move |_, _, cx| {
                remove.update(cx, |this, cx| this.remove_transition(owner, cx))
            }))
            .separator()
            .item(PopupMenuItem::new("Add to chat").on_click(move |_, _, cx| {
                let r = reference.clone();
                mention.update(cx, |this, cx| this.mention(vec![r], cx))
            }))
        }
        Content::Lane {
            at,
            can_paste,
            in_out,
        } => {
            let paste = panel.clone();
            let menu = menu.item(PopupMenuItem::new("Paste").disabled(!can_paste).on_click(
                move |_, _, cx| {
                    paste.update(cx, |this, cx| {
                        this.state.update(cx, |s, cx| s.seek(at, cx));
                        this.paste(cx);
                    })
                },
            ));
            mention_items(menu.separator(), panel, vec![refs::time(at)], in_out)
        }
        Content::Ruler(first, in_out) => mention_items(menu, panel, vec![first], in_out),
    }
}

/// "Sync by Audio" for the selected clips, and keeping only the
/// recorder's sound when there is one.
fn sync_items(
    menu: PopupMenu,
    panel: &Entity<TimelinePanel>,
    clips: Vec<ClipId>,
    recorder: bool,
) -> PopupMenu {
    let item = |label: &'static str, mute: bool| {
        let panel = panel.clone();
        let clips = clips.clone();
        PopupMenuItem::new(label).on_click(move |_, _, cx| {
            let clips = clips.clone();
            panel.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| s.sync_clips(clips, mute, cx))
            })
        })
    };
    let menu = menu.item(item("Sync by Audio", false));
    let menu = if recorder {
        menu.item(item("Sync by Audio, Recorder Sound", true))
    } else {
        menu
    };
    menu.separator()
}

/// "Add to chat" for `refs`, plus the in/out range when one is marked.
fn mention_items(
    menu: PopupMenu,
    panel: &Entity<TimelinePanel>,
    refs: Vec<refs::Ref>,
    in_out: Option<koo_core::TimeRange>,
) -> PopupMenu {
    let mention = panel.clone();
    let menu = menu.item(PopupMenuItem::new("Add to chat").on_click(move |_, _, cx| {
        let r = refs.clone();
        mention.update(cx, |this, cx| this.mention(r, cx))
    }));
    match in_out.filter(|r| !r.is_empty()) {
        Some(range) => {
            let mention = panel.clone();
            menu.item(
                PopupMenuItem::new("Add In/Out to chat").on_click(move |_, _, cx| {
                    mention.update(cx, |this, cx| this.mention(vec![refs::range(range)], cx))
                }),
            )
        }
        None => menu,
    }
}

fn color_items(menu: PopupMenu, panel: &Entity<TimelinePanel>) -> PopupMenu {
    let none = panel.clone();
    let menu = menu
        .item(
            PopupMenuItem::new("None")
                .on_click(move |_, _, cx| none.update(cx, |this, cx| this.set_color(None, cx))),
        )
        .separator();
    ClipColor::ALL.into_iter().fold(menu, |menu, color| {
        let panel = panel.clone();
        menu.item(
            PopupMenuItem::element(move |_, _| {
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().size(px(10.)).rounded_sm().bg(rgb(color.rgb())))
                    .child(color.label())
            })
            .on_click(move |_, _, cx| panel.update(cx, |this, cx| this.set_color(Some(color), cx))),
        )
    })
}
