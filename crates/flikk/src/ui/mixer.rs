//! Mixer (Audio workspace, Fairlight-style): one strip per audio track,
//! top to bottom as on the timeline, then the master. A strip has its
//! track effect inserts (add from the menu or drop an effect from the
//! library; click one for its parameters), pan, a fader beside a peak/RMS
//! meter with peak hold, and mute/solo. Meters read the playback mixer's
//! levels ([`meter`]); every change is a `Command`.

pub mod controls;
pub mod meter;
pub mod stack;

use crate::state::EditorState;
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, Icon, IconName, Panel, PanelHeader, Popover, ScrollableElement as _, Slider, Tooltip,
    h_flex, v_flex,
};
use controls::{ControlHost, Controls, Ctl, Range, Target};
use flikk_core::audiofx::{AudioFx, AudioFxKind};
use flikk_core::command::Command;
use flikk_core::model::{Track, TrackKind};
use flikk_core::{EffectId, TrackId};
use gpui_kit::base::slider::SliderState;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use meter::Display;
use std::collections::HashMap;

const STRIP_MIN_WIDTH: f32 = 100.;
const MASTER_WIDTH: f32 = 96.;
/// Four insert rows (the last one "+") before the list scrolls.
const INSERTS_HEIGHT: f32 = 112.;
const STRIP_HEADER: f32 = 30.;
const FADER: Range = Range {
    min: -60.,
    max: 12.,
    log: false,
};
const PAN: Range = Range {
    min: -1.,
    max: 1.,
    log: false,
};
/// Marks beside a fader, dB.
const FADER_MARKS: [f32; 8] = [12., 6., 0., -6., -12., -24., -40., -60.];
/// Marks beside the master meter, dBFS.
const METER_MARKS: [f32; 8] = [0., -6., -12., -18., -24., -36., -48., -60.];
/// Where a meter turns yellow, then red, dBFS.
const WARN_DB: f32 = -6.;
const CLIP_DB: f32 = -1.;

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

