//! Pointer input on the ruler and lanes: what a point hits, snapping,
//! selecting, and the drags (scrub, move, trim, roll, slip, marquee,
//! transition and track-height) from press to release.

use super::layout::{self, EDGE_PX, RULER_HEIGHT, Row, SNAP_PX};
use super::{MenuTarget, TimelinePanel, Tool, fx, overlay, paint, transition_of, transition_span};
use crate::ui::kit::IconName;
use flikk_core::command::{Command, Edge};
use flikk_core::model::{Clip, ClipContent, Project, Timeline};
use flikk_core::{ChangeItemId, ChangeSetId, ClipId, Time, TimeRange, TrackId};
use gpui_kit::*;

pub(super) enum Drag {
    Scrub,
    Move {
        clips: Vec<ClipId>,
        /// Time under the pointer when the drag started.
        grab: Time,
        from_track: TrackId,
        to_track: TrackId,
        delta: Time,
        moved: bool,
        clicked: ClipId,
    },
    Trim {
        clip: ClipId,
        edge: Edge,
        delta: Time,
        ripple: bool,
    },
    Roll {
        left: ClipId,
        right: ClipId,
        delta: Time,
    },
    Slip {
        clip: ClipId,
        grab: Time,
        /// Timeline time the source moves by (`Command::Slip`).
        delta: Time,
    },
    /// Rubber-band selection from empty lane space.
    Marquee {
        from: (Time, f32),
        to: (Time, f32),
        /// Selection kept when the band started with a modifier.
        base: Vec<ClipId>,
        moved: bool,
    },
    /// A transition's edge: changes its duration.
    TransitionEdge {
        owner: (ClipId, Edge),
        duration: Time,
    },
    /// A track header's bottom border.
    TrackHeight {
        track: TrackId,
        grab: Pixels,
        from: f32,
    },
}

pub(super) enum Hit {
    Ruler,
    Clip {
        row: Row,
        clip: ClipId,
        edge: Option<Edge>,
    },
    Transition {
        row: Row,
        owner: (ClipId, Edge),
        /// Near the block's left or right edge: a duration drag.
        resize: bool,
    },
    Lane {
        row: Option<Row>,
    },
}

/// Room on either side of a media clip's source range, in timeline time:
/// how far its start may move left and its end right. `None` before for
/// clips without a source; `None` after when the source length is unknown.
fn handles(project: &Project, clip: &Clip) -> (Option<Time>, Option<Time>) {
    let ClipContent::Media {
        media, source_in, ..
    } = &clip.content
    else {
        return (None, None);
    };
    let before = (-clip.timeline_offset(-*source_in)).max(Time::ZERO);
    let after = project
        .media(*media)
        .and_then(|m| m.info.as_ref())
        .map(|i| (clip.timeline_offset(i.duration - *source_in) - clip.duration).max(Time::ZERO));
    (Some(before), after)
}

/// `t` rounded down to a whole number of `frame`s.
fn floor_to(t: Time, frame: Time) -> Time {
    Time(t.0 - t.0.rem_euclid(frame.0.max(1)))
}

/// The clips on either side of `clip`'s `edge` when another clip touches
/// it there: `(left, right)` for a roll.
fn neighbour(project: &Project, clip: ClipId, edge: Edge) -> Option<(ClipId, ClipId)> {
    let (track, c) = project.timeline.clip(clip)?;
    match edge {
        Edge::Start => track
            .clips
            .iter()
            .find(|o| o.end() == c.start && o.id != clip)
            .map(|o| (o.id, clip)),
        Edge::End => track
            .clips
            .iter()
            .find(|o| o.start == c.end() && o.id != clip)
            .map(|o| (clip, o.id)),
    }
}

