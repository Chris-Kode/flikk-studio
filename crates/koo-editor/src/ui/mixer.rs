//! Mixer (Audio workspace, Fairlight-style): one strip per audio track,
//! top to bottom as on the timeline, then the master. A strip has its
//! track effect inserts (add from the menu or drop an effect from the
//! library; click one for its parameters), pan, a fader beside a peak/RMS
//! meter with peak hold, and mute/solo. Meters read the playback mixer's
//! levels ([`meter`]); every change is a `Command`.

pub mod clip;
pub mod controls;
pub mod meter;
pub mod stack;

use crate::state::EditorState;
use controls::{ControlHost, Controls, Ctl, Range, Target};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::slider::Slider;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::audiofx::{AudioFx, AudioFxKind};
use koo_core::command::Command;
use koo_core::model::{Track, TrackKind};
use koo_core::{EffectId, TrackId};
use meter::Display;
use std::collections::HashMap;

const STRIP_WIDTH: f32 = 100.;
const METER_WIDTH: f32 = 6.;
/// Inserts shown before a strip scrolls them.
const INSERT_ROWS: usize = 4;
const ROW: f32 = 22.;

pub struct MixerPanel {
    state: Entity<EditorState>,
    controls: Controls,
    meters: HashMap<Option<TrackId>, Display>,
    /// The insert whose parameters are open.
    open: Option<(TrackId, EffectId)>,
    _observe: Subscription,
}

impl ControlHost for MixerPanel {
    fn controls(&mut self) -> &mut Controls {
        &mut self.controls
    }

    fn editor(&self) -> &Entity<EditorState> {
        &self.state
    }
}

