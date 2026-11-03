//! Audio effects on clips and tracks (Fairlight-style inserts). Built-in
//! processors only; each has a fixed parameter list so the inspector, the
//! agent tools, the preview mixer and the render agree on names and ranges.
//!
//! Signal flow, the same in the viewer's mixer and the render:
//!
//! ```text
//! clip:   source → retime (pitch-preserving) → clip effects
//!         → volume (gain or its keyframes) × fades × crossfade ramps
//! track:  Σ clips → track volume → pan → track effects
//! master: Σ tracks (ducking under dialogue) → loudness
//! ```
//!
//! [`dsp`] holds the real-time processors the viewer runs; the render maps
//! each effect to FFmpeg filters tuned to match them (`flikk-deliver`).
//! [`spans`] resolves what each audio clip plays, including the source
//! handles crossfades use.

pub mod dsp;
pub mod stretch;

use crate::fx::{ParamKind, ParamSpec, ParamValue, transitions};
use crate::ids::EffectId;
use crate::model::{Clip, Track};
use crate::time::{Time, TimeRange};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioFxKind {
    /// Low shelf, peaking mid, high shelf.
    Equalizer,
    HighPass,
    LowPass,
    Compressor,
    Limiter,
    Gate,
    DeEsser,
    NoiseReduction,
    Reverb,
    Echo,
}

