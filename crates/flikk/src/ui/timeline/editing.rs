//! Selection edits: clipboard (copy, cut, paste at the playhead,
//! duplicate), nudge, enable/disable, delete, clip properties from the
//! context menu, and "Add to chat". Every change is a `Command`.

use super::{TimelinePanel, refs, transition_of};
use flikk_core::command::{Command, Edge, InsertMode, PlacedClip};
use flikk_core::model::{Clip, ClipColor, Timeline};
use flikk_core::{ClipId, Time, TimeRange, TrackId};
use gpui_kit::*;

impl TimelinePanel {
    /// Selected clips with their tracks, in timeline order.
    fn selected_clips(&self, cx: &App) -> Vec<(TrackId, Clip)> {
        let state = self.state.read(cx);
        let tl = &state.bus.project().timeline;
        let mut out: Vec<(TrackId, Clip)> = state
            .selection
            .clips
            .iter()
            .filter_map(|id| tl.clip(*id).map(|(t, c)| (t.id, c.clone())))
            .collect();
        out.sort_by_key(|(_, c)| c.start);
        out
    }

    pub(super) fn copy(&mut self, cx: &mut Context<Self>) {
        let clips = self.selected_clips(cx);
        if !clips.is_empty() {
            self.clipboard = clips;
        }
    }

    pub(super) fn cut(&mut self, cx: &mut Context<Self>) {
        self.copy(cx);
        self.delete_selection(false, cx);
    }

    /// Paste at the playhead: same tracks, same offsets between clips.
    pub(super) fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(first) = self.clipboard.iter().map(|(_, c)| c.start).min() else {
            return;
        };
        let playhead = self.state.read(cx).playhead;
        let clips = self.clipboard.clone();
        self.place(clips, playhead - first, "paste", cx);
    }

    /// Copies of the selection right after it.
    pub(super) fn duplicate(&mut self, cx: &mut Context<Self>) {
        let clips = self.selected_clips(cx);
        let (Some(first), Some(last)) = (
            clips.iter().map(|(_, c)| c.start).min(),
            clips.iter().map(|(_, c)| c.end()).max(),
        ) else {
            return;
        };
        self.place(clips, last - first, "duplicate", cx);
    }

    /// Place copies of `clips` shifted by `by` (overwriting what is there)
    /// and select them.
    fn place(
        &mut self,
        clips: Vec<(TrackId, Clip)>,
        by: Time,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        let placed: Vec<PlacedClip> = {
            let tl = &self.state.read(cx).bus.project().timeline;
            clips
                .into_iter()
                .filter(|(track, _)| tl.track(*track).is_some())
                .map(|(track, mut clip)| {
                    clip.start = clip.start + by;
                    PlacedClip { track, clip }
                })
                .filter(|p| p.clip.start >= Time::ZERO)
                .collect()
        };
        if placed.is_empty() {
            return;
        }
        let spots: Vec<(TrackId, TimeRange)> =
            placed.iter().map(|p| (p.track, p.clip.range())).collect();
        let before: Vec<ClipId> = all_clips(&self.state.read(cx).bus.project().timeline);
        let command = Command::PlaceClips {
            clips: placed,
            mode: InsertMode::Overwrite,
        };
        let added = self.state.update(cx, |s, cx| {
            s.edit_batch(vec![command], label, cx)?;
            let tl = &s.bus.project().timeline;
            Some(
                spots
                    .iter()
                    .filter_map(|(track, range)| {
                        tl.track(*track)?
                            .clips
                            .iter()
                            .find(|c| c.range() == *range && !before.contains(&c.id))
                            .map(|c| c.id)
                    })
                    .collect::<Vec<_>>(),
            )
        });
        if let Some(added) = added {
            self.select(added, cx);
        }
    }

    /// Move the selection by `frames` (one undo step).
    pub(super) fn nudge(&mut self, frames: i64, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let tl = &s.bus.project().timeline;
            let delta = Time(tl.settings.rate.frame_duration().0 * frames);
            let mut moving: Vec<(ClipId, Time)> = s
                .selection
                .clips
                .iter()
                .filter_map(|id| tl.clip(*id).map(|(_, c)| (c.id, c.start)))
                .collect();
            if moving.is_empty() || moving.iter().any(|(_, start)| *start + delta < Time::ZERO) {
                return;
            }
            // Move the leading clip first so neighbours in the selection
            // never collide with each other.
            moving.sort_by_key(|(_, start)| *start);
            if frames > 0 {
                moving.reverse();
            }
            let commands = moving
                .into_iter()
                .map(|(clip, start)| Command::MoveClip {
                    clip,
                    at: start + delta,
                    track: None,
                })
                .collect();
            s.edit_batch(commands, "nudge", cx);
        });
    }

    /// `d`: disable the selection, or enable it when it is all disabled.
    pub(super) fn toggle_enabled(&mut self, cx: &mut Context<Self>) {
        let clips: Vec<Clip> = self
            .selected_clips(cx)
            .into_iter()
            .map(|(_, c)| c)
            .collect();
        if clips.is_empty() {
            return;
        }
        let enabled = clips.iter().all(|c| !c.enabled);
        let ids = clips.iter().map(|c| c.id).collect();
        self.set_enabled(ids, enabled, cx);
    }

    pub(super) fn set_enabled(
        &mut self,
        clips: Vec<ClipId>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            s.edit(Command::SetClipsEnabled { clips, enabled }, cx);
        });
    }

    /// Delete the selected clips, or the selected transition when no clip is
    /// selected.
    pub(super) fn delete_selection(&mut self, ripple: bool, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        if let Some(owner) = state.selected_transition
            && state.selection.clips.is_empty()
        {
            self.remove_transition(owner, cx);
            return;
        }
        self.state.update(cx, |s, cx| {
            let clips = s.selection.clips.clone();
            if clips.is_empty() {
                return;
            }
            if s.edit(Command::DeleteClips { clips, ripple }, cx).is_some() {
                s.select_clips(vec![], cx);
            }
        });
    }

    pub(super) fn remove_transition(
        &mut self,
        (clip, edge): (ClipId, Edge),
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            if transition_of(s.bus.project(), (clip, edge)).is_some() {
                s.edit(
                    Command::SetTransition {
                        clip,
                        edge,
                        transition: None,
                    },
                    cx,
                );
            }
            s.select_transition(None, cx);
        });
    }

    pub(super) fn set_color(&mut self, color: Option<ClipColor>, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let clips = s.selection.clips.clone();
            if !clips.is_empty() {
                s.edit(Command::SetClipColor { clips, color }, cx);
            }
        });
    }

    /// "Add to chat" for the clip selection.
    pub(super) fn mention_selection(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let project = s.bus.project();
            let refs: Vec<refs::Ref> = s
                .selection
                .clips
                .iter()
                .filter_map(|id| project.timeline.clip(*id))
                .map(|(_, c)| refs::clip(c, project))
                .collect();
            if !refs.is_empty() {
                s.mention(refs, cx);
            }
        });
    }

    pub(super) fn mention(&mut self, refs: Vec<refs::Ref>, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.mention(refs, cx));
    }
}

fn all_clips(tl: &Timeline) -> Vec<ClipId> {
    tl.tracks
        .iter()
        .flat_map(|t| &t.clips)
        .map(|c| c.id)
        .collect()
}
