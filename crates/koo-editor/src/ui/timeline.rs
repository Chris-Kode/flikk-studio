//! Multi-track timeline (PRD E1, E2, E4, E8, AG4): track headers, a painted
//! ruler and lanes, select/move/trim/blade with snapping, markers, in/out,
//! JKL, the pending change-set overlay and the silence tool.

pub(crate) mod glyph;
mod layout;
mod overlay;
mod paint;
mod silence;
mod thumbs;
mod waveform;

use crate::app::TogglePlay;
use crate::state::EditorState;
use glyph::{Glyph, glyph};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::command::{Command, Edge, InsertMode};
use koo_core::model::{ClipContent, MarkerKind, MediaKind, Project, Timeline, TrackKind, TrackMix};
use koo_core::{ChangeItemId, ChangeSetId, ClipId, Time, TimeRange, TrackId};
use layout::{EDGE_PX, HEADER_WIDTH, RULER_HEIGHT, Row, SNAP_PX, TOOLBAR_HEIGHT, Viewport};
use overlay::OverlayItem;
use paint::{ClipScene, Colors, Ghost, Scene, TrackScene};
use silence::SilenceCache;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use thumbs::Thumbs;
use waveform::Waveforms;

actions!(
    timeline,
    [
        SelectTool,
        BladeTool,
        ToggleSnapping,
        DeleteSelection,
        RippleDeleteSelection,
        AddMarker,
        AddProtectedMarker,
        MarkIn,
        MarkOut,
        ClearInOut,
        ShuttleReverse,
        ShuttleStop,
        ShuttleForward,
        PrevFrame,
        NextFrame,
        ZoomIn,
        ZoomOut,
        ZoomToFit,
        PrevCut,
        NextCut,
    ]
);

fn bind_keys(cx: &mut App) {
    static BOUND: AtomicBool = AtomicBool::new(false);
    if BOUND.swap(true, Ordering::SeqCst) {
        return;
    }
    let t = Some("Timeline");
    let mut bindings = vec![
        KeyBinding::new("v", SelectTool, t),
        KeyBinding::new("a", SelectTool, t),
        KeyBinding::new("b", BladeTool, t),
        KeyBinding::new("n", ToggleSnapping, t),
        KeyBinding::new("delete", DeleteSelection, t),
        KeyBinding::new("backspace", DeleteSelection, t),
        KeyBinding::new("shift-delete", RippleDeleteSelection, t),
        KeyBinding::new("shift-backspace", RippleDeleteSelection, t),
        KeyBinding::new("m", AddMarker, t),
        KeyBinding::new("shift-m", AddProtectedMarker, t),
        KeyBinding::new("i", MarkIn, t),
        KeyBinding::new("o", MarkOut, t),
        KeyBinding::new("alt-x", ClearInOut, t),
        KeyBinding::new("=", ZoomIn, t),
        KeyBinding::new("-", ZoomOut, t),
        KeyBinding::new("shift-z", ZoomToFit, t),
    ];
    // Transport keys work in the viewer too.
    for context in ["Timeline", "Viewer"] {
        let c = Some(context);
        bindings.extend([
            KeyBinding::new("j", ShuttleReverse, c),
            KeyBinding::new("k", ShuttleStop, c),
            KeyBinding::new("l", ShuttleForward, c),
            KeyBinding::new("left", PrevFrame, c),
            KeyBinding::new("right", NextFrame, c),
            KeyBinding::new("up", PrevCut, c),
            KeyBinding::new("down", NextCut, c),
        ]);
    }
    cx.bind_keys(bindings);
}

