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
mod pointer;
mod refs;
mod scene;
mod silence;
mod thumbs;
mod toolbar;
mod waveform;

use super::transport;
use crate::state::EditorState;
use crate::ui::kit::{ContextMenuExt as _, Icon, IconName, h_flex, popover, theme as t, v_flex};
use flikk_core::command::{Command, Edge, InsertMode};
use flikk_core::model::{Clip, MarkerKind, MediaKind, Project, Timeline, TrackKind, TrackMix};
use flikk_core::{ChangeItemId, ChangeSetId, ClipId, Time, TimeRange, TrackId};
use gpui_kit::base::slider::{SliderEvent, SliderScale, SliderState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use layout::{RULER_HEIGHT, Row, Viewport};
use paint::DropHint;
use pointer::{Drag, Hit};
use silence::SilenceTool;
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

/// An empty-noted marker of `kind` over `range`, as one edit.
fn add_marker_over(
    s: &mut EditorState,
    range: TimeRange,
    kind: MarkerKind,
    cx: &mut Context<EditorState>,
) {
    s.edit(
        Command::AddMarker {
            range,
            kind,
            note: String::new(),
        },
        cx,
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Blade,
    /// Body drags slip, edge drags ripple-trim, shared edges roll.
    Trim,
}

/// What a right click landed on; the context menu reads it.
#[derive(Clone, Copy, Debug)]
enum MenuTarget {
    Clip(ClipId),
    Transition((ClipId, Edge)),
    Lane(Time),
    Ruler(Time, Option<flikk_core::MarkerId>),
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
    silence: SilenceTool,
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
    _zoom: Subscription,
}

impl TimelinePanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        bind_keys(cx);
        let focus = cx.focus_handle();
        // Transport and editing keys work from the start.
        window.focus(&focus, cx);
        let _observe = cx.observe(&state, |_, _, cx| cx.notify());
        let silence = SilenceTool::new(cx);
        let zoom = cx.new(|_| {
            SliderState::new()
                .min(layout::MIN_PX_PER_SEC)
                .max(layout::MAX_PX_PER_SEC)
                .scale(SliderScale::Logarithmic)
                .step(0.01)
                .default_value(Viewport::default().px_per_sec)
        });
        let _zoom = cx.subscribe(&zoom, |this, _, ev: &SliderEvent, cx| {
            let (SliderEvent::Change(v) | SliderEvent::Release(v)) = ev;
            let factor = v.end() / this.viewport.px_per_sec;
            this.zoom(factor, cx);
        });
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
            silence,
            zoom,
            drop: None,
            heights: HashMap::new(),
            clipboard: Vec::new(),
            alt: false,
            menu_target: None,
            menu_at: Point::default(),
            popup: None,
            _observe,
            _zoom,
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

    // ---------------------------------------------------------------- input

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
        media: Option<&[flikk_core::MediaId]>,
        cx: &mut Context<Self>,
    ) {
        let hint = if !bounds.contains(&p) {
            None
        } else if let Some(media) = media {
            // Several items show as one span on the first one's track.
            let landings = self.media_landings(media, p, cx);
            landings
                .first()
                .zip(landings.last())
                .map(|(first, last)| DropHint::Insert {
                    track: first.track,
                    range: TimeRange::new(first.range.start, last.range.end),
                    linked: first.linked,
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

    /// Where `media` dropped at `p` land, one after another, each its
    /// marked part (all of it without marks).
    fn media_landings(
        &mut self,
        media: &[flikk_core::MediaId],
        p: Point<Pixels>,
        cx: &App,
    ) -> Vec<Landing> {
        let (x, y) = self.local(p);
        let state = self.state.read(cx);
        let project = state.bus.project();
        let rows = self.rows(&project.timeline);
        let row = layout::row_at(&rows, y).map(|r| r.track);
        let rate = project.timeline.settings.rate;
        let mut at = self
            .snap_time(self.viewport.time(x).max(Time::ZERO), &[], cx)
            .floor_to_frame(rate);
        let mut out = Vec::new();
        for &id in media {
            let Some(item) = project.media(id) else {
                continue;
            };
            let source = state.source_range(item);
            let range = TimeRange::new(at, at + source.duration());
            let Some((track, linked)) = place(&project.timeline, item, row, range) else {
                continue;
            };
            out.push(Landing {
                media: id,
                from: source.start,
                track,
                range,
                linked,
            });
            at = range.end;
        }
        out
    }

    fn drop_media(&mut self, drag: &super::MediaDrag, window: &Window, cx: &mut Context<Self>) {
        self.drop = None;
        let landings = self.media_landings(&drag.all(), window.mouse_position(), cx);
        self.snap = None;
        let commands: Vec<Command> = landings
            .iter()
            .flat_map(|l| {
                let insert = move |track| Command::InsertClip {
                    media: l.media,
                    source: TimeRange::new(l.from, l.from + l.range.duration()),
                    track,
                    at: l.range.start,
                    mode: InsertMode::Overwrite,
                };
                std::iter::once(insert(l.track)).chain(l.linked.map(insert))
            })
            .collect();
        match commands.len() {
            0 => cx.notify(),
            1 => {
                let command = commands.into_iter().next().unwrap();
                self.state.update(cx, |s, cx| s.edit(command, cx));
            }
            _ => {
                self.state
                    .update(cx, |s, cx| s.edit_batch(commands, "insert_clip", cx));
            }
        }
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
        let drag = super::MediaDrag::one(media, drag.name.clone());
        self.drop_media(&drag, window, cx);
    }

    // -------------------------------------------------------------- actions

    /// A note marker at the playhead.
    fn add_marker(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let range = TimeRange::new(s.playhead, s.playhead);
            add_marker_over(s, range, MarkerKind::Note, cx);
        });
    }

    /// A protected marker over the in/out range, else over the selected
    /// clips; with neither, the status line says how to make one.
    fn add_protected_marker(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let tl = &s.bus.project().timeline;
            let selected = s
                .selection
                .clips
                .iter()
                .filter_map(|id| tl.clip(*id).map(|(_, c)| c.range()))
                .reduce(|a, b| TimeRange::new(a.start.min(b.start), a.end.max(b.end)));
            match s.in_out.filter(|r| !r.is_empty()).or(selected) {
                Some(range) => add_marker_over(s, range, MarkerKind::Protected, cx),
                None => {
                    s.status = Some("Mark in and out, or select clips, to protect a range".into());
                    cx.notify();
                }
            }
        });
    }

    fn toggle_mute(&mut self, track: TrackId, cx: &mut Context<Self>) {
        self.edit_mix(track, |mix| mix.mute = !mix.mute, cx);
    }

    fn toggle_solo(&mut self, track: TrackId, cx: &mut Context<Self>) {
        self.edit_mix(track, |mix| mix.solo = !mix.solo, cx);
    }

    /// `change` applied to `track`'s mix, as one edit.
    fn edit_mix(
        &mut self,
        track: TrackId,
        change: impl FnOnce(&mut TrackMix),
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            let Some(t) = s.bus.project().timeline.track(track) else {
                return;
            };
            let mut mix = t.mix.clone();
            change(&mut mix);
            s.edit(Command::SetTrackMix { track, mix }, cx);
        });
    }

    fn add_track(&mut self, kind: TrackKind, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let (index, name) = s.bus.project().timeline.new_track_slot(kind);
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

    // --------------------------------------------------------------- render

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

/// The transition stored on a clip edge.
fn transition_of(
    project: &Project,
    (clip, edge): (ClipId, Edge),
) -> Option<&flikk_core::fx::Transition> {
    let (_, c) = project.timeline.clip(clip)?;
    match edge {
        Edge::Start => c.transition_in.as_ref(),
        Edge::End => c.transition_out.as_ref(),
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
/// Where one item of a media drop goes.
struct Landing {
    media: flikk_core::MediaId,
    /// Where in the item the clip starts.
    from: Time,
    track: TrackId,
    range: TimeRange,
    /// The audio track a picture's sound goes to.
    linked: Option<TrackId>,
}

/// The track `item` lands on over `row` for `range`, and the audio track
/// its sound goes to. Over a track that cannot take it, or below the
/// tracks, it goes to the first track of its kind that is free there,
/// else the first of its kind.
fn place(
    tl: &flikk_core::model::Timeline,
    item: &flikk_core::model::MediaItem,
    row: Option<TrackId>,
    range: TimeRange,
) -> Option<(TrackId, Option<TrackId>)> {
    let wanted = match item.kind {
        MediaKind::Audio => TrackKind::Audio,
        _ => TrackKind::Video,
    };
    let has_audio = item.info.as_ref().is_some_and(|i| !i.audio.is_empty());
    let fits = |kind: TrackKind| match kind {
        TrackKind::Audio => item.kind != MediaKind::Image && (item.info.is_none() || has_audio),
        TrackKind::Video | TrackKind::Overlay => item.kind != MediaKind::Audio,
        TrackKind::Title | TrackKind::Caption => false,
    };
    let hovered = row.filter(|id| tl.track(*id).is_some_and(|t| fits(t.kind)));
    let mut kin = tl.tracks.iter().filter(|t| t.kind == wanted);
    let free = kin
        .clone()
        .find(|t| t.clips.iter().all(|c| !c.range().overlaps(&range)));
    let track = hovered.or(free.or_else(|| kin.next()).map(|t| t.id))?;
    // Video with sound lands as a linked pair: the mixer and renderer
    // read audio from audio tracks only.
    let linked = match tl.track(track).map(|t| t.kind) {
        Some(TrackKind::Video | TrackKind::Overlay) if has_audio => matching_audio_track(tl, track),
        _ => None,
    };
    Some((track, linked))
}

pub(crate) fn matching_audio_track(
    timeline: &flikk_core::model::Timeline,
    video: TrackId,
) -> Option<TrackId> {
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
        self.close_stale_popup(window, cx);
        let bounds = self.bounds.get();
        let duration = self.state.read(cx).bus.project().timeline.duration();
        if !self.fitted && bounds.size.width > px(0.) && duration > Time::ZERO {
            self.viewport.fit(duration, f32::from(bounds.size.width));
            self.fitted = true;
        }
        // Keep the playhead in view while playing.
        let (playing, playhead) = {
            let s = self.state.read(cx);
            (s.is_playing(), s.playhead)
        };
        let width = f32::from(bounds.size.width);
        if playing && width > 0. && self.drag.is_none() {
            let x = self.viewport.x(playhead);
            if x > width * 0.92 || x < 0. {
                self.viewport.scroll_x = (self.viewport.scroll_x + x - width * 0.1).max(0.);
            }
        }
        if self.silence.open {
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
                // Leaving the window sends no last move.
                let e = entity.clone();
                window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        e.update(cx, |this, cx| {
                            let hovered = this.hovered_item.take().is_some();
                            if this.hover.take().is_some() || hovered {
                                cx.notify();
                            }
                        });
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
            // The tool stays put while a drag runs: the drag was made with it.
            .on_action(cx.listener(|this, _: &TrimTool, _, cx| {
                if this.drag.is_some() {
                    return;
                }
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
                if this.drag.is_some() {
                    return;
                }
                this.tool = Tool::Select;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BladeTool, _, cx| {
                if this.drag.is_some() {
                    return;
                }
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
            .on_action(cx.listener(|this, _: &AddMarker, _, cx| this.add_marker(cx)))
            .on_action(
                cx.listener(|this, _: &AddProtectedMarker, _, cx| this.add_protected_marker(cx)),
            )
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(1.25, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(0.8, cx)))
            .on_action(cx.listener(|this, _: &ZoomToFit, _, cx| this.fit(cx)))
            .map(|d| transport::with_shared_handlers(d, &self.state))
            .on_action(
                cx.listener(|this, _: &PrevFrame, _, cx| transport::prev_frame(&this.state, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NextFrame, _, cx| transport::next_frame(&this.state, cx)),
            )
            .on_action(cx.listener(|this, _: &PrevCut, _, cx| {
                transport::prev_cut(&this.state, cx);
                this.reveal_playhead(cx);
            }))
            .on_action(cx.listener(|this, _: &NextCut, _, cx| {
                transport::next_cut(&this.state, cx);
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
                                    let media = ev.drag(cx).all();
                                    this.media_drag_moved(
                                        ev.event.position,
                                        ev.bounds,
                                        Some(&media),
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
