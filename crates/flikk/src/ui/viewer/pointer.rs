//! Pointer and key input on the picture: clicks that pick, press and drag
//! items and masks, held clicks replayed once a layout lands, zoom and
//! pan by wheel, and nudging.

use super::gizmo::{self, Screen};
use super::{Request, ViewerPanel, ZOOM_STEP};
use flikk_core::Time;
use flikk_core::compose;
use flikk_core::model::{Project, TrackKind};
use gpui_kit::*;

/// What a click on the frame does.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum ClickMode {
    /// Picks a depth effect's focus.
    Focus((flikk_core::ClipId, flikk_core::EffectId)),
    Masks,
    Items,
}

/// A click that came before its frame was laid out, kept until it is.
pub(super) struct HeldClick {
    at: Point<Pixels>,
    modifiers: Modifiers,
    /// Where the frame was on screen at the press.
    screen: Screen,
    mode: ClickMode,
    /// The frame it was made on.
    pub(super) request: Request,
    /// The mouse is already up: a click, not the start of a drag.
    released: bool,
}

/// Mouse down on the frame.
pub(super) struct Press {
    /// The items under it, topmost first.
    under: Vec<flikk_core::ClipId>,
    /// Alt: a click picks the next item under the mouse.
    alt: bool,
    /// Shift on a selected item: a click takes it out of the selection.
    deselect: Option<flikk_core::ClipId>,
}

/// The `selected` clips that show a picture or lettering at `playhead`.
fn picked_clips<'a>(
    selected: &[flikk_core::ClipId],
    project: &'a Project,
    playhead: Time,
) -> Vec<(&'a flikk_core::model::Track, &'a flikk_core::model::Clip)> {
    selected
        .iter()
        .filter_map(|id| project.timeline.clip(*id))
        .filter(|(track, clip)| track.kind != TrackKind::Audio && clip.range().contains(playhead))
        .collect()
}