impl AudioFxKind {
    pub const ALL: [AudioFxKind; 10] = [
        Self::Equalizer,
        Self::HighPass,
        Self::LowPass,
        Self::Compressor,
        Self::Limiter,
        Self::Gate,
        Self::DeEsser,
        Self::NoiseReduction,
        Self::Reverb,
        Self::Echo,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Equalizer => "Equalizer",
            Self::HighPass => "High Pass",
            Self::LowPass => "Low Pass",
            Self::Compressor => "Compressor",
            Self::Limiter => "Limiter",
            Self::Gate => "Noise Gate",
            Self::DeEsser => "De-esser",
            Self::NoiseReduction => "Noise Reduction",
            Self::Reverb => "Reverb",
            Self::Echo => "Echo",
        }
    }

    /// Tool / JSON name, e.g. `high_pass`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Equalizer => "equalizer",
            Self::HighPass => "high_pass",
            Self::LowPass => "low_pass",
            Self::Compressor => "compressor",
            Self::Limiter => "limiter",
            Self::Gate => "gate",
            Self::DeEsser => "de_esser",
            Self::NoiseReduction => "noise_reduction",
            Self::Reverb => "reverb",
            Self::Echo => "echo",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    /// The spec of parameter `name`.
    pub fn param(self, name: &str) -> Option<ParamSpec> {
        self.params().into_iter().find(|p| p.name == name)
    }

    /// Parameters, all [`ParamKind::Float`] with a unit in the label.
    pub fn params(self) -> Vec<ParamSpec> {
        let p = |name: &str, label: &str, min: f64, max: f64, default: f64| ParamSpec {
            name: name.into(),
            label: label.into(),
            kind: ParamKind::Float { min, max },
            default: ParamValue::Number(default),
        };
        match self {
            Self::Equalizer => vec![
                p("low_freq", "Low (Hz)", 20.0, 1000.0, 100.0),
                p("low_gain_db", "Low gain (dB)", -24.0, 24.0, 0.0),
                p("mid_freq", "Mid (Hz)", 100.0, 10000.0, 1000.0),
                p("mid_gain_db", "Mid gain (dB)", -24.0, 24.0, 0.0),
                p("mid_q", "Mid Q", 0.1, 10.0, 1.0),
                p("high_freq", "High (Hz)", 1000.0, 20000.0, 8000.0),
                p("high_gain_db", "High gain (dB)", -24.0, 24.0, 0.0),
            ],
            Self::HighPass => vec![p("freq", "Cutoff (Hz)", 20.0, 2000.0, 80.0)],
            Self::LowPass => vec![p("freq", "Cutoff (Hz)", 1000.0, 20000.0, 12000.0)],
            Self::Compressor => vec![
                p("threshold_db", "Threshold (dB)", -60.0, 0.0, -18.0),
                p("ratio", "Ratio", 1.0, 20.0, 3.0),
                p("attack_ms", "Attack (ms)", 0.1, 200.0, 10.0),
                p("release_ms", "Release (ms)", 5.0, 2000.0, 150.0),
                p("makeup_db", "Makeup (dB)", 0.0, 24.0, 0.0),
            ],
            Self::Limiter => vec![
                p("ceiling_db", "Ceiling (dB)", -12.0, 0.0, -1.0),
                p("release_ms", "Release (ms)", 1.0, 1000.0, 50.0),
            ],
            Self::Gate => vec![
                p("threshold_db", "Threshold (dB)", -80.0, 0.0, -50.0),
                p("attack_ms", "Attack (ms)", 0.1, 100.0, 5.0),
                p("release_ms", "Release (ms)", 5.0, 2000.0, 200.0),
            ],
            Self::DeEsser => vec![
                p("freq", "Frequency (Hz)", 2000.0, 12000.0, 6000.0),
                p("amount", "Amount", 0.0, 1.0, 0.5),
            ],
            Self::NoiseReduction => vec![
                p("amount_db", "Reduction (dB)", 0.0, 40.0, 12.0),
                p("floor_db", "Noise floor (dB)", -80.0, -20.0, -50.0),
            ],
            Self::Reverb => vec![
                p("room", "Room size", 0.0, 1.0, 0.5),
                p("damping", "Damping", 0.0, 1.0, 0.5),
                p("mix", "Mix", 0.0, 1.0, 0.25),
            ],
            Self::Echo => vec![
                p("delay_ms", "Delay (ms)", 10.0, 2000.0, 300.0),
                p("feedback", "Feedback", 0.0, 0.95, 0.35),
                p("mix", "Mix", 0.0, 1.0, 0.3),
            ],
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioFx {
    #[serde(default)]
    pub id: EffectId,
    pub kind: AudioFxKind,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Values that differ from the defaults, by parameter name.
    #[serde(default)]
    pub params: BTreeMap<String, f64>,
}

impl AudioFx {
    pub fn new(kind: AudioFxKind) -> Self {
        Self {
            id: EffectId::new(),
            kind,
            enabled: true,
            params: BTreeMap::new(),
        }
    }

    /// Enabled and not neutral at its settings (a flat equalizer, a 1:1
    /// compressor without makeup, a dry reverb…), so it changes the sound.
    pub fn is_active(&self) -> bool {
        if !self.enabled {
            return false;
        }
        let zero = |name: &str| self.value(name).abs() < 1e-6;
        match self.kind {
            AudioFxKind::Equalizer => {
                !(zero("low_gain_db") && zero("mid_gain_db") && zero("high_gain_db"))
            }
            AudioFxKind::Compressor => !(self.value("ratio") <= 1.0 + 1e-6 && zero("makeup_db")),
            AudioFxKind::DeEsser => !zero("amount"),
            AudioFxKind::NoiseReduction => !zero("amount_db"),
            AudioFxKind::Reverb | AudioFxKind::Echo => !zero("mix"),
            _ => true,
        }
    }

    /// Set `name`, clamped to its range; a value equal to the default is
    /// dropped so `params` keeps only what differs.
    pub fn set(&mut self, name: &str, value: f64) {
        let Some(spec) = self.kind.param(name) else {
            return;
        };
        let v = match spec.kind {
            ParamKind::Float { min, max } => value.clamp(min, max),
            _ => value,
        };
        if spec.default.as_f64().is_some_and(|d| (d - v).abs() < 1e-9) {
            self.params.remove(name);
        } else {
            self.params.insert(name.into(), v);
        }
    }

    /// The value of `name`, clamped to its range; the default when unset.
    pub fn value(&self, name: &str) -> f64 {
        let Some(spec) = self.kind.param(name) else {
            return self.params.get(name).copied().unwrap_or(0.0);
        };
        let default = spec.default.as_f64().unwrap_or(0.0);
        let v = self.params.get(name).copied().unwrap_or(default);
        match spec.kind {
            ParamKind::Float { min, max } => v.clamp(min, max),
            _ => v,
        }
    }
}

/// Equal-power gain at progress `p` (0..1) of a crossfade: rising for the
/// incoming side, falling for the outgoing one. The two sum to constant
/// power.
pub fn equal_power(p: f64, rising: bool) -> f64 {
    let p = p.clamp(0.0, 1.0);
    let a = p * std::f64::consts::FRAC_PI_2;
    if rising { a.sin() } else { a.cos() }
}

/// An equal-power ramp over `range`: rising ramps are 0 before it and 1
/// after, falling ones the reverse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ramp {
    pub range: TimeRange,
    pub rising: bool,
}

impl Ramp {
    pub fn progress(&self, t: Time) -> f64 {
        let d = self.range.duration().0.max(1) as f64;
        ((t - self.range.start).0 as f64 / d).clamp(0.0, 1.0)
    }

    pub fn gain(&self, t: Time) -> f64 {
        equal_power(self.progress(t), self.rising)
    }
}

/// What one audio clip plays on its track: the clip's range widened into
/// its source handles by crossfades, and the ramps over it.
#[derive(Clone, Debug)]
pub struct Span<'a> {
    pub clip: &'a Clip,
    /// Timeline time heard. Contains the clip's own range.
    pub range: TimeRange,
    pub ramps: Vec<Ramp>,
}

