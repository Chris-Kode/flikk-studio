//! The toolbar (tools, snapping, silence tool, zoom) and the track
//! headers with their mix toggles and add-track menu.

use super::layout::{self, HEADER_WIDTH, RULER_HEIGHT, TOOLBAR_HEIGHT};
use super::{Drag, TimelinePanel, Tool, refs, silence_content};
use crate::ui::RefDrag;
use crate::ui::kit::{
    Button, ContextMenuExt as _, DropdownMenu as _, Icon, IconName, Popover, PopupMenu,
    PopupMenuItem, Slider, Tooltip, h_flex, theme as t, v_flex,
};
use flikk_core::model::{Timeline, TrackKind};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;

/// "Add … track" items, one per kind.
fn add_track_items(menu: PopupMenu, panel: &Entity<TimelinePanel>) -> PopupMenu {
    [
        (TrackKind::Video, "Video track"),
        (TrackKind::Audio, "Audio track"),
        (TrackKind::Caption, "Caption track"),
        (TrackKind::Title, "Title track"),
        (TrackKind::Overlay, "Overlay track"),
    ]
    .into_iter()
    .fold(menu, |menu, (kind, label)| {
        let panel = panel.clone();
        menu.item(
            PopupMenuItem::new(label)
                .on_click(move |_, _, cx| panel.update(cx, |this, cx| this.add_track(kind, cx))),
        )
    })
}

/// A header's M or S button: `color` fills it while `on`.
fn mix_toggle(
    id: SharedString,
    label: &'static str,
    on: bool,
    color: Rgba,
    tip: &'static str,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(18.))
        .rounded(px(3.))
        .border_1()
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .map(|el| {
            if on {
                el.bg(color).border_color(color).text_color(t::G)
            } else {
                el.border_color(t::LN)
                    .text_color(t::T3)
                    .hover(|s| s.bg(t::HV).text_color(t::T1))
            }
        })
        .tooltip(Tooltip::text(tip))
        .child(label)
}

/// The header chips: `V1` is the picture track nearest the audio, `A1` the
/// topmost audio track; captions and titles count down from the top.
fn track_codes(tl: &Timeline) -> Vec<String> {
    let up = |kind: TrackKind| matches!(kind, TrackKind::Video | TrackKind::Overlay);
    let order = tl
        .tracks
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, t)| up(t.kind))
        .chain(tl.tracks.iter().enumerate().filter(|(_, t)| !up(t.kind)));
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut codes = vec![String::new(); tl.tracks.len()];
    for (i, track) in order {
        let prefix = track.kind.prefix();
        let n = counts.entry(prefix).or_default();
        *n += 1;
        codes[i] = format!("{prefix}{n}");
    }
    codes
}

/// A header's name when the track has only its code.
fn kind_label(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "Video",
        TrackKind::Audio => "Audio",
        TrackKind::Caption => "Captions",
        TrackKind::Title => "Titles",
        TrackKind::Overlay => "Overlay",
    }
}

