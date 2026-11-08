//! Values over time: easing between keyframes, a transform's properties
//! by name, and a clip's keyframed values and source mapping.

use super::{Clip, ClipContent, Ease, Keyframe, Property, Transform};
use crate::time::{Time, TimeRange};

impl Ease {
    /// Progress `u` in 0..1 between two keyframes → eased 0..1.
    pub fn apply(self, u: f32) -> f32 {
        let u = u.clamp(0.0, 1.0);
        match self {
            Ease::Linear => u,
            Ease::EaseIn => u * u * u,
            Ease::EaseOut => 1.0 - (1.0 - u).powi(3),
            Ease::EaseInOut if u < 0.5 => 4.0 * u * u * u,
            Ease::EaseInOut => 1.0 - (-2.0 * u + 2.0).powi(3) / 2.0,
            Ease::Hold => 0.0,
        }
    }
}

impl Transform {
    /// [`Property::Volume`] is not a transform property and reads 0.
    pub fn get(&self, property: Property) -> f32 {
        match property {
            Property::PositionX => self.position[0],
            Property::PositionY => self.position[1],
            Property::Scale => self.scale,
            Property::Rotation => self.rotation_deg,
            Property::Opacity => self.opacity,
            Property::CropLeft => self.crop[0],
            Property::CropTop => self.crop[1],
            Property::CropRight => self.crop[2],
            Property::CropBottom => self.crop[3],
            Property::ScaleX => self.stretch[0],
            Property::ScaleY => self.stretch[1],
            Property::SkewX => self.skew_deg[0],
            Property::SkewY => self.skew_deg[1],
            Property::AnchorX => self.anchor[0],
            Property::AnchorY => self.anchor[1],
            Property::Volume => 0.0,
            p => match pin_index(p) {
                Some((corner, axis)) => self.corner_pin[corner][axis],
                None => 0.0,
            },
        }
    }

    /// Setting [`Property::Volume`] does nothing.
    pub fn set(&mut self, property: Property, value: f32) {
        match property {
            Property::PositionX => self.position[0] = value,
            Property::PositionY => self.position[1] = value,
            Property::Scale => self.scale = value,
            Property::Rotation => self.rotation_deg = value,
            Property::Opacity => self.opacity = value,
            Property::CropLeft => self.crop[0] = value,
            Property::CropTop => self.crop[1] = value,
            Property::CropRight => self.crop[2] = value,
            Property::CropBottom => self.crop[3] = value,
            Property::ScaleX => self.stretch[0] = value,
            Property::ScaleY => self.stretch[1] = value,
            Property::SkewX => self.skew_deg[0] = value,
            Property::SkewY => self.skew_deg[1] = value,
            Property::AnchorX => self.anchor[0] = value,
            Property::AnchorY => self.anchor[1] = value,
            Property::Volume => {}
            p => {
                if let Some((corner, axis)) = pin_index(p) {
                    self.corner_pin[corner][axis] = value;
                }
            }
        }
    }
}

/// The corner and axis of a [`Property::PIN`] property.
fn pin_index(p: Property) -> Option<(usize, usize)> {
    let i = Property::PIN.iter().position(|q| *q == p)?;
    Some((i / 2, i % 2))
}

impl Clip {
    /// The un-keyframed value of `property`: the [`Transform`] field, or the
    /// clip gain for [`Property::Volume`].
    pub fn base_value(&self, property: Property) -> f32 {
        match property {
            Property::Volume => self.audio.gain_db,
            p => self.transform.get(p),
        }
    }

    /// Value of `property` at clip-relative time `t`. Without keyframes for
    /// it, [`Clip::base_value`]; before the first and after the
    /// last keyframe, that keyframe's value; in between, eased by the earlier
    /// keyframe's [`Ease`].
    pub fn value_at(&self, property: Property, t: Time) -> f32 {
        let (mut prev, mut next): (Option<&Keyframe>, Option<&Keyframe>) = (None, None);
        for k in self.keyframes.iter().filter(|k| k.property == property) {
            if k.at <= t {
                if prev.is_none_or(|p| k.at >= p.at) {
                    prev = Some(k);
                }
            } else if next.is_none_or(|n| k.at < n.at) {
                next = Some(k);
            }
        }
        match (prev, next) {
            (None, None) => self.base_value(property),
            (Some(k), None) | (None, Some(k)) => k.value,
            (Some(a), Some(b)) => {
                let u = (t - a.at).0 as f64 / (b.at - a.at).0 as f64;
                a.value + (b.value - a.value) * a.ease.apply(u as f32)
            }
        }
    }

    /// The full transform at clip-relative time `t`.
    pub fn transform_at(&self, t: Time) -> Transform {
        let mut out = self.transform.clone();
        for p in Property::TRANSFORM {
            out.set(p, self.value_at(p, t));
        }
        out
    }

    /// Where a source range lands on the timeline through this clip's speed
    /// (and ramp), unclamped (it may lie outside the clip).
    pub fn source_to_timeline_range(&self, source: TimeRange) -> TimeRange {
        let source_in = match &self.content {
            ClipContent::Media { source_in, .. } => *source_in,
            _ => Time::ZERO,
        };
        TimeRange::new(
            self.start + self.timeline_offset(source.start - source_in),
            self.start + self.timeline_offset(source.end - source_in),
        )
    }

    /// Source range this clip plays, for media clips (`duration × speed`,
    /// or what its ramp plays).
    pub fn source_range(&self) -> Option<TimeRange> {
        match &self.content {
            ClipContent::Media { source_in, .. } => Some(TimeRange::new(
                *source_in,
                *source_in + self.source_length(),
            )),
            _ => None,
        }
    }
}