/// JKL, frame steps: shared by the timeline and the viewer.
pub(crate) fn transport(state: &Entity<EditorState>, action: &dyn Action, cx: &mut App) {
    state.update(cx, |s, cx| {
        let any = action.as_any();
        if any.is::<ShuttleForward>() {
            let speed = if s.playing && s.play_speed > 0. {
                (s.play_speed * 2.).min(8.)
            } else {
                1.
            };
            s.shuttle(speed, cx);
        } else if any.is::<ShuttleReverse>() {
            let speed = if s.playing && s.play_speed < 0. {
                (s.play_speed * 2.).max(-8.)
            } else {
                -1.
            };
            s.shuttle(speed, cx);
        } else if any.is::<ShuttleStop>() {
            s.shuttle(0., cx);
        } else if any.is::<PrevFrame>() || any.is::<NextFrame>() {
            let frame = s.bus.project().timeline.settings.rate.frame_duration();
            let rate = s.bus.project().timeline.settings.rate;
            let t = s.playhead.floor_to_frame(rate);
            let t = if any.is::<NextFrame>() {
                t + frame
            } else {
                t - frame
            };
            s.set_playing(false, cx);
            s.seek(t, cx);
        } else if any.is::<PrevCut>() || any.is::<NextCut>() {
            step_cut(s, any.is::<NextCut>(), cx);
        } else if any.is::<TogglePlay>() {
            s.toggle_play(cx);
        }
    });
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

/// Where a new track of `kind` goes, and its name: captions on top,
/// picture tracks above the topmost video track, audio at the bottom.
fn new_track_slot(tl: &Timeline, kind: TrackKind) -> (usize, String) {
    let prefix = match kind {
        TrackKind::Video => "V",
        TrackKind::Audio => "A",
        TrackKind::Caption => "C",
        TrackKind::Title => "T",
        TrackKind::Overlay => "O",
    };
    let mut n = tl.tracks.iter().filter(|t| t.kind == kind).count() + 1;
    while tl.tracks.iter().any(|t| t.name == format!("{prefix}{n}")) {
        n += 1;
    }
    let index = match kind {
        TrackKind::Caption => 0,
        TrackKind::Audio => tl.tracks.len(),
        _ => tl
            .tracks
            .iter()
            .position(|t| t.kind == TrackKind::Video)
            .or_else(|| tl.tracks.iter().position(|t| t.kind == TrackKind::Audio))
            .unwrap_or(tl.tracks.len()),
    };
    (index, format!("{prefix}{n}"))
}

/// "Add … track" items, one per kind.
fn add_track_items(menu: PopupMenu, panel: &Entity<TimelinePanel>) -> PopupMenu {
    [
        (TrackKind::Video, "Video track"),
        (TrackKind::Audio, "Audio track"),
        (TrackKind::Caption, "Caption track"),
        (TrackKind::Title, "Title track"),
        (TrackKind::Overlay, "Overlay track"),
    ]
    .into_iter()
    .fold(menu, |menu, (kind, label)| {
        let panel = panel.clone();
        menu.item(
            PopupMenuItem::new(label)
                .on_click(move |_, _, cx| panel.update(cx, |this, cx| this.add_track(kind, cx))),
        )
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Blade,
}

enum Drag {
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
}

enum Hit {
    Ruler,
    Clip {
        row: Row,
        clip: ClipId,
        edge: Option<Edge>,
    },
    Lane {
        row: Option<Row>,
    },
}

pub struct TimelinePanel {
    state: Entity<EditorState>,
    focus: FocusHandle,
    viewport: Viewport,
    fitted: bool,
    tool: Tool,
    snapping: bool,
    drag: Option<Drag>,
    snap: Option<Time>,
    /// Pointer position while over the lanes, in window coordinates.
    hover: Option<Point<Pixels>>,
    hovered_item: Option<(ChangeSetId, ChangeItemId)>,
    /// Last focused change item scrolled into view.
    revealed: Option<ChangeItemId>,
    /// Ruler + lanes canvas bounds from the last paint.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    waveforms: Waveforms,
    thumbs: Thumbs,
    silence_open: bool,
    silence_min: Entity<SliderState>,
    silence_padding: Entity<SliderState>,
    silences: SilenceCache,
    drop_row: Option<TrackId>,
    _observe: Subscription,
    _sliders: [Subscription; 2],
}

impl TimelinePanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        bind_keys(cx);
        let focus = cx.focus_handle();
        // Transport and editing keys work from the start.
        window.focus(&focus, cx);
        let _observe = cx.observe(&state, |_, _, cx| cx.notify());
        let silence_min = cx.new(|_| {
            SliderState::new()
                .min(0.2)
                .max(3.0)
                .step(0.1)
                .default_value(0.6)
        });
        let silence_padding = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(0.5)
                .step(0.02)
                .default_value(0.1)
        });
        let _sliders = [
            cx.subscribe(&silence_min, |_, _, _: &SliderEvent, cx| cx.notify()),
            cx.subscribe(&silence_padding, |_, _, _: &SliderEvent, cx| cx.notify()),
        ];
        Self {
            state,
            focus,
            viewport: Viewport::default(),
            revealed: None,
            fitted: false,
            tool: Tool::Select,
            snapping: true,
            drag: None,
            snap: None,
            hover: None,
            hovered_item: None,
            bounds: Rc::new(Cell::new(Bounds::default())),
            waveforms: Waveforms::default(),
            thumbs: Thumbs::default(),
            silence_open: false,
            silence_min,
            silence_padding,
            silences: SilenceCache::default(),
            drop_row: None,
            _observe,
            _sliders,
        }
    }

    // ------------------------------------------------------------- geometry

    fn lanes_height(&self) -> f32 {
        (f32::from(self.bounds.get().size.height) - RULER_HEIGHT).max(0.)
    }

    /// Lane-local x and y (scrolled) of a window position.
    fn local(&self, p: Point<Pixels>) -> (f32, f32) {
        let b = self.bounds.get();
        (
            f32::from(p.x - b.origin.x),
            f32::from(p.y - b.origin.y) - RULER_HEIGHT + self.viewport.scroll_y,
        )
    }

    fn hit(&self, p: Point<Pixels>, cx: &App) -> Hit {
        let b = self.bounds.get();
        if p.y < b.origin.y + px(RULER_HEIGHT) {
            return Hit::Ruler;
        }
        let (x, y) = self.local(p);
        let project = self.state.read(cx).bus.project();
        let rows = layout::rows(&project.timeline);
        let Some(row) = layout::row_at(&rows, y).copied() else {
            return Hit::Lane { row: None };
        };
        let Some(track) = project.timeline.track(row.track) else {
            return Hit::Lane { row: None };
        };
        let vp = &self.viewport;
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
        let rows = layout::rows(&project.timeline);
        let items = overlay::collect(&project.timeline, state.bus.change_sets());
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

    fn snap_time(&mut self, t: Time, exclude: &[ClipId], cx: &App) -> Time {
        if !self.snapping {
            self.snap = None;
            return t;
        }
        let candidates = self.snap_candidates(exclude, cx);
        self.snap = layout::snap(t, &candidates, self.viewport.span(SNAP_PX));
        self.snap.unwrap_or(t)
    }

    // ---------------------------------------------------------------- input

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let rate = self.state.read(cx).bus.project().timeline.settings.rate;
        let (x, _) = self.local(ev.position);
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
                Hit::Clip { row, .. } => Some(*row),
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

        match hit {
            Hit::Ruler => {
                self.drag = Some(Drag::Scrub);
                self.state
                    .update(cx, |s, cx| s.seek(at.floor_to_frame(rate), cx));
            }
            Hit::Lane { .. } => {
                if !ev.modifiers.shift {
                    self.state.update(cx, |s, cx| s.select_clips(vec![], cx));
                }
                self.drag = Some(Drag::Scrub);
                self.state
                    .update(cx, |s, cx| s.seek(at.floor_to_frame(rate), cx));
            }
            Hit::Clip { row, clip, edge } => {
                let locked = self
                    .state
                    .read(cx)
                    .bus
                    .project()
                    .timeline
                    .track(row.track)
                    .is_some_and(|t| t.locked);
                let selected = self.state.read(cx).selection.clips.clone();
                if ev.modifiers.shift {
                    let mut next = selected;
                    if let Some(i) = next.iter().position(|c| *c == clip) {
                        next.remove(i);
                    } else {
                        next.push(clip);
                    }
                    self.state.update(cx, |s, cx| s.select_clips(next, cx));
                    return;
                }
                if !selected.contains(&clip) || edge.is_some() {
                    self.state
                        .update(cx, |s, cx| s.select_clips(vec![clip], cx));
                }
                if locked {
                    return;
                }
                self.drag = Some(match edge {
                    Some(edge) => Drag::Trim {
                        clip,
                        edge,
                        delta: Time::ZERO,
                        ripple: ev.modifiers.alt,
                    },
                    None => Drag::Move {
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

    fn mouse_moved(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.drag.is_some() {
            if ev.dragging() {
                self.drag_to(ev.position, ev.modifiers, cx);
            }
            return;
        }
        let inside = self.bounds.get().contains(&ev.position);
        let hover = inside.then_some(ev.position);
        let item = if inside {
            self.overlay_at(ev.position, cx)
        } else {
            None
        };
        let changed = item != self.hovered_item
            || (self.tool == Tool::Blade && hover != self.hover)
            || hover.is_some() != self.hover.is_some()
            || self.edge_hover(self.hover, cx) != self.edge_hover(hover, cx);
        self.hover = hover;
        self.hovered_item = item;
        if changed {
            cx.notify();
        }
    }

    fn edge_hover(&self, p: Option<Point<Pixels>>, cx: &App) -> bool {
        p.is_some_and(|p| matches!(self.hit(p, cx), Hit::Clip { edge: Some(_), .. }))
    }

    fn drag_to(&mut self, p: Point<Pixels>, modifiers: Modifiers, cx: &mut Context<Self>) {
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
                self.state.update(cx, |s, cx| s.seek(t, cx));
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
                    let rows = layout::rows(&project.timeline);
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
                let ripple = ripple || modifiers.alt;
                let Some((track, c)) = project.timeline.clip(clip) else {
                    return;
                };
                let edge_t = match edge {
                    Edge::Start => c.start,
                    Edge::End => c.end(),
                };
                let target = self.snap_time(at, &[clip], cx);
                let mut delta = target - edge_t;
                delta = Time(delta.0 - delta.0.rem_euclid(frame.0.max(1)));
                let source_len = c
                    .media()
                    .and_then(|m| project.media(m))
                    .and_then(|m| m.info.as_ref())
                    .map(|i| i.duration);
                let source_in = match &c.content {
                    ClipContent::Media { source_in, .. } => Some(*source_in),
                    _ => None,
                };
                let ix = track.clips.iter().position(|o| o.id == clip).unwrap_or(0);
                match edge {
                    Edge::Start => {
                        let mut lo = -c.start.0;
                        if let Some(si) = source_in {
                            lo = lo.max(-si.0);
                        }
                        if !ripple && ix > 0 {
                            lo = lo.max(track.clips[ix - 1].end().0 - c.start.0);
                        }
                        delta = Time(delta.0.clamp(lo, (c.duration - frame).0));
                    }
                    Edge::End => {
                        let mut hi = i64::MAX;
                        if let (Some(si), Some(len)) = (source_in, source_len) {
                            hi = hi.min((len - si - c.duration).0);
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
            None => {}
        }
        cx.notify();
    }

    fn mouse_up(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        self.snap = None;
        match drag {
            Drag::Scrub => {}
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

    fn scroll(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let (dx, dy) = match ev.delta {
            ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
            ScrollDelta::Lines(l) => (l.x * 40., l.y * 40.),
        };
        let (x, _) = self.local(ev.position);
        if ev.modifiers.secondary() || ev.modifiers.control {
            self.viewport.zoom_at((dy * 0.004).exp(), x.max(0.));
        } else if ev.modifiers.shift {
            self.viewport.scroll_x -= dy + dx;
        } else {
            self.viewport.scroll_x -= dx;
            let rows = layout::rows(&self.state.read(cx).bus.project().timeline);
            let overflow = layout::rows_height(&rows) + layout::BOTTOM_PAD - self.lanes_height();
            if overflow > 0. && dy.abs() > dx.abs() {
                self.viewport.scroll_y = (self.viewport.scroll_y - dy).clamp(0., overflow);
            } else if dx == 0. {
                self.viewport.scroll_x -= dy;
            }
        }
        self.clamp_scroll(cx);
        cx.notify();
    }

    fn pinch(&mut self, ev: &PinchEvent, cx: &mut Context<Self>) {
        let (x, _) = self.local(ev.position);
        self.viewport.zoom_at(1. + ev.delta, x.max(0.));
        self.clamp_scroll(cx);
        cx.notify();
    }

    fn clamp_scroll(&mut self, cx: &App) {
        let duration = self.state.read(cx).bus.project().timeline.duration();
        let width = f32::from(self.bounds.get().size.width);
        let max = (self.viewport.x(duration) + self.viewport.scroll_x - width * 0.5).max(0.);
        self.viewport.scroll_x = self.viewport.scroll_x.clamp(0., max);
    }

    fn zoom(&mut self, factor: f32, cx: &mut Context<Self>) {
        let playhead = self.state.read(cx).playhead;
        let width = f32::from(self.bounds.get().size.width);
        let x = self.viewport.x(playhead);
        let anchor = if (0.0..=width).contains(&x) {
            x
        } else {
            width / 2.
        };
        self.viewport.zoom_at(factor, anchor);
        self.clamp_scroll(cx);
        cx.notify();
    }

    fn fit(&mut self, cx: &mut Context<Self>) {
        let duration = self.state.read(cx).bus.project().timeline.duration();
        self.viewport
            .fit(duration, f32::from(self.bounds.get().size.width));
        cx.notify();
    }

    /// Scroll a change item focused elsewhere (the chat) into view.
    fn reveal_focused(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let focused = state.focused_change_item().map(|(_, i)| i);
        if focused == self.revealed {
            return;
        }
        self.revealed = focused;
        let Some(item) = focused else {
            return;
        };
        let range = overlay::collect(&state.bus.project().timeline, state.bus.change_sets())
            .into_iter()
            .filter(|i| i.item == item)
            .map(|i| i.range)
            .reduce(|a, b| TimeRange::new(a.start.min(b.start), a.end.max(b.end)));
        let width = f32::from(self.bounds.get().size.width);
        if let Some(r) = range
            && width > 0.
        {
            let (a, b) = (self.viewport.x(r.start), self.viewport.x(r.end));
            if a < 0. || b > width {
                self.viewport.scroll_x = (self.viewport.scroll_x + a - width * 0.2).max(0.);
            }
        }
    }

    fn drop_media(&mut self, drag: &super::MediaDrag, window: &Window, cx: &mut Context<Self>) {
        self.drop_row = None;
        let p = window.mouse_position();
        let (x, y) = self.local(p);
        let state = self.state.read(cx);
        let project = state.bus.project();
        let Some(item) = project.media(drag.media) else {
            return;
        };
        let rows = layout::rows(&project.timeline);
        let wanted = match item.kind {
            koo_core::model::MediaKind::Audio => TrackKind::Audio,
            _ => TrackKind::Video,
        };
        let track = layout::row_at(&rows, y).map(|r| r.track).or_else(|| {
            project
                .timeline
                .tracks
                .iter()
                .find(|t| t.kind == wanted)
                .map(|t| t.id)
        });
        let Some(track) = track else {
            return;
        };
        let len = item
            .info
            .as_ref()
            .map(|i| i.duration)
            .filter(|d| d.0 > 0)
            .unwrap_or(Time::from_secs_f64(5.));
        let rate = project.timeline.settings.rate;
        let at = self
            .snap_time(self.viewport.time(x).max(Time::ZERO), &[], cx)
            .floor_to_frame(rate);
        self.snap = None;
        let insert = |track| Command::InsertClip {
            media: drag.media,
            source: TimeRange::new(Time::ZERO, len),
            track,
            at,
            mode: InsertMode::Overwrite,
        };
        let state = self.state.read(cx);
        let project = state.bus.project();
        let has_audio = project
            .media(drag.media)
            .and_then(|m| m.info.as_ref())
            .is_some_and(|i| !i.audio.is_empty());
        // Video with sound lands as a linked pair: the mixer and renderer
        // read audio from audio tracks only.
        let linked = match project.timeline.track(track).map(|t| t.kind) {
            Some(TrackKind::Video | TrackKind::Overlay) if has_audio => {
                matching_audio_track(&project.timeline, track)
            }
            _ => None,
        };
        self.state.update(cx, |s, cx| match linked {
            Some(audio) => {
                s.edit_batch(vec![insert(track), insert(audio)], "insert_clip", cx);
            }
            None => {
                s.edit(insert(track), cx);
            }
        });
    }

    // -------------------------------------------------------------- actions

    fn delete_selection(&mut self, ripple: bool, cx: &mut Context<Self>) {
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

    fn add_marker(&mut self, protected: bool, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let range = if protected {
                let tl = &s.bus.project().timeline;
                let selected = s
                    .selection
                    .clips
                    .iter()
                    .filter_map(|id| tl.clip(*id).map(|(_, c)| c.range()))
                    .reduce(|a, b| TimeRange::new(a.start.min(b.start), a.end.max(b.end)));
                match s.in_out.filter(|r| !r.is_empty()).or(selected) {
                    Some(r) => r,
                    None => {
                        s.status =
                            Some("Mark in and out, or select clips, to protect a range".into());
                        cx.notify();
                        return;
                    }
                }
            } else {
                TimeRange::new(s.playhead, s.playhead)
            };
            let kind = if protected {
                MarkerKind::Protected
            } else {
                MarkerKind::Note
            };
            s.edit(
                Command::AddMarker {
                    range,
                    kind,
                    note: String::new(),
                },
                cx,
            );
        });
    }

    fn toggle_mix(&mut self, track: TrackId, solo: bool, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let Some(t) = s.bus.project().timeline.track(track) else {
                return;
            };
            let mix = TrackMix {
                mute: if solo { t.mix.mute } else { !t.mix.mute },
                solo: if solo { !t.mix.solo } else { t.mix.solo },
                ..t.mix.clone()
            };
            s.edit(Command::SetTrackMix { track, mix }, cx);
        });
    }

    fn add_track(&mut self, kind: TrackKind, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let (index, name) = new_track_slot(&s.bus.project().timeline, kind);
            s.edit(Command::AddTrack { kind, index, name }, cx);
        });
    }

    /// Delete a track with its clips (one undo step).
    fn remove_track(&mut self, track: TrackId, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let Some(t) = s.bus.project().timeline.track(track) else {
                return;
            };
            let clips: Vec<ClipId> = t.clips.iter().map(|c| c.id).collect();
            if s.edit(Command::RemoveTrack { track }, cx).is_some() {
                let selection = s
                    .selection
                    .clips
                    .iter()
                    .copied()
                    .filter(|c| !clips.contains(c))
                    .collect();
                s.select_clips(selection, cx);
            }
        });
    }

    /// Scroll so the playhead is in view, e.g. after jumping to a cut.
    fn reveal_playhead(&mut self, cx: &mut Context<Self>) {
        let width = f32::from(self.bounds.get().size.width);
        let x = self.viewport.x(self.state.read(cx).playhead);
        if width > 0. && (x < 0. || x > width) {
            self.viewport.scroll_x = (self.viewport.scroll_x + x - width * 0.3).max(0.);
            cx.notify();
        }
    }

    fn toggle_lock(&mut self, track: TrackId, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let Some(t) = s.bus.project().timeline.track(track) else {
                return;
            };
            let locked = !t.locked;
            s.edit(Command::SetTrackLocked { track, locked }, cx);
        });
    }

    fn apply_silences(&mut self, cx: &mut Context<Self>) {
        let (min, pad) = self.silence_params(cx);
        let ranges = {
            let project = self.state.read(cx).bus.project();
            self.silences.timeline_silences(project, min, pad)
        };
        if ranges.is_empty() {
            return;
        }
        self.silence_open = false;
        let commands = silence::removal_commands(&ranges);
        self.state.update(cx, |s, cx| {
            s.edit_batch(commands, "Remove silences", cx);
        });
        cx.notify();
    }

    fn silence_params(&self, cx: &App) -> (Time, Time) {
        let value = |s: &Entity<SliderState>| match s.read(cx).value() {
            SliderValue::Single(v) | SliderValue::Range(_, v) => v as f64,
        };
        (
            Time::from_secs_f64(value(&self.silence_min)),
            Time::from_secs_f64(value(&self.silence_padding)),
        )
    }

    /// Ask the analysis engine for source silences we don't have yet.
    fn request_silences(&mut self, cx: &mut Context<Self>) {
        let Some(analysis) = self.state.read(cx).services.analysis.clone() else {
            return;
        };
        let (min, _) = self.silence_params(cx);
        let missing = {
            let project = self.state.read(cx).bus.project().clone();
            self.silences.missing(&project, min)
        };
        for item in missing {
            let analysis = analysis.clone();
            cx.spawn(async move |this, cx| {
                let id = item.id;
                let found = cx
                    .background_executor()
                    .spawn(async move { silence::detect(analysis.as_ref(), &item, min) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.silences.insert(min, id, found);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    // --------------------------------------------------------------- render

    fn scene(&mut self, cx: &mut Context<Self>) -> Scene {
        let colors = Colors::new(cx.theme());
        let state = self.state.read(cx);
        let project = state.bus.project();
        let media = state.services.media.clone();
        let selection = state.selection.clips.clone();
        let playhead = state.playhead;
        let in_out = state.in_out;
        let focused = state.focused_change().map(|i| i.id);
        let overlay: Vec<OverlayItem> =
            overlay::collect(&project.timeline, state.bus.change_sets());
        let tl = &project.timeline;
        let any_solo = tl
            .tracks
            .iter()
            .any(|t| t.kind == TrackKind::Audio && t.mix.solo);

        let lifted: Vec<ClipId> = match &self.drag {
            Some(Drag::Move {
                clips, moved: true, ..
            }) => clips.clone(),
            _ => vec![],
        };
        let mut ghosts = Vec::new();
        match &self.drag {
            Some(Drag::Move {
                clips,
                to_track,
                from_track,
                delta,
                moved: true,
                ..
            }) => {
                for id in clips {
                    if let Some((track, c)) = tl.clip(*id) {
                        let track = if track.id == *from_track {
                            *to_track
                        } else {
                            track.id
                        };
                        ghosts.push(Ghost {
                            track,
                            range: c.range().shift(*delta),
                        });
                    }
                }
            }
            Some(Drag::Trim {
                clip, edge, delta, ..
            }) => {
                if let Some((track, c)) = tl.clip(*clip) {
                    let r = c.range();
                    let range = match edge {
                        Edge::Start => TimeRange::new(r.start + *delta, r.end),
                        Edge::End => TimeRange::new(r.start, r.end + *delta),
                    };
                    ghosts.push(Ghost {
                        track: track.id,
                        range,
                    });
                }
            }
            _ => {}
        }

        let rows = layout::rows(tl);
        let width = f32::from(self.bounds.get().size.width);
        // Only clips in view are built and painted: a long transcript makes
        // thousands of captions.
        let visible = TimeRange::new(
            self.viewport.time(-SNAP_PX).max(Time::ZERO),
            self.viewport.time(width + SNAP_PX),
        );
        let mut requests = Vec::new();
        let tracks = rows
            .iter()
            .zip(&tl.tracks)
            .map(|(row, track)| {
                let clips = track
                    .clips
                    .iter()
                    .filter(|clip| clip.end() >= visible.start && clip.start <= visible.end)
                    .map(|clip| {
                        let peaks = if track.kind == TrackKind::Audio && media.is_some() {
                            let (peaks, request) = self.waveforms.lookup(clip, project);
                            requests.extend(request);
                            peaks
                        } else {
                            None
                        };
                        let tiles = match (&media, clip.media().and_then(|m| project.media(m))) {
                            (Some(_), Some(item))
                                if matches!(track.kind, TrackKind::Video | TrackKind::Overlay)
                                    && item.kind != MediaKind::Audio =>
                            {
                                self.tiles(clip, item, row, width)
                            }
                            _ => vec![],
                        };
                        ClipScene {
                            tiles,
                            range: clip.range(),
                            label: clip_label(clip, project).into(),
                            selected: selection.contains(&clip.id),
                            lifted: lifted.contains(&clip.id),
                            peaks,
                            fade_in: clip.audio.fade_in,
                            fade_out: clip.audio.fade_out,
                        }
                    })
                    .collect();
                TrackScene {
                    row: *row,
                    clips,
                    dimmed: track.mix.mute
                        || (any_solo && track.kind == TrackKind::Audio && !track.mix.solo),
                }
            })
            .collect();

        let silences = if self.silence_open {
            let (min, pad) = self.silence_params(cx);
            self.silences.timeline_silences(project, min, pad)
        } else {
            vec![]
        };

        let blade = match (self.tool, self.hover, &self.drag) {
            (Tool::Blade, Some(p), None) => {
                let (x, y) = self.local(p);
                let row = layout::row_at(&rows, y).map(|r| r.track);
                let candidates = vec![playhead];
                let t = self.viewport.time(x);
                let t = layout::snap(t, &candidates, self.viewport.span(SNAP_PX))
                    .unwrap_or(t)
                    .floor_to_frame(tl.settings.rate);
                (p.y > self.bounds.get().origin.y + px(RULER_HEIGHT)).then_some((t, row))
            }
            _ => None,
        };

        let scene = Scene {
            viewport: self.viewport,
            rate: tl.settings.rate,
            tracks,
            markers: tl.markers.clone(),
            in_out,
            playhead,
            overlay,
            focused,
            hovered: self.hovered_item.map(|(_, i)| i),
            silences,
            ghosts,
            snap: self.snap,
            blade,
            drop_row: self.drop_row,
            colors,
        };

        if let Some(media) = media {
            for request in requests {
                let media = media.clone();
                cx.spawn(async move |this, cx| {
                    let key = request.key;
                    let peaks = cx
                        .background_executor()
                        .spawn(async move { request.run(media.as_ref()) })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        this.waveforms.insert(key, peaks);
                        cx.notify();
                    });
                })
                .detach();
            }
        }

        self.thumbs.trim_queue(160);
        self.pump_thumbs(cx);

        scene
    }

    /// Visible filmstrip tiles of a video clip.
    fn tiles(
        &mut self,
        clip: &koo_core::model::Clip,
        item: &koo_core::model::MediaItem,
        row: &Row,
        width: f32,
    ) -> Vec<(f32, f32, std::sync::Arc<RenderImage>)> {
        let vp = self.viewport;
        let tile_w = ((row.height - 6.) * 16. / 9.).max(8.);
        let (x0, x1) = (vp.x(clip.start), vp.x(clip.end()));
        let (v0, v1) = (x0.max(0.), x1.min(width));
        if v1 <= v0 {
            return vec![];
        }
        let step = thumbs::step((tile_w / vp.px_per_sec) as f64);
        let first = ((v0 - x0) / tile_w).floor() as i64;
        let last = ((v1 - x0) / tile_w).ceil() as i64;
        let mut out = Vec::new();
        for i in first.max(0)..last.min(first + 200) {
            let offset = i as f32 * tile_w;
            let Some(source) = clip.to_source(clip.start + vp.span(offset)) else {
                continue;
            };
            if let Some(image) = self.thumbs.get(item, source, step) {
                out.push((offset, tile_w, image));
            }
        }
        out
    }

    fn pump_thumbs(&mut self, cx: &mut Context<Self>) {
        let Some(media) = self.state.read(cx).services.media.clone() else {
            return;
        };
        for (key, item, t) in self.thumbs.next_requests() {
            let media = media.clone();
            cx.spawn(async move |this, cx| {
                let image = cx
                    .background_executor()
                    .spawn(async move { thumbs::fetch(media.as_ref(), &item, t) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.thumbs.finish(key, image);
                    this.pump_thumbs(cx);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (fg, muted, border) = (theme.foreground, theme.muted_foreground, theme.border);
        let tool = |id: &'static str, g: Glyph, active: bool, tip: &'static str| {
            Button::new(id)
                .ghost()
                .xsmall()
                .selected(active)
                .tooltip(tip)
                .child(glyph(g, px(14.), if active { fg } else { muted }))
        };
        let separator = || div().w(px(1.)).h(px(16.)).mx_1().bg(border);
        let entity = cx.entity();
        let silence = Popover::new("silence-tool")
            .anchor(Anchor::TopLeft)
            .open(self.silence_open)
            .on_open_change({
                let entity = entity.clone();
                move |open, _, cx| {
                    entity.update(cx, |this, cx| {
                        this.silence_open = *open;
                        cx.notify();
                    })
                }
            })
            .trigger(tool(
                "silence",
                Glyph::Silence,
                self.silence_open,
                "Remove silences",
            ))
            .content(move |_, _, cx| silence_content(&entity, cx));

        h_flex()
            .flex_none()
            .h(px(TOOLBAR_HEIGHT))
            .px_2()
            .gap_0p5()
            .border_b_1()
            .border_color(border)
            .child(
                tool(
                    "tool-select",
                    Glyph::Pointer,
                    self.tool == Tool::Select,
                    "Select (V)",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.tool = Tool::Select;
                    cx.notify();
                })),
            )
            .child(
                tool(
                    "tool-blade",
                    Glyph::Blade,
                    self.tool == Tool::Blade,
                    "Blade (B)",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.tool = Tool::Blade;
                    cx.notify();
                })),
            )
            .child(separator())
            .child(
                tool("snapping", Glyph::Magnet, self.snapping, "Snapping (N)").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.snapping = !this.snapping;
                        cx.notify();
                    }),
                ),
            )
            .child(
                tool("marker", Glyph::Marker, false, "Add marker (M)")
                    .on_click(cx.listener(|this, _, _, cx| this.add_marker(false, cx))),
            )
            .child(separator())
            .child(
                tool("prev-cut", Glyph::PrevCut, false, "Previous cut (Up)").on_click(cx.listener(
                    |this, _, _, cx| {
                        transport(&this.state, &PrevCut, cx);
                        this.reveal_playhead(cx);
                    },
                )),
            )
            .child(
                tool("next-cut", Glyph::NextCut, false, "Next cut (Down)").on_click(cx.listener(
                    |this, _, _, cx| {
                        transport(&this.state, &NextCut, cx);
                        this.reveal_playhead(cx);
                    },
                )),
            )
            .child(separator())
            .child(silence)
            .child(div().flex_1())
            .child(
                tool("zoom-out", Glyph::Minus, false, "Zoom out (-)")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(0.8, cx))),
            )
            .child(
                tool("zoom-in", Glyph::Plus, false, "Zoom in (=)")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(1.25, cx))),
            )
            .child(
                tool("zoom-fit", Glyph::Fit, false, "Zoom to fit (Shift Z)")
                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
            )
    }

    fn headers(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (fg, muted, border) = (theme.foreground, theme.muted_foreground, theme.border);
        let tl = &self.state.read(cx).bus.project().timeline;
        let any_solo = tl
            .tracks
            .iter()
            .any(|t| t.kind == TrackKind::Audio && t.mix.solo);
        let rows: Vec<_> = layout::rows(tl)
            .into_iter()
            .zip(tl.tracks.iter().cloned())
            .collect();
        let icon = |id: SharedString, g: Glyph, on: bool, tip: &'static str| {
            Button::new(ElementId::Name(id))
                .ghost()
                .xsmall()
                .tooltip(tip)
                .child(glyph(g, px(13.), if on { fg } else { muted.opacity(0.7) }))
        };
        let panel = cx.entity();
        let children = rows.into_iter().map(|(row, track)| {
            let id = track.id;
            let locked = track.locked;
            let panel = panel.clone();
            let audio = track.kind == TrackKind::Audio;
            let dimmed = track.mix.mute || (any_solo && audio && !track.mix.solo);
            let key = |name: &str| SharedString::from(format!("{name}-{}", track.id));
            let (mute_glyph, mute_tip) = match (audio, track.mix.mute) {
                (true, false) => (Glyph::Speaker, "Mute"),
                (true, true) => (Glyph::SpeakerOff, "Unmute"),
                (false, false) => (Glyph::Eye, "Hide"),
                (false, true) => (Glyph::EyeOff, "Show"),
            };
            h_flex()
                .id(ElementId::Name(key("header")))
                .h(px(row.height))
                .px_2()
                .gap_0p5()
                .border_b_1()
                .border_color(border.opacity(0.5))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(if dimmed { muted } else { fg })
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(track.name.clone()),
                )
                .child(
                    icon(key("mute"), mute_glyph, track.mix.mute, mute_tip).on_click(
                        cx.listener(move |this, _, _, cx| this.toggle_mix(id, false, cx)),
                    ),
                )
                .when(audio, |el| {
                    el.child(
                        icon(key("solo"), Glyph::Headphones, track.mix.solo, "Solo").on_click(
                            cx.listener(move |this, _, _, cx| this.toggle_mix(id, true, cx)),
                        ),
                    )
                })
                .child(
                    icon(
                        key("lock"),
                        if track.locked {
                            Glyph::Lock
                        } else {
                            Glyph::Unlock
                        },
                        track.locked,
                        if track.locked { "Unlock" } else { "Lock" },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_lock(id, cx))),
                )
                .context_menu(move |menu, _, _| {
                    let remove = panel.clone();
                    add_track_items(menu, &panel)
                        .item(PopupMenuItem::separator())
                        .item(
                            PopupMenuItem::new("Delete track")
                                .disabled(locked)
                                .on_click(move |_, _, cx| {
                                    remove.update(cx, |this, cx| this.remove_track(id, cx))
                                }),
                        )
                })
        });
        v_flex()
            .flex_none()
            .w(px(HEADER_WIDTH))
            .h_full()
            .border_r_1()
            .border_color(border)
            .child(
                h_flex()
                    .flex_none()
                    .h(px(RULER_HEIGHT))
                    .px_1()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        Button::new("add-track")
                            .ghost()
                            .xsmall()
                            .tooltip("Add track")
                            .child(glyph(Glyph::Plus, px(12.), muted))
                            .dropdown_menu({
                                let panel = cx.entity();
                                move |menu, _, _| add_track_items(menu, &panel)
                            }),
                    ),
            )
            .child(
                div().flex_1().min_h_0().overflow_hidden().relative().child(
                    v_flex()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(-self.viewport.scroll_y))
                        .children(children),
                ),
            )
    }

    fn tooltip(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.drag.is_some() {
            return None;
        }
        let (set, item) = self.hovered_item?;
        let p = self.hover?;
        let state = self.state.read(cx);
        let hovered = overlay::collect(&state.bus.project().timeline, state.bus.change_sets())
            .into_iter()
            .find(|i| i.set == set && i.item == item)?;
        let theme = cx.theme();
        let b = self.bounds.get();
        let x = (p.x - b.origin.x + px(14.))
            .min(b.size.width - px(260.))
            .max(px(4.));
        let warnings = hovered.warnings;
        // Below the pointer, or above it when that would leave the panel.
        let height = px(26. + 18. * warnings.len() as f32);
        let below = p.y - b.origin.y + px(18.);
        let y = if below + height > b.size.height {
            (p.y - b.origin.y - height - px(8.)).max(px(0.))
        } else {
            below
        };
        Some(
            v_flex()
                .absolute()
                .left(x)
                .top(y)
                .max_w(px(320.))
                .px_2()
                .py_1()
                .gap_0p5()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .text_xs()
                .text_color(theme.popover_foreground)
                .child(hovered.reason)
                .children(
                    warnings
                        .into_iter()
                        .map(|w| div().text_color(theme.warning).child(w)),
                ),
        )
    }
}

