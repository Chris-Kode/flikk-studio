//! The grade's scalar fields as slider specs, and the wheels' ranges.

use super::super::param::{Key, Spec};
use koo_core::model::Grade;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    Exposure,
    Contrast,
    Saturation,
    Hue,
    Temperature,
    Tint,
    Gamma,
    Shadows,
    Highlights,
}

pub const PRIMARY: [Field; 7] = [
    Field::Exposure,
    Field::Contrast,
    Field::Saturation,
    Field::Hue,
    Field::Temperature,
    Field::Tint,
    Field::Gamma,
];

pub const TONE: [Field; 2] = [Field::Shadows, Field::Highlights];

impl Field {
    /// Shared with the inspector's other sliders where one exists.
    pub fn spec(self) -> Spec {
        let s = |label, min, max, step, decimals, factor, default| Spec {
            label,
            min,
            max,
            step,
            decimals,
            factor,
            default,
            field: true,
        };
        match self {
            Field::Exposure => Key::Exposure.spec(),
            Field::Contrast => Key::Contrast.spec(),
            Field::Saturation => Key::Saturation.spec(),
            Field::Gamma => Key::Gamma.spec(),
            Field::Temperature => Key::Temperature.spec(),
            Field::Tint => Key::Tint.spec(),
            Field::Hue => s("Hue", -180., 180., 1., 0, 1., 0.),
            Field::Shadows => s("Shadows", -100., 100., 1., 0, 100., 0.),
            Field::Highlights => s("Highlights", -100., 100., 1., 0, 100., 0.),
        }
    }

    pub fn get(self, g: &Grade) -> f32 {
        match self {
            Field::Exposure => g.exposure,
            Field::Contrast => g.contrast,
            Field::Saturation => g.saturation,
            Field::Hue => g.hue,
            Field::Temperature => g.temperature,
            Field::Tint => g.tint,
            Field::Gamma => g.gamma,
            Field::Shadows => g.shadows,
            Field::Highlights => g.highlights,
        }
    }

    pub fn set(self, g: &mut Grade, v: f32) {
        match self {
            Field::Exposure => g.exposure = v,
            Field::Contrast => g.contrast = v,
            Field::Saturation => g.saturation = v,
            Field::Hue => g.hue = v,
            Field::Temperature => g.temperature = v,
            Field::Tint => g.tint = v,
            Field::Gamma => g.gamma = v,
            Field::Shadows => g.shadows = v,
            Field::Highlights => g.highlights = v,
        }
    }
}

/// The four color wheels: per-channel triples with a neutral value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Wheel {
    Lift,
    Gamma,
    Gain,
    Offset,
}

pub const WHEELS: [Wheel; 4] = [Wheel::Lift, Wheel::Gamma, Wheel::Gain, Wheel::Offset];

impl Wheel {
    pub fn label(self) -> &'static str {
        match self {
            Wheel::Lift => "Lift",
            Wheel::Gamma => "Gamma",
            Wheel::Gain => "Gain",
            Wheel::Offset => "Offset",
        }
    }

    pub fn neutral(self) -> f32 {
        match self {
            Wheel::Lift | Wheel::Offset => 0.0,
            Wheel::Gamma | Wheel::Gain => 1.0,
        }
    }

    /// Largest master change either way, in model units.
    pub fn range(self) -> f32 {
        let spec = match self {
            Wheel::Lift => Key::Lift(0).spec(),
            Wheel::Gamma => Key::GammaRgb(0).spec(),
            Wheel::Gain => Key::Gain(0).spec(),
            Wheel::Offset => return 0.25,
        };
        (spec.max / spec.factor - self.neutral())
            .abs()
            .min((spec.min / spec.factor - self.neutral()).abs())
            .max(0.25)
    }

    pub fn get(self, g: &Grade) -> [f32; 3] {
        match self {
            Wheel::Lift => g.lift,
            Wheel::Gamma => g.gamma_rgb,
            Wheel::Gain => g.gain,
            Wheel::Offset => g.offset,
        }
    }

    pub fn set(self, g: &mut Grade, v: [f32; 3]) {
        match self {
            Wheel::Lift => g.lift = v,
            Wheel::Gamma => g.gamma_rgb = v,
            Wheel::Gain => g.gain = v,
            Wheel::Offset => g.offset = v,
        }
    }
}
