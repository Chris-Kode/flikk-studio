//! Play buttons bound to a media item: they show whether the source
//! preview plays it and toggle it on click.

use crate::state::EditorState;
use crate::ui::kit::play::{play_button, play_dot};
use crate::ui::kit::{Button, Tooltip};
use flikk_core::model::MediaItem;
use gpui_kit::*;

/// Play or pause `item` from a row or form: an icon button that holds the
/// hover surface while it plays.
pub(crate) fn listen_button(
    id: impl Into<ElementId>,
    state: &Entity<EditorState>,
    item: &MediaItem,
    cx: &App,
) -> Button {
    let preview = state.read(cx).source.as_ref();
    let playing = preview.is_some_and(|p| p.plays(item));
    let (state, item) = (state.clone(), item.clone());
    play_button(id, playing)
        .tooltip(if playing { "Pause" } else { "Play" })
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            state.update(cx, |s, cx| s.toggle_source(&item, cx));
        })
}

/// A round play button `size` across; white while `item` plays.
pub(crate) fn listen_dot(
    id: impl Into<ElementId>,
    state: &Entity<EditorState>,
    item: &MediaItem,
    size: Pixels,
    cx: &App,
) -> Stateful<Div> {
    let preview = state.read(cx).source.as_ref();
    let playing = preview.is_some_and(|p| p.plays(item));
    let (state, item) = (state.clone(), item.clone());
    play_dot(id, playing, size)
        .tooltip(Tooltip::text(if playing { "Pause" } else { "Play" }))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            state.update(cx, |s, cx| s.toggle_source(&item, cx));
        })
}
