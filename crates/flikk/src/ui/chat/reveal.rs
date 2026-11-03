//! Clicking a reference in the chat shows its object in the editor.

use crate::state::EditorState;
use flikk_core::command::Edge;
use flikk_core::query::{self, Reference};
use flikk_core::{ClipId, Time};
use gpui_kit::*;

/// Select and seek to what `token` names: a clip (or the clip of an effect
/// or transition), a marker, range or time; open media as the source; select
/// the clips of a track or the clips using an FX asset or audio effect.
pub fn reveal(state: &Entity<EditorState>, token: &str, cx: &mut App) {
    state.update(cx, |s, cx| {
        let project = s.bus.project();
        let tl = &project.timeline;
        let Some(reference) = query::resolve_project_references(project, token)
            .into_iter()
            .next()
            .map(|r| r.reference)
        else {
            return;
        };
        let clip_start = |clip: ClipId| tl.clip(clip).map(|(_, c)| (c.start, c.end()));
        let mut transition = None;
        let (clips, seek): (Option<Vec<ClipId>>, Option<Time>) = match reference {
            Reference::Clip { clip } | Reference::Effect { clip, .. } => {
                (Some(vec![clip]), clip_start(clip).map(|(start, _)| start))
            }
            Reference::Transition { clip, edge } => {
                transition = Some((clip, edge));
                let at = clip_start(clip).map(|(start, end)| match edge {
                    Edge::Start => start,
                    Edge::End => end,
                });
                (Some(vec![clip]), at)
            }
            Reference::Media { media } => {
                s.open_source(media, cx);
                return;
            }
            Reference::Marker { marker } => (None, tl.marker(marker).map(|m| m.range.start)),
            Reference::Track { track } => (
                tl.track(track)
                    .map(|t| t.clips.iter().map(|c| c.id).collect()),
                None,
            ),
            Reference::Fx { fx } => (Some(flikk_core::fx::uses(project, &fx)), None),
            Reference::AudioFx { kind } => (
                Some(
                    tl.tracks
                        .iter()
                        .flat_map(|t| &t.clips)
                        .filter(|c| c.audio.effects.iter().any(|e| e.kind == kind))
                        .map(|c| c.id)
                        .collect(),
                ),
                None,
            ),
            Reference::Range { range } => (None, Some(range.start)),
            Reference::At { time } => (None, Some(time)),
            Reference::Bin { .. } | Reference::Unresolved { .. } => return,
        };
        if let Some(clips) = clips {
            s.select_clips(clips, cx);
        }
        if transition.is_some() {
            s.select_transition(transition, cx);
        }
        if let Some(t) = seek {
            s.seek(t, cx);
        }
    });
}
