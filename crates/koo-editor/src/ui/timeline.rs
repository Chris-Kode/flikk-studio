//! Multi-track timeline (PRD E1, E2, E4, E8, AG4): track headers, a painted
//! ruler and lanes, select/move/trim/blade with snapping, markers, in/out,
//! JKL, the pending change-set overlay and the silence tool. Pro editing
//! (§8): transitions as objects, FX library drops, multi-select, clipboard,
//! nudge, a trim mode (slip, ripple, roll), clip menus, and references
//! dragged (Alt) or sent to the chat.

mod drops;
mod editing;
mod fx;
mod layout;
mod menu;
mod overlay;
mod paint;
mod refs;
mod silence;
mod thumbs;
mod waveform;

use super::RefDrag;
use crate::app::TogglePlay;
use crate::state::EditorState;
use crate::ui::kit::{
    Button, ContextMenuExt as _, DropdownMenu as _, Icon, IconName, Popover, PopupMenu,
    PopupMenuItem, Slider, Spinner, Tooltip, h_flex, popover, theme as t, v_flex,
};
use gpui_kit::base::slider::{SliderEvent, SliderScale, SliderState, SliderValue};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::command::{Command, Edge, InsertMode};
use koo_core::model::{
    Clip, ClipContent, MarkerKind, MediaKind, Project, Timeline, TrackKind, TrackMix,
};
use koo_core::{ChangeItemId, ChangeSetId, ClipId, Rate, Time, TimeRange, TrackId};
use layout::{EDGE_PX, HEADER_WIDTH, RULER_HEIGHT, Row, SNAP_PX, TOOLBAR_HEIGHT, Viewport};
use overlay::OverlayItem;
use paint::{ClipScene, DropHint, Ghost, Marquee, Scene, TrackScene, TransitionScene, Warp};
use silence::SilenceCache;
use std::cell::Cell;
use std::collections::HashMap;
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
        TrimTool,
        CopyClips,
        CutClips,
        PasteClips,
        DuplicateClips,
        NudgeLeft,
        NudgeRight,
        NudgeLeftMore,
        NudgeRightMore,
        ToggleEnabled,
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
        KeyBinding::new("t", TrimTool, t),
        KeyBinding::new("d", ToggleEnabled, t),
        KeyBinding::new(",", NudgeLeft, t),
        KeyBinding::new(".", NudgeRight, t),
        // Shifted punctuation arrives as the typed character on some
        // platforms and as shift + key on others.
        KeyBinding::new("shift-,", NudgeLeftMore, t),
        KeyBinding::new("shift-.", NudgeRightMore, t),
        KeyBinding::new("<", NudgeLeftMore, t),
        KeyBinding::new(">", NudgeRightMore, t),
    ];
    for m in ["cmd", "ctrl"] {
        bindings.extend([
            KeyBinding::new(&format!("{m}-c"), CopyClips, t),
            KeyBinding::new(&format!("{m}-x"), CutClips, t),
            KeyBinding::new(&format!("{m}-v"), PasteClips, t),
            KeyBinding::new(&format!("{m}-d"), DuplicateClips, t),
        ]);
    }
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
    let prefix = kind_prefix(kind);
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
    /// Body drags slip, edge drags ripple-trim, shared edges roll.
    Trim,
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

