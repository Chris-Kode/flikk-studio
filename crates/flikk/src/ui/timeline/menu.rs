//! Right-click menus on clips, transitions, lanes and the ruler, and the
//! Change speed / Rename / Fit to duration popovers they open.

use super::{
    DeleteSelection, MenuTarget, PasteClips, RippleDeleteSelection, TimelinePanel, ToggleEnabled,
    refs, transition_of,
};
use crate::ui::kit::{
    Button, IconName, Input, PopoverPanel, PopupMenu, PopupMenuItem, Switch, h_flex, popover,
    theme as t,
};
use flikk_core::ClipId;
use flikk_core::command::Command;
use flikk_core::model::ClipColor;
use flikk_core::music_fit::Target;
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

#[derive(Clone, Copy, PartialEq)]
pub enum PopupKind {
    Speed {
        ripple: bool,
    },
    Rename,
    /// Fit a music clip to a typed length.
    FitDuration,
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
                PopupKind::FitDuration => c.duration.to_string(),
            }
        };
        let input = cx.new(|cx| {
            let state = InputState::new(window, cx).default_value(value);
            match kind {
                PopupKind::Speed { .. } | PopupKind::FitDuration => state,
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

    /// Close the popup once its clip is gone (undone, or removed by the
    /// chat).
    pub(super) fn close_stale_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let gone = self.popup.as_ref().is_some_and(|p| {
            let project = self.state.read(cx).bus.project();
            project.timeline.clip(p.clip).is_none()
        });
        if gone {
            self.close_popup(window, cx);
        }
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
            PopupKind::FitDuration => {
                let Some(length) =
                    flikk_core::Time::parse_clock(&text).filter(|t| *t > flikk_core::Time::ZERO)
                else {
                    return;
                };
                let clip = popup.clip;
                self.state.update(cx, |s, cx| {
                    s.fit_music(clip, Target::Duration(length), cx);
                });
                self.close_popup(window, cx);
                return;
            }
        };
        self.state.update(cx, |s, cx| {
            s.edit(command, cx);
        });
        self.close_popup(window, cx);
    }

    pub(super) fn render_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let popup = self.popup.as_ref()?;
        let (title, field) = match popup.kind {
            PopupKind::Speed { .. } => ("Change speed", Some("Speed")),
            PopupKind::Rename => ("Rename", None),
            PopupKind::FitDuration => ("Fit to duration", Some("Duration")),
        };
        let input = Input::new(&popup.input)
            .small()
            .when(matches!(popup.kind, PopupKind::Speed { .. }), |i| {
                i.suffix("%")
            });
        let row = |label: &'static str, control: AnyElement| {
            h_flex()
                .min_h(px(26.))
                .gap(px(8.))
                .child(
                    div()
                        .w(px(72.))
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(t::T2)
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(control))
        };
        let panel = PopoverPanel::new(title)
            .w(px(260.))
            .on_close(cx.listener(|this, _, window, cx| this.close_popup(window, cx)))
            .child(match field {
                Some(label) => row(label, input.into_any_element()).into_any_element(),
                None => input.into_any_element(),
            })
            .when_some(
                match popup.kind {
                    PopupKind::Speed { ripple } => Some(ripple),
                    PopupKind::Rename | PopupKind::FitDuration => None,
                },
                |panel, ripple| {
                    panel.child(row(
                        "Ripple",
                        Switch::new("speed-ripple")
                            .checked(ripple)
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                if let Some(p) = &mut this.popup {
                                    p.kind = PopupKind::Speed { ripple: *on };
                                    cx.notify();
                                }
                            }))
                            .into_any_element(),
                    ))
                },
            )
            .button(
                Button::new("popup-cancel")
                    .ghost()
                    .label("Cancel")
                    .on_click(cx.listener(|this, _, window, cx| this.close_popup(window, cx))),
            )
            .button(
                Button::new("popup-apply")
                    .primary()
                    .label("Apply")
                    .kbd("Enter")
                    .on_click(cx.listener(|this, _, window, cx| this.apply_popup(window, cx))),
            );
        let card = popover::surface()
            .id("clip-popup")
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_popup(window, cx)))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    this.close_popup(window, cx);
                }
            }))
            .child(panel);
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
        /// A media clip on an audio track, which can be fitted to a length.
        music: bool,
        in_out: Option<flikk_core::TimeRange>,
    },
    Transition((ClipId, flikk_core::command::Edge), refs::Ref),
    Lane {
        at: flikk_core::Time,
        can_paste: bool,
        in_out: Option<flikk_core::TimeRange>,
    },
    Ruler(refs::Ref, Option<flikk_core::TimeRange>),
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
                music: clip.media().is_some() && track.kind == flikk_core::model::TrackKind::Audio,
                in_out: state.in_out.filter(|r| !r.is_empty()),
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
            music,
            in_out,
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
            let fit = panel.clone();
            let fit_duration = open(PopupKind::FitDuration);
            let menu = menu.item(
                PopupMenuItem::new("Add to chat")
                    .icon(IconName::MessageSquarePlus)
                    .on_click(move |_, _, cx| {
                        mention.update(cx, |this, cx| this.mention_selection(cx))
                    }),
            );
            let menu = in_out_item(menu, panel, in_out)
                .separator()
                .item(
                    PopupMenuItem::new("Rename…")
                        .icon(IconName::Pencil)
                        .disabled(locked)
                        .on_click(open(PopupKind::Rename)),
                )
                .submenu("Clip color", window, cx, move |menu, _, _| {
                    color_items(menu, &colors)
                })
                .item(
                    PopupMenuItem::new("Change speed…")
                        .icon(IconName::Gauge)
                        .disabled(locked || !media)
                        .on_click(open(PopupKind::Speed { ripple: false })),
                );
            let menu = if music {
                menu.submenu("Fit to", window, cx, move |menu, _, _| {
                    fit_items(menu, &fit, id, locked, in_out, fit_duration.clone())
                })
            } else {
                menu
            };
            let menu = match sync {
                Some(recorder) => sync_items(menu, panel, selected.clone(), recorder),
                None => menu,
            };
            let ripple = panel.clone();
            let remove = panel.clone();
            menu.item(
                PopupMenuItem::new(if enabled { "Disable" } else { "Enable" })
                    .icon(if enabled {
                        IconName::EyeOff
                    } else {
                        IconName::Eye
                    })
                    .action(Box::new(ToggleEnabled))
                    .disabled(locked)
                    .on_click(move |_, _, cx| {
                        let clips = selected.clone();
                        toggle.update(cx, |this, cx| this.set_enabled(clips, !enabled, cx))
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new("Ripple delete")
                    .action(Box::new(RippleDeleteSelection))
                    .disabled(locked)
                    .on_click(move |_, _, cx| {
                        ripple.update(cx, |this, cx| this.delete_selection(true, cx))
                    }),
            )
            .item(
                PopupMenuItem::new("Delete")
                    .icon(IconName::Trash)
                    .action(Box::new(DeleteSelection))
                    .destructive()
                    .disabled(locked)
                    .on_click(move |_, _, cx| {
                        remove.update(cx, |this, cx| this.delete_selection(false, cx))
                    }),
            )
        }
        Content::Transition(owner, reference) => {
            let remove = panel.clone();
            mention_items(menu, panel, vec![reference], None)
                .separator()
                .item(
                    PopupMenuItem::new("Delete")
                        .icon(IconName::Trash)
                        .destructive()
                        .on_click(move |_, _, cx| {
                            remove.update(cx, |this, cx| this.remove_transition(owner, cx))
                        }),
                )
        }
        Content::Lane {
            at,
            can_paste,
            in_out,
        } => {
            let paste = panel.clone();
            let menu = menu.item(
                PopupMenuItem::new("Paste")
                    .icon(IconName::ClipboardPaste)
                    .action(Box::new(PasteClips))
                    .disabled(!can_paste)
                    .on_click(move |_, _, cx| {
                        paste.update(cx, |this, cx| {
                            this.state.update(cx, |s, cx| s.seek(at, cx));
                            this.paste(cx);
                        })
                    }),
            );
            mention_items(menu.separator(), panel, vec![refs::time(at)], in_out)
        }
        Content::Ruler(first, in_out) => mention_items(menu, panel, vec![first], in_out),
    }
}