/// The audio track paired with a video track: V1 ↔ A1, V2 ↔ A2, counting
/// video tracks up from the one nearest the audio. Falls back to the first
/// unlocked audio track.
fn matching_audio_track(timeline: &koo_core::model::Timeline, video: TrackId) -> Option<TrackId> {
    let videos: Vec<_> = timeline
        .tracks
        .iter()
        .filter(|t| matches!(t.kind, TrackKind::Video | TrackKind::Overlay))
        .rev()
        .collect();
    let audios: Vec<_> = timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio && !t.locked)
        .collect();
    let rank = videos.iter().position(|t| t.id == video)?;
    audios.get(rank).or(audios.first()).map(|t| t.id)
}

fn clip_label(clip: &koo_core::model::Clip, project: &Project) -> String {
    if let Some(name) = &clip.name {
        return name.clone();
    }
    match &clip.content {
        ClipContent::Media { media, .. } => project
            .media(*media)
            .and_then(|m| m.path.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        ClipContent::Title { text, .. } | ClipContent::Caption { text, .. } => text.clone(),
        ClipContent::Overlay { asset, .. } => asset
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        ClipContent::Generator { .. } => clip.display_name(project),
    }
}

fn silence_content(panel: &Entity<TimelinePanel>, cx: &mut App) -> AnyElement {
    let this = panel.read(cx);
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    if this.state.read(cx).services.analysis.is_none() {
        return div()
            .text_xs()
            .text_color(muted)
            .child("Analysis is not available")
            .into_any_element();
    }
    let (min, pad) = this.silence_params(cx);
    let project = this.state.read(cx).bus.project();
    let found = this.silences.timeline_silences(project, min, pad);
    let total: f64 = found.iter().map(|r| r.duration().as_secs_f64()).sum();
    let loading = this.silences.is_loading();
    let row = |label: &'static str, value: String| {
        h_flex()
            .justify_between()
            .text_xs()
            .child(div().text_color(muted).child(label))
            .child(value)
    };
    let label = if loading {
        "Analyzing…".to_string()
    } else if found.is_empty() {
        "No silences".to_string()
    } else {
        format!("Remove {} · {:.1} s", found.len(), total)
    };
    let panel = panel.clone();
    v_flex()
        .w(px(240.))
        .gap_2()
        .child(row("Minimum", format!("{:.1} s", min.as_secs_f64())))
        .child(Slider::new(&this.silence_min))
        .child(row("Padding", format!("{:.2} s", pad.as_secs_f64())))
        .child(Slider::new(&this.silence_padding))
        .child(
            Button::new("apply-silences")
                .primary()
                .small()
                .w_full()
                .label(label)
                .disabled(found.is_empty())
                .on_click(move |_, _, cx| panel.update(cx, |this, cx| this.apply_silences(cx))),
        )
        .into_any_element()
}

