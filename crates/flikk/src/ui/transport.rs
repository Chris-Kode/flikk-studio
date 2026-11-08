//! Program transport, shared by the timeline and the viewer: play, JKL
//! shuttle, frame and cut steps, and the in/out marks. Each function acts
//! on the shared [`EditorState`]; [`with_shared_handlers`] binds the ones
//! both panels handle the same way to their actions.

use crate::app::TogglePlay;
use crate::state::EditorState;
use crate::ui::timeline::{
    ClearInOut, MarkIn, MarkOut, ShuttleForward, ShuttleReverse, ShuttleStop,
};
use flikk_core::Time;
use gpui_kit::*;

/// Fastest shuttle, in either direction, as a multiple of real time.
const MAX_SHUTTLE: f64 = 8.;

/// Registers the handlers both panels share on `el`: play/pause, JKL
/// shuttle and the in/out marks. Frame and cut steps differ per panel and
/// stay with them.
pub(crate) fn with_shared_handlers<E: InteractiveElement>(el: E, state: &Entity<EditorState>) -> E {
    let handler = |f: fn(&Entity<EditorState>, &mut App)| {
        let state = state.clone();
        move |cx: &mut App| f(&state, cx)
    };
    let (play, forward, reverse, stop) = (
        handler(toggle_play),
        handler(shuttle_forward),
        handler(shuttle_reverse),
        handler(shuttle_stop),
    );
    let (mark_in, mark_out, clear) = (
        handler(self::mark_in),
        handler(self::mark_out),
        handler(clear_marks),
    );
    el.on_action(move |_: &TogglePlay, _, cx| play(cx))
        .on_action(move |_: &ShuttleForward, _, cx| forward(cx))
        .on_action(move |_: &ShuttleReverse, _, cx| reverse(cx))
        .on_action(move |_: &ShuttleStop, _, cx| stop(cx))
        .on_action(move |_: &MarkIn, _, cx| mark_in(cx))
        .on_action(move |_: &MarkOut, _, cx| mark_out(cx))
        .on_action(move |_: &ClearInOut, _, cx| clear(cx))
}

/// Space: play or pause the program.
pub(crate) fn toggle_play(state: &Entity<EditorState>, cx: &mut App) {
    state.update(cx, |s, cx| s.toggle_play(cx));
}

/// L: play forward, doubling the speed (up to 8×) while already playing
/// forward.
pub(crate) fn shuttle_forward(state: &Entity<EditorState>, cx: &mut App) {
    state.update(cx, |s, cx| {
        let speed = if s.is_playing() && s.play_speed() > 0. {
            (s.play_speed() * 2.).min(MAX_SHUTTLE)
        } else {
            1.
        };
        s.shuttle(speed, cx);
    });
}

/// J: play backward, doubling the speed (up to 8×) while already playing
/// backward.
pub(crate) fn shuttle_reverse(state: &Entity<EditorState>, cx: &mut App) {
    state.update(cx, |s, cx| {
        let speed = if s.is_playing() && s.play_speed() < 0. {
            (s.play_speed() * 2.).max(-MAX_SHUTTLE)
        } else {
            -1.
        };
        s.shuttle(speed, cx);
    });
}

/// K: stop shuttling.
pub(crate) fn shuttle_stop(state: &Entity<EditorState>, cx: &mut App) {
    state.update(cx, |s, cx| s.shuttle(0., cx));
}

/// Left: pause and move the playhead to the frame before the one it is on.
pub(crate) fn prev_frame(state: &Entity<EditorState>, cx: &mut App) {
    step_frame(state, false, cx);
}

/// Right: pause and move the playhead to the frame after the one it is on.
pub(crate) fn next_frame(state: &Entity<EditorState>, cx: &mut App) {
    step_frame(state, true, cx);
}

fn step_frame(state: &Entity<EditorState>, forward: bool, cx: &mut App) {
    state.update(cx, |s, cx| {
        let frame = s.bus.project().timeline.settings.rate.frame_duration();
        let rate = s.bus.project().timeline.settings.rate;
        let t = s.playhead.floor_to_frame(rate);
        let t = if forward { t + frame } else { t - frame };
        s.set_playing(false, cx);
        s.seek(t, cx);
    });
}

/// Up: see [`step_cut`].
pub(crate) fn prev_cut(state: &Entity<EditorState>, cx: &mut App) {
    state.update(cx, |s, cx| step_cut(s, false, cx));
}

/// Down: see [`step_cut`].
pub(crate) fn next_cut(state: &Entity<EditorState>, cx: &mut App) {
    state.update(cx, |s, cx| step_cut(s, true, cx));
}

/// I: mark in at the playhead.
pub(crate) fn mark_in(state: &Entity<EditorState>, cx: &mut App) {
    state.update(cx, |s, cx| s.mark_in(cx));
}

/// O: mark out at the playhead.
pub(crate) fn mark_out(state: &Entity<EditorState>, cx: &mut App) {
    state.update(cx, |s, cx| s.mark_out(cx));
}

/// Alt-X: clear the in and out marks.
pub(crate) fn clear_marks(state: &Entity<EditorState>, cx: &mut App) {
    state.update(cx, |s, cx| s.clear_marks(cx));
}

/// Up/Down. With a clip selected, select the previous or next clip on its
/// track; otherwise move to the previous or next cut on any track. Either
/// way the playhead lands on the cut.
fn step_cut(s: &mut EditorState, forward: bool, cx: &mut Context<EditorState>) {
    let tl = &s.bus.project().timeline;
    let selected = s.selection.clips.last().and_then(|id| tl.clip(*id));
    if let Some((track, clip)) = selected {
        let i = track
            .clips
            .iter()
            .position(|c| c.id == clip.id)
            .unwrap_or(0);
        let next = if forward {
            track.clips.get(i + 1)
        } else {
            i.checked_sub(1).and_then(|i| track.clips.get(i))
        };
        if let Some(next) = next {
            let (id, start) = (next.id, next.start);
            s.set_playing(false, cx);
            s.select_clips(vec![id], cx);
            s.seek(start, cx);
        }
        return;
    }
    let mut cuts: Vec<Time> = tl
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .flat_map(|c| [c.start, c.end()])
        .chain([Time::ZERO])
        .collect();
    cuts.sort();
    cuts.dedup();
    let t = s.playhead;
    let target = if forward {
        cuts.into_iter().find(|c| *c > t)
    } else {
        cuts.into_iter().rev().find(|c| *c < t)
    };
    if let Some(t) = target {
        s.set_playing(false, cx);
        s.seek(t, cx);
    }
}
