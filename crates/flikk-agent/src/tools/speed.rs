//! `set_speed`: one speed, a ramp (points or a preset), and how slowed
//! frames and ramped audio play.

use super::ToolCtx;
use crate::args::*;
use flikk_core::command::Command;
use flikk_core::speed::{Interpolation, RampAudio, SpeedPreset};
use serde_json::Value;

pub fn set_speed(ctx: &mut ToolCtx, args: &Args) -> ToolResultOf<Vec<Command>> {
    let project = ctx.project();
    let id = clip_id(project, args.str("clip")?)?;
    let Some((_, clip)) = project.timeline.clip(id) else {
        return fail("no such clip");
    };
    if clip.media().is_none() {
        return fail("only media clips change speed");
    }
    let speed = args.opt_f64("speed")?;
    if let Some(s) = speed
        && !(0.01..=100.0).contains(&s)
    {
        return fail("`speed` is 0.01..100 (1 = normal)");
    }
    let ripple = args.opt_bool("ripple")?.unwrap_or(false);
    let keep_length = args.opt_bool("keep_length")?.unwrap_or(false);
    let preset = match args.opt_str("preset")? {
        Some(p) => Some(SpeedPreset::from_key(p).ok_or_else(|| {
            let keys: Vec<_> = SpeedPreset::ALL.iter().map(|p| p.key()).collect();
            ToolError(format!("`preset` is one of {}", keys.join(", ")))
        })?),
        None => None,
    };
    let ramp = match args.value("ramp") {
        None => None,
        Some(Value::Array(points)) => Some(
            points
                .iter()
                .map(|p| match p.as_array().map(Vec::as_slice) {
                    Some([u, s]) => match (u.as_f64(), s.as_f64()) {
                        (Some(u), Some(s)) => Ok([u, s]),
                        _ => fail("`ramp` is [[position 0..1, speed], …]"),
                    },
                    _ => fail("`ramp` is [[position 0..1, speed], …]"),
                })
                .collect::<ToolResultOf<Vec<[f64; 2]>>>()?,
        ),
        Some(_) => return fail("`ramp` is [[position 0..1, speed], …]"),
    };
    let interpolation = match args.opt_str("interpolation")? {
        Some(i) => Some(Interpolation::from_key(i).ok_or_else(|| {
            ToolError("`interpolation` is nearest, blend or optical_flow".into())
        })?),
        None => None,
    };
    let audio = match args.opt_str("ramp_audio")? {
        Some("stretch") => Some(RampAudio::Stretch),
        Some("mute") => Some(RampAudio::Mute),
        Some(_) => return fail("`ramp_audio` is stretch or mute"),
        None => None,
    };
    let mut out = Vec::new();
    let points = match (preset, ramp) {
        (Some(_), Some(_)) => return fail("give `preset` or `ramp`, not both"),
        (Some(p), None) => Some(p.points(speed.unwrap_or(clip.speed))),
        (None, Some(r)) => Some(r),
        // One speed on a ramped clip ends the ramp.
        (None, None) if clip.is_ramped() => speed.map(|s| vec![[0.0, s]]),
        (None, None) => {
            if let Some(speed) = speed {
                out.push(Command::SetSpeed {
                    clip: id,
                    speed,
                    ripple,
                });
            }
            None
        }
    };
    if let Some(points) = points {
        out.push(Command::SetSpeedRamp {
            clip: id,
            points,
            keep_length,
            ripple,
        });
    }
    if interpolation.is_some() || audio.is_some() {
        out.push(Command::SetRetime {
            clip: id,
            interpolation,
            audio,
        });
    }
    if out.is_empty() {
        return fail("give `speed`, `ramp`, `preset`, `interpolation` or `ramp_audio`");
    }
    Ok(out)
}