impl Focusable for TimelinePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TimelinePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.thumbs.retired.drain(..) {
            let _ = window.drop_image(image);
        }
        let bounds = self.bounds.get();
        let duration = self.state.read(cx).bus.project().timeline.duration();
        if !self.fitted && bounds.size.width > px(0.) && duration > Time::ZERO {
            self.viewport.fit(duration, f32::from(bounds.size.width));
            self.fitted = true;
        }
        // Keep the playhead in view while playing.
        let (playing, playhead) = {
            let s = self.state.read(cx);
            (s.playing, s.playhead)
        };
        let width = f32::from(bounds.size.width);
        if playing && width > 0. && self.drag.is_none() {
            let x = self.viewport.x(playhead);
            if x > width * 0.92 || x < 0. {
                self.viewport.scroll_x = (self.viewport.scroll_x + x - width * 0.1).max(0.);
            }
        }
        if self.silence_open {
            self.request_silences(cx);
        }
        self.reveal_focused(cx);

        let scene = self.scene(cx);
        let cell = self.bounds.clone();
        let entity = cx.entity();
        let first_paint = bounds.size.width <= px(0.);
        let lanes = canvas(
            move |bounds, _, _| cell.set(bounds),
            move |bounds, (), window, cx| {
                paint::paint(scene, bounds, window, cx);
                let e = entity.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        e.update(cx, |this, cx| this.mouse_moved(ev, cx));
                    }
                });
                let e = entity.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && ev.button == MouseButton::Left {
                        e.update(cx, |this, cx| this.mouse_up(cx));
                    }
                });
                if first_paint {
                    let e = entity.clone();
                    window.on_next_frame(move |_, cx| e.update(cx, |_, cx| cx.notify()));
                }
            },
        )
        .size_full();

        let cursor = if self.tool == Tool::Blade {
            CursorStyle::Crosshair
        } else if matches!(self.drag, Some(Drag::Trim { .. })) || self.edge_hover(self.hover, cx) {
            CursorStyle::ResizeLeftRight
        } else {
            CursorStyle::Arrow
        };
        let theme = cx.theme();

        v_flex()
            .size_full()
            .bg(theme.background)
            .track_focus(&self.focus)
            .key_context("Timeline")
            .on_action(cx.listener(|this, _: &SelectTool, _, cx| {
                this.tool = Tool::Select;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BladeTool, _, cx| {
                this.tool = if this.tool == Tool::Blade {
                    Tool::Select
                } else {
                    Tool::Blade
                };
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleSnapping, _, cx| {
                this.snapping = !this.snapping;
                cx.notify();
            }))
            .on_action(
                cx.listener(|this, _: &DeleteSelection, _, cx| this.delete_selection(false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &RippleDeleteSelection, _, cx| {
                    this.delete_selection(true, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &AddMarker, _, cx| this.add_marker(false, cx)))
            .on_action(cx.listener(|this, _: &AddProtectedMarker, _, cx| this.add_marker(true, cx)))
            .on_action(
                cx.listener(|this, _: &MarkIn, _, cx| this.state.update(cx, |s, cx| s.mark_in(cx))),
            )
            .on_action(
                cx.listener(|this, _: &MarkOut, _, cx| {
                    this.state.update(cx, |s, cx| s.mark_out(cx))
                }),
            )
            .on_action(cx.listener(|this, _: &ClearInOut, _, cx| {
                this.state.update(cx, |s, cx| s.set_in_out(None, cx))
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(1.25, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(0.8, cx)))
            .on_action(cx.listener(|this, _: &ZoomToFit, _, cx| this.fit(cx)))
            .on_action(cx.listener(|this, a: &TogglePlay, _, cx| transport(&this.state, a, cx)))
            .on_action(cx.listener(|this, a: &ShuttleForward, _, cx| transport(&this.state, a, cx)))
            .on_action(cx.listener(|this, a: &ShuttleReverse, _, cx| transport(&this.state, a, cx)))
            .on_action(cx.listener(|this, a: &ShuttleStop, _, cx| transport(&this.state, a, cx)))
            .on_action(cx.listener(|this, a: &PrevFrame, _, cx| transport(&this.state, a, cx)))
            .on_action(cx.listener(|this, a: &NextFrame, _, cx| transport(&this.state, a, cx)))
            .on_action(cx.listener(|this, a: &PrevCut, _, cx| {
                transport(&this.state, a, cx);
                this.reveal_playhead(cx);
            }))
            .on_action(cx.listener(|this, a: &NextCut, _, cx| {
                transport(&this.state, a, cx);
                this.reveal_playhead(cx);
            }))
            .child(self.toolbar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(self.headers(cx))
                    .child(
                        div()
                            .id("timeline-lanes")
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_hidden()
                            .cursor(cursor)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                                    this.mouse_down(ev, window, cx)
                                }),
                            )
                            .on_scroll_wheel(
                                cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                                    this.scroll(ev, cx)
                                }),
                            )
                            .on_pinch(
                                cx.listener(|this, ev: &PinchEvent, _, cx| this.pinch(ev, cx)),
                            )
                            .on_drag_move(cx.listener(
                                |this, ev: &DragMoveEvent<super::MediaDrag>, _, cx| {
                                    let p = ev.event.position;
                                    let row = if ev.bounds.contains(&p) {
                                        let (_, y) = this.local(p);
                                        let tl = &this.state.read(cx).bus.project().timeline;
                                        layout::row_at(&layout::rows(tl), y).map(|r| r.track)
                                    } else {
                                        None
                                    };
                                    if row != this.drop_row {
                                        this.drop_row = row;
                                        cx.notify();
                                    }
                                },
                            ))
                            .on_drop(cx.listener(|this, drag: &super::MediaDrag, window, cx| {
                                this.drop_media(drag, window, cx)
                            }))
                            .child(lanes)
                            .children(self.tooltip(cx)),
                    ),
            )
    }
}
