//! On-screen transform of the selected clip (PRD E5): a box around what it
//! draws, with handles to move, scale, stretch, rotate, skew it and move its
//! anchor, snapping to the frame, safe areas and other items. Everything
//! here works in sequence pixels; the viewer maps them onto the screen.
//!
//! A drag shows live and commits once on release, keyframe-aware: a
//! property with keyframes gets one at the playhead, others change the
//! static transform. Dragging a caption moves every caption on its track
//! the same way; with Alt, only that one.

use super::Request;
use gpui_kit::*;
use koo_core::command::Command;
use koo_core::compose::{self, Shape};
use koo_core::model::{Clip, ClipContent, Project, Property, Track, Transform};
use koo_core::services::Placement;
use koo_core::{ClipId, Time};

/// Side of a square handle, in screen pixels.
const HANDLE: f32 = 8.0;
/// How far a handle reaches for the mouse, in screen pixels.
const REACH: f32 = 7.0;
/// Distance of the rotate handle above the box, in screen pixels.
const ROTATE_OFFSET: f32 = 24.0;
/// How close an edge must come to snap, in screen pixels.
const SNAP: f32 = 6.0;
/// Margins of the action- and title-safe areas, as a fraction of the frame.
pub(super) const SAFE: [f32; 2] = [0.05, 0.1];
/// Mouse travel that turns a click into a drag, in screen pixels.
const CLICK_SLOP: f32 = 3.0;

/// Part of the box under the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Handle {
    Body,
    /// Top-left, top-right, bottom-right, bottom-left.
    Corner(usize),
    /// Top, right, bottom, left.
    Edge(usize),
    Rotate,
    Anchor,
}

/// What a drag changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Move,
    Scale(usize),
    /// Per axis, from a corner.
    Free(usize),
    Stretch(usize),
    Skew(usize),
    Rotate,
    Anchor,
}

/// A snapping line, in sequence pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Guide {
    Vertical(f32),
    Horizontal(f32),
}

struct Drag {
    mode: Mode,
    clip: ClipId,
    /// The clip's layer when the drag began.
    shape: Shape,
    /// The clip's transform at the playhead when the drag began.
    start: Transform,
    /// Where the drag began, in sequence and screen pixels.
    from: [f32; 2],
    from_screen: Point<Pixels>,
    /// A caption moves alone (Alt held).
    alone: bool,
    moved: bool,
    /// Transform at the last mouse move.
    now: Transform,
}

/// The box drawn for a layer, in sequence pixels.
#[derive(Clone, Debug)]
pub(super) struct Outline {
    pub corners: [[f32; 2]; 4],
    pub anchor: [f32; 2],
    /// The selected clip's box, with handles.
    pub active: bool,
}

/// Everything painted over the program, in sequence pixels.
#[derive(Clone, Debug, Default)]
pub(super) struct Overlay {
    pub outlines: Vec<Outline>,
    pub guides: Vec<Guide>,
    pub safe: bool,
    /// Handles can be used.
    pub editable: bool,
}

#[derive(Default)]
pub(super) struct Gizmo {
    /// Layout of a paused frame and what it was made for.
    pub shapes: Option<(Request, Vec<Shape>)>,
    drag: Option<Drag>,
    pub hover: Option<Handle>,
    guides: Vec<Guide>,
    pub safe_areas: bool,
    /// A just-dragged clip's placement, shown until a newer layout arrives.
    held: Option<(ClipId, Placement)>,
}

/// Maps sequence pixels onto the screen.
#[derive(Clone, Copy)]
pub(super) struct Screen {
    pub frame: Bounds<Pixels>,
    pub sequence: (u32, u32),
}

impl Screen {
    /// Screen pixels per sequence pixel.
    pub fn scale(self) -> f32 {
        f32::from(self.frame.size.width) / self.sequence.0.max(1) as f32
    }

    pub fn to_screen(self, p: [f32; 2]) -> Point<Pixels> {
        let k = self.scale();
        point(
            self.frame.origin.x + px(p[0] * k),
            self.frame.origin.y + px(p[1] * k),
        )
    }

