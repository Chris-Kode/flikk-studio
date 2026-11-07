//! On-screen transform of the selected clip (PRD E5): a box around what it
//! draws, with handles to move, scale, stretch, rotate, skew it and move its
//! anchor, snapping to the frame, safe areas and other items. Everything
//! here works in sequence pixels; the viewer maps them onto the screen.
//!
//! A drag shows live and commits once on release, keyframe-aware: a
//! property with keyframes gets one at the playhead, others change the
//! static transform. Dragging a caption moves every caption on its track
//! the same way; with Alt, only that one.
//!
//! With several items selected, one box goes around what they draw: its
//! body moves them, its corners scale and its knob turns them together
//! about the box's centre ([`GroupMove`]), each clip's own transform
//! changed to match, as one undo step. Group members move alone, captions
//! included.
//!
//! In corner-pin mode the selected picture's corners move one by one,
//! drawing it in perspective; captions and titles have no corner pin (see
//! `compose::lettering_transform`).

use super::Request;
use crate::ui::kit::theme as t;
use flikk_core::command::Command;
use flikk_core::compose::{self, GroupMove, Shape};
use flikk_core::model::{Clip, ClipContent, Project, Property, Track, Transform};
use flikk_core::services::Placement;
use flikk_core::{ClipId, Time};
use gpui_kit::*;

/// Side of a square handle, in screen pixels.
const HANDLE: f32 = 6.0;
/// Radius of the rotate knob and the corner-pin handles.
const KNOB: f32 = 3.5;
/// Radius of the anchor's ring.
const ANCHOR: f32 = 4.5;
/// How far a handle reaches for the mouse, in screen pixels.
pub(super) const REACH: f32 = 7.0;
/// Distance of the rotate handle above the box, in screen pixels.
const ROTATE_OFFSET: f32 = 24.0;
/// How close an edge must come to snap, in screen pixels.
const SNAP: f32 = 6.0;
/// Margins of the action- and title-safe areas, as a fraction of the frame.
pub(super) const SAFE: [f32; 2] = [0.05, 0.1];
/// Mouse travel that turns a click into a drag, in screen pixels.
pub(super) const CLICK_SLOP: f32 = 3.0;

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
    /// One corner, in corner-pin mode.
    Pin(usize),
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
    /// The playhead when the drag began, where its keyframes go.
    at: Time,
    /// Where the drag began, in sequence and screen pixels.
    from: [f32; 2],
    from_screen: Point<Pixels>,
    /// A caption moves alone (Alt held).
    alone: bool,
    moved: bool,
    /// Transform at the last mouse move.
    now: Transform,
}

/// A selected item moving with the group.
struct Member {
    clip: ClipId,
    /// Its layer when the drag began.
    shape: Shape,
    /// Its transform at the playhead when the drag began.
    start: Transform,
}

/// A drag of the group box.
struct GroupDrag {
    mode: Mode,
    members: Vec<Member>,
    /// The box when the drag began.
    corners: [[f32; 2]; 4],
    /// The playhead when the drag began, where its keyframes go.
    at: Time,
    from: [f32; 2],
    from_screen: Point<Pixels>,
    moved: bool,
    /// The move at the last mouse move.
    now: GroupMove,
}

/// The box drawn for a layer, in sequence pixels.
#[derive(Clone, Debug)]
pub(super) struct Outline {
    pub corners: [[f32; 2]; 4],
    pub anchor: [f32; 2],
    /// The selected clip's box, with handles.
    pub active: bool,
    pub kind: OutlineKind,
}

/// Which handles an active box has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OutlineKind {
    Item,
    /// Around several items: corners and the rotate knob.
    Group,
    /// A picture's pinned corners.
    Pin,
}

/// Everything painted over the program, in sequence pixels.
#[derive(Clone, Debug, Default)]
pub(super) struct Overlay {
    pub outlines: Vec<Outline>,
    pub guides: Vec<Guide>,
    pub safe: bool,
    /// Handles can be used.
    pub editable: bool,
    /// The handle being dragged, drawn filled.
    pub grabbed: Option<Handle>,
}

