//! The viewer's chrome around the picture: the header with zoom and
//! framing, the mask tools, the transport bar and the frame label.

use super::{REFRAME, TRANSPORT_HEIGHT, ViewerPanel, ZOOMS, edit_source, masks};
use crate::state::EditorState;
use crate::ui::format::item_name;
use crate::ui::kit::play::seek_bar;
use crate::ui::kit::{
    Button, DropdownMenu as _, Icon, IconName, PanelHeader, PopupMenuItem, Segmented, Tooltip,
    h_flex, theme as t,
};
use crate::ui::transport;
use flikk_core::Time;
use flikk_core::command::{Command, InsertMode};
use flikk_core::model::{Clip, Fit, Transform};
use flikk_core::query::topmost_video_clip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// `t` as a timecode, without the hours when there are none.
fn short_timecode(t: Time, rate: flikk_core::Rate) -> String {
    let tc = t.timecode(rate);
    match tc.strip_prefix("00:") {
        Some(rest) => rest.to_string(),
        None => tc,
    }
}

impl ViewerPanel {
    /// The panel header: Source · Program, the view size and the frame
    /// toggles.
    pub(super) fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let opened = state.source_picture().is_some();
        let recall = self
            .last_source
            .filter(|id| state.bus.project().media(*id).is_some());
        let header = if opened || recall.is_some() {
            let entity = self.state.clone();
            PanelHeader::new().leading(
                Segmented::new("viewer-monitor")
                    .compact()
                    .items(["Source", "Program"])
                    .selected(if opened { 0 } else { 1 })
                    .on_click(move |ix, _, cx| {
                        entity.update(cx, |s, cx| match (*ix, recall) {
                            (0, Some(id)) if s.source_picture().is_none() => s.open_source(id, cx),
                            (1, _) => s.close_source(cx),
                            _ => {}
                        })
                    }),
            )
        } else {
            PanelHeader::new().title("Program")
        };

