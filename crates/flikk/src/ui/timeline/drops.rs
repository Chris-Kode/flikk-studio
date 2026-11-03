//! Drops from the effects library: effects on clips, transitions on cuts
//! and clip edges, generators on lanes, audio effects on audio clips and
//! audio track headers. Each lands as one command.

use super::fx::{self, EdgeTarget};
use super::layout::{self, RULER_HEIGHT};
use super::paint::DropHint;
use super::{Hit, TimelinePanel};
use crate::ui::{AudioFxDrag, FxDrag};
use flikk_core::audiofx::AudioFx;
use flikk_core::command::Command;
use flikk_core::fx::{FxInstance, FxKind, Transition};
use flikk_core::model::TrackKind;
use flikk_core::{ClipId, Time, TimeRange, TrackId};
use gpui_kit::*;

/// Pointer distance (px) within which a transition snaps to a clip edge.
const EDGE_SNAP_PX: f32 = 14.;

enum FxTarget {
    Effect(TrackId, ClipId, TimeRange),
    Transition(TrackId, EdgeTarget),
    Generator(TrackId, TimeRange),
}

impl TimelinePanel {
    /// Lanes below the ruler contain `p`.
    fn over_lanes(&self, p: Point<Pixels>) -> bool {
        let b = self.bounds.get();
        b.contains(&p) && p.y >= b.origin.y + px(RULER_HEIGHT)
    }

    fn fx_target(&self, drag: &FxDrag, p: Point<Pixels>, cx: &App) -> Option<FxTarget> {
        if !self.over_lanes(p) {
            return None;
        }
        let (x, y) = self.local(p);
        let project = self.state.read(cx).bus.project();
        let tl = &project.timeline;
        let row = *layout::row_at(&self.rows(tl), y)?;
        let track = tl.track(row.track).filter(|t| !t.locked)?;
        let at = self.viewport.time(x).max(Time::ZERO);
        match drag.kind {
            FxKind::Effect => {
                if !fx::takes_pictures(track.kind) {
                    return None;
                }
                let clip = track.clip_at(at)?;
                Some(FxTarget::Effect(track.id, clip.id, clip.range()))
            }
            FxKind::Transition => {
                let target = fx::edge_target(track, at, self.viewport.span(EDGE_SNAP_PX))?;
                Some(FxTarget::Transition(track.id, target))
            }
            FxKind::Generator => {
                if !fx::takes_pictures(track.kind) {
                    return None;
                }
                let at = at.floor_to_frame(tl.settings.rate);
                let want = Time::from_secs_f64(fx::GENERATOR_SECS);
                let range = fx::free_span(track, at, want)?;
                Some(FxTarget::Generator(track.id, range))
            }
        }
    }

    pub(super) fn fx_drag_moved(
        &mut self,
        drag: &FxDrag,
        p: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let hint = self.fx_target(drag, p, cx).map(|t| match t {
            FxTarget::Effect(track, _, range) => DropHint::Clip(track, range),
            FxTarget::Transition(track, t) => DropHint::Span(track, t.preview),
            FxTarget::Generator(track, range) => DropHint::Span(track, range),
        });
        self.set_drop(hint, cx);
    }

    pub(super) fn set_drop(&mut self, hint: Option<DropHint>, cx: &mut Context<Self>) {
        if hint != self.drop {
            self.drop = hint;
            cx.notify();
        }
    }