/// One strip's place in the mix.
struct StripState {
    /// "A1": its position among the audio tracks.
    code: SharedString,
    /// Holds a selected clip.
    selected: bool,
    /// Another track is soloed and this one is not: it is silent.
    soloed_out: bool,
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
        strip: StripState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = track.id;
        let fader = controls::knob(self, Ctl::Fader(id), track.mix.volume_db, FADER, window, cx);
        let pan = controls::knob(self, Ctl::Pan(id), track.mix.pan, PAN, window, cx);
        let volume = controls::shown(self, &Ctl::Fader(id)).unwrap_or(track.mix.volume_db);
        let pan_shown = controls::shown(self, &Ctl::Pan(id)).unwrap_or(track.mix.pan);
        let inserts = self.inserts(ix, track, &strip.code, window, cx);
        let display = self.meters.get(&Some(id)).copied().unwrap_or_default();
        let (mute_track, solo_track) = (track.clone(), track.clone());
        let (mute_state, solo_state) = (self.state.clone(), self.state.clone());
        let header = h_flex()
            .h(px(STRIP_HEADER))
            .flex_none()
            .gap(px(6.))
            .px(px(8.))
            .border_b_1()
            .border_color(t::LN)
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .overflow_hidden()
            .text_color(if track.mix.mute { t::T2 } else { t::T1 })
            .child(
                div()
                    .h(px(16.))
                    .px(px(4.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .rounded(px(3.))
                    .bg(t::alpha(t::A, 0.15))
                    .text_color(t::A)
                    .text_size(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(track.mix.mute, |d| d.opacity(0.6))
                    .child(strip.code.clone()),
            )
            .child(div().min_w_0().truncate().child(track.name.clone()));
        let pan_row = h_flex()
            .h(px(34.))
            .flex_none()
            .gap(px(6.))
            .px(px(8.))
            .border_b_1()
            .border_color(t::LN)
            .child(Slider::new(&pan).bipolar().flex_1().min_w_0())
            .child(
                div()
                    .w(px(24.))
                    .flex_none()
                    .text_right()
                    .font_family(t::mono())
                    .text_size(px(10.))
                    .text_color(t::T2)
                    .child(pan_label(pan_shown)),
            );
        let fader_zone = h_flex()
            .flex_1()
            .min_h(px(80.))
            .justify_center()
            .gap(px(4.))
            .pt(px(12.))
            .pb(px(10.))
            .child(marks(&FADER_MARKS, fader_position))
            .child(Slider::new(&fader).vertical().h_full())
            .child(meter_bar(display, 5.).ml(px(6.)));
        let readout = h_flex()
            .h(px(22.))
            .flex_none()
            .justify_center()
            .gap(px(8.))
            .font_family(t::mono())
            .text_size(px(11.))
            .child(controls::format(volume, "dB"))
            .child(peak_readout(display));
        let buttons = h_flex()
            .flex_none()
            .gap(px(4.))
            .pt(px(6.))
            .px(px(8.))
            .pb(px(8.))
            .child(
                mix_toggle(("mute", ix), "M", track.mix.mute, t::T2, "Mute").on_click(
                    move |_, _, cx| set_mix(&mute_state, &mute_track, |m| m.mute = !m.mute, cx),
                ),
            )
            .child(
                mix_toggle(("solo", ix), "S", track.mix.solo, t::WARN, "Solo").on_click(
                    move |_, _, cx| set_mix(&solo_state, &solo_track, |m| m.solo = !m.solo, cx),
                ),
            );
        v_flex()
            .id(("strip", ix))
            .flex_1()
            .min_w(px(STRIP_MIN_WIDTH))
            .h_full()
            .border_r_1()
            .border_color(t::LN)
            .when(strip.selected, |s| s.bg(t::alpha(t::HV, 0.25)))
            .when(strip.soloed_out, |s| s.opacity(0.45))
            .child(header)
            .child(inserts)
            .child(pan_row)
            .child(fader_zone)
            .child(readout)
            .child(buttons)
    }

    /// Track inserts: one row each (bypass, name opening its parameters,
    /// remove), then "+"; the whole block takes dropped effects.
    fn inserts(
        &mut self,
        ix: usize,
        track: &Track,
        code: &SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = Target::Track(track.id);
        let context: SharedString = format!("{code} \u{00b7} {}", track.name).into();
        let mut rows = Vec::new();
        for (i, fx) in track.mix.effects.iter().enumerate() {
            rows.push(self.insert(ix * 64 + i, track.id, fx, context.clone(), window, cx));
        }
        let add = stack::add_menu(
            Button::new(("add-fx", ix))
                .ghost()
                .xsmall()
                .icon(IconName::Plus)
                .tooltip("Add effect")
                .w_full()
                .h(px(22.))
                .border_dashed()
                .border_color(t::LN)
                .text_color(t::T3),
            &self.state,
            target,
        );
        let list = v_flex()
            .id(("inserts", ix))
            .h(px(INSERTS_HEIGHT))
            .flex_none()
            .gap(px(3.))
            .p(px(6.))
            .border_b_1()
            .border_color(t::LN)
            .overflow_y_scroll()
            .children(rows)
            .child(div().flex_none().child(add));
        stack::drop_target(list, &self.state, target).into_any_element()
    }

    fn insert(
        &mut self,
        key: usize,
        track: TrackId,
        fx: &AudioFx,
        context: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let target = Target::Track(track);
        let open = self.open == Some((track, fx.id));
        let rows = if open {
            self.param_rows(target, fx, window, cx)
        } else {
            Vec::new()
        };
        let (state, id, enabled) = (self.state.clone(), fx.id, fx.enabled);
        let entity = cx.entity();
        let title: SharedString = fx.kind.label().into();
        let group = SharedString::from(format!("insert-{key}"));
        let (toggle_state, remove_state) = (state.clone(), state);
        let power = move |key: &'static str, size: f32| {
            let state = toggle_state.clone();
            div()
                .id(key)
                .flex()
                .flex_none()
                .items_center()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .tooltip(Tooltip::text(if enabled { "Bypass" } else { "Enable" }))
                .child(Icon::new(IconName::Power).size(px(size)).color(if enabled {
                    t::AC
                } else {
                    t::T3
                }))
                .on_click(move |_, _, cx| stack::toggle(&state, target, id, cx))
        };
        let trigger = Button::new(("fx-name", key))
            .xsmall()
            .w_full()
            .h(px(22.))
            .px(px(6.))
            .gap(px(5.))
            .justify_start()
            .font_weight(FontWeight::NORMAL)
            .tooltip(fx.kind.label())
            .when(open, |b| {
                b.border_color(t::AC).shadow(vec![BoxShadow {
                    color: t::alpha(t::AC, 0.15).into(),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.),
                    inset: false,
                }])
            })
            .child(power.clone()("fx-on", 11.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if enabled { t::T1 } else { t::T3 })
                    .when(!enabled, |d| d.line_through())
                    .child(short_label(fx.kind)),
            )
            .child(
                div()
                    .id("fx-remove")
                    .flex()
                    .flex_none()
                    .items_center()
                    .invisible()
                    .group_hover(group.clone(), |s| s.visible())
                    .cursor_pointer()
                    .text_color(t::T3)
                    .hover(|s| s.text_color(t::T1))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .tooltip(Tooltip::text("Remove"))
                    .child(Icon::new(IconName::X).size(px(11.)))
                    .on_click(move |_, _, cx| stack::remove(&remove_state, target, id, cx)),
            );
        let popover = Popover::new(("fx-params", key))
            .anchor(Anchor::TopLeft)
            .open(open)
            .on_open_change(move |open, _, cx| {
                entity.update(cx, |this, cx| {
                    this.open = open.then_some((track, id));
                    cx.notify();
                })
            })
            .w_full()
            .trigger(trigger)
            .content(move |_, _, _| {
                v_flex()
                    .w(px(300.))
                    .pt(px(10.))
                    .px(px(12.))
                    .pb(px(8.))
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .pb(px(4.))
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(power("fx-popover-on", 12.))
                            .child(div().flex_1().min_w_0().truncate().child(title.clone()))
                            .child(
                                div()
                                    .flex_none()
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(t::T3)
                                    .child(context.clone()),
                            ),
                    )
                    .children(rows.iter().map(param_row))
            });
        div().flex_none().group(group).child(popover)
    }

    /// The sliders of `fx`'s parameters, for its popover.
    fn param_rows(
        &mut self,
        target: Target,
        fx: &AudioFx,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<ParamRow> {
        fx.kind
            .params()
            .into_iter()
            .map(|spec| {
                let ctl = Ctl::Param(target, fx.id, spec.name.clone());
                let range = controls::param_range(fx, &spec.name);
                let value = fx.value(&spec.name) as f32;
                let slider = controls::knob(self, ctl.clone(), value, range, window, cx);
                let (label, unit) = controls::split_label(&spec.label);
                let shown = controls::shown(self, &ctl).unwrap_or(value);
                let value = controls::format(shown, unit);
                ParamRow {
                    label: label.to_owned().into(),
                    slider,
                    value: if unit.is_empty() {
                        value.into()
                    } else {
                        format!("{value} {unit}").into()
                    },
                }
            })
            .collect()
    }

    fn master(&self) -> Div {
        let display = self.meters.get(&None).copied().unwrap_or_default();
        v_flex()
            .w(px(MASTER_WIDTH))
            .flex_none()
            .h_full()
            .bg(t::alpha(t::G, 0.35))
            .child(
                h_flex()
                    .h(px(STRIP_HEADER))
                    .flex_none()
                    .justify_center()
                    .border_b_1()
                    .border_color(t::LN)
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Master"),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h(px(80.))
                    .justify_center()
                    .gap(px(6.))
                    .pt(px(12.))
                    .pb(px(10.))
                    .child(marks(&METER_MARKS, db_position))
                    .child(meter_bar(display, 9.)),
            )
            .child(
                v_flex()
                    .flex_none()
                    .items_center()
                    .pb(px(10.))
                    .font_family(t::mono())
                    .text_size(px(11.))
                    .child(peak_readout(display))
                    .child(div().text_size(px(10.)).text_color(t::T3).child("dBFS")),
            )
    }
}