        let viewer = cx.entity();
        let zoom = self.zoom;
        let zoom_menu = Button::new("viewer-zoom")
            .ghost()
            .small()
            .label(match zoom {
                None => "Fit".to_string(),
                Some(z) => format!("{:.0} %", z * 100.0),
            })
            .trailing_icon(IconName::ChevronDown)
            .when(zoom.is_some(), |b| b.text_color(t::T1))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                let v = viewer.clone();
                menu =
                    menu.item(PopupMenuItem::new("Fit").checked(zoom.is_none()).on_click(
                        move |_, _, cx| v.update(cx, |this, cx| this.set_zoom(None, cx)),
                    ));
                for z in ZOOMS {
                    let v = viewer.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!("{:.0} %", z * 100.0))
                            .checked(zoom == Some(z))
                            .on_click(move |_, _, cx| {
                                v.update(cx, |this, cx| this.set_zoom(Some(z), cx))
                            }),
                    );
                }
                menu
            });

        let toggle = |id: &'static str, icon, tip: &'static str, on: bool| {
            Button::new(id)
                .ghost()
                .small()
                .icon(icon)
                .tooltip(tip)
                .selected(on)
                .accent(on)
        };
        let viewer = cx.entity();
        let safe = toggle(
            "viewer-safe",
            IconName::Scan,
            "Safe areas",
            self.gizmo.safe_areas,
        )
        .on_click(move |_, _, cx| {
            viewer.update(cx, |this, cx| {
                this.gizmo.safe_areas = !this.gizmo.safe_areas;
                cx.notify();
            })
        });
        let viewer = cx.entity();
        let masks = toggle("viewer-masks", IconName::Drama, "Masks", self.masks.on).on_click(
            move |_, _, cx| {
                viewer.update(cx, |this, cx| {
                    this.masks.on = !this.masks.on;
                    this.masks.cancel();
                    this.masks.tool = None;
                    if this.masks.on {
                        this.gizmo.corner_pin = false;
                    }
                    cx.notify();
                })
            },
        );
        let viewer = cx.entity();
        let pin = toggle(
            "viewer-corner-pin",
            IconName::SquareDashed,
            "Corner pin",
            self.gizmo.corner_pin,
        )
        .on_click(move |_, _, cx| {
            viewer.update(cx, |this, cx| {
                this.gizmo.corner_pin = !this.gizmo.corner_pin;
                if this.gizmo.corner_pin {
                    this.masks.on = false;
                    this.masks.cancel();
                }
                cx.notify();
            })
        });
        header
            .when(!opened, |h| h.children(self.framing_menu(cx)))
            .child(zoom_menu)
            .child(div().flex_none().w(px(1.)).h(px(14.)).bg(t::LN))
            .child(safe)
            .when(!opened, |h| h.child(masks).child(pin))
    }

    /// Fit or fill every framed clip at once, or have the agent reframe
    /// them around their subjects; `None` without footage.
    fn framing_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let state = self.state.read(cx);
        let busy = state.agent_busy();
        let clips: Vec<Clip> = state.bus.project().framed_clips().cloned().collect();
        if clips.is_empty() {
            return None;
        }
        let all = |fit: Fit| clips.iter().all(|c| c.transform.fit == fit);
        let (fit, fill) = (all(Fit::Contain), all(Fit::Cover));
        let entity = self.state.clone();
        Some(
            Button::new("viewer-framing")
                .ghost()
                .small()
                .icon(IconName::Crop)
                .trailing_icon(IconName::ChevronDown)
                .tooltip("Framing")
                .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                    let set = |fit: Fit| {
                        let (state, clips) = (entity.clone(), clips.clone());
                        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                            let commands: Vec<Command> = clips
                                .iter()
                                .filter(|c| c.transform.fit != fit)
                                .map(|c| Command::SetTransform {
                                    clip: c.id,
                                    transform: Transform {
                                        fit,
                                        ..c.transform.clone()
                                    },
                                })
                                .collect();
                            if !commands.is_empty() {
                                state.update(cx, |s, cx| {
                                    s.execute_batch(commands, "set_fit", cx);
                                });
                            }
                        }
                    };
                    let state = entity.clone();
                    menu.item(
                        PopupMenuItem::new("Fit all clips")
                            .checked(fit)
                            .on_click(set(Fit::Contain)),
                    )
                    .item(
                        PopupMenuItem::new("Fill all clips")
                            .checked(fill)
                            .on_click(set(Fit::Cover)),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("Reframe around subjects")
                            .disabled(busy)
                            .on_click(move |_, _, cx| {
                                let text = REFRAME.to_owned();
                                state.update(cx, |s, cx| s.send_chat(text, cx));
                            }),
                    )
                }),
        )
    }

    /// Mask mode's tools under the header: background removal, the
    /// drawing tools and the cut-away tint, with the picture's mask count.
    pub(super) fn mask_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let count = self
            .gizmo
            .active(&state.selection.clips, self.requested)
            .and_then(|shape| state.bus.project().timeline.clip(shape.clip))
            .map_or(0, |(_, clip)| clip.masks.len());
        let tool = self.masks.tool;
        let viewer = cx.entity();
        let person = {
            let viewer = viewer.clone();
            Button::new("viewer-mask-person")
                .ghost()
                .small()
                .icon(IconName::UserRound)
                .tooltip("Remove background")
                .on_click(move |_, _, cx| {
                    viewer.update(cx, |this, cx| {
                        this.masks.cancel();
                        this.masks.tool = None;
                        this.remove_background(cx);
                        cx.notify();
                    })
                })
        };
        let tools = [
            (masks::Tool::Subject, IconName::WandSparkles, "Subject"),
            (masks::Tool::Rectangle, IconName::Square, "Rectangle"),
            (masks::Tool::Ellipse, IconName::Circle, "Ellipse"),
            (masks::Tool::Pen, IconName::PenTool, "Pen"),
        ]
        .into_iter()
        .map(|(t, icon, tip)| {
            let viewer = viewer.clone();
            Button::new(SharedString::from(format!("viewer-mask-{tip}")))
                .ghost()
                .small()
                .icon(icon)
                .tooltip(tip)
                .selected(tool == Some(t))
                .on_click(move |_, _, cx| {
                    viewer.update(cx, |this, cx| {
                        this.masks.cancel();
                        if this.masks.tool == Some(t) {
                            this.masks.tool = None;
                        } else if t == masks::Tool::Subject {
                            this.pick_subject_tool(cx);
                        } else {
                            this.masks.tool = Some(t);
                        }
                        cx.notify();
                    })
                })
        })
        .collect::<Vec<_>>();
        let tinted = self.masks.tinted;
        let tint = Button::new("viewer-mask-tint")
            .ghost()
            .small()
            .icon(IconName::Contrast)
            .tooltip("Show what masks cut away")
            .selected(tinted)
            .accent(tinted)
            .on_click(move |_, _, cx| {
                viewer.update(cx, |this, cx| {
                    this.masks.tinted = !this.masks.tinted;
                    cx.notify();
                })
            });
        h_flex()
            .flex_none()
            .h(t::PANEL_HEADER)
            .px(px(8.))
            .gap(px(2.))
            .bg(t::G)
            .border_b_1()
            .border_color(t::LN)
            .child(person)
            .children(tools)
            .child(div().flex_none().w(px(1.)).h(px(14.)).mx(px(4.)).bg(t::LN))
            .child(tint)
            .child(div().flex_1())
            .when(count > 0, |d| {
                d.child(
                    div()
                        .text_size(px(10.))
                        .text_color(t::T3)
                        .child(match count {
                            1 => "1 mask".to_string(),
                            n => format!("{n} masks"),
                        }),
                )
            })
    }

    /// The transport: timecode and duration, the transport buttons around
    /// play, and the shuttle speed and in/out points (a source's seek bar).
    pub(super) fn transport(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let project = state.monitor_project();
        let rate = project.timeline.settings.rate;
        let playing = state.monitor_playing();
        let source = state.source_picture();
        // Narrow viewers lose the duration first; the timecode stays whole.
        let timecode = h_flex()
            .flex_1()
            .items_baseline()
            .gap(px(10.))
            .whitespace_nowrap()
            .font_family(t::mono())
            .child(
                div()
                    .flex_none()
                    .text_size(px(20.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t::T1)
                    .child(state.monitor_time().timecode(rate)),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_size(px(11.))
                    .text_color(t::T3)
                    .child(format!("/ {}", project.timeline.duration().timecode(rate))),
            );

        let entity = self.state.clone();
        let play = div()
            .id("viewer-play")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(32.))
            .mx(px(4.))
            .rounded_full()
            .bg(t::T1)
            .cursor_pointer()
            .hover(|s| s.bg(t::alpha(t::T1, 0.85)))
            .tooltip(Tooltip::with_kbd(
                if playing { "Pause" } else { "Play" },
                Some("Space"),
            ))
            .on_click(move |_, _, cx| entity.update(cx, |s, cx| s.toggle_play(cx)))
            .child(
                Icon::new(if playing {
                    IconName::Pause
                } else {
                    IconName::Play
                })
                .size(px(14.))
                .color(t::G),
            );

        let marked = match source {
            Some(s) => state.source_marks.get(&s.item().id).copied(),
            None => state.in_out,
        };
        let step = |id: &'static str, icon, tip: &'static str, kbd: &'static str| {
            Button::new(id)
                .ghost()
                .small()
                .icon(icon)
                .tooltip_kbd(tip, kbd)
        };
        let action = |f: fn(&Entity<EditorState>, &mut App)| {
            let state = self.state.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| f(&state, cx)
        };
        let (mark_in, mark_out) = (self.state.clone(), self.state.clone());
        let source_frames = source.is_some_and(|s| s.can_play());
        let buttons = h_flex().flex_none().gap(px(2.)).map(|d| {
            if source.is_some() && !source_frames {
                return d.child(play);
            }
            if source.is_some() {
                let (mark_in, mark_out) = (self.state.clone(), self.state.clone());
                let (insert, overwrite) = (self.state.clone(), self.state.clone());
                let (back, on) = (self.state.clone(), self.state.clone());
                return d
                    .child(
                        step("source-mark-in", IconName::ArrowRightToLine, "Mark in", "I")
                            .when(marked.is_some(), |b| b.text_color(t::T1))
                            .on_click(move |_, _, cx| mark_in.update(cx, |s, cx| s.mark_in(cx))),
                    )
                    .child(
                        step(
                            "source-prev-frame",
                            IconName::StepBack,
                            "Previous frame",
                            "Left",
                        )
                        .on_click(move |_, _, cx| {
                            back.update(cx, |s, cx| s.step_source(false, cx));
                        }),
                    )
                    .child(play)
                    .child(
                        step(
                            "source-next-frame",
                            IconName::StepForward,
                            "Next frame",
                            "Right",
                        )
                        .on_click(move |_, _, cx| {
                            on.update(cx, |s, cx| s.step_source(true, cx));
                        }),
                    )
                    .child(
                        step(
                            "source-mark-out",
                            IconName::ArrowLeftToLine,
                            "Mark out",
                            "O",
                        )
                        .when(marked.is_some(), |b| b.text_color(t::T1))
                        .on_click(move |_, _, cx| mark_out.update(cx, |s, cx| s.mark_out(cx))),
                    )
                    .child(div().w(px(1.)).h(px(16.)).mx(px(4.)).bg(t::LN))
                    .child(
                        step(
                            "source-insert",
                            IconName::BetweenHorizontalStart,
                            "Insert at playhead",
                            ",",
                        )
                        .on_click(move |_, _, cx| edit_source(&insert, InsertMode::Insert, cx)),
                    )
                    .child(
                        step(
                            "source-overwrite",
                            IconName::Replace,
                            "Overwrite at playhead",
                            ".",
                        )
                        .on_click(move |_, _, cx| {
                            edit_source(&overwrite, InsertMode::Overwrite, cx)
                        }),
                    );
            }
            d.child(
                step("viewer-mark-in", IconName::ArrowRightToLine, "Mark in", "I")
                    .when(marked.is_some(), |b| b.text_color(t::T1))
                    .on_click(move |_, _, cx| mark_in.update(cx, |s, cx| s.mark_in(cx))),
            )
            .child(
                step("viewer-prev-cut", IconName::SkipBack, "Previous cut", "Up")
                    .on_click(action(transport::prev_cut)),
            )
            .child(
                step(
                    "viewer-prev-frame",
                    IconName::StepBack,
                    "Previous frame",
                    "Left",
                )
                .on_click(action(transport::prev_frame)),
            )
            .child(play)
            .child(
                step(
                    "viewer-next-frame",
                    IconName::StepForward,
                    "Next frame",
                    "Right",
                )
                .on_click(action(transport::next_frame)),
            )
            .child(
                step("viewer-next-cut", IconName::SkipForward, "Next cut", "Down")
                    .on_click(action(transport::next_cut)),
            )
            .child(
                step(
                    "viewer-mark-out",
                    IconName::ArrowLeftToLine,
                    "Mark out",
                    "O",
                )
                .when(marked.is_some(), |b| b.text_color(t::T1))
                .on_click(move |_, _, cx| mark_out.update(cx, |s, cx| s.mark_out(cx))),
            )
        });

        let speed = state.play_speed();
        let shuttling = source.is_none() && state.is_playing() && speed != 1.0;
        let mark = |label: &'static str, at: Time| {
            h_flex()
                .gap(px(4.))
                .child(div().text_color(t::T3).child(label))
                .child(div().font_family(t::mono()).child(short_timecode(at, rate)))
        };
        let seek = source.filter(|s| s.can_play()).map(|s| {
            let item = s.item().clone();
            let state = self.state.clone();
            let length = s.project().timeline.duration().0.max(1) as f32;
            let range = marked.map(|r| (r.start.0 as f32 / length, r.end.0 as f32 / length));
            seek_bar(
                s.fraction(),
                range,
                move |f, cx| state.update(cx, |s, cx| s.seek_source(&item, f, cx)),
                cx,
            )
        });
        let trailing = h_flex()
            .flex_1()
            .min_w_0()
            .justify_end()
            .gap(px(10.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(11.))
            .text_color(t::T2)
            .when(shuttling, |d| {
                d.child(
                    div()
                        .px(px(5.))
                        .rounded(t::RADIUS_SM)
                        .bg(t::R1)
                        .font_family(t::mono())
                        .text_size(px(10.))
                        .child(format!(
                            "{}{}×",
                            if speed < 0.0 { "\u{2212}" } else { "" },
                            speed.abs()
                        )),
                )
            })
            // A source's marks show on its seek bar.
            .children(marked.filter(|_| source.is_none()).map(|r| {
                h_flex()
                    .gap(px(10.))
                    .child(mark("In", r.start))
                    .child(mark("Out", r.end))
            }))
            .children(seek);

        h_flex()
            .flex_none()
            .h(px(TRANSPORT_HEIGHT))
            .px(px(12.))
            .gap(px(12.))
            .border_t_1()
            .border_color(t::LN)
            .child(timecode)
            .child(buttons)
            .child(trailing)
    }

    /// What the frame's corner names while paused: the source, or the
    /// track and file of the topmost picture and where in it the playhead
    /// is.
    pub(super) fn frame_label(&self, state: &EditorState) -> Option<SharedString> {
        if let Some(source) = state.source_picture() {
            return Some(item_name(source.item()).into());
        }
        let project = state.monitor_project();
        let t = state.monitor_time();
        let (track, clip) = topmost_video_clip(&project.timeline, t)?;
        let item = project.media(clip.media()?)?;
        let rate = item
            .info
            .as_ref()
            .and_then(|i| i.video.as_ref())
            .map(|v| v.rate)
            .unwrap_or(project.timeline.settings.rate);
        let at = clip.to_source(t)?.timecode(rate);
        Some(format!("{} · {} · {at}", track.name, item_name(item)).into())
    }
}