enum Hit {
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

/// What a right click landed on; the context menu reads it.
#[derive(Clone, Copy, Debug)]
enum MenuTarget {
    Clip(ClipId),
    Transition((ClipId, Edge)),
    Lane(Time),
    Ruler(Time, Option<koo_core::MarkerId>),
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
    /// The toolbar's zoom slider, in px per second.
    zoom: Entity<SliderState>,
    /// What a media or FX drag over the lanes would hit.
    drop: Option<DropHint>,
    /// Heights of tracks the user resized.
    heights: HashMap<TrackId, f32>,
    /// Copied clips with their tracks, at their timeline positions.
    clipboard: Vec<(TrackId, Clip)>,
    /// Alt is held: objects drag to the chat as references.
    alt: bool,
    menu_target: Option<MenuTarget>,
    /// Window position of the last right click: popups open there.
    menu_at: Point<Pixels>,
    popup: Option<menu::Popup>,
    _observe: Subscription,
    _sliders: [Subscription; 3],
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
        let zoom = cx.new(|_| {
            SliderState::new()
                .min(layout::MIN_PX_PER_SEC)
                .max(layout::MAX_PX_PER_SEC)
                .scale(SliderScale::Logarithmic)
                .step(0.01)
                .default_value(Viewport::default().px_per_sec)
        });
        let _sliders = [
            cx.subscribe(&silence_min, |_, _, _: &SliderEvent, cx| cx.notify()),
            cx.subscribe(&silence_padding, |_, _, _: &SliderEvent, cx| cx.notify()),
            cx.subscribe(&zoom, |this, _, ev: &SliderEvent, cx| {
                let (SliderEvent::Change(v) | SliderEvent::Release(v)) = ev;
                let factor = v.end() / this.viewport.px_per_sec;
                this.zoom(factor, cx);
            }),
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
            zoom,
            drop: None,
            heights: HashMap::new(),
            clipboard: Vec::new(),
            alt: false,
            menu_target: None,
            menu_at: Point::default(),
            popup: None,
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

    fn rows(&self, tl: &Timeline) -> Vec<Row> {
        layout::rows(tl, &self.heights)
    }

    fn hit(&self, p: Point<Pixels>, cx: &App) -> Hit {
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
                self.state
                    .update(cx, |s, cx| s.seek(at.floor_to_frame(rate), cx));
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
    fn right_mouse_down(
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
    fn marker_at(&self, x: f32, cx: &App) -> Option<koo_core::MarkerId> {
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
    fn select(&self, clips: Vec<ClipId>, cx: &mut App) {
        self.state.update(cx, |s, cx| {
            if s.selected_transition.is_some() {
                s.select_transition(None, cx);
            }
            s.select_clips(clips, cx);
        });
    }

    fn select_transition(&self, owner: (ClipId, Edge), cx: &mut App) {
        self.state.update(cx, |s, cx| {
            if !s.selection.clips.is_empty() {
                s.select_clips(vec![], cx);
            }
            s.select_transition(Some(owner), cx);
        });
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
    fn marquee_hits(&self, from: (Time, f32), to: (Time, f32), tl: &Timeline) -> Vec<ClipId> {
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

    fn mouse_moved(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        self.set_alt(ev.modifiers.alt, cx);
        if self.drag.is_some() {
            if ev.dragging() {
                self.drag_to(ev.position, cx);
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

    fn set_alt(&mut self, alt: bool, cx: &mut Context<Self>) {
        if alt != self.alt {
            self.alt = alt;
            cx.notify();
        }
    }

    /// Over something an edge drag resizes (or, in trim mode, any clip).
    fn edge_hover(&self, p: Option<Point<Pixels>>, cx: &App) -> bool {
        p.is_some_and(|p| match self.hit(p, cx) {
            Hit::Clip { edge, .. } => edge.is_some() || self.tool == Tool::Trim,
            Hit::Transition { resize, .. } => resize,
            _ => false,
        })
    }

    /// The clip edge under `p` that a drag would trim.
    fn hovered_edge(&self, p: Option<Point<Pixels>>, cx: &App) -> Option<(ClipId, Edge)> {
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
    fn trim_mode(&self, cx: &App) -> Option<(IconName, &'static str)> {
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
                    return;
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
                    return;
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
                    return;
                };
                let (Some(before), after) = handles(&project, c) else {
                    return;
                };
                // Dragging right shows earlier source: the in point moves left.
                let delta = floor_to(grab - at, frame);
                let hi = after.map_or(i64::MAX, |a| a.0);
                let delta = Time(delta.0.clamp(-before.0, hi));
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
                    return;
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

    fn mouse_up(&mut self, cx: &mut Context<Self>) {
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
                    let transition = Some(koo_core::fx::Transition { duration, ..t });
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
            let rows = self.rows(&self.state.read(cx).bus.project().timeline);
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
        let range = overlay::collect(&state.bus.project().timeline, state.bus.chat_change_sets())
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

    /// Show where a media drag at `p` would land; a sound not yet in the
    /// pool (no `media`) shows only the row.
    fn media_drag_moved(
        &mut self,
        p: Point<Pixels>,
        bounds: Bounds<Pixels>,
        media: Option<koo_core::MediaId>,
        cx: &mut Context<Self>,
    ) {
        let hint = if !bounds.contains(&p) {
            None
        } else if let Some(media) = media {
            self.media_landing(media, p, cx)
                .map(|(track, range, linked)| DropHint::Insert {
                    track,
                    range,
                    linked,
                })
        } else {
            let (_, y) = self.local(p);
            let tl = &self.state.read(cx).bus.project().timeline;
            layout::row_at(&self.rows(tl), y).map(|r| DropHint::Row(r.track))
        };
        if hint.is_none() {
            self.snap = None;
        }
        self.set_drop(hint, cx);
    }

    /// Where `media` dropped at `p` lands: its track, its span there, and
    /// the audio track its sound goes to.
    fn media_landing(
        &mut self,
        media: koo_core::MediaId,
        p: Point<Pixels>,
        cx: &App,
    ) -> Option<(TrackId, TimeRange, Option<TrackId>)> {
        let (x, y) = self.local(p);
        let project = self.state.read(cx).bus.project();
        let item = project.media(media)?;
        let rows = self.rows(&project.timeline);
        let wanted = match item.kind {
            MediaKind::Audio => TrackKind::Audio,
            _ => TrackKind::Video,
        };
        let track = layout::row_at(&rows, y).map(|r| r.track).or_else(|| {
            project
                .timeline
                .tracks
                .iter()
                .find(|t| t.kind == wanted)
                .map(|t| t.id)
        })?;
        let len = item
            .info
            .as_ref()
            .map(|i| i.duration)
            .filter(|d| d.0 > 0)
            .unwrap_or(Time::from_secs_f64(5.));
        let has_audio = item.info.as_ref().is_some_and(|i| !i.audio.is_empty());
        // Video with sound lands as a linked pair: the mixer and renderer
        // read audio from audio tracks only.
        let linked = match project.timeline.track(track).map(|t| t.kind) {
            Some(TrackKind::Video | TrackKind::Overlay) if has_audio => {
                matching_audio_track(&project.timeline, track)
            }
            _ => None,
        };
        let rate = project.timeline.settings.rate;
        let at = self
            .snap_time(self.viewport.time(x).max(Time::ZERO), &[], cx)
            .floor_to_frame(rate);
        Some((track, TimeRange::new(at, at + len), linked))
    }

    fn drop_media(&mut self, drag: &super::MediaDrag, window: &Window, cx: &mut Context<Self>) {
        self.drop = None;
        let landing = self.media_landing(drag.media, window.mouse_position(), cx);
        self.snap = None;
        let Some((track, range, linked)) = landing else {
            cx.notify();
            return;
        };
        let insert = |track| Command::InsertClip {
            media: drag.media,
            source: TimeRange::new(Time::ZERO, range.duration()),
            track,
            at: range.start,
            mode: InsertMode::Overwrite,
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

    /// Add the sound to the pool, then drop it like pool media.
    fn drop_sound(&mut self, drag: &super::SoundDrag, window: &Window, cx: &mut Context<Self>) {
        let media = self.state.update(cx, |s, cx| {
            super::library::shared::use_sound(s, drag.sound, cx)
        });
        let Some(media) = media else {
            self.drop = None;
            return;
        };
        let drag = super::MediaDrag {
            media,
            name: drag.name.clone(),
        };
        self.drop_media(&drag, window, cx);
    }

    // -------------------------------------------------------------- actions

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
        let hover_edge = self
            .hovered_edge(self.hover, cx)
            .filter(|_| self.drag.is_none());
        let state = self.state.read(cx);
        let project = state.bus.project();
        let media = state.services.media.clone();
        let mut selection = state.selection.clips.clone();
        let selected_transition = state.selected_transition;
        let playhead = state.playhead;
        let in_out = state.in_out;
        let focused = state.focused_change().map(|i| i.id);
        let overlay: Vec<OverlayItem> =
            overlay::collect(&project.timeline, state.bus.chat_change_sets());
        let tl = &project.timeline;
        let any_solo = tl
            .tracks
            .iter()
            .any(|t| t.kind == TrackKind::Audio && t.mix.solo);
        let rate = tl.settings.rate;

        let mut marquee = None;
        if let Some(Drag::Marquee {
            from,
            to,
            base,
            moved: true,
        }) = &self.drag
        {
            selection = base.clone();
            selection.extend(self.marquee_hits(*from, *to, tl));
            marquee = Some(Marquee {
                from: *from,
                to: *to,
            });
        }

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
                            label: None,
                            edge: None,
                        });
                    }
                }
            }
            Some(Drag::Trim {
                clip, edge, delta, ..
            }) => {
                if let Some((track, c)) = tl.clip(*clip) {
                    let r = c.range();
                    let (range, moved) = match edge {
                        Edge::Start => (TimeRange::new(r.start + *delta, r.end), r.start + *delta),
                        Edge::End => (TimeRange::new(r.start, r.end + *delta), r.end + *delta),
                    };
                    ghosts.push(Ghost {
                        track: track.id,
                        range,
                        label: Some(signed_timecode(*delta, rate).into()),
                        edge: Some(moved),
                    });
                }
            }
            Some(Drag::Roll { left, right, delta }) => {
                if let (Some((track, l)), Some((_, r))) = (tl.clip(*left), tl.clip(*right)) {
                    let cut = l.end() + *delta;
                    for (range, label) in [
                        (
                            TimeRange::new(l.start, cut),
                            Some(signed_timecode(*delta, rate)),
                        ),
                        (TimeRange::new(cut, r.end()), None),
                    ] {
                        ghosts.push(Ghost {
                            track: track.id,
                            range,
                            label: label.map(Into::into),
                            edge: Some(cut),
                        });
                    }
                }
            }
            Some(Drag::Slip { clip, delta, .. }) => {
                if let Some((track, c)) = tl.clip(*clip) {
                    ghosts.push(Ghost {
                        track: track.id,
                        range: c.range(),
                        label: Some(signed_timecode(*delta, rate).into()),
                        edge: None,
                    });
                }
            }
            _ => {}
        }

        let rows = self.rows(tl);
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
                        let icon = if !clip.enabled {
                            Some(IconName::EyeOff)
                        } else if matches!(clip.content, ClipContent::Generator { .. }) {
                            Some(IconName::PaintBucket)
                        } else if track.kind == TrackKind::Title {
                            Some(IconName::Type)
                        } else {
                            None
                        };
                        ClipScene {
                            tiles,
                            range: clip.range(),
                            label: clip.display_name(project).into(),
                            color: clip.color.map(|c| rgb(c.rgb())),
                            enabled: clip.enabled,
                            offline: clip.media().is_some_and(|m| state.offline.contains(&m)),
                            icon,
                            effects: !clip.effects.is_empty(),
                            speed: if clip.is_ramped() {
                                Some("ramp".into())
                            } else {
                                ((clip.speed - 1.).abs() > 1e-6)
                                    .then(|| format!("{}%", (clip.speed * 100.).round()).into())
                            },
                            warp: (clip.media().is_some() && clip.is_retimed()).then(|| {
                                let clip = clip.clone();
                                std::sync::Arc::new(move |t: Time| clip.source_offset(t)) as Warp
                            }),
                            selected: selection.contains(&clip.id),
                            lifted: lifted.contains(&clip.id),
                            hover_edge: hover_edge
                                .filter(|(id, _)| *id == clip.id)
                                .map(|(_, edge)| edge),
                            peaks,
                            fade_in: clip.audio.fade_in,
                            fade_out: clip.audio.fade_out,
                        }
                    })
                    .collect();
                let audio = track.kind == TrackKind::Audio;
                let hidden = !audio && track.mix.mute;
                let silent = audio && (track.mix.mute || (any_solo && !track.mix.solo));
                let fade = if hidden {
                    0.3
                } else if silent {
                    0.45
                } else {
                    1.
                };
                TrackScene {
                    row: *row,
                    clips,
                    fade: if track.locked { fade * 0.55 } else { fade },
                    locked: track.locked,
                    hidden,
                }
            })
            .collect();

        let resizing = match &self.drag {
            Some(Drag::TransitionEdge { owner, duration }) => Some((*owner, *duration)),
            _ => None,
        };
        let mut transitions = Vec::new();
        for track in &tl.tracks {
            for t in fx::placed(track) {
                let range = match resizing {
                    Some((owner, d)) if owner == t.owner => t.range_with(d),
                    _ => t.range,
                };
                if range.end < visible.start || range.start > visible.end {
                    continue;
                }
                transitions.push(TransitionScene {
                    track: track.id,
                    range,
                    label: format!("{:.1}s", range.duration().as_secs_f64()).into(),
                    selected: selected_transition == Some(t.owner),
                });
            }
        }

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
            scrubbing: matches!(self.drag, Some(Drag::Scrub)),
            overlay,
            focused,
            hovered: self.hovered_item.map(|(_, i)| i),
            silences,
            transitions,
            ghosts,
            snap: self.snap,
            blade,
            drop: self.drop,
            marquee,
            render: self.state.read(cx).render.status(),
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
        let (_, strip) = layout::filmstrip(row.height - layout::CLIP_INSET * 2.);
        let tile_w = (strip * 16. / 9.).max(8.);
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
        let tool = |id: &'static str, icon: IconName, which: Tool, tip: &'static str, kbd| {
            Button::new(id)
                .ghost()
                .small()
                .w(px(24.))
                .icon(icon)
                .selected(self.tool == which)
                .tooltip_kbd(tip, kbd)
        };
        let toggle = |id: &'static str, icon: IconName, on: bool| {
            Button::new(id)
                .ghost()
                .small()
                .w(px(24.))
                .icon(icon)
                .selected(on)
                .accent(on)
        };
        let separator = || div().flex_none().w(px(1.)).h(px(14.)).mx(px(4.)).bg(t::LN);
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
            .trigger(
                toggle("silence", IconName::AudioLines, self.silence_open)
                    .tooltip("Remove silences"),
            )
            .content(move |_, _, cx| silence_content(&entity, cx));
        let settings = &self.state.read(cx).bus.project().timeline.settings;
        let fps = format!("{:.2}", settings.rate.fps());
        let format = format!(
            "{}×{} · {} fps",
            settings.width,
            settings.height,
            fps.trim_end_matches('0').trim_end_matches('.')
        );
        let mode = self.trim_mode(cx);
        let pps = self.viewport.px_per_sec;

        h_flex()
            .flex_none()
            .h(px(TOOLBAR_HEIGHT))
            .px(px(8.))
            .gap(px(4.))
            .border_b_1()
            .border_color(t::LN)
            .child(
                tool(
                    "tool-select",
                    IconName::MousePointer2,
                    Tool::Select,
                    "Select",
                    "V",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.tool = Tool::Select;
                    cx.notify();
                })),
            )
            .child(
                tool("tool-blade", IconName::Scissors, Tool::Blade, "Blade", "B").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.tool = Tool::Blade;
                        cx.notify();
                    }),
                ),
            )
            .child(
                tool(
                    "tool-trim",
                    IconName::MoveHorizontal,
                    Tool::Trim,
                    "Trim",
                    "T",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.tool = Tool::Trim;
                    cx.notify();
                })),
            )
            .children(mode.map(|(icon, label)| {
                h_flex()
                    .flex_none()
                    .h(px(20.))
                    .ml(px(4.))
                    .px(px(7.))
                    .gap(px(5.))
                    .rounded(t::RADIUS_SM)
                    .border_1()
                    .border_color(t::LN)
                    .bg(t::G)
                    .text_size(px(11.))
                    .text_color(t::T1)
                    .child(Icon::new(icon).size(px(12.)).color(t::T2))
                    .child(label)
            }))
            .child(separator())
            .child(
                toggle("snapping", IconName::Magnet, self.snapping)
                    .tooltip_kbd("Snapping", "N")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.snapping = !this.snapping;
                        cx.notify();
                    })),
            )
            .child(silence)
            .child(
                Button::new("marker")
                    .ghost()
                    .small()
                    .w(px(24.))
                    .icon(IconName::BookmarkPlus)
                    .tooltip_kbd("Add marker", "M")
                    .on_click(cx.listener(|this, _, _, cx| this.add_marker(false, cx))),
            )
            .child(
                div()
                    .ml(px(10.))
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(t::T3)
                    .child(format),
            )
            .child(div().flex_1())
            .child(
                Button::new("zoom-out")
                    .ghost()
                    .small()
                    .icon(IconName::ZoomOut)
                    .tooltip_kbd("Zoom out", "-")
                    .disabled(pps <= layout::MIN_PX_PER_SEC)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(0.8, cx))),
            )
            .child(Slider::new(&self.zoom).flex_none().w(px(90.)))
            .child(
                Button::new("zoom-in")
                    .ghost()
                    .small()
                    .icon(IconName::ZoomIn)
                    .tooltip_kbd("Zoom in", "=")
                    .disabled(pps >= layout::MAX_PX_PER_SEC)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(1.25, cx))),
            )
            .child(
                Button::new("zoom-fit")
                    .ghost()
                    .small()
                    .icon(IconName::Maximize2)
                    .tooltip_kbd("Zoom to fit", "Shift Z")
                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
            )
    }

    fn headers(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let alt = self.alt;
        let state = self.state.read(cx);
        let timecode = state
            .playhead
            .timecode(state.bus.project().timeline.settings.rate);
        let tl = &state.bus.project().timeline;
        let any_solo = tl
            .tracks
            .iter()
            .any(|t| t.kind == TrackKind::Audio && t.mix.solo);
        let rows: Vec<_> = self
            .rows(tl)
            .into_iter()
            .zip(tl.tracks.iter().cloned())
            .zip(track_codes(tl))
            .collect();
        let panel = cx.entity();
        let children = rows.into_iter().map(|((row, track), code)| {
            let id = track.id;
            let locked = track.locked;
            let panel = panel.clone();
            let audio = track.kind == TrackKind::Audio;
            let muted = audio && (track.mix.mute || (any_solo && !track.mix.solo));
            let hidden = !audio && track.mix.mute;
            let key = |name: &str| SharedString::from(format!("{name}-{}", track.id));
            let name = if track.name == code {
                kind_label(track.kind).to_string()
            } else {
                track.name.clone()
            };
            let reference = refs::track(&track);
            h_flex()
                .id(ElementId::Name(key("header")))
                .relative()
                .flex_none()
                .h(px(row.height))
                .pl(px(8.))
                .pr(px(6.))
                .gap(px(4.))
                .border_b_1()
                .border_color(t::LN)
                .text_size(px(11.))
                .hover(|s| s.bg(t::HV))
                .when(audio, |el| {
                    el.drag_over::<super::AudioFxDrag>(|style, _, _, _| {
                        style.bg(t::alpha(t::AC, 0.15))
                    })
                    .on_drop(cx.listener(
                        move |this, drag: &super::AudioFxDrag, _, cx| {
                            this.drop_audio_fx_on_track(drag, id, cx)
                        },
                    ))
                })
                .when(alt, |el| {
                    // Alt-drag hands the track to the chat.
                    el.on_drag(
                        RefDrag {
                            refs: vec![reference.clone()],
                        },
                        |d, _, _, cx| cx.new(|_| d.clone()),
                    )
                })
                .child(
                    div()
                        .id(ElementId::Name(key("resize")))
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(-2.))
                        .h(px(5.))
                        .cursor(CursorStyle::ResizeUpDown)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.drag = Some(Drag::TrackHeight {
                                    track: id,
                                    grab: ev.position.y,
                                    from: row.height,
                                });
                            }),
                        ),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .justify_center()
                        .w(px(24.))
                        .h(px(18.))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(t::LN)
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t::T2)
                        .child(code),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(if muted || hidden { t::T3 } else { t::T2 })
                        .child(name),
                )
                .child(
                    Button::new(ElementId::Name(key("lock")))
                        .ghost()
                        .xsmall()
                        .icon(if locked {
                            IconName::Lock
                        } else {
                            IconName::LockOpen
                        })
                        .selected(locked)
                        .tooltip(if locked { "Unlock" } else { "Lock" })
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_lock(id, cx))),
                )
                .when(!audio, |el| {
                    el.child(
                        Button::new(ElementId::Name(key("hide")))
                            .ghost()
                            .xsmall()
                            .icon(if hidden {
                                IconName::EyeOff
                            } else {
                                IconName::Eye
                            })
                            .selected(hidden)
                            .tooltip(if hidden { "Show" } else { "Hide" })
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.toggle_mix(id, false, cx)),
                            ),
                    )
                })
                .when(audio, |el| {
                    el.child(
                        mix_toggle(key("mute"), "M", track.mix.mute, t::T2, "Mute").on_click(
                            cx.listener(move |this, _, _, cx| this.toggle_mix(id, false, cx)),
                        ),
                    )
                    .child(
                        mix_toggle(key("solo"), "S", track.mix.solo, t::WARN, "Solo").on_click(
                            cx.listener(move |this, _, _, cx| this.toggle_mix(id, true, cx)),
                        ),
                    )
                })
                .context_menu(move |menu, _, _| {
                    let remove = panel.clone();
                    let mention = panel.clone();
                    let reference = reference.clone();
                    add_track_items(menu, &panel)
                        .separator()
                        .item(
                            PopupMenuItem::new("Add to chat")
                                .icon(IconName::MessageSquarePlus)
                                .on_click(move |_, _, cx| {
                                    let r = reference.clone();
                                    mention.update(cx, |this, cx| this.mention(vec![r], cx))
                                }),
                        )
                        .separator()
                        .item(
                            PopupMenuItem::new("Delete track")
                                .icon(IconName::Trash)
                                .destructive()
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
            .border_color(t::LN)
            .child(
                h_flex()
                    .flex_none()
                    .h(px(RULER_HEIGHT))
                    .px(px(8.))
                    .border_b_1()
                    .border_color(t::LN)
                    .font_family(t::mono())
                    .text_size(px(11.))
                    .text_color(t::AC)
                    .child(timecode),
            )
            .child(
                div().flex_1().min_h_0().overflow_hidden().relative().child(
                    v_flex()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(-self.viewport.scroll_y))
                        .children(children)
                        .child(
                            div().px(px(6.)).py(px(6.)).child(
                                Button::new("add-track")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Plus)
                                    .label("Track")
                                    .dropdown_menu({
                                        let panel = cx.entity();
                                        move |menu, _, _| add_track_items(menu, &panel)
                                    }),
                            ),
                        ),
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
        let hovered = overlay::collect(&state.bus.project().timeline, state.bus.chat_change_sets())
            .into_iter()
            .find(|i| i.set == set && i.item == item)?;
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
            popover::surface()
                .absolute()
                .left(x)
                .top(y)
                .max_w(px(320.))
                .px(px(8.))
                .py(px(5.))
                .rounded(t::RADIUS)
                .flex()
                .flex_col()
                .gap(px(2.))
                .text_size(px(11.))
                .child(hovered.reason)
                .children(warnings.into_iter().map(|w| {
                    h_flex()
                        .gap(px(4.))
                        .text_color(t::WARN)
                        .child(Icon::new(IconName::TriangleAlert).size(px(11.)))
                        .child(w)
                })),
        )
    }
}