impl TimelinePanel {
    pub(super) fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tool = |id: &'static str, icon: IconName, which: Tool, tip: &'static str, kbd| {
            Button::new(id)
                .ghost()
                .small()
                .w(px(24.))
                .icon(icon)
                .selected(self.tool == which)
                .tooltip_kbd(tip, kbd)
        };
        let toggle = |id: &'static str, icon: IconName, on: bool| {
            Button::new(id)
                .ghost()
                .small()
                .w(px(24.))
                .icon(icon)
                .selected(on)
                .accent(on)
        };
        let separator = || div().flex_none().w(px(1.)).h(px(14.)).mx(px(4.)).bg(t::LN);
        let entity = cx.entity();
        let silence = Popover::new("silence-tool")
            .anchor(Anchor::TopLeft)
            .open(self.silence_open)
            .on_open_change({
                let entity = entity.clone();
                move |open, _, cx| {
                    entity.update(cx, |this, cx| {
                        if *open && !this.silence_open {
                            this.silences.retry_failed();
                        }
                        this.silence_open = *open;
                        cx.notify();
                    })
                }
            })
            .trigger(
                toggle("silence", IconName::AudioLines, self.silence_open)
                    .tooltip("Remove silences"),
            )
            .content(move |_, _, cx| silence_content(&entity, cx));
        let settings = &self.state.read(cx).bus.project().timeline.settings;
        let fps = format!("{:.2}", settings.rate.fps());
        let format = format!(
            "{}×{} · {} fps",
            settings.width,
            settings.height,
            fps.trim_end_matches('0').trim_end_matches('.')
        );
        let mode = self.trim_mode(cx);
        let pps = self.viewport.px_per_sec;

        h_flex()
            .flex_none()
            .h(px(TOOLBAR_HEIGHT))
            .px(px(8.))
            .gap(px(4.))
            .border_b_1()
            .border_color(t::LN)
            .child(
                tool(
                    "tool-select",
                    IconName::MousePointer2,
                    Tool::Select,
                    "Select",
                    "V",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.tool = Tool::Select;
                    cx.notify();
                })),
            )
            .child(
                tool("tool-blade", IconName::Scissors, Tool::Blade, "Blade", "B").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.tool = Tool::Blade;
                        cx.notify();
                    }),
                ),
            )
            .child(
                tool(
                    "tool-trim",
                    IconName::MoveHorizontal,
                    Tool::Trim,
                    "Trim",
                    "T",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.tool = Tool::Trim;
                    cx.notify();
                })),
            )
            .children(mode.map(|(icon, label)| {
                h_flex()
                    .flex_none()
                    .h(px(20.))
                    .ml(px(4.))
                    .px(px(7.))
                    .gap(px(5.))
                    .rounded(t::RADIUS_SM)
                    .border_1()
                    .border_color(t::LN)
                    .bg(t::G)
                    .text_size(px(11.))
                    .text_color(t::T1)
                    .child(Icon::new(icon).size(px(12.)).color(t::T2))
                    .child(label)
            }))
            .child(separator())
            .child(
                toggle("snapping", IconName::Magnet, self.snapping)
                    .tooltip_kbd("Snapping", "N")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.snapping = !this.snapping;
                        cx.notify();
                    })),
            )
            .child(silence)
            .child(
                Button::new("marker")
                    .ghost()
                    .small()
                    .w(px(24.))
                    .icon(IconName::BookmarkPlus)
                    .tooltip_kbd("Add marker", "M")
                    .on_click(cx.listener(|this, _, _, cx| this.add_marker(false, cx))),
            )
            .child(
                div()
                    .ml(px(10.))
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(t::T3)
                    .child(format),
            )
            .child(div().flex_1())
            .child(
                Button::new("zoom-out")
                    .ghost()
                    .small()
                    .icon(IconName::ZoomOut)
                    .tooltip_kbd("Zoom out", "-")
                    .disabled(pps <= layout::MIN_PX_PER_SEC)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(0.8, cx))),
            )
            .child(Slider::new(&self.zoom).flex_none().w(px(90.)))
            .child(
                Button::new("zoom-in")
                    .ghost()
                    .small()
                    .icon(IconName::ZoomIn)
                    .tooltip_kbd("Zoom in", "=")
                    .disabled(pps >= layout::MAX_PX_PER_SEC)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(1.25, cx))),
            )
            .child(
                Button::new("zoom-fit")
                    .ghost()
                    .small()
                    .icon(IconName::Maximize2)
                    .tooltip_kbd("Zoom to fit", "Shift Z")
                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
            )
    }

    pub(super) fn headers(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let alt = self.alt;
        let state = self.state.read(cx);
        let timecode = state
            .playhead
            .timecode(state.bus.project().timeline.settings.rate);
        let tl = &state.bus.project().timeline;
        let rows: Vec<_> = self
            .rows(tl)
            .into_iter()
            .zip(tl.tracks.iter().map(|t| (t.clone(), tl.audible(t))))
            .zip(track_codes(tl))
            .collect();
        let panel = cx.entity();
        let children = rows.into_iter().map(|((row, (track, heard)), code)| {
            let id = track.id;
            let locked = track.locked;
            let panel = panel.clone();
            let audio = track.kind == TrackKind::Audio;
            let muted = audio && !heard;
            let hidden = !audio && track.mix.mute;
            let key = |name: &str| SharedString::from(format!("{name}-{}", track.id));
            let name = if track.name == code {
                kind_label(track.kind).to_string()
            } else {
                track.name.clone()
            };
            let reference = refs::track(&track);
            h_flex()
                .id(ElementId::Name(key("header")))
                .relative()
                .flex_none()
                .h(px(row.height))
                .pl(px(8.))
                .pr(px(6.))
                .gap(px(4.))
                .border_b_1()
                .border_color(t::LN)
                .text_size(px(11.))
                .hover(|s| s.bg(t::HV))
                .when(audio, |el| {
                    el.drag_over::<crate::ui::AudioFxDrag>(|style, _, _, _| {
                        style.bg(t::alpha(t::AC, 0.15))
                    })
                    .on_drop(cx.listener(
                        move |this, drag: &crate::ui::AudioFxDrag, _, cx| {
                            this.drop_audio_fx_on_track(drag, id, cx)
                        },
                    ))
                })
                .when(alt, |el| {
                    // Alt-drag hands the track to the chat.
                    el.on_drag(
                        RefDrag {
                            refs: vec![reference.clone()],
                        },
                        |d, _, _, cx| cx.new(|_| d.clone()),
                    )
                })
                .child(
                    div()
                        .id(ElementId::Name(key("resize")))
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(-2.))
                        .h(px(5.))
                        .cursor(CursorStyle::ResizeUpDown)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.drag = Some(Drag::TrackHeight {
                                    track: id,
                                    grab: ev.position.y,
                                    from: row.height,
                                });
                            }),
                        ),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .justify_center()
                        .w(px(24.))
                        .h(px(18.))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(t::LN)
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t::T2)
                        .child(code),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(if muted || hidden { t::T3 } else { t::T2 })
                        .child(name),
                )
                .child(
                    Button::new(ElementId::Name(key("lock")))
                        .ghost()
                        .xsmall()
                        .icon(if locked {
                            IconName::Lock
                        } else {
                            IconName::LockOpen
                        })
                        .selected(locked)
                        .tooltip(if locked { "Unlock" } else { "Lock" })
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_lock(id, cx))),
                )
                .when(!audio, |el| {
                    el.child(
                        Button::new(ElementId::Name(key("hide")))
                            .ghost()
                            .xsmall()
                            .icon(if hidden {
                                IconName::EyeOff
                            } else {
                                IconName::Eye
                            })
                            .selected(hidden)
                            .tooltip(if hidden { "Show" } else { "Hide" })
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.toggle_mix(id, false, cx)),
                            ),
                    )
                })
                .when(audio, |el| {
                    el.child(
                        mix_toggle(key("mute"), "M", track.mix.mute, t::T2, "Mute").on_click(
                            cx.listener(move |this, _, _, cx| this.toggle_mix(id, false, cx)),
                        ),
                    )
                    .child(
                        mix_toggle(key("solo"), "S", track.mix.solo, t::WARN, "Solo").on_click(
                            cx.listener(move |this, _, _, cx| this.toggle_mix(id, true, cx)),
                        ),
                    )
                })
                .context_menu(move |menu, _, _| {
                    let remove = panel.clone();
                    let mention = panel.clone();
                    let reference = reference.clone();
                    add_track_items(menu, &panel)
                        .separator()
                        .item(
                            PopupMenuItem::new("Add to chat")
                                .icon(IconName::MessageSquarePlus)
                                .on_click(move |_, _, cx| {
                                    let r = reference.clone();
                                    mention.update(cx, |this, cx| this.mention(vec![r], cx))
                                }),
                        )
                        .separator()
                        .item(
                            PopupMenuItem::new("Delete track")
                                .icon(IconName::Trash)
                                .destructive()
                                .disabled(locked)
                                .on_click(move |_, _, cx| {
                                    remove.update(cx, |this, cx| this.remove_track(id, cx))
                                }),
                        )
                })
        });
        v_flex()
            .flex_none()
            .w(px(HEADER_WIDTH))
            .h_full()
            .border_r_1()
            .border_color(t::LN)
            .child(
                h_flex()
                    .flex_none()
                    .h(px(RULER_HEIGHT))
                    .px(px(8.))
                    .border_b_1()
                    .border_color(t::LN)
                    .font_family(t::mono())
                    .text_size(px(11.))
                    .text_color(t::AC)
                    .child(timecode),
            )
            .child(
                div().flex_1().min_h_0().overflow_hidden().relative().child(
                    v_flex()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(-self.viewport.scroll_y))
                        .children(children)
                        .child(
                            div().px(px(6.)).py(px(6.)).child(
                                Button::new("add-track")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Plus)
                                    .label("Track")
                                    .dropdown_menu({
                                        let panel = cx.entity();
                                        move |menu, _, _| add_track_items(menu, &panel)
                                    }),
                            ),
                        ),
                ),
            )
    }
}
