//! On-screen masks of the selected picture (the viewer's mask mode): its
//! masks are outlined on the frame, the picked one with handles to move
//! it, size its box or drag its points, and a knob above it that widens
//! its soft edge. The rectangle and ellipse tools draw a new mask by
//! dragging (Shift keeps it square), the pen adds a point per click and
//! closes the path on its first point. Everything is measured in the
//! picture's own uv, so masks follow its transform; an edit shows live and
//! commits once, keyframing the shape at the playhead when the mask
//! already has keyframes. Captions and titles take no masks (see
//! `koo_core::mask`).

use super::gizmo::{CLICK_SLOP, HANDLE, REACH, Screen, clip_time, near};
use gpui_kit::*;
use koo_core::command::Command;
use koo_core::compose::Shape;
use koo_core::mask::{Mask, MaskShape, MaskState};
use koo_core::model::Project;
use koo_core::{ClipId, MaskId, Time};

/// Screen distance of the feather knob above the mask at no feather.
const KNOB_OFFSET: f32 = 16.0;

/// What a click or drag on the frame makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tool {
    Rectangle,
    Ellipse,
    Pen,
}

/// What a drag changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edit {
    Move,
    /// A corner of a rectangle's or ellipse's box: top-left, top-right,
    /// bottom-right, bottom-left.
    Corner(usize),
    /// A path point.
    Point(usize),
    Feather,
    /// A new rectangle or ellipse, from where the drag began.
    Create(Tool),
}

struct Drag {
    clip: ClipId,
    /// The clip's layer when the drag began.
    shape: Shape,
    /// The clip's masks when the drag began.
    masks: Vec<Mask>,
    /// The mask being changed (past the end for a new one).
    index: usize,
    /// Its state at the playhead when the drag began.
    start: MaskState,
    /// Clip-relative playhead.
    at: Time,
    edit: Edit,
    /// Where the drag began, in picture uv and on screen.
    from: [f32; 2],
    from_screen: Point<Pixels>,
    moved: bool,
    /// The masks at the last mouse move.
    now: Vec<Mask>,
}

#[derive(Default)]
pub(super) struct MaskEditor {
    /// Mask mode: the selected picture's masks are shown and edited.
    pub on: bool,
    pub tool: Option<Tool>,
    pub selected: Option<MaskId>,
    /// The path being drawn with the pen, in picture uv.
    pen: Vec<[f32; 2]>,
    drag: Option<Drag>,
}

/// Everything painted for masks, in sequence pixels.
#[derive(Clone, Debug, Default)]
pub(super) struct MaskOverlay {
    /// Each mask's outline, and whether it is the picked one.
    pub outlines: Vec<(Vec<[f32; 2]>, bool)>,
    /// The picked mask's handles.
    pub handles: Vec<[f32; 2]>,
    /// The feather knob and the outline point it hangs from.
    pub knob: Option<([f32; 2], Point<Pixels>)>,
    pub pen: Vec<[f32; 2]>,
}

/// The picture's width over its height.
fn aspect(shape: &Shape) -> f32 {
    shape.rest_size[0].max(1e-3) / shape.rest_size[1].max(1e-3)
}

/// Picture uv (before flip) → sequence pixels.
fn to_seq(shape: &Shape, uv: [f32; 2]) -> [f32; 2] {
    let flip = shape.placement.flip;
    let u = if flip[0] { 1.0 - uv[0] } else { uv[0] };
    let v = if flip[1] { 1.0 - uv[1] } else { uv[1] };
    shape.placement.to_canvas([u, v])
}

/// Sequence pixels → picture uv (before flip).
fn to_uv(shape: &Shape, p: [f32; 2]) -> Option<[f32; 2]> {
    let [u, v] = shape.placement.to_picture(p)?;
    let flip = shape.placement.flip;
    Some([
        if flip[0] { 1.0 - u } else { u },
        if flip[1] { 1.0 - v } else { v },
    ])
}

/// A rectangle's or ellipse's box corners in uv, turned with it.
fn box_corners(state: &MaskState, aspect: f32) -> Option<[[f32; 2]; 4]> {
    let (center, size, rotation) = match state.shape {
        MaskShape::Rectangle {
            center,
            size,
            rotation_deg,
            ..
        }
        | MaskShape::Ellipse {
            center,
            size,
            rotation_deg,
        } => (center, size, rotation_deg),
        MaskShape::Path { .. } => return None,
    };
    let (s, c) = rotation.to_radians().sin_cos();
    let (hw, hh) = (size[0] * aspect / 2.0, size[1] / 2.0);
    let cx = center[0] * aspect;
    Some(
        [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]]
            .map(|[x, y]| [(cx + c * x - s * y) / aspect, center[1] + s * x + c * y]),
    )
}