#[derive(Default)]
pub(super) struct Gizmo {
    /// Layout of a paused frame and what it was made for.
    pub shapes: Option<(Request, Vec<Shape>)>,
    /// The number of the layout in `shapes`.
    epoch: u64,
    drag: Option<Drag>,
    group_drag: Option<GroupDrag>,
    pub hover: Option<Handle>,
    guides: Vec<Guide>,
    pub safe_areas: bool,
    /// Corners move one by one (corner-pin mode).
    pub corner_pin: bool,
    /// Just-dragged clips' placements, shown until a newer layout arrives.
    held: Vec<(ClipId, Placement)>,
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

/// Where `shape`'s anchor sits, in sequence pixels (measured without the
/// corner pin, as the transform places it).
fn anchor_point(shape: &Shape, tf: &Transform) -> [f32; 2] {
    shape
        .placement
        .unpinned_to_canvas([0.5 + tf.anchor[0], 0.5 + tf.anchor[1]])
}

/// The whole picture's corners in uv: top-left, top-right, bottom-right,
/// bottom-left.
const UV_CORNERS: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

/// Where the whole picture's corners land, pin included.
fn pin_corners(shape: &Shape) -> [[f32; 2]; 4] {
    UV_CORNERS.map(|uv| shape.placement.to_canvas(uv))
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

/// Screen positions of the group box's handles: corners and the knob.
fn group_handles(corners: &[[f32; 2]; 4], screen: &Screen) -> Vec<(Handle, Point<Pixels>)> {
    let mut out = vec![(Handle::Rotate, rotate_handle(corners, screen))];
    for (i, c) in corners.iter().enumerate() {
        out.push((Handle::Corner(i), screen.to_screen(*c)));
    }
    out
}

/// Screen positions of a pinned picture's corner handles.
fn pin_handles(corners: &[[f32; 2]; 4], screen: &Screen) -> Vec<(Handle, Point<Pixels>)> {
    (0..4)
        .map(|i| (Handle::Corner(i), screen.to_screen(corners[i])))
        .collect()
}

/// The corners of the box `[x0, y0, x1, y1]`.
fn box_corners([x0, y0, x1, y1]: [f32; 4]) -> [[f32; 2]; 4] {
    [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// The centre of a box with `corners`.
fn box_center(corners: &[[f32; 2]; 4]) -> [f32; 2] {
    mid(corners[0], corners[2])
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

pub(super) fn near(a: Point<Pixels>, b: Point<Pixels>, reach: f32) -> bool {
    f32::from(a.x - b.x).hypot(f32::from(a.y - b.y)) <= reach
}

impl Gizmo {
    pub fn dragging(&self) -> bool {
        self.drag.is_some() || self.group_drag.is_some()
    }

    /// Shapes of the frame `request` shows, if laid out.
    fn shapes_for(&self, request: Option<Request>) -> Option<&[Shape]> {
        self.shapes
            .as_ref()
            .filter(|(r, _)| Some(*r) == request)
            .map(|(_, s)| s.as_slice())
    }

    /// Layout number `epoch` arrived: it replaces an older one only.
    pub fn set_shapes(&mut self, request: Request, epoch: u64, shapes: Vec<Shape>) {
        if epoch <= self.epoch {
            return;
        }
        self.epoch = epoch;
        self.shapes = Some((request, shapes));
        if !self.dragging() {
            self.held.clear();
        }
    }

    /// The selected clips' layers, in selection order, with their
    /// placements as last dragged.
    fn selected_shapes(&self, selected: &[ClipId], request: Option<Request>) -> Vec<Shape> {
        let Some((_, shapes)) = self.shapes.as_ref() else {
            return Vec::new();
        };
        // While a drag runs or until its layout arrives, older shapes do.
        let fresh = self.shapes_for(request).is_some();
        if !fresh && !self.dragging() && self.held.is_empty() {
            return Vec::new();
        }
        selected
            .iter()
            .filter_map(|id| shapes.iter().find(|s| s.clip == *id))
            .map(|shape| {
                let mut shape = shape.clone();
                if let Some(d) = self.drag.as_ref().filter(|d| d.clip == shape.clip) {
                    shape.placement = shape.placed(&d.now);
                } else if let Some(g) = &self.group_drag
                    && let Some(m) = g.members.iter().find(|m| m.clip == shape.clip)
                {
                    shape.placement = m.shape.placed(&g.now.transform(&m.shape, &m.start));
                } else if let Some((_, place)) = self.held.iter().find(|(c, _)| *c == shape.clip) {
                    shape.placement = *place;
                }
                shape
            })
            .collect()
    }

    /// The selected clip's layer, with its placement as last dragged: the
    /// first selected clip laid out on this frame.
    pub fn active(&self, selected: &[ClipId], request: Option<Request>) -> Option<Shape> {
        self.selected_shapes(selected, request).into_iter().next()
    }

    /// The selected layers when there are several, and the box around them.
    fn group(
        &self,
        selected: &[ClipId],
        request: Option<Request>,
    ) -> Option<(Vec<Shape>, [[f32; 2]; 4])> {
        let shapes = self.selected_shapes(selected, request);
        if shapes.len() < 2 {
            return None;
        }
        let corners = match &self.group_drag {
            Some(g) => g.corners.map(|c| g.now.apply(c)),
            None => box_corners(compose::group_bounds(&shapes)?),
        };
        Some((shapes, corners))
    }

    /// Whether `shape`'s corners move one by one: corner-pin mode, on a
    /// picture.
    fn pinning(&self, shape: &Shape) -> bool {
        self.corner_pin && !shape.lettering
    }

    /// Whether several selected items share one box.
    pub fn grouped(&self, selected: &[ClipId], request: Option<Request>) -> bool {
        self.selected_shapes(selected, request).len() >= 2
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
        if let Some((shapes, corners)) = self.group(selected, request) {
            outlines.extend(shapes.iter().map(|s| Outline {
                corners: s.corners(),
                anchor: s.placement.center,
                active: false,
                kind: OutlineKind::Item,
            }));
            outlines.push(Outline {
                corners,
                anchor: box_center(&corners),
                active: true,
                kind: OutlineKind::Group,
            });
            return Overlay {
                outlines,
                guides: self.guides.clone(),
                safe: self.safe_areas,
                editable,
                grabbed: self.grabbed(),
            };
        }
        if let Some(shape) = self.active(selected, request)
            && let Some(tf) = self.active_transform(project, shape.clip, playhead)
        {
            let pin = self.pinning(&shape);
            outlines.push(Outline {
                corners: if pin {
                    pin_corners(&shape)
                } else {
                    shape.corners()
                },
                anchor: anchor_point(&shape, &tf),
                active: true,
                kind: if pin {
                    OutlineKind::Pin
                } else {
                    OutlineKind::Item
                },
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
                    kind: OutlineKind::Item,
                });
            }
        }
        Overlay {
            outlines,
            guides: self.guides.clone(),
            safe: self.safe_areas,
            editable,
            grabbed: self.grabbed(),
        }
    }

    /// The handle a drag holds, when it is drawn differently.
    fn grabbed(&self) -> Option<Handle> {
        let mode = self.drag.as_ref().map(|d| d.mode);
        match mode.or(self.group_drag.as_ref().map(|g| g.mode))? {
            Mode::Rotate => Some(Handle::Rotate),
            Mode::Anchor => Some(Handle::Anchor),
            Mode::Pin(i) => Some(Handle::Corner(i)),
            _ => None,
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
        if let Some((_, corners)) = self.group(selected, request) {
            for (handle, at) in group_handles(&corners, screen) {
                if near(at, p, REACH) {
                    return Some(handle);
                }
            }
            let quad = corners.map(|c| screen.to_screen(c));
            return inside(&quad, p).then_some(Handle::Body);
        }
        let shape = self.active(selected, request)?;
        if self.pinning(&shape) {
            let corners = pin_corners(&shape);
            for (handle, at) in pin_handles(&corners, screen) {
                if near(at, p, REACH) {
                    return Some(handle);
                }
            }
            let quad = corners.map(|c| screen.to_screen(c));
            return inside(&quad, p).then_some(Handle::Body);
        }
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
            Handle::Corner(i) if self.pinning(&shape) => Mode::Pin(i),
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
        self.held.clear();
        // One drag at a time, even if the last one's release was lost.
        self.group_drag = None;
        self.drag = Some(Drag {
            mode,
            clip,
            shape,
            now: start.clone(),
            start,
            at: playhead,
            from: screen.to_sequence(p),
            from_screen: p,
            alone: modifiers.alt,
            moved: false,
        });
        true
    }

    /// Start dragging `handle` of the box around the `selected` items from
    /// `p`. Items on locked tracks stay put.
    #[allow(clippy::too_many_arguments)]
    pub fn begin_group(
        &mut self,
        handle: Handle,
        p: Point<Pixels>,
        screen: &Screen,
        project: &Project,
        selected: &[ClipId],
        playhead: Time,
        request: Option<Request>,
    ) -> bool {
        let Some((shapes, corners)) = self.group(selected, request) else {
            return false;
        };
        let mode = match handle {
            Handle::Corner(i) => Mode::Scale(i),
            Handle::Rotate => Mode::Rotate,
            _ => Mode::Move,
        };
        let members: Vec<Member> = shapes
            .into_iter()
            .filter_map(|shape| {
                let (track, c) = project.timeline.clip(shape.clip)?;
                (!track.locked).then(|| Member {
                    clip: shape.clip,
                    start: c.transform_at(clip_time(c, playhead)),
                    shape,
                })
            })
            .collect();
        if members.is_empty() {
            return false;
        }
        self.held.clear();
        self.drag = None;
        self.group_drag = Some(GroupDrag {
            mode,
            members,
            now: GroupMove::about(box_center(&corners)),
            corners,
            at: playhead,
            from: screen.to_sequence(p),
            from_screen: p,
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
    ) -> Option<Vec<Command>> {
        if self.group_drag.is_some() {
            return self.group_drag_to(p, modifiers, screen, project);
        }
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
        Some(transform_commands(clip, d.at, &d.now))
    }

    /// [`Self::drag_to`] for the group box.
    fn group_drag_to(
        &mut self,
        p: Point<Pixels>,
        modifiers: Modifiers,
        screen: &Screen,
        project: &Project,
    ) -> Option<Vec<Command>> {
        let safe = self.safe_areas;
        let g = self.group_drag.as_mut()?;
        let others: Vec<[f32; 4]> = self
            .shapes
            .as_ref()
            .map(|(_, s)| s.as_slice())
            .unwrap_or_default()
            .iter()
            .filter(|s| g.members.iter().all(|m| m.clip != s.clip))
            .map(Shape::bounds)
            .collect();
        if !g.moved && !near(p, g.from_screen, CLICK_SLOP) {
            g.moved = true;
        }
        if !g.moved {
            return None;
        }
        let m = screen.to_sequence(p);
        let pivot = box_center(&g.corners);
        let mut now = GroupMove::about(pivot);
        let mut guides = Vec::new();
        match g.mode {
            Mode::Scale(i) => {
                let c0 = sub(g.corners[i], pivot);
                let len = dot(c0, c0);
                if len > 1e-6 {
                    now.scale = (dot(sub(m, pivot), c0) / len).max(0.01);
                }
            }
            Mode::Rotate => {
                let angle = |p: [f32; 2]| (p[1] - pivot[1]).atan2(p[0] - pivot[0]).to_degrees();
                let mut turn = angle(m) - angle(g.from);
                if modifiers.shift {
                    turn = (turn / 15.0).round() * 15.0;
                }
                now.rotation_deg = turn;
            }
            _ => {
                let [x0, y0] = g.corners[0];
                let [x1, y1] = g.corners[2];
                let delta = sub(m, g.from);
                now.offset = snap_move(
                    [x0, y0, x1, y1],
                    delta,
                    modifiers,
                    screen,
                    &others,
                    safe,
                    &mut guides,
                );
            }
        }
        g.now = now;
        self.guides = guides;
        Some(group_commands(g, project))
    }

    /// End the drag: the commands to commit (none for a click), and whether
    /// it was a click.
    pub fn end(&mut self, project: &Project) -> (Vec<Command>, bool) {
        self.guides.clear();
        if let Some(g) = self.group_drag.take() {
            if !g.moved {
                return (Vec::new(), true);
            }
            self.held = g
                .members
                .iter()
                .map(|m| (m.clip, m.shape.placed(&g.now.transform(&m.shape, &m.start))))
                .collect();
            return (group_commands(&g, project), false);
        }
        let Some(d) = self.drag.take() else {
            return (Vec::new(), false);
        };
        if !d.moved {
            return (Vec::new(), true);
        }
        self.held = vec![(d.clip, d.shape.placed(&d.now))];
        let caption = d.shape.lettering && !d.alone;
        (commit(project, d.clip, d.at, &d.now, caption), false)
    }

    /// Show `clip` with transform `tf` until a newer layout arrives.
    pub fn hold(&mut self, clip: ClipId, tf: &Transform) {
        let shape = self
            .shapes
            .as_ref()
            .and_then(|(_, shapes)| shapes.iter().find(|s| s.clip == clip));
        if let Some(shape) = shape {
            self.held.retain(|(c, _)| *c != clip);
            self.held.push((clip, shape.placed(tf)));
        }
    }

    /// Drop the drag without committing it, and any held placement.
    pub fn cancel(&mut self) {
        self.drag = None;
        self.group_drag = None;
        self.held.clear();
        self.guides.clear();
    }
}

/// The commands that give every member of `g` its place in the group's
/// current move, keyed where the drag began.
fn group_commands(g: &GroupDrag, project: &Project) -> Vec<Command> {
    g.members
        .iter()
        .filter_map(|m| {
            let (_, clip) = project.timeline.clip(m.clip)?;
            let tf = g.now.transform(&m.shape, &m.start);
            Some(transform_commands(clip, g.at, &tf))
        })
        .flatten()
        .collect()
}

/// `delta` (sequence pixels) for a box `bounds` being moved, snapped to
/// the frame, the safe areas (with `safe`) and the `others` boxes, with
/// the guides it snapped to added to `guides`. Shift keeps to one axis;
/// Ctrl/Cmd turns snapping off.
fn snap_move(
    bounds: [f32; 4],
    mut delta: [f32; 2],
    modifiers: Modifiers,
    screen: &Screen,
    others: &[[f32; 4]],
    safe: bool,
    guides: &mut Vec<Guide>,
) -> [f32; 2] {
    let (cw, ch) = (
        screen.sequence.0.max(1) as f32,
        screen.sequence.1.max(1) as f32,
    );
    if modifiers.shift {
        if delta[0].abs() > delta[1].abs() {
            delta[1] = 0.0;
        } else {
            delta[0] = 0.0;
        }
    }
    if modifiers.secondary() {
        return delta;
    }
    let reach = SNAP / screen.scale().max(1e-6);
    let [x0, y0, x1, y1] = bounds;
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
    delta
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
            let delta = snap_move(
                shape.bounds(),
                sub(m, d.from),
                modifiers,
                screen,
                others,
                safe,
                &mut guides,
            );
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
        Mode::Pin(i) => {
            // The corner snaps like a point; the pin is measured in the
            // picture's own uv, before the rest of the transform.
            let plain = Modifiers {
                shift: false,
                ..modifiers
            };
            let snapped = snap_move(
                [m[0], m[1], m[0], m[1]],
                [0.0; 2],
                plain,
                screen,
                others,
                safe,
                &mut guides,
            );
            let target = [m[0] + snapped[0], m[1] + snapped[1]];
            if let Some(uv) = place.unpinned_to_picture(target) {
                let rest = UV_CORNERS[i];
                tf.corner_pin[i] = [uv[0] - rest[0], uv[1] - rest[1]];
            }
        }
        Mode::Anchor => {
            if let Some(uv) = place.unpinned_to_picture(m) {
                let mut anchor = [uv[0] - 0.5, uv[1] - 0.5];
                // Snap to the corners, edge midpoints and centre of the box.
                let [u0, v0, u1, v1] = shape.content;
                let reach = SNAP / screen.scale().max(1e-6);
                for (u, v) in [u0, (u0 + u1) / 2.0, u1]
                    .into_iter()
                    .flat_map(|u| [v0, (v0 + v1) / 2.0, v1].map(|v| (u, v)))
                {
                    let p = place.unpinned_to_canvas([u, v]);
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

/// A polyline through `pts`, closed or not, `width` wide; dashed by
/// `dash` (on, off) when given.
fn stroke(
    pts: &[Point<Pixels>],
    close: bool,
    width: f32,
    dash: Option<[f32; 2]>,
) -> Option<Path<Pixels>> {
    let mut b = PathBuilder::stroke(px(width));
    if let Some([on, off]) = dash {
        b = b.dash_array(&[px(on), px(off)]);
    }
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
    b.build().ok()
}

/// Paint [`stroke`] in `color`.
pub(super) fn draw(
    window: &mut Window,
    pts: &[Point<Pixels>],
    close: bool,
    width: f32,
    dash: Option<[f32; 2]>,
    color: Rgba,
) {
    if let Some(path) = stroke(pts, close, width, dash) {
        window.paint_path(path, color);
    }
}

/// A filled disc of radius `r` at `at`.
fn disc(at: Point<Pixels>, r: f32) -> Option<Path<Pixels>> {
    let r = px(r);
    let mut b = PathBuilder::fill();
    b.move_to(point(at.x + r, at.y));
    b.arc_to(point(r, r), px(0.), false, true, point(at.x - r, at.y));
    b.arc_to(point(r, r), px(0.), false, true, point(at.x + r, at.y));
    b.close();
    b.build().ok()
}

/// A disc of `fill` ringed 1 px in `ring`.
pub(super) fn ringed(at: Point<Pixels>, r: f32, fill: Rgba, ring: Rgba, window: &mut Window) {
    if let Some(path) = disc(at, r + 1.0) {
        window.paint_path(path, ring);
    }
    if let Some(path) = disc(at, r) {
        window.paint_path(path, fill);
    }
}

/// A square handle centred on `at`: white, ringed in the accent.
pub(super) fn square(at: Point<Pixels>, window: &mut Window) {
    let half = px(HANDLE / 2.0);
    let b = Bounds::new(point(at.x - half, at.y - half), size(half * 2., half * 2.));
    window.paint_quad(fill(b, t::ON_AC).border_widths(px(1.)).border_color(t::AC));
}

/// Paint `overlay` over the program at `screen`.
pub(super) fn paint(overlay: &Overlay, screen: &Screen, window: &mut Window) {
    let (w, h) = (screen.sequence.0 as f32, screen.sequence.1 as f32);
    if overlay.safe {
        for (f, a) in SAFE.into_iter().zip([0.3, 0.18]) {
            let q = [
                [w * f, h * f],
                [w * (1.0 - f), h * f],
                [w * (1.0 - f), h * (1.0 - f)],
                [w * f, h * (1.0 - f)],
            ]
            .map(|p| screen.to_screen(p));
            draw(
                window,
                &q,
                true,
                1.0,
                Some([3.0, 3.0]),
                t::alpha(t::ON_AC, a),
            );
        }
        let c = screen.to_screen([w / 2.0, h / 2.0]);
        let r = px(6.0);
        let cross = t::alpha(t::ON_AC, 0.35);
        draw(
            window,
            &[point(c.x - r, c.y), point(c.x + r, c.y)],
            false,
            1.0,
            None,
            cross,
        );
        draw(
            window,
            &[point(c.x, c.y - r), point(c.x, c.y + r)],
            false,
            1.0,
            None,
            cross,
        );
    }
    for g in &overlay.guides {
        let (a, b) = match *g {
            Guide::Vertical(x) => ([x, 0.0], [x, h]),
            Guide::Horizontal(y) => ([0.0, y], [w, y]),
        };
        let snap = t::alpha(t::WARN, 0.9);
        draw(
            window,
            &[screen.to_screen(a), screen.to_screen(b)],
            false,
            1.0,
            None,
            snap,
        );
    }
    for o in &overlay.outlines {
        let q = o.corners.map(|c| screen.to_screen(c));
        let color = if o.active {
            t::AC
        } else {
            t::alpha(t::AC, 0.6)
        };
        draw(window, &q, true, 1.0, None, color);
        if !o.active || !overlay.editable {
            continue;
        }
        if o.kind == OutlineKind::Pin {
            for (handle, at) in pin_handles(&o.corners, screen) {
                let (fill, ring) = if overlay.grabbed == Some(handle) {
                    (t::AC, t::ON_AC)
                } else {
                    (t::ON_AC, t::AC)
                };
                ringed(at, KNOB, fill, ring, window);
            }
            continue;
        }
        let top = screen.to_screen(mid(o.corners[0], o.corners[1]));
        let knob = rotate_handle(&o.corners, screen);
        draw(window, &[top, knob], false, 1.0, None, t::AC);
        let shown = if o.kind == OutlineKind::Group {
            group_handles(&o.corners, screen)
        } else {
            handles(&o.corners, o.anchor, screen)
        };
        for (handle, at) in shown {
            let grabbed = overlay.grabbed == Some(handle);
            match handle {
                Handle::Anchor => {
                    if grabbed && let Some(path) = disc(at, ANCHOR + 1.0) {
                        window.paint_path(path, t::alpha(t::AC, 0.33));
                    }
                    let ring: Vec<_> = (0..=24)
                        .map(|i| {
                            let a = i as f32 / 24.0 * std::f32::consts::TAU;
                            point(at.x + px(ANCHOR * a.cos()), at.y + px(ANCHOR * a.sin()))
                        })
                        .collect();
                    let r = px(ANCHOR);
                    for pts in [
                        ring,
                        vec![point(at.x - r, at.y), point(at.x + r, at.y)],
                        vec![point(at.x, at.y - r), point(at.x, at.y + r)],
                    ] {
                        draw(window, &pts, false, 1.0, None, t::AC);
                    }
                }
                Handle::Rotate => {
                    let fill = if grabbed { t::AC } else { t::ON_AC };
                    ringed(at, KNOB, fill, t::AC, window);
                }
                _ => square(at, window),
            }
        }
    }
}