/// One parameter in an insert's popover.
struct ParamRow {
    label: SharedString,
    slider: Entity<SliderState>,
    value: SharedString,
}

/// Label, slider and readout, 28 px high.
fn param_row(row: &ParamRow) -> Div {
    h_flex()
        .h(px(28.))
        .gap(px(8.))
        .text_size(px(11.))
        .child(
            div()
                .w(px(76.))
                .flex_none()
                .truncate()
                .text_color(t::T2)
                .child(row.label.clone()),
        )
        .child(Slider::new(&row.slider).flex_1().min_w_0())
        .child(
            div()
                .w(px(62.))
                .flex_none()
                .whitespace_nowrap()
                .text_right()
                .font_family(t::mono())
                .text_color(t::T2)
                .child(row.value.clone()),
        )
}

fn set_mix(
    state: &Entity<EditorState>,
    track: &Track,
    f: impl FnOnce(&mut flikk_core::model::TrackMix),
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

/// Mute or solo: outlined, filled with `color` while on.
fn mix_toggle(
    id: impl Into<ElementId>,
    label: &'static str,
    on: bool,
    color: Rgba,
    tip: &'static str,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .h(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(t::RADIUS_SM)
        .border_1()
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .map(|el| {
            if on {
                el.bg(color).border_color(color).text_color(t::G)
            } else {
                el.border_color(t::LN)
                    .text_color(t::T2)
                    .hover(|s| s.bg(t::HV).text_color(t::T1))
            }
        })
        .tooltip(Tooltip::text(tip))
        .child(label)
}

/// "C" in the middle, else the side and how far: "L10", "R100".
fn pan_label(pan: f32) -> String {
    let amount = (pan.abs() * 100.).round();
    match () {
        _ if amount < 1. => "C".into(),
        _ if pan < 0. => format!("L{amount:.0}"),
        _ => format!("R{amount:.0}"),
    }
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

/// A fader value's height on its track, 0–1.
fn fader_position(db: f32) -> f32 {
    ((db - FADER.min) / (FADER.max - FADER.min)).clamp(0., 1.)
}

/// A dBFS level's height on a meter, 0–1 (as [`meter::position`]).
fn db_position(db: f32) -> f32 {
    ((db + 60.) / 60.).clamp(0., 1.)
}

/// The colour of a level: green, yellow from -6 dBFS, red from -1 dBFS.
fn tone(db: f32) -> Rgba {
    if db >= CLIP_DB {
        t::ERR
    } else if db >= WARN_DB {
        t::WARN
    } else {
        t::OK
    }
}

/// A scale column: each mark at `position(mark)` from the bottom.
fn marks(marks: &[f32], position: impl Fn(f32) -> f32) -> Div {
    div()
        .relative()
        .w(px(20.))
        .h_full()
        .flex_none()
        .font_family(t::mono())
        .text_size(px(9.))
        .text_color(t::T3)
        .children(marks.iter().map(|&db| {
            let text: SharedString = if db > 0. {
                format!("+{db:.0}").into()
            } else if db < 0. {
                format!("\u{2212}{:.0}", -db).into()
            } else {
                "0".into()
            };
            div()
                .absolute()
                .right_0()
                .bottom(relative(position(db)))
                .mb(px(-6.))
                .h(px(12.))
                .flex()
                .items_center()
                .child(text)
        }))
}

/// The held peak in dBFS, coloured by its zone; a dash when silent.
fn peak_readout(d: Display) -> Div {
    if d.hold <= 1e-5 {
        return div().text_color(t::T3).child("\u{2014}");
    }
    let db = 20. * d.hold.log10();
    let color = if db >= WARN_DB { tone(db) } else { t::T3 };
    div().text_color(color).child(controls::format(db, "dB"))
}

/// Vertical meter: the falling peak as a faint bar and RMS as a solid
/// core, both green, then yellow and red near the top; the held peak as a
/// line.
fn meter_bar(d: Display, width: f32) -> Div {
    let warn = db_position(WARN_DB);
    let clip = db_position(CLIP_DB);
    let zones = [(0., warn, t::OK), (warn, clip, t::WARN), (clip, 1., t::ERR)];
    let bar = |level: f32, alpha: f32, inset: f32| {
        zones
            .into_iter()
            .filter(move |(from, _, _)| level > *from)
            .map(move |(from, to, color)| {
                div()
                    .absolute()
                    .left(px(inset))
                    .right(px(inset))
                    .bottom(relative(from))
                    .h(relative(level.min(to) - from))
                    .bg(t::alpha(color, alpha))
            })
    };
    let hold = meter::position(d.hold);
    div()
        .relative()
        .w(px(width))
        .h_full()
        .flex_none()
        .rounded(px(1.))
        .overflow_hidden()
        .bg(t::R1)
        .children(bar(meter::position(d.peak), 0.45, 0.))
        .children(bar(meter::position(d.rms), 1., 1.))
        .when(hold > 0., |el| {
            let db = 20. * d.hold.log10();
            el.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(relative(hold))
                    .mb(px(-1.))
                    .h(px(2.))
                    .bg(tone(db)),
            )
        })
}

impl Render for MixerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.controls.prune();
        let state = self.state.read(cx);
        let playing = state.is_playing();
        let project = state.bus.project();
        let rate = project.timeline.settings.sample_rate;
        let tracks: Vec<Track> = project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .cloned()
            .collect();
        let selected: Vec<TrackId> = state
            .selection
            .clips
            .iter()
            .filter_map(|id| project.timeline.clip(*id).map(|(t, _)| t.id))
            .collect();
        let soloing = project.timeline.soloing();
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
            .map(|(i, track)| {
                let strip = StripState {
                    code: format!("A{}", i + 1).into(),
                    selected: selected.contains(&track.id),
                    soloed_out: soloing && !track.mix.solo,
                };
                self.strip(i, track, strip, window, cx)
            })
            .collect();
        let master = self.master();
        Panel::new()
            .child(
                PanelHeader::new()
                    .title("Mixer")
                    .context(format!("{} kHz \u{00b7} stereo", rate as f32 / 1000.)),
            )
            .child(
                div()
                    .id("mixer")
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scrollbar()
                    .child(
                        h_flex()
                            .min_w_full()
                            .h_full()
                            .min_h(px(300.))
                            .items_start()
                            .children(strips)
                            .child(master),
                    ),
            )
    }
}
