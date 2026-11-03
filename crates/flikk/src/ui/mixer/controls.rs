//! Controls for the mixer and the clip effect stack. Each commits once as a
//! `Command` (faders and pan as `SetTrackMix`, clip volume as `SetGain` or a
//! volume keyframe at the playhead, effect parameters as `SetTrackMix` or
//! `SetAudioEffects`): a slider on release, its readout following the drag;
//! a number field when a scrub is released or a typed value entered.

use crate::state::EditorState;
use crate::ui::kit::{NumberFieldEvent, NumberFieldState};
use flikk_core::audiofx::AudioFx;
use flikk_core::command::Command;
use flikk_core::fx::ParamKind;
use flikk_core::model::{Clip, Project, Property};
use flikk_core::{ClipId, EffectId, Time, TrackId};
use gpui_kit::base::slider::{SliderEvent, SliderScale, SliderState};
use gpui_kit::*;
use std::collections::HashMap;

/// Whose effect stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Track(TrackId),
    Clip(ClipId),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ctl {
    Fader(TrackId),
    Pan(TrackId),
    ClipVolume(ClipId),
    Param(Target, EffectId, String),
}

/// Range of a control in model units, and whether a slider moves on a log
/// scale.
#[derive(Clone, Copy)]
pub struct Range {
    pub min: f32,
    pub max: f32,
    pub log: bool,
}

struct Knob {
    slider: Entity<SliderState>,
    /// Model value last shown.
    value: f32,
    /// Value under the pointer while dragging.
    live: Option<f32>,
    _sub: Subscription,
}

impl Knob {
    /// What the readout shows.
    fn shown(&self) -> f32 {
        self.live.unwrap_or(self.value)
    }
}

struct Field {
    state: Entity<NumberFieldState>,
    _sub: Subscription,
}

#[derive(Default)]
pub struct Controls {
    knobs: HashMap<Ctl, Knob>,
    fields: HashMap<Ctl, Field>,
}

/// A view that owns [`Controls`].
pub trait ControlHost: Sized + 'static {
    fn controls(&mut self) -> &mut Controls;
    fn editor(&self) -> &Entity<EditorState>;
}

/// The slider for `ctl`, created on first use and synced to `value` unless
/// it is being dragged.
pub fn knob<T: ControlHost>(
    host: &mut T,
    ctl: Ctl,
    value: f32,
    range: Range,
    window: &mut Window,
    cx: &mut Context<T>,
) -> Entity<SliderState> {
    if let Some(k) = host.controls().knobs.get_mut(&ctl) {
        if k.live.is_none() && (k.value.is_nan() || (k.value - value).abs() > 1e-6) {
            k.value = value;
            k.slider.update(cx, |s, cx| s.set_value(value, window, cx));
        }
        return k.slider.clone();
    }
    let step = if range.log {
        (range.max - range.min) / 1000.
    } else {
        (range.max - range.min) / 200.
    };
    let slider = cx.new(|_| {
        // Bounds are checked against each other as they are set (the
        // defaults are 0..100), and the scale against the bounds.
        let s = SliderState::new();
        let s = if range.min < 100. {
            s.min(range.min).max(range.max)
        } else {
            s.max(range.max).min(range.min)
        };
        let s = if range.log {
            s.scale(SliderScale::Logarithmic)
        } else {
            s
        };
        s.step(step).default_value(value)
    });
    let key = ctl.clone();
    let _sub = cx.subscribe(&slider, move |this: &mut T, _, ev: &SliderEvent, cx| {
        let v = match ev {
            SliderEvent::Change(v) | SliderEvent::Release(v) => v.start(),
        };
        let release = matches!(ev, SliderEvent::Release(_));
        if let Some(k) = this.controls().knobs.get_mut(&key) {
            k.live = (!release).then_some(v);
            if release {
                // Resync from the model on the next render.
                k.value = f32::NAN;
            }
        }
        if release {
            commit(this.editor(), &key, v, cx);
        }
        cx.notify();
    });
    host.controls().knobs.insert(
        ctl,
        Knob {
            slider: slider.clone(),
            value,
            live: None,
            _sub,
        },
    );
    slider
}

/// The readout value of `ctl`, if it has a slider.
pub fn shown<T: ControlHost>(host: &mut T, ctl: &Ctl) -> Option<f32> {
    host.controls().knobs.get(ctl).map(Knob::shown)
}

/// The number field for `ctl`, created on first use and following `value`
/// except while it is scrubbed or typed in.
pub fn field<T: ControlHost>(
    host: &mut T,
    ctl: Ctl,
    value: f32,
    range: Range,
    decimals: usize,
    window: &mut Window,
    cx: &mut Context<T>,
) -> Entity<NumberFieldState> {
    if let Some(f) = host.controls().fields.get(&ctl) {
        let state = f.state.clone();
        state.update(cx, |s, cx| s.set_value(value as f64, cx));
        return state;
    }
    let state = cx.new(|cx| {
        NumberFieldState::new(window, cx)
            .min(range.min as f64)
            .max(range.max as f64)
            .step(10f64.powi(-(decimals as i32)))
            .decimals(decimals)
            .default_value(value as f64)
    });
    let key = ctl.clone();
    let _sub = cx.subscribe(&state, move |this: &mut T, _, ev: &NumberFieldEvent, cx| {
        if let NumberFieldEvent::Commit(v) = *ev {
            commit(this.editor(), &key, v as f32, cx);
        }
    });
    host.controls().fields.insert(
        ctl,
        Field {
            state: state.clone(),
            _sub,
        },
    );
    state
}