/// A header's M or S button: `color` fills it while `on`.
fn mix_toggle(
    id: SharedString,
    label: &'static str,
    on: bool,
    color: Rgba,
    tip: &'static str,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(18.))
        .rounded(px(3.))
        .border_1()
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .map(|el| {
            if on {
                el.bg(color).border_color(color).text_color(t::G)
            } else {
                el.border_color(t::LN)
                    .text_color(t::T3)
                    .hover(|s| s.bg(t::HV).text_color(t::T1))
            }
        })
        .tooltip(Tooltip::text(tip))
        .child(label)
}

/// The header chips: `V1` is the picture track nearest the audio, `A1` the
/// topmost audio track; captions and titles count down from the top.
fn track_codes(tl: &Timeline) -> Vec<String> {
    let up = |kind: TrackKind| matches!(kind, TrackKind::Video | TrackKind::Overlay);
    let order = tl
        .tracks
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, t)| up(t.kind))
        .chain(tl.tracks.iter().enumerate().filter(|(_, t)| !up(t.kind)));
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut codes = vec![String::new(); tl.tracks.len()];
    for (i, track) in order {
        let prefix = kind_prefix(track.kind);
        let n = counts.entry(prefix).or_default();
        *n += 1;
        codes[i] = format!("{prefix}{n}");
    }
    codes
}

