//! Clip transform at a timeline time, with keyframes applied. Small on
//! purpose: the core stream adds the shared evaluator the renderer uses.

use koo_core::Time;
use koo_core::model::{Clip, Ease, Keyframe, Property, Transform};

pub fn evaluate(clip: &Clip, t: Time) -> Transform {
    let at = t - clip.start;
    let base = &clip.transform;
    let value = |property: Property, base: f32| {
        let mut keys: Vec<&Keyframe> = clip
            .keyframes
            .iter()
            .filter(|k| k.property == property)
            .collect();
        keys.sort_by_key(|k| k.at);
        sample(&keys, at).unwrap_or(base)
    };
    Transform {
        position: [
            value(Property::PositionX, base.position[0]),
            value(Property::PositionY, base.position[1]),
        ],
        scale: value(Property::Scale, base.scale),
        rotation_deg: value(Property::Rotation, base.rotation_deg),
        opacity: value(Property::Opacity, base.opacity).clamp(0., 1.),
    }
}

fn sample(keys: &[&Keyframe], at: Time) -> Option<f32> {
    let first = keys.first()?;
    if at <= first.at {
        return Some(first.value);
    }
    for pair in keys.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if at < b.at {
            let span = (b.at - a.at).0.max(1) as f32;
            let x = ((at - a.at).0 as f32 / span).clamp(0., 1.);
            return Some(a.value + (b.value - a.value) * ease(a.ease, x));
        }
    }
    keys.last().map(|k| k.value)
}

fn ease(ease: Ease, x: f32) -> f32 {
    match ease {
        Ease::Linear => x,
        Ease::EaseInOut => x * x * (3. - 2. * x),
        Ease::EaseIn => x * x,
        Ease::EaseOut => 1. - (1. - x) * (1. - x),
        Ease::Hold => 0.,
    }
}