/// The handles of `state` in uv: box corners or path points.
fn handles(state: &MaskState, aspect: f32) -> Vec<[f32; 2]> {
    match &state.shape {
        MaskShape::Path { points, .. } => points.clone(),
        _ => box_corners(state, aspect)
            .map(Vec::from)
            .unwrap_or_default(),
    }
}

/// Screen pixels per picture height around `uv`.
fn screen_unit(shape: &Shape, uv: [f32; 2], screen: &Screen) -> f32 {
    let a = screen.to_screen(to_seq(shape, uv));
    let b = screen.to_screen(to_seq(shape, [uv[0], uv[1] + 0.1]));
    (f32::from(b.x - a.x).hypot(f32::from(b.y - a.y)) / 0.1).max(1e-3)
}

/// The feather knob of `state`: its screen position and the outline point
/// (sequence pixels) it hangs from.
fn knob(shape: &Shape, state: &MaskState, screen: &Screen) -> Option<(Point<Pixels>, [f32; 2])> {
    let outline = state.shape.outline(aspect(shape));
    let top = outline
        .iter()
        .map(|uv| (to_seq(shape, *uv), *uv))
        .min_by(|a, b| a.0[1].total_cmp(&b.0[1]))?;
    let unit = screen_unit(shape, top.1, screen);
    let at = screen.to_screen(top.0);
    Some((
        point(at.x, at.y - px(KNOB_OFFSET + state.feather * unit)),
        top.0,
    ))
}

/// Whether `p` is inside the screen polygon `poly` (even-odd).
fn inside(poly: &[Point<Pixels>], p: Point<Pixels>) -> bool {
    let (x, y) = (f32::from(p.x), f32::from(p.y));
    let mut hit = false;
    let mut j = poly.len().wrapping_sub(1);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        let (ax, ay, bx, by) = (
            f32::from(a.x),
            f32::from(a.y),
            f32::from(b.x),
            f32::from(b.y),
        );
        if (ay > y) != (by > y) && x < ax + (y - ay) * (bx - ax) / (by - ay) {
            hit = !hit;
        }
        j = i;
    }
    hit
}

impl MaskEditor {
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Whether the pen has points down.
    pub fn drawing(&self) -> bool {
        !self.pen.is_empty()
    }

    /// Drop the pen's points and any drag.
    pub fn cancel(&mut self) {
        self.pen.clear();
        self.drag = None;
    }

    /// `clip`'s masks: as being dragged, else as in `project`.
    fn masks(&self, project: &Project, clip: ClipId) -> Vec<Mask> {
        if let Some(d) = self.drag.as_ref().filter(|d| d.clip == clip) {
            return d.now.clone();
        }
        project
            .timeline
            .clip(clip)
            .map(|(_, c)| c.masks.clone())
            .unwrap_or_default()
    }

    /// What to paint for `shape`'s clip at the playhead.
    pub fn overlay(
        &self,
        project: &Project,
        shape: &Shape,
        playhead: Time,
        screen: &Screen,
    ) -> MaskOverlay {
        let mut out = MaskOverlay::default();
        let Some((_, clip)) = project.timeline.clip(shape.clip) else {
            return out;
        };
        let at = clip_time(clip, playhead);
        let aspect = aspect(shape);
        for mask in self.masks(project, shape.clip) {
            let state = mask.state_at(at);
            let picked = self.selected == Some(mask.id);
            let outline = state
                .shape
                .outline(aspect)
                .into_iter()
                .map(|uv| to_seq(shape, uv))
                .collect();
            out.outlines.push((outline, picked));
            if picked {
                out.handles = handles(&state, aspect)
                    .into_iter()
                    .map(|uv| to_seq(shape, uv))
                    .collect();
                out.knob = knob(shape, &state, screen).map(|(k, top)| (top, k));
            }
        }
        out.pen = self.pen.iter().map(|uv| to_seq(shape, *uv)).collect();
        out
    }

