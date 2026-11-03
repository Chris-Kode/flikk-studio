//! `FLIKK_DEMO_DEPTH=1`: the selected clip with a lens blur (its focus
//! pick armed), a 3D camera move and a depth mask, for looking at the
//! depth controls. The demo's media are not real files, so the effects
//! show the picture unchanged.

use crate::state::EditorState;
use flikk_core::bus::Author;
use flikk_core::command::Command;
use flikk_core::fx::{FxInstance, FxRef, depth};
use flikk_core::mask::{Mask, MaskShape};

pub fn seed(state: &mut EditorState) {
    if std::env::var("FLIKK_DEMO_DEPTH").is_err() {
        return;
    }
    let Some(&clip) = state.selection.clips.first() else {
        return;
    };
    let blur = FxInstance::new(FxRef::builtin(depth::LENS_BLUR));
    let pick = blur.id;
    let commands = vec![
        Command::AddEffect {
            clips: vec![clip],
            effect: blur,
            index: None,
        },
        Command::AddEffect {
            clips: vec![clip],
            effect: FxInstance::new(FxRef::builtin(depth::PARALLAX)),
            index: None,
        },
        Command::SetMasks {
            clip,
            masks: vec![Mask::new(MaskShape::Depth {
                near: 0.5,
                far: 1.0,
                softness: 0.05,
            })],
        },
    ];
    for command in commands {
        if let Err(e) = state.bus.execute(command, Author::User, "depth demo") {
            eprintln!("depth demo: {e}");
        }
    }
    // The first add keeps the instance's id.
    let added = state
        .bus
        .project()
        .timeline
        .clip(clip)
        .and_then(|(_, c)| {
            c.effects
                .iter()
                .find(|e| e.id == pick)
                .or(c.effects.first())
        })
        .map(|e| e.id);
    state.depth_pick = added.map(|e| (clip, e));
}