fn kind_prefix(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "V",
        TrackKind::Audio => "A",
        TrackKind::Caption => "C",
        TrackKind::Title => "T",
        TrackKind::Overlay => "O",
    }
}

/// A header's name when the track has only its code.
fn kind_label(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "Video",
        TrackKind::Audio => "Audio",
        TrackKind::Caption => "Captions",
        TrackKind::Title => "Titles",
        TrackKind::Overlay => "Overlay",
    }
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

/// The transition stored on a clip edge.
fn transition_of(
    project: &Project,
    (clip, edge): (ClipId, Edge),
) -> Option<&koo_core::fx::Transition> {
    let (_, c) = project.timeline.clip(clip)?;
    match edge {
        Edge::Start => c.transition_in.as_ref(),
        Edge::End => c.transition_out.as_ref(),
    }
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

/// Lane-local x span of a transition block, at least 6 px wide.
fn transition_span(vp: &Viewport, range: TimeRange) -> (f32, f32) {
    let (a, b) = (vp.x(range.start), vp.x(range.end));
    let pad = ((6. - (b - a)) / 2.).max(0.);
    (a - pad, b + pad)
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

/// A signed offset as `+S:FF` (seconds and frames).
fn signed_timecode(t: Time, rate: Rate) -> String {
    let fps = rate.rounded_fps() as i64;
    let sign = if t.0 < 0 { '-' } else { '+' };
    let frames = Time(t.0.abs()).to_frame(rate);
    format!("{sign}{}:{:02}", frames / fps.max(1), frames % fps.max(1))
}

/// The silence tool's popover: thresholds, what they find, and Remove.
fn silence_content(panel: &Entity<TimelinePanel>, cx: &mut App) -> AnyElement {
    let this = panel.read(cx);
    let close = {
        let panel = panel.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            panel.update(cx, |this, cx| {
                this.silence_open = false;
                cx.notify();
            })
        }
    };
    let card = v_flex()
        .w(px(236.))
        .px(px(12.))
        .py(px(10.))
        .gap(px(9.))
        .child(
            h_flex()
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::MEDIUM)
                        .child("Remove silences"),
                )
                .child(
                    Button::new("silence-close")
                        .ghost()
                        .xsmall()
                        .icon(IconName::X)
                        .on_click(close),
                ),
        );
    if this.state.read(cx).services.analysis.is_none() {
        return card
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(t::T2)
                    .child("Analysis is not available"),
            )
            .into_any_element();
    }
    let (min, pad) = this.silence_params(cx);
    let project = this.state.read(cx).bus.project();
    let found = this.silences.timeline_silences(project, min, pad);
    let total: f64 = found.iter().map(|r| r.duration().as_secs_f64()).sum();
    let loading = this.silences.is_loading();
    let row = |label: &'static str, slider: &Entity<SliderState>, value: String| {
        h_flex()
            .gap(px(8.))
            .text_size(px(11.))
            .child(div().w(px(62.)).flex_none().text_color(t::T2).child(label))
            .child(Slider::new(slider).flex_1())
            .child(
                div()
                    .w(px(44.))
                    .flex_none()
                    .text_right()
                    .font_family(t::mono())
                    .child(value),
            )
    };
    let summary = if loading {
        h_flex()
            .gap(px(6.))
            .text_color(t::T2)
            .child(Spinner::new().size(px(11.)))
            .child("Analyzing…")
    } else if found.is_empty() {
        h_flex().text_color(t::T3).child("Nothing found")
    } else {
        h_flex()
            .font_family(t::mono())
            .text_color(t::T2)
            .child(format!("{} regions · ", found.len()))
            .child(div().text_color(t::ERR_TX).child(format!("−{total:.1} s")))
    };
    let panel = panel.clone();
    card.child(row(
        "Min length",
        &this.silence_min,
        format!("{:.1} s", min.as_secs_f64()),
    ))
    .child(row(
        "Padding",
        &this.silence_padding,
        format!("{:.2} s", pad.as_secs_f64()),
    ))
    .child(
        h_flex()
            .gap(px(8.))
            .pt(px(9.))
            .border_t_1()
            .border_color(t::LN)
            .text_size(px(11.))
            .child(summary.flex_1())
            .child(
                Button::new("apply-silences")
                    .primary()
                    .small()
                    .label("Remove")
                    .disabled(found.is_empty())
                    .on_click(move |_, _, cx| panel.update(cx, |this, cx| this.apply_silences(cx))),
            ),
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
        self.alt = window.modifiers().alt;
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
        } else if matches!(self.drag, Some(Drag::TrackHeight { .. })) {
            CursorStyle::ResizeUpDown
        } else if matches!(
            self.drag,
            Some(
                Drag::Trim { .. }
                    | Drag::Roll { .. }
                    | Drag::Slip { .. }
                    | Drag::TransitionEdge { .. }
            )
        ) || self.edge_hover(self.hover, cx)
        {
            CursorStyle::ResizeLeftRight
        } else {
            CursorStyle::Arrow
        };
        let overlays = self.ref_overlays(cx);
        let popup = self.render_popup(cx);
        let menu_panel = cx.entity();
        // Keep the zoom slider on the viewport's zoom, however it changed.
        let pps = self.viewport.px_per_sec;
        if (self.zoom.read(cx).value().end() - pps).abs() > pps * 1e-3 {
            self.zoom.update(cx, |s, cx| s.set_value(pps, window, cx));
        }

        let main = v_flex()
            .size_full()
            .bg(t::P)
            .track_focus(&self.focus)
            .key_context("Timeline")
            .on_modifiers_changed(cx.listener(|this, ev: &ModifiersChangedEvent, _, cx| {
                this.set_alt(ev.modifiers.alt, cx)
            }))
            .on_action(cx.listener(|this, _: &TrimTool, _, cx| {
                this.tool = if this.tool == Tool::Trim {
                    Tool::Select
                } else {
                    Tool::Trim
                };
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CopyClips, _, cx| this.copy(cx)))
            .on_action(cx.listener(|this, _: &CutClips, _, cx| this.cut(cx)))
            .on_action(cx.listener(|this, _: &PasteClips, _, cx| this.paste(cx)))
            .on_action(cx.listener(|this, _: &DuplicateClips, _, cx| this.duplicate(cx)))
            .on_action(cx.listener(|this, _: &NudgeLeft, _, cx| this.nudge(-1, cx)))
            .on_action(cx.listener(|this, _: &NudgeRight, _, cx| this.nudge(1, cx)))
            .on_action(cx.listener(|this, _: &NudgeLeftMore, _, cx| this.nudge(-10, cx)))
            .on_action(cx.listener(|this, _: &NudgeRightMore, _, cx| this.nudge(10, cx)))
            .on_action(cx.listener(|this, _: &ToggleEnabled, _, cx| this.toggle_enabled(cx)))
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
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                                    this.right_mouse_down(ev, window, cx)
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
                                    let media = ev.drag(cx).media;
                                    this.media_drag_moved(
                                        ev.event.position,
                                        ev.bounds,
                                        Some(media),
                                        cx,
                                    )
                                },
                            ))
                            .on_drop(cx.listener(|this, drag: &super::MediaDrag, window, cx| {
                                this.drop_media(drag, window, cx)
                            }))
                            .on_drag_move(cx.listener(
                                |this, ev: &DragMoveEvent<super::SoundDrag>, _, cx| {
                                    this.media_drag_moved(ev.event.position, ev.bounds, None, cx)
                                },
                            ))
                            .on_drop(cx.listener(|this, drag: &super::SoundDrag, window, cx| {
                                this.drop_sound(drag, window, cx)
                            }))
                            .on_drag_move(cx.listener(
                                |this, ev: &DragMoveEvent<super::FxDrag>, _, cx| {
                                    let drag = ev.drag(cx).clone();
                                    this.fx_drag_moved(&drag, ev.event.position, cx)
                                },
                            ))
                            .on_drop(cx.listener(|this, drag: &super::FxDrag, window, cx| {
                                this.drop_fx(drag, window, cx)
                            }))
                            .on_drag_move(cx.listener(
                                |this, ev: &DragMoveEvent<super::AudioFxDrag>, _, cx| {
                                    this.audio_fx_drag_moved(ev.event.position, cx)
                                },
                            ))
                            .on_drop(cx.listener(|this, drag: &super::AudioFxDrag, window, cx| {
                                this.drop_audio_fx(drag, window, cx)
                            }))
                            .context_menu(move |menu, window, cx| {
                                menu::build(&menu_panel, menu, window, cx)
                            })
                            .child(lanes)
                            .children(overlays)
                            .children(self.tooltip(cx)),
                    ),
            );
        div().size_full().child(main).children(popup)
    }
}