    pub fn to_sequence(self, p: Point<Pixels>) -> [f32; 2] {
        let k = self.scale().max(1e-6);
        [
            f32::from(p.x - self.frame.origin.x) / k,
            f32::from(p.y - self.frame.origin.y) / k,
        ]
    }
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn mid(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

/// `m⁻¹ · v` for a 2×2 `[a, b, c, d]`.
fn solve(m: [f32; 4], v: [f32; 2]) -> Option<[f32; 2]> {
    let [a, b, c, d] = m;
    let det = a * d - b * c;
    (det.abs() > 1e-9).then(|| [(d * v[0] - b * v[1]) / det, (a * v[1] - c * v[0]) / det])
}

/// Where `shape`'s anchor sits, in sequence pixels.
fn anchor_point(shape: &Shape, tf: &Transform) -> [f32; 2] {
    shape
        .placement
        .to_canvas([0.5 + tf.anchor[0], 0.5 + tf.anchor[1]])
}

/// The rotate handle's screen position for a box with `corners`.
fn rotate_handle(corners: &[[f32; 2]; 4], screen: &Screen) -> Point<Pixels> {
    let top = screen.to_screen(mid(corners[0], corners[1]));
    let bottom = screen.to_screen(mid(corners[3], corners[2]));
    let (dx, dy) = (f32::from(top.x - bottom.x), f32::from(top.y - bottom.y));
    let len = dx.hypot(dy).max(1e-3);
    point(
        top.x + px(dx / len * ROTATE_OFFSET),
        top.y + px(dy / len * ROTATE_OFFSET),
    )
}

/// Screen positions of the handles of a box with `corners`.
fn handles(
    corners: &[[f32; 2]; 4],
    anchor: [f32; 2],
    screen: &Screen,
) -> Vec<(Handle, Point<Pixels>)> {
    let mut out = vec![
        (Handle::Anchor, screen.to_screen(anchor)),
        (Handle::Rotate, rotate_handle(corners, screen)),
    ];
    for i in 0..4 {
        out.push((Handle::Corner(i), screen.to_screen(corners[i])));
        out.push((
            Handle::Edge(i),
            screen.to_screen(mid(corners[i], corners[(i + 1) % 4])),
        ));
    }
    out
}

/// Whether `p` is inside the quadrilateral `q` (screen pixels); never
/// inside a flat one.
fn inside(q: &[Point<Pixels>; 4], p: Point<Pixels>) -> bool {
    let mut sign = 0.0f32;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        let cross = f32::from(b.x - a.x) * f32::from(p.y - a.y)
            - f32::from(b.y - a.y) * f32::from(p.x - a.x);
        if cross.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    sign != 0.0
}

fn near(a: Point<Pixels>, b: Point<Pixels>, reach: f32) -> bool {
    f32::from(a.x - b.x).hypot(f32::from(a.y - b.y)) <= reach
}

impl Gizmo {
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Shapes of the frame `request` shows, if laid out.
    fn shapes_for(&self, request: Option<Request>) -> Option<&[Shape]> {
        self.shapes
            .as_ref()
            .filter(|(r, _)| Some(*r) == request)
            .map(|(_, s)| s.as_slice())
    }

    /// A newer layout arrived.
    pub fn set_shapes(&mut self, request: Request, shapes: Vec<Shape>) {
        self.shapes = Some((request, shapes));
        if self.drag.is_none() {
            self.held = None;
        }
    }

    /// The selected clip's layer, with its placement as last dragged.
    fn active(&self, selected: &[ClipId], request: Option<Request>) -> Option<Shape> {
        let shapes = self.shapes.as_ref().map(|(_, s)| s.as_slice())?;
        // While a drag runs or until its layout arrives, older shapes do.
        let fresh = self.shapes_for(request).is_some();
        if !fresh && self.drag.is_none() && self.held.is_none() {
            return None;
        }
        let first = selected.first()?;
        let mut shape = shapes.iter().find(|s| s.clip == *first)?.clone();
        if let Some(d) = self.drag.as_ref().filter(|d| d.clip == shape.clip) {
            shape.placement = placed(&shape, &d.now);
        } else if let Some((_, place)) = self.held.filter(|(c, _)| *c == shape.clip) {
            shape.placement = place;
        }
        Some(shape)
    }

    /// The selected clip's current transform at the playhead.
    fn active_transform(
        &self,
        project: &Project,
        clip: ClipId,
        playhead: Time,
    ) -> Option<Transform> {
        if let Some(d) = self.drag.as_ref().filter(|d| d.clip == clip) {
            return Some(d.now.clone());
        }
        let (_, c) = project.timeline.clip(clip)?;
        Some(c.transform_at(clip_time(c, playhead)))
    }

    /// What to paint over the program.
    pub fn overlay(
        &self,
        project: &Project,
        selected: &[ClipId],
        playhead: Time,
        request: Option<Request>,
        editable: bool,
    ) -> Overlay {
        let mut outlines = Vec::new();
        if let Some(shape) = self.active(selected, request)
            && let Some(tf) = self.active_transform(project, shape.clip, playhead)
        {
            outlines.push(Outline {
                corners: shape.corners(),
                anchor: anchor_point(&shape, &tf),
                active: true,
            });
        }
        if let Some(shapes) = self.shapes_for(request) {
            for s in shapes
                .iter()
                .filter(|s| selected.iter().skip(1).any(|c| *c == s.clip))
            {
                outlines.push(Outline {
                    corners: s.corners(),
                    anchor: s.placement.center,
                    active: false,
                });
            }
        }
        Overlay {
            outlines,
            guides: self.guides.clone(),
            safe: self.safe_areas,
            editable,
        }
    }

    /// The handle of the selected clip's box under `p`.
    pub fn handle_at(
        &self,
        p: Point<Pixels>,
        screen: &Screen,
        project: &Project,
        selected: &[ClipId],
        playhead: Time,
        request: Option<Request>,
    ) -> Option<Handle> {
        let shape = self.active(selected, request)?;
        let tf = self.active_transform(project, shape.clip, playhead)?;
        let corners = shape.corners();
        for (handle, at) in handles(&corners, anchor_point(&shape, &tf), screen) {
            if near(at, p, REACH) {
                return Some(handle);
            }
        }
        let quad = corners.map(|c| screen.to_screen(c));
        inside(&quad, p).then_some(Handle::Body)
    }

    /// Layers under `p`, topmost first.
    pub fn clips_at(
        &self,
        p: Point<Pixels>,
        screen: &Screen,
        request: Option<Request>,
    ) -> Vec<ClipId> {
        let at = screen.to_sequence(p);
        self.shapes_for(request)
            .map(|shapes| {
                shapes
                    .iter()
                    .filter(|s| s.contains(at))
                    .map(|s| s.clip)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Start dragging `handle` of `clip`'s box from `p`.
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        &mut self,
        handle: Handle,
        clip: ClipId,
        p: Point<Pixels>,
        modifiers: Modifiers,
        screen: &Screen,
        project: &Project,
        playhead: Time,
        request: Option<Request>,
    ) -> bool {
        let Some(shape) = self.active(&[clip], request) else {
            return false;
        };
        let Some((track, c)) = project.timeline.clip(clip) else {
            return false;
        };
        if track.locked {
            return false;
        }
        let mode = match handle {
            Handle::Body => Mode::Move,
            Handle::Corner(i) if modifiers.shift => Mode::Free(i),
            Handle::Corner(i) => Mode::Scale(i),
            Handle::Edge(i) if modifiers.secondary() => Mode::Skew(i),
            Handle::Edge(i) => Mode::Stretch(i),
            Handle::Rotate => Mode::Rotate,
            Handle::Anchor if modifiers.alt => Mode::Anchor,
            // Without Alt the anchor is part of the body.
            Handle::Anchor => Mode::Move,
        };
        let start = c.transform_at(clip_time(c, playhead));
        self.held = None;
        self.drag = Some(Drag {
            mode,
            clip,
            shape,
            now: start.clone(),
            start,
            from: screen.to_sequence(p),
            from_screen: p,
            alone: modifiers.alt,
            moved: false,
        });
        true
    }

    /// Follow the mouse; the commands that show the drag live, if it moved.
    pub fn drag_to(
        &mut self,
        p: Point<Pixels>,
        modifiers: Modifiers,
        screen: &Screen,
        project: &Project,
        playhead: Time,
    ) -> Option<Vec<Command>> {
        let others: Vec<[f32; 4]> = self
            .shapes
            .as_ref()
            .map(|(_, s)| s.as_slice())
            .unwrap_or_default()
            .iter()
            .filter(|s| self.drag.as_ref().is_none_or(|d| d.clip != s.clip))
            .map(Shape::bounds)
            .collect();
        let safe = self.safe_areas;
        let d = self.drag.as_mut()?;
        if !d.moved && !near(p, d.from_screen, CLICK_SLOP) {
            d.moved = true;
        }
        if !d.moved {
            return None;
        }
        let (tf, guides) = apply(d, screen.to_sequence(p), modifiers, screen, &others, safe);
        d.now = tf;
        self.guides = guides;
        let (_, clip) = project.timeline.clip(d.clip)?;
        Some(transform_commands(clip, playhead, &d.now))
    }

    /// End the drag: the commands to commit (none for a click), and whether
    /// it was a click.
    pub fn end(&mut self, project: &Project, playhead: Time) -> (Vec<Command>, bool) {
        self.guides.clear();
        let Some(d) = self.drag.take() else {
            return (Vec::new(), false);
        };
        if !d.moved {
            return (Vec::new(), true);
        }
        self.held = Some((d.clip, placed(&d.shape, &d.now)));
        let caption = d.shape.lettering && !d.alone;
        (commit(project, d.clip, playhead, &d.now, caption), false)
    }

    /// Show `clip` with transform `tf` until a newer layout arrives.
    pub fn hold(&mut self, clip: ClipId, tf: &Transform) {
        let shape = self
            .shapes
            .as_ref()
            .and_then(|(_, shapes)| shapes.iter().find(|s| s.clip == clip));
        if let Some(shape) = shape {
            self.held = Some((clip, placed(shape, tf)));
        }
    }

    /// Drop the drag without committing it, and any held placement.
    pub fn cancel(&mut self) {
        self.drag = None;
        self.held = None;
        self.guides.clear();
    }
}

/// Where `shape` lands with transform `tf` instead of its own.
fn placed(shape: &Shape, tf: &Transform) -> Placement {
    let s = &shape.placement;
    let tf = if shape.lettering {
        compose::lettering_transform(tf.clone())
    } else {
        Transform {
            crop: s.crop,
            flip: s.flip,
            blend: s.blend,
            ..tf.clone()
        }
    };
    compose::placement_from(&tf, shape.rest_center, shape.rest_size, shape.canvas)
}

/// The transform `d` makes with the mouse at `m` (sequence pixels), and
/// the guides it snapped to.
fn apply(
    d: &Drag,
    m: [f32; 2],
    modifiers: Modifiers,
    screen: &Screen,
    others: &[[f32; 4]],
    safe: bool,
) -> (Transform, Vec<Guide>) {
    let shape = &d.shape;
    let start = &d.start;
    let place = &shape.placement;
    let (cw, ch) = (
        screen.sequence.0.max(1) as f32,
        screen.sequence.1.max(1) as f32,
    );
    let a = anchor_point(shape, start);
    let mut tf = start.clone();
    let mut guides = Vec::new();
    let corners = shape.corners();
    match d.mode {
        Mode::Move => {
            let mut delta = sub(m, d.from);
            if modifiers.shift {
                if delta[0].abs() > delta[1].abs() {
                    delta[1] = 0.0;
                } else {
                    delta[0] = 0.0;
                }
            }
            if !modifiers.secondary() {
                let reach = SNAP / screen.scale().max(1e-6);
                let [x0, y0, x1, y1] = shape.bounds();
                let mut xs = vec![0.0, cw / 2.0, cw];
                let mut ys = vec![0.0, ch / 2.0, ch];
                if safe {
                    for f in SAFE {
                        xs.extend([cw * f, cw * (1.0 - f)]);
                        ys.extend([ch * f, ch * (1.0 - f)]);
                    }
                }
                for b in others {
                    xs.extend([b[0], (b[0] + b[2]) / 2.0, b[2]]);
                    ys.extend([b[1], (b[1] + b[3]) / 2.0, b[3]]);
                }
                let snap = |edges: [f32; 3], targets: &[f32]| -> Option<(f32, f32)> {
                    let mut best: Option<(f32, f32)> = None;
                    for e in edges {
                        for t in targets {
                            let gap = t - e;
                            if gap.abs() <= reach && best.is_none_or(|(g, _)| gap.abs() < g.abs()) {
                                best = Some((gap, *t));
                            }
                        }
                    }
                    best
                };
                let moved_x = [x0 + delta[0], (x0 + x1) / 2.0 + delta[0], x1 + delta[0]];
                let moved_y = [y0 + delta[1], (y0 + y1) / 2.0 + delta[1], y1 + delta[1]];
                if !(modifiers.shift && delta[0] == 0.0)
                    && let Some((gap, at)) = snap(moved_x, &xs)
                {
                    delta[0] += gap;
                    guides.push(Guide::Vertical(at));
                }
                if !(modifiers.shift && delta[1] == 0.0)
                    && let Some((gap, at)) = snap(moved_y, &ys)
                {
                    delta[1] += gap;
                    guides.push(Guide::Horizontal(at));
                }
            }
            tf.position = [
                start.position[0] + delta[0] / cw,
                start.position[1] + delta[1] / ch,
            ];
        }
        Mode::Scale(i) => {
            let c0 = sub(corners[i], a);
            let len = dot(c0, c0);
            if len > 1e-6 {
                let f = dot(sub(m, a), c0) / len;
                tf.scale = (start.scale * f).max(0.01);
            }
        }
        Mode::Free(i) => {
            let l = place.linear();
            if let (Some(lm), Some(lc)) = (solve(l, sub(m, a)), solve(l, sub(corners[i], a))) {
                for k in 0..2 {
                    if lc[k].abs() > 1e-3 {
                        tf.stretch[k] = (start.stretch[k] * lm[k] / lc[k]).max(0.01);
                    }
                }
            }
        }
        Mode::Stretch(i) => {
            let l = place.linear();
            let e = mid(corners[i], corners[(i + 1) % 4]);
            // Top and bottom edges stretch y; left and right, x.
            let k = if i % 2 == 0 { 1 } else { 0 };
            if let (Some(lm), Some(le)) = (solve(l, sub(m, a)), solve(l, sub(e, a)))
                && le[k].abs() > 1e-3
            {
                tf.stretch[k] = (start.stretch[k] * lm[k] / le[k]).max(0.01);
            }
        }
        Mode::Skew(i) => {
            // Measured in the box's turned (unskewed) frame.
            let (s, c) = place.rotation_deg.to_radians().sin_cos();
            let dm = sub(m, d.from);
            let local = [c * dm[0] + s * dm[1], -s * dm[0] + c * dm[1]];
            let [u0, v0, u1, v1] = shape.content;
            let size = place.size;
            let max = compose::MAX_SKEW;
            if i % 2 == 0 {
                // Top or bottom edge: lean sideways.
                let v = if i == 0 { v0 } else { v1 };
                let y = (v - 0.5 - start.anchor[1]) * size[1];
                if y.abs() > 1e-3 {
                    let t = start.skew_deg[0].to_radians().tan() + local[0] / y;
                    tf.skew_deg[0] = t.atan().to_degrees().clamp(-max, max);
                }
            } else {
                let u = if i == 1 { u1 } else { u0 };
                let x = (u - 0.5 - start.anchor[0]) * size[0];
                if x.abs() > 1e-3 {
                    let t = start.skew_deg[1].to_radians().tan() + local[1] / x;
                    tf.skew_deg[1] = t.atan().to_degrees().clamp(-max, max);
                }
            }
        }
        Mode::Rotate => {
            let angle = |p: [f32; 2]| (p[1] - a[1]).atan2(p[0] - a[0]).to_degrees();
            let mut turn = start.rotation_deg + angle(m) - angle(d.from);
            if modifiers.shift {
                turn = (turn / 15.0).round() * 15.0;
            }
            tf.rotation_deg = turn;
        }
        Mode::Anchor => {
            if let Some(uv) = place.to_picture(m) {
                let mut anchor = [uv[0] - 0.5, uv[1] - 0.5];
                // Snap to the corners, edge midpoints and centre of the box.
                let [u0, v0, u1, v1] = shape.content;
                let reach = SNAP / screen.scale().max(1e-6);
                for (u, v) in [u0, (u0 + u1) / 2.0, u1]
                    .into_iter()
                    .flat_map(|u| [v0, (v0 + v1) / 2.0, v1].map(|v| (u, v)))
                {
                    let p = place.to_canvas([u, v]);
                    if sub(p, m)[0].hypot(sub(p, m)[1]) <= reach {
                        anchor = [u - 0.5, v - 0.5];
                    }
                }
                tf.anchor = anchor;
                // Keep the picture where it is (see `placement_from`).
                let da = sub(anchor, start.anchor);
                let rest = shape.rest_size;
                let drawn = place.size;
                let [la, lb, lc, ld] = place.linear();
                let turned = [da[0] * drawn[0], da[1] * drawn[1]];
                let shift = [
                    da[0] * rest[0] - (la * turned[0] + lb * turned[1]),
                    da[1] * rest[1] - (lc * turned[0] + ld * turned[1]),
                ];
                tf.position = [
                    start.position[0] - shift[0] / cw,
                    start.position[1] - shift[1] / ch,
                ];
            }
        }
    }
    (tf, guides)
}

/// Playhead relative to `clip`'s start, clamped inside it.
pub(super) fn clip_time(clip: &Clip, playhead: Time) -> Time {
    (playhead - clip.start).clamp(Time::ZERO, Time((clip.duration.0 - 1).max(0)))
}

/// The commands that give `clip` transform `tf` at the playhead (see
/// [`Command::transform_at`]).
pub(super) fn transform_commands(clip: &Clip, playhead: Time, tf: &Transform) -> Vec<Command> {
    Command::transform_at(clip, clip_time(clip, playhead), tf)
}

/// The other captions on `track` moved as `clip` was from `before` to
/// `after`: offsets added, scales multiplied, keyframes included.
fn follow(track: &Track, clip: &Clip, before: &Transform, after: &Transform) -> Vec<Command> {
    let changes: Vec<(Property, f32, f32)> = Property::GEOMETRY
        .into_iter()
        .map(|p| (p, before.get(p), after.get(p)))
        .filter(|(_, a, b)| (a - b).abs() > 1e-6)
        .collect();
    let shift = |p: Property, from: f32, to: f32, v: f32| match p {
        Property::Scale | Property::ScaleX | Property::ScaleY if from.abs() > 1e-6 => v * to / from,
        _ => v + to - from,
    };
    let mut out = Vec::new();
    for other in track
        .clips
        .iter()
        .filter(|c| c.id != clip.id && matches!(c.content, ClipContent::Caption { .. }))
    {
        let mut fixed = other.transform.clone();
        for &(p, from, to) in &changes {
            fixed.set(p, shift(p, from, to, fixed.get(p)));
            for k in other.keyframes.iter().filter(|k| k.property == p) {
                out.push(Command::SetKeyframe {
                    clip: other.id,
                    property: p,
                    at: other.start + k.at,
                    value: shift(p, from, to, k.value),
                    ease: k.ease,
                });
            }
        }
        if fixed != other.transform {
            out.push(Command::SetTransform {
                clip: other.id,
                transform: fixed,
            });
        }
    }
    out
}

/// The commands that give `clip` transform `after` at the playhead; with
/// `track_wide`, a caption's track-mates follow it.
pub(super) fn commit(
    project: &Project,
    clip: ClipId,
    playhead: Time,
    after: &Transform,
    track_wide: bool,
) -> Vec<Command> {
    let Some((track, c)) = project.timeline.clip(clip) else {
        return Vec::new();
    };
    let mut commands = transform_commands(c, playhead, after);
    if track_wide && matches!(c.content, ClipContent::Caption { .. }) {
        let before = c.transform_at(clip_time(c, playhead));
        commands.extend(follow(track, c, &before, after));
    }
    commands
}

/// `clip`'s transform at the playhead moved by `delta` sequence pixels on
/// a `canvas`.
pub(super) fn nudged(
    clip: &Clip,
    playhead: Time,
    delta: [f32; 2],
    canvas: (u32, u32),
) -> Transform {
    let mut tf = clip.transform_at(clip_time(clip, playhead));
    tf.position[0] += delta[0] / canvas.0.max(1) as f32;
    tf.position[1] += delta[1] / canvas.1.max(1) as f32;
    tf
}

/// Cursor for `handle`.
pub(super) fn cursor(handle: Option<Handle>, dragging: bool) -> CursorStyle {
    match handle {
        Some(Handle::Body) if dragging => CursorStyle::ClosedHand,
        Some(Handle::Body) => CursorStyle::OpenHand,
        Some(Handle::Corner(0 | 2)) => CursorStyle::ResizeUpLeftDownRight,
        Some(Handle::Corner(_)) => CursorStyle::ResizeUpRightDownLeft,
        Some(Handle::Edge(0 | 2)) => CursorStyle::ResizeUpDown,
        Some(Handle::Edge(_)) => CursorStyle::ResizeLeftRight,
        Some(Handle::Rotate | Handle::Anchor) => CursorStyle::Crosshair,
        None => CursorStyle::Arrow,
    }
}

/// Paint `overlay` over the program at `screen`.
pub(super) fn paint(overlay: &Overlay, screen: &Screen, colors: [Hsla; 3], window: &mut Window) {
    let [line, guide, safe] = colors;
    let stroke =
        |pts: &[Point<Pixels>], close: bool, width: f32, color: Hsla, window: &mut Window| {
            let mut b = PathBuilder::stroke(px(width));
            for (i, p) in pts.iter().enumerate() {
                if i == 0 {
                    b.move_to(*p);
                } else {
                    b.line_to(*p);
                }
            }
            if close {
                b.close();
            }
            if let Ok(path) = b.build() {
                window.paint_path(path, color);
            }
        };
    let (w, h) = (screen.sequence.0 as f32, screen.sequence.1 as f32);
    if overlay.safe {
        for f in SAFE {
            let q = [
                [w * f, h * f],
                [w * (1.0 - f), h * f],
                [w * (1.0 - f), h * (1.0 - f)],
                [w * f, h * (1.0 - f)],
            ]
            .map(|p| screen.to_screen(p));
            stroke(&q, true, 1.0, safe, window);
        }
    }
    for g in &overlay.guides {
        let (a, b) = match *g {
            Guide::Vertical(x) => ([x, 0.0], [x, h]),
            Guide::Horizontal(y) => ([0.0, y], [w, y]),
        };
        stroke(
            &[screen.to_screen(a), screen.to_screen(b)],
            false,
            1.0,
            guide,
            window,
        );
    }
    for o in &overlay.outlines {
        let q = o.corners.map(|c| screen.to_screen(c));
        let color = if o.active { line } else { line.opacity(0.6) };
        stroke(&q, true, if o.active { 1.5 } else { 1.0 }, color, window);
        if !o.active || !overlay.editable {
            continue;
        }
        let top = screen.to_screen(mid(o.corners[0], o.corners[1]));
        let knob = rotate_handle(&o.corners, screen);
        stroke(&[top, knob], false, 1.0, line, window);
        for (handle, at) in handles(&o.corners, o.anchor, screen) {
            let half = px(HANDLE / 2.0);
            match handle {
                Handle::Anchor => {
                    let r = px(HANDLE * 0.75);
                    stroke(
                        &[point(at.x - r, at.y), point(at.x + r, at.y)],
                        false,
                        1.0,
                        line,
                        window,
                    );
                    stroke(
                        &[point(at.x, at.y - r), point(at.x, at.y + r)],
                        false,
                        1.0,
                        line,
                        window,
                    );
                }
                Handle::Rotate => {
                    let r = half;
                    let mut b = PathBuilder::fill();
                    b.move_to(point(at.x + r, at.y));
                    b.arc_to(point(r, r), px(0.), false, true, point(at.x - r, at.y));
                    b.arc_to(point(r, r), px(0.), false, true, point(at.x + r, at.y));
                    b.close();
                    if let Ok(path) = b.build() {
                        window.paint_path(path, line);
                    }
                }
                _ => {
                    let square =
                        Bounds::new(point(at.x - half, at.y - half), size(half * 2., half * 2.));
                    window.paint_quad(
                        fill(square, white())
                            .border_widths(px(1.))
                            .border_color(line),
                    );
                }
            }
        }
    }
}