/// Set `ctl` to `v` in the model.
pub fn commit(state: &Entity<EditorState>, ctl: &Ctl, v: f32, cx: &mut App) {
    let commands = {
        let s = state.read(cx);
        commands(s.bus.project(), s.playhead, ctl, v)
    };
    state.update(cx, |s, cx| {
        for c in commands {
            s.execute(c, cx);
        }
    });
}

/// The commands that set `ctl` to `v`.
fn commands(project: &Project, playhead: Time, ctl: &Ctl, v: f32) -> Vec<Command> {
    let tl = &project.timeline;
    let track_mix = |track: TrackId, f: &dyn Fn(&mut flikk_core::model::TrackMix)| {
        tl.track(track).map(|t| {
            let mut mix = t.mix.clone();
            f(&mut mix);
            Command::SetTrackMix { track, mix }
        })
    };
    let command = match ctl {
        Ctl::Fader(track) => track_mix(*track, &|m| m.volume_db = v),
        Ctl::Pan(track) => track_mix(*track, &|m| m.pan = v),
        Ctl::ClipVolume(clip) => tl.clip(*clip).map(|(_, c)| {
            if c.keyframes.iter().any(|k| k.property == Property::Volume) {
                Command::SetKeyframe {
                    clip: c.id,
                    property: Property::Volume,
                    at: c.start + clip_time(c, playhead),
                    value: v,
                    ease: Default::default(),
                }
            } else {
                Command::SetGain { clip: c.id, db: v }
            }
        }),
        Ctl::Param(target, effect, name) => {
            let set = |effects: &mut Vec<AudioFx>| {
                if let Some(fx) = effects.iter_mut().find(|f| f.id == *effect) {
                    fx.set(name, v as f64);
                }
            };
            edit_effects(project, *target, set)
        }
    };
    command.into_iter().collect()
}

/// `SetTrackMix` or `SetAudioEffects` with `f` applied to the stack.
pub fn edit_effects(
    project: &Project,
    target: Target,
    f: impl FnOnce(&mut Vec<AudioFx>),
) -> Option<Command> {
    let tl = &project.timeline;
    match target {
        Target::Track(track) => {
            let mut mix = tl.track(track)?.mix.clone();
            f(&mut mix.effects);
            Some(Command::SetTrackMix { track, mix })
        }
        Target::Clip(clip) => {
            let mut effects = tl.clip(clip)?.1.audio.effects.clone();
            f(&mut effects);
            Some(Command::SetAudioEffects { clip, effects })
        }
    }
}

/// Playhead relative to the clip's start, clamped inside it.
pub fn clip_time(clip: &Clip, playhead: Time) -> Time {
    (playhead - clip.start).clamp(Time::ZERO, Time((clip.duration.0 - 1).max(0)))
}

/// Range of an effect parameter: frequencies and times on a log scale.
pub fn param_range(fx: &AudioFx, name: &str) -> Range {
    let (min, max) = match fx.kind.param(name).map(|p| p.kind) {
        Some(ParamKind::Float { min, max }) => (min as f32, max as f32),
        _ => (0., 1.),
    };
    let log = min > 0. && (name.contains("freq") || name.ends_with("_ms"));
    Range { min, max, log }
}

/// A parameter label without its unit, and the unit: "Low (Hz)" → ("Low", "Hz").
pub fn split_label(label: &str) -> (&str, &str) {
    match label.split_once(" (") {
        Some((name, unit)) => (name, unit.trim_end_matches(')')),
        None => (label, ""),
    }
}

/// Decimals a value in `unit` is typed and shown with.
pub fn decimals(unit: &str, range: Range) -> usize {
    match unit {
        "Hz" => 0,
        "dB" => 1,
        "ms" if range.min < 1. => 1,
        "ms" => 0,
        _ => 2,
    }
}

/// Compact readout: "1.2k" for kHz, one decimal for dB, whole ms; minus
/// signs as in the number fields.
pub fn format(v: f32, unit: &str) -> String {
    let s = match unit {
        "Hz" if v >= 1000. => format!("{:.1}k", v / 1000.),
        "Hz" => format!("{v:.0}"),
        "ms" if v < 10. => format!("{v:.1}"),
        "ms" => format!("{v:.0}"),
        "dB" => format!("{v:.1}"),
        _ => format!("{v:.2}"),
    };
    match s.strip_prefix('-') {
        Some(abs) if abs.chars().all(|c| c == '0' || c == '.') => abs.to_owned(),
        Some(abs) => format!("\u{2212}{abs}"),
        None => s,
    }
}