impl TimelinePanel {
    pub(super) fn hit(&self, p: Point<Pixels>, cx: &App) -> Hit {
        let b = self.bounds.get();
        if p.y < b.origin.y + px(RULER_HEIGHT) {
            return Hit::Ruler;
        }
        let (x, y) = self.local(p);
        let project = self.state.read(cx).bus.project();
        let rows = self.rows(&project.timeline);
        let Some(row) = layout::row_at(&rows, y).copied() else {
            return Hit::Lane { row: None };
        };
        let Some(track) = project.timeline.track(row.track) else {
            return Hit::Lane { row: None };
        };
        let vp = &self.viewport;
        // Transitions sit on top of the clip edges they cover.
        for t in fx::placed(track) {
            let (a, b) = transition_span(vp, t.range);
            if x >= a - 2. && x <= b + 2. {
                let resize = b - a >= 10. && ((x - a).abs() <= 4. || (x - b).abs() <= 4.);
                return Hit::Transition {
                    row,
                    owner: t.owner,
                    resize,
                };
            }
        }
        let mut best = None;
        for clip in &track.clips {
            let (a, b) = (vp.x(clip.start), vp.x(clip.end()));
            let edge = (b - a).min(EDGE_PX * 3.) / 3.;
            if x >= a - edge.min(EDGE_PX) && x <= b + edge.min(EDGE_PX) {
                let near_start = (x - a).abs() <= edge.max(2.);
                let near_end = (x - b).abs() <= edge.max(2.);
                let hit_edge = if near_end && (!near_start || (x - b).abs() < (x - a).abs()) {
                    Some(Edge::End)
                } else if near_start {
                    Some(Edge::Start)
                } else if x > a && x < b {
                    None
                } else {
                    continue;
                };
                // Prefer the clip whose body is under the pointer.
                let inside = x >= a && x <= b;
                if best.is_none() || inside {
                    best = Some((clip.id, hit_edge));
                }
            }
        }
        match best {
            Some((clip, edge)) => Hit::Clip { row, clip, edge },
            None => Hit::Lane { row: Some(row) },
        }
    }

    fn overlay_at(&self, p: Point<Pixels>, cx: &App) -> Option<(ChangeSetId, ChangeItemId)> {
        let state = self.state.read(cx);
        let project = state.bus.project();
        let rows = self.rows(&project.timeline);
        let items = overlay::collect(&project.timeline, state.bus.chat_change_sets());
        let b = self.bounds.get();
        let lanes = Bounds::new(
            point(b.origin.x, b.origin.y + px(RULER_HEIGHT)),
            size(b.size.width, b.size.height - px(RULER_HEIGHT)),
        );
        items.iter().rev().find_map(|item| {
            let row = rows.iter().find(|r| Some(r.track) == item.track)?;
            paint::overlay_rect(item, row, &self.viewport, lanes, b.origin.x)
                .contains(&p)
                .then_some((item.set, item.item))
        })
    }

    fn snap_candidates(&self, exclude: &[ClipId], cx: &App) -> Vec<Time> {
        let state = self.state.read(cx);
        let tl = &state.bus.project().timeline;
        let mut out = vec![Time::ZERO, state.playhead];
        for clip in tl.tracks.iter().flat_map(|t| &t.clips) {
            if !exclude.contains(&clip.id) {
                out.extend([clip.start, clip.end()]);
            }
        }
        for m in &tl.markers {
            out.extend([m.range.start, m.range.end]);
        }
        if let Some(r) = state.in_out {
            out.extend([r.start, r.end]);
        }
        out
    }

    pub(super) fn snap_time(&mut self, t: Time, exclude: &[ClipId], cx: &App) -> Time {
        if !self.snapping {
            self.snap = None;
            return t;
        }
        let candidates = self.snap_candidates(exclude, cx);
        self.snap = layout::snap(t, &candidates, self.viewport.span(SNAP_PX));
        self.snap.unwrap_or(t)
    }

    pub(super) fn mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        // Alt-drags hand a reference to the chat (the overlays start them).
        if ev.modifiers.alt {
            return;
        }
        let rate = self.state.read(cx).bus.project().timeline.settings.rate;
        let (x, y) = self.local(ev.position);
        let at = self.viewport.time(x).max(Time::ZERO);
        let hit = self.hit(ev.position, cx);

        if !matches!(hit, Hit::Ruler) {
            let item = self.overlay_at(ev.position, cx);
            self.state.update(cx, |s, cx| s.focus_change_item(item, cx));
            if item.is_some() && !matches!(hit, Hit::Clip { .. }) {
                return;
            }
        }

