//! Audio effects on clips and tracks (Fairlight-style inserts). Built-in
//! processors only; each has a fixed parameter list so the inspector, the
//! agent tools, the preview mixer and the render agree on names and ranges.

use crate::fx::{ParamKind, ParamSpec, ParamValue};
use crate::ids::EffectId;
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

    /// The value of `name`, clamped to its range; the default when unset.
    pub fn value(&self, name: &str) -> f64 {
        let Some(spec) = self.kind.params().into_iter().find(|p| p.name == name) else {
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