/// "Sync by audio" for the selected clips, and keeping only the
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
    let menu = menu.item(item("Sync by audio", false).icon(IconName::AudioWaveform));
    if recorder {
        menu.item(item("Sync by audio, recorder sound", true))
    } else {
        menu
    }
}

/// "Add to chat" for `refs`, plus the in/out range when one is marked.
fn mention_items(
    menu: PopupMenu,
    panel: &Entity<TimelinePanel>,
    refs: Vec<refs::Ref>,
    in_out: Option<flikk_core::TimeRange>,
) -> PopupMenu {
    let mention = panel.clone();
    let menu = menu.item(
        PopupMenuItem::new("Add to chat")
            .icon(IconName::MessageSquarePlus)
            .on_click(move |_, _, cx| {
                let r = refs.clone();
                mention.update(cx, |this, cx| this.mention(r, cx))
            }),
    );
    in_out_item(menu, panel, in_out)
}

/// "Add In/Out to chat" when a range is marked.
fn in_out_item(
    menu: PopupMenu,
    panel: &Entity<TimelinePanel>,
    in_out: Option<flikk_core::TimeRange>,
) -> PopupMenu {
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

/// Targets a music clip can be fitted to.
fn fit_items(
    menu: PopupMenu,
    panel: &Entity<TimelinePanel>,
    clip: ClipId,
    locked: bool,
    in_out: Option<flikk_core::TimeRange>,
    duration: impl Fn(&ClickEvent, &mut Window, &mut App) + Clone + 'static,
) -> PopupMenu {
    let item = |label: &'static str, target: Option<Target>| {
        let panel = panel.clone();
        PopupMenuItem::new(label)
            .disabled(locked || target.is_none())
            .on_click(move |_, _, cx| {
                if let Some(target) = target {
                    panel.update(cx, |this, cx| {
                        this.state.update(cx, |s, cx| s.fit_music(clip, target, cx))
                    })
                }
            })
    };
    menu.item(item("Sequence End", Some(Target::SequenceEnd)))
        .item(item("In/Out", in_out.map(Target::Range)))
        .item(
            PopupMenuItem::new("Duration…")
                .disabled(locked)
                .on_click(duration),
        )
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
                    .gap(px(8.))
                    .child(div().size(px(10.)).rounded(px(3.)).bg(rgb(color.rgb())))
                    .child(color.label())
            })
            .on_click(move |_, _, cx| panel.update(cx, |this, cx| this.set_color(Some(color), cx))),
        )
    })
}