        if self.tool == Tool::Blade && !matches!(hit, Hit::Ruler) {
            let row = match &hit {
                Hit::Clip { row, .. } | Hit::Transition { row, .. } => Some(*row),
                Hit::Lane { row } => *row,
                Hit::Ruler => None,
            };
            let at = self.snap_time(at, &[], cx).floor_to_frame(rate);
            self.snap = None;
            let tracks = if ev.modifiers.shift {
                None
            } else {
                row.map(|r| vec![r.track])
            };
            self.state.update(cx, |s, cx| {
                s.edit(Command::Blade { at, tracks }, cx);
            });
            return;
        }

        let locked = |row: &Row, cx: &App| {
            self.state
                .read(cx)
                .bus
                .project()
                .timeline
                .track(row.track)
                .is_some_and(|t| t.locked)
        };
        match hit {
            Hit::Ruler => {
                self.drag = Some(Drag::Scrub);
                self.scrub_to(at.floor_to_frame(rate), cx);
            }
            Hit::Lane { .. } => {
                let additive = ev.modifiers.shift || ev.modifiers.secondary();
                let base = if additive {
                    self.state.read(cx).selection.clips.clone()
                } else {
                    self.select(vec![], cx);
                    vec![]
                };
                self.drag = Some(Drag::Marquee {
                    from: (at, y),
                    to: (at, y),
                    base,
                    moved: false,
                });
            }
            Hit::Transition { row, owner, resize } => {
                self.select_transition(owner, cx);
                if resize && !locked(&row, cx) {
                    let duration = transition_of(self.state.read(cx).bus.project(), owner)
                        .map_or(Time::ZERO, |t| t.duration);
                    self.drag = Some(Drag::TransitionEdge { owner, duration });
                }
            }
            Hit::Clip { row, clip, edge } => {
                let selected = self.state.read(cx).selection.clips.clone();
                if ev.modifiers.secondary() {
                    let mut next = selected;
                    if let Some(i) = next.iter().position(|c| *c == clip) {
                        next.remove(i);
                    } else {
                        next.push(clip);
                    }
                    self.select(next, cx);
                    return;
                }
                if ev.modifiers.shift {
                    let next = self.range_selection(row.track, clip, selected, cx);
                    self.select(next, cx);
                    return;
                }
                if !selected.contains(&clip) || edge.is_some() || self.tool == Tool::Trim {
                    self.select(vec![clip], cx);
                } else if self.state.read(cx).selected_transition.is_some() {
                    self.state.update(cx, |s, cx| s.select_transition(None, cx));
                }
                if locked(&row, cx) {
                    return;
                }
                self.drag = Some(match (self.tool, edge) {
                    (Tool::Trim, None) => Drag::Slip {
                        clip,
                        grab: self.viewport.time(x),
                        delta: Time::ZERO,
                    },
                    (Tool::Trim, Some(edge)) => {
                        match neighbour(self.state.read(cx).bus.project(), clip, edge) {
                            Some((left, right)) => Drag::Roll {
                                left,
                                right,
                                delta: Time::ZERO,
                            },
                            None => Drag::Trim {
                                clip,
                                edge,
                                delta: Time::ZERO,
                                ripple: true,
                            },
                        }
                    }
                    (_, Some(edge)) => Drag::Trim {
                        clip,
                        edge,
                        delta: Time::ZERO,
                        ripple: false,
                    },
                    (_, None) => Drag::Move {
                        clips: self.state.read(cx).selection.clips.clone(),
                        grab: self.viewport.time(x),
                        from_track: row.track,
                        to_track: row.track,
                        delta: Time::ZERO,
                        moved: false,
                        clicked: clip,
                    },
                });
            }
        }
        cx.notify();
    }

    /// Right click: remember the target for the context menu, and select
    /// what was clicked unless it is already part of the selection.
    pub(super) fn right_mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.menu_at = ev.position;
        let (x, _) = self.local(ev.position);
        let at = self.viewport.time(x).max(Time::ZERO);
        self.menu_target = Some(match self.hit(ev.position, cx) {
            Hit::Clip { clip, .. } => {
                if !self.state.read(cx).selection.clips.contains(&clip) {
                    self.select(vec![clip], cx);
                }
                MenuTarget::Clip(clip)
            }
            Hit::Transition { owner, .. } => {
                self.select_transition(owner, cx);
                MenuTarget::Transition(owner)
            }
            Hit::Lane { .. } => MenuTarget::Lane(at),
            Hit::Ruler => MenuTarget::Ruler(at, self.marker_at(x, cx)),
        });
    }

    /// The marker whose flag is under lane-local `x` on the ruler.
    fn marker_at(&self, x: f32, cx: &App) -> Option<flikk_core::MarkerId> {
        let tl = &self.state.read(cx).bus.project().timeline;
        tl.markers
            .iter()
            .filter(|m| {
                let (a, b) = (self.viewport.x(m.range.start), self.viewport.x(m.range.end));
                x >= a - 5. && x <= b.max(a) + 5.
            })
            .min_by_key(|m| (self.viewport.x(m.range.start) - x).abs() as i64)
            .map(|m| m.id)
    }

    /// Select clips; a clip selection and a transition selection exclude
    /// each other.
    pub(super) fn select(&self, clips: Vec<ClipId>, cx: &mut App) {
        self.state.update(cx, |s, cx| s.select_clips(clips, cx));
    }

    pub(super) fn select_transition(&self, owner: (ClipId, Edge), cx: &mut App) {
        self.state
            .update(cx, |s, cx| s.select_transition(Some(owner), cx));
    }

    /// Shift-click: everything on `track` from the last selected clip there
    /// to `clip`, added to the selection.
    fn range_selection(
        &self,
        track: TrackId,
        clip: ClipId,
        mut selected: Vec<ClipId>,
        cx: &App,
    ) -> Vec<ClipId> {
        let tl = &self.state.read(cx).bus.project().timeline;
        let Some(t) = tl.track(track) else {
            return selected;
        };
        let index = |id: &ClipId| t.clips.iter().position(|c| c.id == *id);
        let Some(to) = index(&clip) else {
            return selected;
        };
        let from = selected.iter().rev().find_map(index).unwrap_or(to);
        for c in &t.clips[from.min(to)..=from.max(to)] {
            if !selected.contains(&c.id) {
                selected.push(c.id);
            }
        }
        selected
    }

    /// Clips a rubber band covers.
    pub(super) fn marquee_hits(
        &self,
        from: (Time, f32),
        to: (Time, f32),
        tl: &Timeline,
    ) -> Vec<ClipId> {
        let range = TimeRange::new(from.0.min(to.0), from.0.max(to.0));
        let (y0, y1) = (from.1.min(to.1), from.1.max(to.1));
        self.rows(tl)
            .iter()
            .zip(&tl.tracks)
            .filter(|(row, _)| row.y < y1 && row.y + row.height > y0)
            .flat_map(|(_, t)| &t.clips)
            .filter(|c| c.start < range.end && c.end() > range.start)
            .map(|c| c.id)
            .collect()
    }

    pub(super) fn mouse_moved(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        self.set_alt(ev.modifiers.alt, cx);
        // A drop hint outlives a drag that ended elsewhere or was cancelled.
        if self.drop.is_some() && !cx.has_active_drag() {
            self.drop = None;
            self.snap = None;
            cx.notify();
        }
        let inside = self.bounds.get().contains(&ev.position);
        let hover = inside.then_some(ev.position);
        if self.drag.is_some() {
            // Kept current so the cursor and hints are right when it ends.
            self.hover = hover;
            if ev.dragging() {
                self.drag_to(ev.position, cx);
            } else {
                // The button came up where we didn't see it.
                self.mouse_up(cx);
            }
            return;
        }
        let item = if inside {
            self.overlay_at(ev.position, cx)
        } else {
            None
        };
        let changed = item != self.hovered_item
            || (self.tool != Tool::Select && hover != self.hover)
            || hover.is_some() != self.hover.is_some()
            || self.edge_hover(self.hover, cx) != self.edge_hover(hover, cx)
            || self.hovered_edge(self.hover, cx) != self.hovered_edge(hover, cx);
        self.hover = hover;
        self.hovered_item = item;
        if changed {
            cx.notify();
        }
    }

    pub(super) fn set_alt(&mut self, alt: bool, cx: &mut Context<Self>) {
        if alt != self.alt {
            self.alt = alt;
            cx.notify();
        }
    }

    /// Over something an edge drag resizes (or, in trim mode, any clip).
    pub(super) fn edge_hover(&self, p: Option<Point<Pixels>>, cx: &App) -> bool {
        p.is_some_and(|p| match self.hit(p, cx) {
            Hit::Clip { edge, .. } => edge.is_some() || self.tool == Tool::Trim,
            Hit::Transition { resize, .. } => resize,
            _ => false,
        })
    }

    /// The clip edge under `p` that a drag would trim.
    pub(super) fn hovered_edge(
        &self,
        p: Option<Point<Pixels>>,
        cx: &App,
    ) -> Option<(ClipId, Edge)> {
        match self.hit(p?, cx) {
            Hit::Clip {
                clip,
                edge: Some(edge),
                ..
            } => Some((clip, edge)),
            _ => None,
        }
    }

    /// What the trim tool does now: the drag under way, or the one the
    /// pointer would start.
    pub(super) fn trim_mode(&self, cx: &App) -> Option<(IconName, &'static str)> {
        const SLIP: (IconName, &str) = (IconName::UnfoldHorizontal, "Slip");
        const RIPPLE: (IconName, &str) = (IconName::ArrowRightToLine, "Ripple");
        const ROLL: (IconName, &str) = (IconName::ChevronsLeftRight, "Roll");
        if self.tool != Tool::Trim {
            return None;
        }
        match &self.drag {
            Some(Drag::Slip { .. }) => Some(SLIP),
            Some(Drag::Trim { .. }) => Some(RIPPLE),
            Some(Drag::Roll { .. }) => Some(ROLL),
            Some(_) => None,
            None => match self.hit(self.hover?, cx) {
                Hit::Clip { edge: None, .. } => Some(SLIP),
                Hit::Clip {
                    clip,
                    edge: Some(edge),
                    ..
                } => Some(
                    match neighbour(self.state.read(cx).bus.project(), clip, edge) {
                        Some(_) => ROLL,
                        None => RIPPLE,
                    },
                ),
                _ => None,
            },
        }
    }

    /// Move the playhead for a ruler scrub. The scrub owns the playhead, so
    /// playback stops for it and stays stopped after, like the frame and cut
    /// steps.
    fn scrub_to(&self, t: Time, cx: &mut App) {
        self.state.update(cx, |s, cx| {
            if s.is_playing() {
                s.set_playing(false, cx);
            }
            s.seek(t, cx);
        });
    }

    fn drag_to(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) {
        let (x, y) = self.local(p);
        let at = self.viewport.time(x);
        let state = self.state.read(cx);
        let project = state.bus.project().clone();
        let rate = project.timeline.settings.rate;
        let frame = rate.frame_duration();
        match self.drag.take() {
            Some(Drag::Scrub) => {
                self.drag = Some(Drag::Scrub);
                let t = self
                    .snap_time(at.max(Time::ZERO), &[], cx)
                    .floor_to_frame(rate);
                self.scrub_to(t, cx);
            }
            Some(Drag::Move {
                clips,
                grab,
                from_track,
                mut to_track,
                mut delta,
                mut moved,
                clicked,
            }) => {
                let raw = at - grab;
                if moved || raw.0.abs() > self.viewport.span(3.).0 {
                    moved = true;
                    let dragged: Vec<_> = clips
                        .iter()
                        .filter_map(|id| project.timeline.clip(*id).map(|(_, c)| c.clone()))
                        .collect();
                    delta = raw;
                    self.snap = None;
                    if self.snapping {
                        let candidates = self.snap_candidates(&clips, cx);
                        let threshold = self.viewport.span(SNAP_PX);
                        let best = dragged
                            .iter()
                            .flat_map(|c| [c.start + raw, c.end() + raw])
                            .filter_map(|edge| {
                                layout::snap(edge, &candidates, threshold).map(|s| (s, s - edge))
                            })
                            .min_by_key(|(_, d)| d.0.abs());
                        if let Some((line, d)) = best {
                            delta = raw + d;
                            self.snap = Some(line);
                        }
                    }
                    let earliest = dragged.iter().map(|c| c.start).min().unwrap_or(Time::ZERO);
                    delta = Time(delta.0.max(-earliest.0));
                    delta = Time(delta.0 - delta.0.rem_euclid(frame.0.max(1)));
                    let rows = self.rows(&project.timeline);
                    let kind = |t: TrackId| project.timeline.track(t).map(|t| t.kind);
                    let single_track = dragged.iter().all(|c| {
                        project.timeline.clip(c.id).map(|(t, _)| t.id) == Some(from_track)
                    });
                    to_track = match layout::row_at(&rows, y) {
                        Some(row)
                            if single_track
                                && clips.len() == 1
                                && kind(row.track) == kind(from_track)
                                && !project.timeline.track(row.track).is_some_and(|t| t.locked) =>
                        {
                            row.track
                        }
                        _ => to_track,
                    };
                }
                self.drag = Some(Drag::Move {
                    clips,
                    grab,
                    from_track,
                    to_track,
                    delta,
                    moved,
                    clicked,
                });
            }
            Some(Drag::Trim {
                clip, edge, ripple, ..
            }) => {
                let Some((track, c)) = project.timeline.clip(clip) else {
                    return self.cancel_drag(cx);
                };
                let edge_t = match edge {
                    Edge::Start => c.start,
                    Edge::End => c.end(),
                };
                let target = self.snap_time(at, &[clip], cx);
                let mut delta = target - edge_t;
                delta = Time(delta.0 - delta.0.rem_euclid(frame.0.max(1)));
                let (before, after) = handles(&project, c);
                let ix = track.clips.iter().position(|o| o.id == clip).unwrap_or(0);
                match edge {
                    Edge::Start => {
                        let mut lo = -c.start.0;
                        if let Some(before) = before {
                            lo = lo.max(-before.0);
                        }
                        if !ripple && ix > 0 {
                            lo = lo.max(track.clips[ix - 1].end().0 - c.start.0);
                        }
                        delta = Time(delta.0.clamp(lo, (c.duration - frame).0));
                    }
                    Edge::End => {
                        let mut hi = i64::MAX;
                        if let Some(after) = after {
                            hi = hi.min(after.0);
                        }
                        if !ripple && let Some(next) = track.clips.get(ix + 1) {
                            hi = hi.min(next.start.0 - c.end().0);
                        }
                        delta = Time(delta.0.clamp((frame - c.duration).0, hi.max(0)));
                    }
                }
                self.drag = Some(Drag::Trim {
                    clip,
                    edge,
                    delta,
                    ripple,
                });
            }
            Some(Drag::Roll { left, right, .. }) => {
                let tl = &project.timeline;
                let (Some((_, l)), Some((_, r))) = (tl.clip(left), tl.clip(right)) else {
                    return self.cancel_drag(cx);
                };
                let target = self.snap_time(at, &[left, right], cx);
                let mut delta = floor_to(target - l.end(), frame);
                let mut lo = (frame - l.duration).0;
                if let (Some(before), _) = handles(&project, r) {
                    lo = lo.max(-before.0);
                }
                let mut hi = (r.duration - frame).0;
                if let (_, Some(after)) = handles(&project, l) {
                    hi = hi.min(after.0);
                }
                delta = Time(delta.0.clamp(lo.min(0), hi.max(0)));
                self.drag = Some(Drag::Roll { left, right, delta });
            }
            Some(Drag::Slip { clip, grab, .. }) => {
                let Some((_, c)) = project.timeline.clip(clip) else {
                    return self.cancel_drag(cx);
                };
                // Without media there is no source to slip: it stays put
                // until the button is up.
                let delta = match handles(&project, c) {
                    (Some(before), after) => {
                        // Dragging right shows earlier source: the in point moves left.
                        let delta = floor_to(grab - at, frame);
                        let hi = after.map_or(i64::MAX, |a| a.0);
                        Time(delta.0.clamp(-before.0, hi))
                    }
                    (None, _) => Time::ZERO,
                };
                self.drag = Some(Drag::Slip { clip, grab, delta });
            }
            Some(Drag::Marquee {
                from, base, moved, ..
            }) => {
                let moved = moved
                    || (self.viewport.x(at) - self.viewport.x(from.0)).abs() > 3.
                    || (y - from.1).abs() > 3.;
                self.drag = Some(Drag::Marquee {
                    from,
                    to: (at.max(Time::ZERO), y),
                    base,
                    moved,
                });
            }
            Some(Drag::TransitionEdge { owner, .. }) => {
                let placed = project.timeline.clip(owner.0).and_then(|(track, _)| {
                    fx::placed(track).into_iter().find(|p| p.owner == owner)
                });
                let Some(placed) = placed else {
                    return self.cancel_drag(cx);
                };
                let target = self.snap_time(at, &[], cx);
                let duration = floor_to(placed.duration_at(target), frame);
                let duration = Time(duration.0.clamp(frame.0, placed.max.0.max(frame.0)));
                self.drag = Some(Drag::TransitionEdge { owner, duration });
            }
            Some(Drag::TrackHeight { track, grab, from }) => {
                let h = (from + f32::from(p.y - grab))
                    .clamp(layout::MIN_ROW_HEIGHT, layout::MAX_ROW_HEIGHT);
                self.heights.insert(track, h.round());
                self.drag = Some(Drag::TrackHeight { track, grab, from });
            }
            None => {}
        }
        cx.notify();
    }

    /// Drop the drag without applying it (its clip went away).
    fn cancel_drag(&mut self, cx: &mut Context<Self>) {
        self.drag = None;
        self.snap = None;
        cx.notify();
    }

    pub(super) fn mouse_up(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        self.snap = None;
        match drag {
            Drag::Scrub | Drag::TrackHeight { .. } => {}
            Drag::Marquee {
                from,
                to,
                mut base,
                moved,
            } => {
                if moved {
                    let hits =
                        self.marquee_hits(from, to, &self.state.read(cx).bus.project().timeline);
                    for id in hits {
                        if !base.contains(&id) {
                            base.push(id);
                        }
                    }
                    self.select(base, cx);
                } else {
                    let rate = self.state.read(cx).bus.project().timeline.settings.rate;
                    self.state
                        .update(cx, |s, cx| s.seek(from.0.floor_to_frame(rate), cx));
                }
            }
            Drag::Roll { left, right, delta } => {
                if delta != Time::ZERO {
                    self.state.update(cx, |s, cx| {
                        s.edit(Command::Roll { left, right, delta }, cx);
                    });
                }
            }
            Drag::Slip { clip, delta, .. } => {
                if delta != Time::ZERO {
                    self.state.update(cx, |s, cx| {
                        s.edit(Command::Slip { clip, delta }, cx);
                    });
                }
            }
            Drag::TransitionEdge { owner, duration } => {
                let current = transition_of(self.state.read(cx).bus.project(), owner).cloned();
                if let Some(t) = current.filter(|t| t.duration != duration) {
                    let transition = Some(flikk_core::fx::Transition { duration, ..t });
                    self.state.update(cx, |s, cx| {
                        s.edit(
                            Command::SetTransition {
                                clip: owner.0,
                                edge: owner.1,
                                transition,
                            },
                            cx,
                        );
                    });
                }
            }
            Drag::Move {
                clips,
                from_track,
                to_track,
                delta,
                moved,
                clicked,
                ..
            } => {
                if !moved {
                    if clips.len() > 1 {
                        self.state
                            .update(cx, |s, cx| s.select_clips(vec![clicked], cx));
                    }
                } else if delta != Time::ZERO || to_track != from_track {
                    self.state.update(cx, |s, cx| {
                        let tl = &s.bus.project().timeline;
                        let mut moving: Vec<_> = clips
                            .iter()
                            .filter_map(|id| tl.clip(*id).map(|(_, c)| (c.id, c.start)))
                            .collect();
                        moving.sort_by_key(|(_, start)| *start);
                        if delta.0 > 0 {
                            moving.reverse();
                        }
                        let track = (to_track != from_track).then_some(to_track);
                        let commands: Vec<_> = moving
                            .into_iter()
                            .map(|(clip, start)| Command::MoveClip {
                                clip,
                                at: start + delta,
                                track,
                            })
                            .collect();
                        if commands.len() == 1 {
                            s.edit(commands.into_iter().next().unwrap(), cx);
                        } else {
                            s.edit_batch(commands, "move_clip", cx);
                        }
                    });
                }
            }
            Drag::Trim {
                clip,
                edge,
                delta,
                ripple,
            } => {
                if delta != Time::ZERO {
                    self.state.update(cx, |s, cx| {
                        s.edit(
                            Command::Trim {
                                clip,
                                edge,
                                delta,
                                ripple,
                            },
                            cx,
                        );
                    });
                }
            }
        }
        cx.notify();
    }
}
