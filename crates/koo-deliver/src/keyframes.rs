//! Keyframe evaluation, both as numbers and as FFmpeg expressions over time.
//!
//! Kept small on purpose: koo-core is getting a shared evaluator, and this
//! should be swapped for it once that lands. The easing curves here must
//! match it.

use koo_core::Time;
use koo_core::model::{Ease, Keyframe, Property, Transform};

/// Eased progress for `u` in 0..1.
pub fn ease(ease: Ease, u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    match ease {
        Ease::Linear => u,
        Ease::EaseInOut => u * u * (3.0 - 2.0 * u),
        Ease::EaseIn => u * u,
        Ease::EaseOut => u * (2.0 - u),
        Ease::Hold => 0.0,
    }
}

/// The transform's own value for `property`, used when it has no keyframes.
pub fn static_value(transform: &Transform, property: Property) -> f32 {
    match property {
        Property::PositionX => transform.position[0],
        Property::PositionY => transform.position[1],
        Property::Scale => transform.scale,
        Property::Rotation => transform.rotation_deg,
        Property::Opacity => transform.opacity,
    }
}

/// Keyframes of one property, sorted by time.
pub fn track(keys: &[Keyframe], property: Property) -> Vec<&Keyframe> {
    let mut keys: Vec<&Keyframe> = keys.iter().filter(|k| k.property == property).collect();
    keys.sort_by_key(|k| k.at);
    keys
}

/// Value of `property` at clip-relative time `t`.
pub fn value_at(transform: &Transform, keys: &[Keyframe], property: Property, t: Time) -> f32 {
    let keys = track(keys, property);
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
        return static_value(transform, property);
    };
    if t <= first.at {
        return first.value;
    }
    if t >= last.at {
        return last.value;
    }
    let i = keys.windows(2).position(|w| t < w[1].at).unwrap_or(0);
    let (a, b) = (keys[i], keys[i + 1]);
    let u = (t - a.at).as_secs_f64() / (b.at - a.at).as_secs_f64().max(1e-9);
    (a.value as f64 + (b.value - a.value) as f64 * ease(a.ease, u)) as f32
}

/// FFmpeg expression for eased progress, given progress expression `u`.
fn ease_expr(ease: Ease, u: &str) -> String {
    match ease {
        Ease::Linear => u.to_string(),
        Ease::EaseInOut => format!("{u}*{u}*(3-2*{u})"),
        Ease::EaseIn => format!("{u}*{u}"),
        Ease::EaseOut => format!("{u}*(2-{u})"),
        Ease::Hold => "0".into(),
    }
}

/// An animatable value: a constant or an expression in a time variable.
#[derive(Clone, Debug)]
pub enum Anim {
    Const(f64),
    Expr(String),
}

impl Anim {
    pub fn is_const(&self) -> bool {
        matches!(self, Anim::Const(_))
    }

    /// The value as an expression term.
    pub fn term(&self) -> String {
        match self {
            Anim::Const(v) => num(*v),
            Anim::Expr(e) => format!("({e})"),
        }
    }

    /// Constant value, or the value at the end of the animation (what
    /// FFmpeg sees when it evaluates an expression with no time).
    pub fn constant(&self) -> Option<f64> {
        match self {
            Anim::Const(v) => Some(*v),
            Anim::Expr(_) => None,
        }
    }
}

/// `property` of a clip as an [`Anim`] over the time expression `var`,
/// which must evaluate to clip-relative seconds (e.g. `(t+1.5)`).
pub fn anim(transform: &Transform, keys: &[Keyframe], property: Property, var: &str) -> Anim {
    let keys = track(keys, property);
    match keys.as_slice() {
        [] => Anim::Const(static_value(transform, property) as f64),
        [only] => Anim::Const(only.value as f64),
        _ => {
            let last = keys[keys.len() - 1];
            let mut expr = num(last.value as f64);
            for w in keys.windows(2).rev() {
                let (a, b) = (w[0], w[1]);
                let (ta, tb) = (a.at.as_secs_f64(), b.at.as_secs_f64());
                let u = format!("clip(({var}-{})/{},0,1)", num(ta), num((tb - ta).max(1e-6)));
                let seg = format!(
                    "{}+{}*({})",
                    num(a.value as f64),
                    num((b.value - a.value) as f64),
                    ease_expr(a.ease, &u)
                );
                expr = format!("if(lt({var},{}),{seg},{expr})", num(tb));
            }
            let first = keys[0];
            Anim::Expr(format!(
                "if(lt({var},{}),{},{expr})",
                num(first.at.as_secs_f64()),
                num(first.value as f64)
            ))
        }
    }
}

/// Compact decimal for expressions.
pub(crate) fn num(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}