impl MixerPanel {
    pub fn new(state: Entity<EditorState>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let _observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            controls: Controls::default(),
            meters: HashMap::new(),
            open: None,
            _observe,
        }
    }

    fn strip(
        &mut self,
        ix: usize,
        track: &Track,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = track.id;
        let fader = controls::knob(
            self,
            Ctl::Fader(id),
            track.mix.volume_db,
            Range {
                min: -60.,
                max: 12.,
                log: false,
            },
            window,
            cx,
        );
        let pan = controls::knob(
            self,
            Ctl::Pan(id),
            track.mix.pan,
            Range {
                min: -1.,
                max: 1.,
                log: false,
            },
            window,
            cx,
        );
        let volume = controls::shown(self, &Ctl::Fader(id)).unwrap_or(track.mix.volume_db);
        let inserts = self.inserts(ix, track, window, cx);
        let display = self.meters.get(&Some(id)).copied().unwrap_or_default();
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let (mute_track, solo_track) = (track.clone(), track.clone());
        let (mute_state, solo_state) = (self.state.clone(), self.state.clone());
        div()
            .id(("strip", ix))
            .w(px(STRIP_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .p_1()
            .border_r_1()
            .border_color(border)
            .child(
                div()
                    .h(px(ROW))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .truncate()
                    .child(track.name.clone()),
            )
            .child(inserts)
            .child(
                div()
                    .h(px(ROW))
                    .px_1()
                    .flex()
                    .items_center()
                    .child(Slider::new(&pan).w_full()),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h(px(80.))
                    .justify_center()
                    .gap_2()
                    .child(
                        div()
                            .h_full()
                            .py_1()
                            .child(Slider::new(&fader).vertical().h_full()),
                    )
                    .child(meter_bar(display, cx)),
            )
            .child(
                div()
                    .text_xs()
                    .text_center()
                    .text_color(muted)
                    .child(controls::format(volume, "dB")),
            )
            .child(
                h_flex()
                    .justify_center()
                    .gap_1()
                    .child(
                        Button::new(("mute", ix))
                            .ghost()
                            .xsmall()
                            .label("M")
                            .tooltip("Mute")
                            .toggled(track.mix.mute)
                            .on_click(move |_, _, cx| {
                                set_mix(&mute_state, &mute_track, |m| m.mute = !m.mute, cx)
                            }),
                    )
                    .child(
                        Button::new(("solo", ix))
                            .ghost()
                            .xsmall()
                            .label("S")
                            .tooltip("Solo")
                            .toggled(track.mix.solo)
                            .on_click(move |_, _, cx| {
                                set_mix(&solo_state, &solo_track, |m| m.solo = !m.solo, cx)
                            }),
                    ),
            )
    }

    /// Track inserts: one row each (enable, name with its parameters,
    /// remove), then "+"; the whole block takes dropped effects.
    fn inserts(
        &mut self,
        ix: usize,
        track: &Track,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = Target::Track(track.id);
        let mut rows = Vec::new();
        for (i, fx) in track.mix.effects.iter().enumerate() {
            rows.push(self.insert(ix * 64 + i, track.id, fx, window, cx));
        }
        let height = px(ROW * (INSERT_ROWS as f32 + 1.) + 4. * INSERT_ROWS as f32);
        let list = v_flex()
            .id(("inserts", ix))
            .h(height)
            .flex_none()
            .rounded_sm()
            .bg(cx.theme().secondary.opacity(0.5))
            .p_0p5()
            .gap_0p5()
            .overflow_y_scroll()
            .children(rows)
            .child(
                h_flex()
                    .h(px(ROW))
                    .justify_center()
                    .child(stack::add_button(("add-fx", ix), &self.state, target)),
            );
        stack::drop_target(list, &self.state, target, cx).into_any_element()
    }

    fn insert(
        &mut self,
        key: usize,
        track: TrackId,
        fx: &AudioFx,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let target = Target::Track(track);
        let open = self.open == Some((track, fx.id));
        let rows = if open {
            stack::param_rows(self, target, fx, window, cx)
        } else {
            Vec::new()
        };
        let theme = cx.theme();
        let (fg, muted, green) = (theme.foreground, theme.muted_foreground, theme.green);
        let (state, id) = (self.state.clone(), fx.id);
        let entity = cx.entity();
        let title: SharedString = fx.kind.label().into();
        let popover = Popover::new(("fx-params", key))
            .anchor(Anchor::TopLeft)
            .open(open)
            .on_open_change(move |open, _, cx| {
                entity.update(cx, |this, cx| {
                    this.open = open.then_some((track, id));
                    cx.notify();
                })
            })
            .trigger_style(StyleRefinement::default().flex_1().min_w_0())
            .trigger(
                Button::new(("fx-name", key))
                    .ghost()
                    .xsmall()
                    .w_full()
                    .label(short_label(fx.kind))
                    .tooltip(fx.kind.label()),
            )
            .content(move |_, _, cx| {
                let muted = cx.theme().muted_foreground;
                v_flex()
                    .w(px(280.))
                    .gap_0p5()
                    .child(
                        div()
                            .pb_1()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .child(title.clone()),
                    )
                    .children(rows.iter().map(|r| stack::param_row(r, muted)))
            });
        let toggle_state = state.clone();
        h_flex()
            .group("insert")
            .h(px(ROW))
            .gap_0p5()
            .text_color(if fx.enabled { fg } else { muted })
            .child(
                div()
                    .id(("fx-on", key))
                    .size(px(14.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .tooltip(stack::tip(if fx.enabled { "Bypass" } else { "Enable" }))
                    .child(div().size(px(6.)).rounded_full().bg(if fx.enabled {
                        green
                    } else {
                        muted.opacity(0.4)
                    }))
                    .on_click(move |_, _, cx| stack::toggle(&toggle_state, target, id, cx)),
            )
            .child(popover)
            .child(
                div()
                    .flex_none()
                    .invisible()
                    .group_hover("insert", |s| s.visible())
                    .child(
                        Button::new(("fx-remove", key))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip("Remove")
                            .on_click(move |_, _, cx| stack::remove(&state, target, id, cx)),
                    ),
            )
    }

    fn master(&self, cx: &App) -> Stateful<Div> {
        let display = self.meters.get(&None).copied().unwrap_or_default();
        let muted = cx.theme().muted_foreground;
        let peak = if display.hold > 0. {
            controls::format(20. * display.hold.log10(), "dB")
        } else {
            "—".into()
        };
        div()
            .id("master")
            .w(px(STRIP_WIDTH * 0.75))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .p_1()
            .child(
                div()
                    .h(px(ROW))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Master"),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h(px(80.))
                    .justify_center()
                    .child(meter_bar(display, cx).w(px(METER_WIDTH * 2.))),
            )
            .child(div().text_xs().text_center().text_color(muted).child(peak))
            .child(div().h(px(ROW)))
    }
}

fn set_mix(
    state: &Entity<EditorState>,
    track: &Track,
    f: impl FnOnce(&mut koo_core::model::TrackMix),
    cx: &mut App,
) {
    let mut mix = track.mix.clone();
    f(&mut mix);
    let command = Command::SetTrackMix {
        track: track.id,
        mix,
    };
    state.update(cx, |s, cx| {
        s.execute(command, cx);
    });
}

/// A compact insert name for a strip.
fn short_label(kind: AudioFxKind) -> &'static str {
    match kind {
        AudioFxKind::Equalizer => "EQ",
        AudioFxKind::HighPass => "HPF",
        AudioFxKind::LowPass => "LPF",
        AudioFxKind::Compressor => "Comp",
        AudioFxKind::Limiter => "Limit",
        AudioFxKind::Gate => "Gate",
        AudioFxKind::DeEsser => "De-ess",
        AudioFxKind::NoiseReduction => "Denoise",
        AudioFxKind::Reverb => "Reverb",
        AudioFxKind::Echo => "Echo",
    }
}

/// Vertical meter: the falling peak as a bar (yellow above -6 dB, red
/// above -1 dB), RMS as a brighter core, and the held peak as a line.
fn meter_bar(d: Display, cx: &App) -> Div {
    let theme = cx.theme();
    let color = if d.peak > 0.891 {
        theme.red
    } else if d.peak > 0.501 {
        theme.yellow
    } else {
        theme.green
    };
    let hold = meter::position(d.hold);
    div()
        .w(px(METER_WIDTH))
        .h_full()
        .relative()
        .rounded_sm()
        .bg(theme.secondary)
        .child(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(relative(meter::position(d.peak)))
                .rounded_sm()
                .bg(color.opacity(0.55)),
        )
        .child(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(relative(meter::position(d.rms)))
                .rounded_sm()
                .bg(color),
        )
        .when(hold > 0., |el| {
            el.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(relative(hold))
                    .h(px(2.))
                    .bg(if d.hold > 0.891 {
                        theme.red
                    } else {
                        theme.foreground
                    }),
            )
        })
}

impl Render for MixerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let playing = state.playing;
        let tracks: Vec<Track> = state
            .bus
            .project()
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .cloned()
            .collect();
        for key in tracks.iter().map(|t| Some(t.id)).chain([None]) {
            self.meters
                .entry(key)
                .or_default()
                .update(meter::level(key).take());
        }
        self.meters
            .retain(|k, _| k.is_none_or(|id| tracks.iter().any(|t| t.id == id)));
        if playing || self.meters.values().any(|d| !d.is_silent()) {
            window.request_animation_frame();
        }
        let strips: Vec<_> = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| self.strip(i, t, window, cx))
            .collect();
        let master = self.master(cx);
        div().id("mixer").size_full().overflow_x_scrollbar().child(
            h_flex()
                .h_full()
                .min_h(px(300.))
                .items_start()
                .children(strips)
                .child(master),
        )
    }
}