    /// Press at `p` on `shape`'s clip: start a drag, add a pen point, or
    /// pick a mask. The commands to commit when the pen closes a path;
    /// `None` when nothing here takes the press.
    pub fn press(
        &mut self,
        p: Point<Pixels>,
        modifiers: Modifiers,
        screen: &Screen,
        project: &Project,
        shape: &Shape,
        playhead: Time,
    ) -> Option<Vec<Command>> {
        let (track, clip) = project.timeline.clip(shape.clip)?;
        if track.locked {
            return None;
        }
        let at = clip_time(clip, playhead);
        let aspect = aspect(shape);
        let seq = screen.to_sequence(p);
        let uv = to_uv(shape, seq)?;
        let masks = clip.masks.clone();
        let begin = |index: usize, start: MaskState, edit: Edit| Drag {
            clip: clip.id,
            shape: shape.clone(),
            masks: masks.clone(),
            index,
            start,
            at,
            edit,
            from: uv,
            from_screen: p,
            moved: false,
            now: masks.clone(),
        };
        match self.tool {
            Some(Tool::Pen) => {
                let closes = self.pen.len() >= 3
                    && near(screen.to_screen(to_seq(shape, self.pen[0])), p, REACH);
                if !closes {
                    self.pen.push(uv);
                    return Some(Vec::new());
                }
                let mask = Mask::new(MaskShape::Path {
                    points: std::mem::take(&mut self.pen),
                    smooth: !modifiers.alt,
                });
                self.selected = Some(mask.id);
                self.tool = None;
                let mut next = masks.clone();
                next.push(mask);
                return Some(vec![Command::SetMasks {
                    clip: clip.id,
                    masks: next,
                }]);
            }
            Some(tool) => {
                let state = MaskState {
                    shape: MaskShape::Path {
                        points: Vec::new(),
                        smooth: false,
                    },
                    feather: 0.0,
                    expansion: 0.0,
                    opacity: 1.0,
                };
                self.drag = Some(begin(masks.len(), state, Edit::Create(tool)));
                return Some(Vec::new());
            }
            None => {}
        }
        // The picked mask's handles, then any mask's body, topmost first.
        if let Some(i) = masks.iter().position(|m| Some(m.id) == self.selected) {
            let state = masks[i].state_at(at);
            if let Some((k, _)) = knob(shape, &state, screen)
                && near(k, p, REACH)
            {
                self.drag = Some(begin(i, state, Edit::Feather));
                return Some(Vec::new());
            }
            let edit = |h: usize| match state.shape {
                MaskShape::Path { .. } => Edit::Point(h),
                _ => Edit::Corner(h),
            };
            let hit = handles(&state, aspect)
                .iter()
                .position(|h| near(screen.to_screen(to_seq(shape, *h)), p, REACH));
            if let Some(h) = hit {
                let edit = edit(h);
                self.drag = Some(begin(i, state, edit));
                return Some(Vec::new());
            }
        }
        for (i, mask) in masks.iter().enumerate().rev() {
            let state = mask.state_at(at);
            let poly: Vec<Point<Pixels>> = state
                .shape
                .outline(aspect)
                .into_iter()
                .map(|uv| screen.to_screen(to_seq(shape, uv)))
                .collect();
            if inside(&poly, p) {
                self.selected = Some(mask.id);
                self.drag = Some(begin(i, state, Edit::Move));
                return Some(Vec::new());
            }
        }
        self.selected = None;
        None
    }