    pub(super) fn drop_fx(&mut self, drag: &FxDrag, window: &Window, cx: &mut Context<Self>) {
        self.drop = None;
        let Some(target) = self.fx_target(drag, window.mouse_position(), cx) else {
            cx.notify();
            return;
        };
        let Some(program) = drag.program(&self.state, cx) else {
            return;
        };
        match target {
            FxTarget::Effect(_, clip, _) => {
                // On a selected clip: the whole picture selection gets it.
                let clips = {
                    let state = self.state.read(cx);
                    let tl = &state.bus.project().timeline;
                    let selected = &state.selection.clips;
                    if selected.contains(&clip) {
                        selected
                            .iter()
                            .copied()
                            .filter(|id| {
                                tl.clip(*id)
                                    .is_some_and(|(t, _)| fx::takes_pictures(t.kind))
                            })
                            .collect()
                    } else {
                        vec![clip]
                    }
                };
                let effect = FxInstance::new(program);
                self.state.update(cx, |s, cx| {
                    s.edit(
                        Command::AddEffect {
                            clips,
                            effect,
                            index: None,
                        },
                        cx,
                    );
                });
            }
            FxTarget::Transition(_, target) => {
                let (clip, edge) = target.owner;
                let transition = Some(Transition::new(program, target.duration));
                let applied = self.state.update(cx, |s, cx| {
                    s.edit(
                        Command::SetTransition {
                            clip,
                            edge,
                            transition,
                        },
                        cx,
                    )
                    .is_some()
                });
                if applied {
                    self.select_transition(target.owner, cx);
                }
            }
            FxTarget::Generator(track, range) => {
                let fx = FxInstance::new(program);
                let added = self.state.update(cx, |s, cx| {
                    s.edit(Command::AddGenerator { track, range, fx }, cx)?;
                    s.bus
                        .project()
                        .timeline
                        .track(track)?
                        .clips
                        .iter()
                        .find(|c| c.start == range.start)
                        .map(|c| c.id)
                });
                if let Some(id) = added {
                    self.select(vec![id], cx);
                }
            }
        }
        cx.notify();
    }

    /// The audio clip under `p`.
    fn audio_target(&self, p: Point<Pixels>, cx: &App) -> Option<(TrackId, ClipId, TimeRange)> {
        if !self.over_lanes(p) {
            return None;
        }
        let Hit::Clip { row, clip, .. } = self.hit(p, cx) else {
            return None;
        };
        let tl = &self.state.read(cx).bus.project().timeline;
        let (track, c) = tl.clip(clip)?;
        (track.kind == TrackKind::Audio && !track.locked && c.media().is_some())
            .then(|| (row.track, clip, c.range()))
    }

    pub(super) fn audio_fx_drag_moved(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) {
        let hint = self
            .audio_target(p, cx)
            .map(|(track, _, range)| DropHint::Clip(track, range));
        self.set_drop(hint, cx);
    }

    /// Append the effect to the clip under the pointer, or to every
    /// selected audio clip when that clip is part of the selection.
    pub(super) fn drop_audio_fx(
        &mut self,
        drag: &AudioFxDrag,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.drop = None;
        let Some((_, clip, _)) = self.audio_target(window.mouse_position(), cx) else {
            cx.notify();
            return;
        };
        self.state.update(cx, |s, cx| {
            let tl = &s.bus.project().timeline;
            let targets: Vec<ClipId> = if s.selection.clips.contains(&clip) {
                s.selection.clips.clone()
            } else {
                vec![clip]
            };
            let commands: Vec<Command> = targets
                .iter()
                .filter_map(|id| tl.clip(*id))
                .filter(|(t, c)| t.kind == TrackKind::Audio && c.media().is_some())
                .map(|(_, c)| {
                    let mut effects = c.audio.effects.clone();
                    effects.push(AudioFx::new(drag.kind));
                    Command::SetAudioEffects {
                        clip: c.id,
                        effects,
                    }
                })
                .collect();
            if commands.len() == 1 {
                s.edit(commands.into_iter().next().unwrap(), cx);
            } else if !commands.is_empty() {
                s.edit_batch(commands, "set_audio_effects", cx);
            }
        });
        cx.notify();
    }

    /// An audio effect dropped on an audio track header: a track insert.
    pub(super) fn drop_audio_fx_on_track(
        &mut self,
        drag: &AudioFxDrag,
        track: TrackId,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            let Some(t) = s.bus.project().timeline.track(track) else {
                return;
            };
            if t.kind != TrackKind::Audio {
                return;
            }
            let mut mix = t.mix.clone();
            mix.effects.push(AudioFx::new(drag.kind));
            s.edit(Command::SetTrackMix { track, mix }, cx);
        });
    }
}
