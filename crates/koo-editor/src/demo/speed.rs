//! `KOOEDITOR_DEMO_SPEED=1`: the selected clip ramped (Hero) and blending
//! its slowed frames, its Speed tab open.

use koo_core::ClipId;
use koo_core::command::Command;
use koo_core::model::Project;
use koo_core::speed::{Interpolation, SpeedPreset};

pub fn enabled() -> bool {
    super::enabled() && std::env::var("KOOEDITOR_DEMO_SPEED").is_ok_and(|v| v == "1")
}

/// Ramp `clip` and blend its frames, as the user would.
pub fn seed(project: &mut Project, clip: ClipId) {
    if !enabled() {
        return;
    }
    let base = project.timeline.clip(clip).map_or(1.0, |(_, c)| c.speed);
    let commands = [
        Command::SetSpeedRamp {
            clip,
            points: SpeedPreset::Hero.points(base),
            keep_length: true,
            ripple: false,
        },
        Command::SetRetime {
            clip,
            interpolation: Some(Interpolation::Blend),
            audio: None,
        },
    ];
    for command in commands {
        match command.plan(project) {
            Ok(plan) => {
                if let Err(e) = koo_core::ops::apply_all(project, &plan.ops) {
                    eprintln!("demo speed ramp: {e}");
                }
            }
            Err(e) => eprintln!("demo speed ramp: {e}"),
        }
    }
}