impl ViewerPanel {
    pub(super) fn mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let screen = self.screen(cx);
        self.press(ev.position, ev.modifiers, screen, cx);
    }

    /// What a click does now.
    fn click_mode(&self, cx: &App) -> ClickMode {
        match self.state.read(cx).depth_pick {
            Some(pick) => ClickMode::Focus(pick),
            None if self.masks.on => ClickMode::Masks,
            None => ClickMode::Items,
        }
    }

    /// Apply the held click once its frame is laid out, as the click it
    /// was; drop it if the frame or the kind of click changed meanwhile.
    pub(super) fn replay_click(&mut self, cx: &mut Context<Self>) {
        let Some(click) = self.held_click.take() else {
            return;
        };
        if self.requested != Some(click.request) || self.click_mode(cx) != click.mode {
            return;
        }
        let laid_out = self
            .gizmo
            .shapes
            .as_ref()
            .is_some_and(|(r, _)| *r == click.request);
        if !laid_out {
            self.held_click = Some(click);
            return;
        }
        self.press(click.at, click.modifiers, click.screen, cx);
        if click.released {
            self.release(cx);
        }
    }

    /// Mouse down at `p` on the frame at `screen`.
    fn press(
        &mut self,
        p: Point<Pixels>,
        modifiers: Modifiers,
        screen: Screen,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.read(cx);
        if !self.editable(state) {
            return;
        }
        let project = state.bus.project();
        let playhead = state.playhead;
        let editable = state.preview.is_none();
        let mut selected = state.selection.clips.clone();
        let request = self.requested;
        let under = self.gizmo.clips_at(p, &screen, request);
        let handle = self
            .gizmo
            .handle_at(p, &screen, project, &selected, playhead, request);
        // Not laid out yet (just edited or seeked): the box just dragged
        // still works; anything else waits for the layout.
        let laid_out = self
            .gizmo
            .shapes
            .as_ref()
            .is_some_and(|(r, _)| Some(*r) == request);
        if !laid_out && handle.is_none() {
            let mode = self.click_mode(cx);
            self.held_click = request.map(|request| HeldClick {
                at: p,
                modifiers,
                screen,
                mode,
                request,
                released: false,
            });
            return;
        }
        self.held_click = None;
        if let Some(pick) = self.state.read(cx).depth_pick {
            self.pick_focus(pick, p, &screen, cx);
            return;
        }
        if self.masks.on {
            self.mask_press(p, modifiers, &screen, under, cx);
            return;
        }
        let alt = modifiers.alt;
        let mut press = Press {
            under: under.clone(),
            alt,
            deselect: None,
        };
        // Shift-click adds the item under the mouse, or takes a selected
        // one out on release (a drag moves them all instead).
        if modifiers.shift
            && let Some(&clip) = under.first()
        {
            if selected.contains(&clip) {
                press.deselect = Some(clip);
            } else {
                selected.push(clip);
                let clips = selected.clone();
                self.state.update(cx, |s, cx| s.select_clips(clips, cx));
            }
            self.pressed = Some(press);
            if editable {
                let state = self.state.read(cx);
                let (project, request) = (state.bus.project(), self.requested);
                if !self.gizmo.begin_group(
                    gizmo::Handle::Body,
                    p,
                    &screen,
                    project,
                    &selected,
                    playhead,
                    request,
                ) && let Some(&first) = selected.first()
                {
                    self.gizmo.begin(
                        gizmo::Handle::Body,
                        first,
                        p,
                        modifiers,
                        &screen,
                        project,
                        playhead,
                        request,
                    );
                }
            }
            cx.notify();
            return;
        }
        self.pressed = Some(press);
        // The group's box takes its handles, and its body over its items
        // or nothing.
        if self.gizmo.grouped(&selected, request)
            && let Some(handle) = handle
            && (handle != gizmo::Handle::Body || under.first().is_none_or(|c| selected.contains(c)))
        {
            if editable {
                let state = self.state.read(cx);
                self.gizmo.begin_group(
                    handle,
                    p,
                    &screen,
                    state.bus.project(),
                    &selected,
                    playhead,
                    request,
                );
            }
            cx.notify();
            return;
        }
        // The selected box's handles come first; its body only when it is
        // the topmost item there, or with Alt (which cycles on release).
        let body =
            |h: gizmo::Handle| h == gizmo::Handle::Body || (h == gizmo::Handle::Anchor && !alt);
        let own = selected
            .first()
            .is_some_and(|c| under.first() == Some(c) || (alt && under.contains(c)));
        let handle = match (handle, under.first()) {
            (Some(handle), _) if !body(handle) || own => handle,
            (_, Some(&clip)) => {
                selected = vec![clip];
                self.state
                    .update(cx, |s, cx| s.select_clips(vec![clip], cx));
                gizmo::Handle::Body
            }
            (Some(handle), None) => handle,
            (None, None) => {
                if !selected.is_empty() {
                    self.state
                        .update(cx, |s, cx| s.select_clips(Vec::new(), cx));
                }
                return;
            }
        };
        if editable && let Some(&clip) = selected.first() {
            let state = self.state.read(cx);
            self.gizmo.begin(
                handle,
                clip,
                p,
                modifiers,
                &screen,
                state.bus.project(),
                playhead,
                request,
            );
        }
        cx.notify();
    }

    /// A press in mask mode: on the selected picture's masks, else picking
    /// another item (no transform).
    fn mask_press(
        &mut self,
        p: Point<Pixels>,
        modifiers: Modifiers,
        screen: &Screen,
        under: Vec<flikk_core::ClipId>,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.read(cx);
        let (playhead, request) = (state.playhead, self.requested);
        let editable = state.preview.is_none();
        let selected = state.selection.clips.clone();
        let shape = self
            .gizmo
            .active(&selected, request)
            .filter(|s| !s.lettering && editable);
        let commands = shape.and_then(|shape| {
            let subjects = self.subjects(&shape, cx);
            let project = self.state.read(cx).bus.project();
            self.masks
                .press(p, modifiers, screen, project, &shape, playhead, &subjects)
        });
        match commands {
            Some(commands) if !commands.is_empty() => {
                self.state
                    .update(cx, |s, cx| s.edit_batch(commands, "set_masks", cx));
            }
            Some(_) => {}
            None => {
                let pick: Vec<_> = under.first().copied().into_iter().collect();
                if pick.first() != selected.first() {
                    self.masks.cancel();
                    self.masks.selected = None;
                    self.state.update(cx, |s, cx| s.select_clips(pick, cx));
                }
            }
        }
        cx.notify();
    }

    /// Remove the picked mask; whether there was one.
    pub(super) fn remove_mask(&mut self, cx: &mut Context<Self>) -> bool {
        let state = self.state.read(cx);
        if !self.masks.on || state.preview.is_some() {
            return false;
        }
        let Some(shape) = self.gizmo.active(&state.selection.clips, self.requested) else {
            return false;
        };
        let commands = self.masks.remove_selected(state.bus.project(), shape.clip);
        if commands.is_empty() {
            return false;
        }
        self.state
            .update(cx, |s, cx| s.edit_batch(commands, "set_masks", cx));
        true
    }

    /// Drop a gizmo or mask drag once the frame can't be edited (playback
    /// started, or a source opened), committing nothing: its live preview
    /// would restart playback on every move, and its keyframes land at a
    /// time no longer shown. Whether there was one.
    pub(super) fn drop_stale_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let dragging = self.gizmo.dragging() || self.masks.dragging();
        if !dragging || self.editable(self.state.read(cx)) {
            return false;
        }
        self.gizmo.cancel();
        self.masks.cancel_drag();
        self.pressed = None;
        self.state.update(cx, |s, cx| s.show_live(&[], cx));
        true
    }

    pub(super) fn mouse_moved(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if let Some((from, pan)) = self.panning {
            if ev.pressed_button == Some(MouseButton::Middle) {
                self.pan = pan + (ev.position - from);
                cx.notify();
                return;
            }
            // The middle button came up where we didn't see it.
            self.panning = None;
            cx.notify();
        }
        if self.drop_stale_drag(cx) {
            cx.notify();
            return;
        }
        // The left button came up where we didn't see it: end the drag.
        if (self.masks.dragging() || self.gizmo.dragging()) && !ev.dragging() {
            self.release(cx);
            return;
        }
        let screen = self.screen(cx);
        if self.masks.dragging() {
            if ev.dragging() {
                if let Some(commands) = self.masks.drag_to(ev.position, ev.modifiers, &screen) {
                    self.state.update(cx, |s, cx| s.show_live(&commands, cx));
                }
                cx.notify();
            }
            return;
        }
        if self.masks.on {
            if self.gizmo.hover.take().is_some() {
                cx.notify();
            }
            self.hover_subject(ev.position, ev.modifiers, cx);
            return;
        }
        let state = self.state.read(cx);
        let (playhead, request) = (state.playhead, self.requested);
        if self.gizmo.dragging() {
            if !ev.dragging() {
                return;
            }
            let commands =
                self.gizmo
                    .drag_to(ev.position, ev.modifiers, &screen, state.bus.project());
            if let Some(commands) = commands {
                self.state.update(cx, |s, cx| s.show_live(&commands, cx));
            }
            cx.notify();
            return;
        }
        let selected = &state.selection.clips;
        let handle = (self.editable(state) && state.preview.is_none())
            .then(|| {
                self.gizmo.handle_at(
                    ev.position,
                    &screen,
                    state.bus.project(),
                    selected,
                    playhead,
                    request,
                )
            })
            .flatten();
        if handle != self.gizmo.hover {
            self.gizmo.hover = handle;
            cx.notify();
        }
    }

    pub(super) fn mouse_up(&mut self, ev: &MouseUpEvent, cx: &mut Context<Self>) {
        if ev.button == MouseButton::Middle {
            self.panning = None;
            return;
        }
        if ev.button != MouseButton::Left {
            return;
        }
        // A press still waiting for its layout is a click.
        if let Some(click) = self.held_click.as_mut() {
            click.released = true;
            return;
        }
        if self.drop_stale_drag(cx) {
            cx.notify();
            return;
        }
        self.release(cx);
    }

    /// The left button up: end the drag, or act on the click.
    fn release(&mut self, cx: &mut Context<Self>) {
        let pressed = self.pressed.take();
        if self.masks.dragging() {
            let commands = self.masks.end();
            self.state.update(cx, |s, cx| {
                if commands.is_empty() {
                    s.show_live(&[], cx);
                } else {
                    s.edit_batch(commands, "set_masks", cx);
                }
            });
            cx.notify();
            return;
        }
        if !self.gizmo.dragging() {
            return;
        }
        let state = self.state.read(cx);
        let (commands, click) = self.gizmo.end(state.bus.project());
        let selection = state.selection.clips.clone();
        let selected = selection.first().copied();
        let applied = self.state.update(cx, |s, cx| {
            if commands.is_empty() {
                s.show_live(&[], cx);
                false
            } else {
                s.edit_batch(commands, "set_transform", cx).is_some()
            }
        });
        if !applied {
            self.gizmo.cancel();
        }
        // A layout held back by the drag.
        self.lay_out_shown(cx);
        // Shift-click on a selected item: out of the selection.
        if click && let Some(clip) = pressed.as_ref().and_then(|p| p.deselect) {
            let rest: Vec<_> = selection.into_iter().filter(|c| *c != clip).collect();
            self.state.update(cx, |s, cx| s.select_clips(rest, cx));
            cx.notify();
            return;
        }
        // Alt-click: the next item under the mouse.
        if click
            && let Some(Press {
                under, alt: true, ..
            }) = pressed
            && under.len() > 1
        {
            let next = selected
                .and_then(|c| under.iter().position(|u| *u == c))
                .map_or(0, |i| (i + 1) % under.len());
            let clip = under[next];
            self.state
                .update(cx, |s, cx| s.select_clips(vec![clip], cx));
        }
        cx.notify();
    }

    pub(super) fn scrolled(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = ev.delta.pixel_delta(px(20.));
        if ev.modifiers.secondary() {
            let screen = self.screen(cx);
            let current = self
                .zoom
                .unwrap_or_else(|| screen.scale() * self.scale_factor.get());
            // Scrolling up zooms in; a wheel notch is about one step.
            let steps = f32::from(delta.y) / 60.0;
            let zoom = (current * ZOOM_STEP.powf(steps)).clamp(0.05, 8.0);
            // Keep the point under the mouse still.
            let panel = self.frame_rect.get().center() - self.pan;
            let k = zoom / current;
            let offset = ev.position - panel;
            self.pan = point(
                offset.x - (offset.x - self.pan.x) * k,
                offset.y - (offset.y - self.pan.y) * k,
            );
            self.zoom = Some(zoom);
            cx.notify();
        } else if self.zoom.is_some() {
            self.pan += delta;
            cx.notify();
        }
    }

    /// Show the view at `zoom` (`None` fits it), centred.
    pub(super) fn set_zoom(&mut self, zoom: Option<f32>, cx: &mut Context<Self>) {
        self.zoom = zoom;
        self.pan = Point::default();
        cx.notify();
    }

    /// Move the picked items by `delta` sequence pixels. One picked caption
    /// takes its track along, as when dragged.
    pub(super) fn nudge(&mut self, delta: [f32; 2], cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        if state.preview.is_some() {
            return;
        }
        let project = state.bus.project();
        let s = &project.timeline.settings;
        let picked = picked_clips(&state.selection.clips, project, state.playhead);
        let alone = picked.len() > 1;
        let mut moved = Vec::new();
        let mut commands = Vec::new();
        for (track, clip) in picked {
            if track.locked {
                continue;
            }
            let tf = gizmo::nudged(clip, state.playhead, delta, (s.width, s.height));
            let lettering = !alone && compose::is_lettering(track, clip);
            commands.extend(gizmo::commit(
                project,
                clip.id,
                state.playhead,
                &tf,
                lettering,
            ));
            moved.push((clip.id, tf));
        }
        if commands.is_empty() {
            return;
        }
        let applied = self
            .state
            .update(cx, |s, cx| s.edit_batch(commands, "set_transform", cx))
            .is_some();
        if applied {
            for (id, tf) in &moved {
                self.gizmo.hold(*id, tf);
            }
        }
    }

    /// Whether an item is picked on the paused frame: a selected clip shows
    /// a picture or lettering under the playhead.
    pub(super) fn picked(&self, cx: &App) -> bool {
        let state = self.state.read(cx);
        self.editable(state)
            && !picked_clips(&state.selection.clips, state.bus.project(), state.playhead).is_empty()
    }
}