impl Span<'_> {
    /// Fade gain at timeline time `t`: the clip's linear fades, measured
    /// from the span's edges, times the crossfade ramps.
    pub fn envelope(&self, t: Time) -> f64 {
        envelope(self.clip, self.range, &self.ramps, t)
    }

    /// Clip volume in dB at timeline time `t`: the gain, or its
    /// [`crate::model::Property::Volume`] keyframes.
    pub fn volume_db(&self, t: Time) -> f32 {
        self.clip
            .value_at(crate::model::Property::Volume, t - self.clip.start)
    }
}

/// [`Span::envelope`] for a span held in parts.
pub fn envelope(clip: &Clip, range: TimeRange, ramps: &[Ramp], t: Time) -> f64 {
    let mut g = 1.0;
    let audio = &clip.audio;
    if audio.fade_in.0 > 0 {
        g *= ((t - range.start).0 as f64 / audio.fade_in.0 as f64).clamp(0.0, 1.0);
    }
    if audio.fade_out.0 > 0 {
        g *= ((range.end - t).0 as f64 / audio.fade_out.0 as f64).clamp(0.0, 1.0);
    }
    for r in ramps {
        g *= r.gain(t);
    }
    g
}

/// The spans of every enabled media clip on `track`, in timeline order.
/// Transitions on audio tracks are equal-power crossfades: at a cut the
/// outgoing clip keeps playing from its source past its end and the
/// incoming one starts early from before its in point, each over the
/// transition's range (see [`crate::fx::transitions`]); at an open edge the
/// clip fades in from or out to silence. Disabled clips are silent.
pub fn spans(track: &Track) -> Vec<Span<'_>> {
    let mut out: Vec<Span> = track
        .clips
        .iter()
        .filter(|c| c.enabled && c.media().is_some())
        .map(|clip| Span {
            clip,
            range: clip.range(),
            ramps: Vec::new(),
        })
        .collect();
    for placed in transitions(track) {
        for (side, rising) in [(placed.from, false), (placed.to, true)] {
            let Some(clip) = side else { continue };
            let Some(span) = out.iter_mut().find(|s| s.clip.id == clip.id) else {
                continue;
            };
            span.range = TimeRange::new(
                span.range.start.min(placed.range.start),
                span.range.end.max(placed.range.end),
            );
            span.ramps.push(Ramp {
                range: placed.range,
                rising,
            });
        }
    }
    out
}