    /// Follow the mouse; the commands that show the edit live, if it moved.
    pub fn drag_to(
        &mut self,
        p: Point<Pixels>,
        modifiers: Modifiers,
        screen: &Screen,
    ) -> Option<Vec<Command>> {
        let d = self.drag.as_mut()?;
        if !d.moved && !near(p, d.from_screen, CLICK_SLOP) {
            d.moved = true;
        }
        if !d.moved {
            return None;
        }
        let shape = &d.shape;
        let aspect = aspect(shape);
        let m = to_uv(shape, screen.to_sequence(p))?;
        let mut state = d.start.clone();
        match d.edit {
            Edit::Move => {
                state.shape = d.start.shape.moved([m[0] - d.from[0], m[1] - d.from[1]]);
            }
            Edit::Point(i) => {
                if let MaskShape::Path { points, .. } = &mut state.shape
                    && let Some(pt) = points.get_mut(i)
                {
                    *pt = m;
                }
            }
            Edit::Corner(i) => {
                let corners = box_corners(&d.start, aspect)?;
                let o = corners[(i + 2) % 4];
                match &mut state.shape {
                    MaskShape::Rectangle {
                        center,
                        size,
                        rotation_deg,
                        ..
                    }
                    | MaskShape::Ellipse {
                        center,
                        size,
                        rotation_deg,
                    } => {
                        // In the box's own turned frame, square units.
                        let (s, c) = rotation_deg.to_radians().sin_cos();
                        let dx = (m[0] - o[0]) * aspect;
                        let dy = m[1] - o[1];
                        let local = [c * dx + s * dy, -s * dx + c * dy];
                        *center = [(m[0] + o[0]) / 2.0, (m[1] + o[1]) / 2.0];
                        *size = [local[0].abs() / aspect, local[1].abs()];
                    }
                    MaskShape::Path { .. } => {}
                }
            }
            Edit::Feather => {
                let unit = screen_unit(shape, d.from, screen);
                let rise = f32::from(d.from_screen.y - p.y) / unit;
                state.feather = (d.start.feather + rise).max(0.0);
            }
            Edit::Create(tool) => {
                let (a, b) = (d.from, m);
                let (mut w, mut h) = ((b[0] - a[0]) * aspect, b[1] - a[1]);
                if modifiers.shift {
                    let side = w.abs().max(h.abs());
                    w = side.copysign(w);
                    h = side.copysign(h);
                }
                let center = [a[0] + w / aspect / 2.0, a[1] + h / 2.0];
                let size = [w.abs() / aspect, h.abs()];
                state.shape = match tool {
                    Tool::Ellipse => MaskShape::Ellipse {
                        center,
                        size,
                        rotation_deg: 0.0,
                    },
                    _ => MaskShape::Rectangle {
                        center,
                        size,
                        rotation_deg: 0.0,
                        roundness: 0.0,
                    },
                };
            }
        }
        let mut now = d.masks.clone();
        if let Edit::Create(_) = d.edit {
            let mut mask = Mask::new(state.shape.clone());
            if let Some(old) = d.now.get(d.index) {
                mask.id = old.id;
            }
            now.push(mask);
        } else if let Some(mask) = now.get_mut(d.index) {
            *mask = mask.with_state_at(d.at, state);
        }
        d.now = now;
        Some(vec![Command::SetMasks {
            clip: d.clip,
            masks: d.now.clone(),
        }])
    }

    /// End the drag: the commands to commit (none for a click).
    pub fn end(&mut self) -> Vec<Command> {
        let Some(d) = self.drag.take() else {
            return Vec::new();
        };
        if !d.moved {
            return Vec::new();
        }
        if let Edit::Create(_) = d.edit {
            self.tool = None;
            self.selected = d.now.get(d.index).map(|m| m.id);
        }
        vec![Command::SetMasks {
            clip: d.clip,
            masks: d.now,
        }]
    }

    /// The commands that remove the picked mask from `clip`.
    pub fn remove_selected(&mut self, project: &Project, clip: ClipId) -> Vec<Command> {
        let Some(id) = self.selected.take() else {
            return Vec::new();
        };
        let masks = self.masks(project, clip);
        if !masks.iter().any(|m| m.id == id) {
            return Vec::new();
        }
        vec![Command::SetMasks {
            clip,
            masks: masks.into_iter().filter(|m| m.id != id).collect(),
        }]
    }
}

/// Paint `overlay` at `screen`.
pub(super) fn paint(overlay: &MaskOverlay, screen: &Screen, color: Hsla, window: &mut Window) {
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
    for (outline, picked) in &overlay.outlines {
        let pts: Vec<Point<Pixels>> = outline.iter().map(|p| screen.to_screen(*p)).collect();
        let (width, color) = if *picked {
            (1.5, color)
        } else {
            (1.0, color.opacity(0.6))
        };
        stroke(&pts, true, width, color, window);
    }
    let half = px(HANDLE / 2.0);
    let square = |at: Point<Pixels>, window: &mut Window| {
        let b = Bounds::new(point(at.x - half, at.y - half), size(half * 2., half * 2.));
        window.paint_quad(fill(b, white()).border_widths(px(1.)).border_color(color));
    };
    for h in &overlay.handles {
        square(screen.to_screen(*h), window);
    }
    if let Some((top, k)) = overlay.knob {
        stroke(&[screen.to_screen(top), k], false, 1.0, color, window);
        let mut b = PathBuilder::fill();
        b.move_to(point(k.x + half, k.y));
        b.arc_to(
            point(half, half),
            px(0.),
            false,
            true,
            point(k.x - half, k.y),
        );
        b.arc_to(
            point(half, half),
            px(0.),
            false,
            true,
            point(k.x + half, k.y),
        );
        b.close();
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
    }
    if !overlay.pen.is_empty() {
        let pts: Vec<Point<Pixels>> = overlay.pen.iter().map(|p| screen.to_screen(*p)).collect();
        stroke(&pts, false, 1.5, color, window);
        for p in pts {
            square(p, window);
        }
    }
}
