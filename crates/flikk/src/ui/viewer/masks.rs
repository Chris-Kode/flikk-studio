//! On-screen masks of the selected picture (the viewer's mask mode): its
//! masks are outlined on the frame, the picked one with handles to move
//! it, size its box or drag its points, and a knob above it that widens
//! its soft edge. The rectangle and ellipse tools draw a new mask by
//! dragging (Shift keeps it square), the pen adds a point per click and
//! closes the path on its first point. Everything is measured in the
//! picture's own uv, so masks follow its transform; an edit shows live and
//! commits once, keyframing the shape at the playhead when the mask
//! already has keyframes. Captions and titles take no masks (see
//! `flikk_core::mask`).
//!
//! The subject tool picks an object in a clip from the pool: hovering shows
//! what a click would keep (the model's answer, found off the UI thread by
//! the viewer), a click keeps it, a drag boxes it. On the picked subject a
//! click adds a point to the frame's prompt (Alt: a point off the object;
//! Shift starts a new subject instead), and a click on one of the frame's
//! points takes it away; the points can be dragged. A subject is outlined
//! by its matte at the playhead ([`Subjects`]).

use super::gizmo::{CLICK_SLOP, REACH, Screen, clip_time, draw, near, ringed, square};
use crate::ui::kit::theme as t;
use flikk_core::command::Command;
use flikk_core::compose::Shape;
use flikk_core::mask::{Mask, MaskShape, MaskState};
use flikk_core::model::Project;
use flikk_core::person::PersonMatte;
use flikk_core::subject::{Matte, Subject, SubjectPoint, SubjectPrompt};
use flikk_core::{ClipId, MaskId, Rate, Time};
use gpui_kit::*;

/// Screen distance of the feather knob above the mask at no feather.
const KNOB_OFFSET: f32 = 16.0;
/// Radius of a subject's points on screen.
const DOT: f32 = 4.5;

/// What a click or drag on the frame makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tool {
    Rectangle,
    Ellipse,
    Pen,
    Subject,
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
    /// One of the picked subject's points on this frame.
    SubjectPoint(usize),
    /// A click (a point) or a drag (a box) with the subject tool.
    Pick,
    /// A press on a subject's body: picks it, moves nothing.
    Select,
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
    /// Where the mouse is now, in picture uv.
    to: [f32; 2],
    moved: bool,
    /// Alt held at the press: a subject click rules the point out.
    alt: bool,
    /// The source frame under the playhead and the media's rate, for
    /// subject edits.
    frame: Option<(Time, Rate)>,
    /// The masks at the last mouse move.
    now: Vec<Mask>,
}

/// What editing subjects needs from the viewer at the playhead.
#[derive(Clone, Default)]
pub(super) struct Subjects {
    /// The source frame under the playhead and the media's rate; `None`
    /// when the clip is not media from the pool.
    pub frame: Option<(Time, Rate)>,
    /// The mattes of the clip's subject masks there, those made.
    pub mattes: Vec<(MaskId, Matte)>,
    /// The mattes of its person masks there, those made.
    pub people: Vec<(MaskId, PersonMatte)>,
}

impl Subjects {
    fn matte(&self, id: MaskId) -> Option<&Matte> {
        self.mattes.iter().find(|(m, _)| *m == id).map(|(_, m)| m)
    }

    fn person(&self, id: MaskId) -> Option<&PersonMatte> {
        self.people.iter().find(|(m, _)| *m == id).map(|(_, m)| m)
    }

    /// `mask`'s prompt on this frame.
    fn prompt<'a>(&self, mask: &'a Mask) -> Option<&'a SubjectPrompt> {
        let (at, rate) = self.frame?;
        mask.picks()?.prompt_on(at.to_frame(rate), rate)
    }
}

/// What a click with the subject tool would keep.
pub(super) struct Hover {
    pub clip: ClipId,
    pub prompt: SubjectPrompt,
    /// In picture uv.
    pub outlines: Vec<Vec<[f32; 2]>>,
}

pub(super) struct MaskEditor {
    /// Mask mode: the selected picture's masks are shown and edited.
    pub on: bool,
    pub tool: Option<Tool>,
    pub selected: Option<MaskId>,
    /// What the masks cut away is tinted over the whole picture, instead
    /// of hidden.
    pub tinted: bool,
    /// The path being drawn with the pen, in picture uv.
    pen: Vec<[f32; 2]>,
    /// The clip the pen's points were placed on.
    pen_clip: Option<ClipId>,
    drag: Option<Drag>,
    pub hover: Option<Hover>,
}

impl Default for MaskEditor {
    fn default() -> Self {
        Self {
            on: false,
            tool: None,
            selected: None,
            tinted: true,
            pen: Vec::new(),
            pen_clip: None,
            drag: None,
            hover: None,
        }
    }
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
    /// The picked subject's points on this frame, and whether each is on
    /// the object.
    pub points: Vec<([f32; 2], bool)>,
    /// Its box on this frame, or one being drawn: corners.
    pub boxes: Vec<[[f32; 2]; 4]>,
    /// What a click would keep.
    pub preview: Vec<Vec<[f32; 2]>>,
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
        MaskShape::Path { .. }
        | MaskShape::Subject
        | MaskShape::Person { .. }
        | MaskShape::Depth { .. } => return None,
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

/// `mask`'s outlines in uv at the playhead: its shape's, or its subject's
/// matte's.
fn outlines(
    mask: &Mask,
    state: &MaskState,
    aspect: f32,
    subjects: &Subjects,
) -> Vec<Vec<[f32; 2]>> {
    match state.shape {
        MaskShape::Subject => subjects
            .matte(mask.id)
            .map(Matte::outlines)
            .unwrap_or_default(),
        _ => vec![state.shape.outline(aspect)],
    }
}

/// Corners of a box in uv, from two opposite ones.
fn rect_corners(a: [f32; 2], b: [f32; 2]) -> [[f32; 2]; 4] {
    [[a[0], a[1]], [b[0], a[1]], [b[0], b[1]], [a[0], b[1]]]
}

/// Screen pixels per picture height around `uv`.
fn screen_unit(shape: &Shape, uv: [f32; 2], screen: &Screen) -> f32 {
    let a = screen.to_screen(to_seq(shape, uv));
    let b = screen.to_screen(to_seq(shape, [uv[0], uv[1] + 0.1]));
    (f32::from(b.x - a.x).hypot(f32::from(b.y - a.y)) / 0.1).max(1e-3)
}

/// The feather knob of a mask outlined by `outlines` with `feather`: its
/// screen position and the outline point (sequence pixels) it hangs from.
fn knob(
    shape: &Shape,
    outlines: &[Vec<[f32; 2]>],
    feather: f32,
    screen: &Screen,
) -> Option<(Point<Pixels>, [f32; 2])> {
    let top = outlines
        .iter()
        .flatten()
        .map(|uv| (to_seq(shape, *uv), *uv))
        .min_by(|a, b| a.0[1].total_cmp(&b.0[1]))?;
    let unit = screen_unit(shape, top.1, screen);
    let at = screen.to_screen(top.0);
    Some((point(at.x, at.y - px(KNOB_OFFSET + feather * unit)), top.0))
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

    /// Drop the pen's points, any drag and the subject preview.
    pub fn cancel(&mut self) {
        self.pen.clear();
        self.drag = None;
        self.hover = None;
    }

    /// Drop the drag in progress, committing nothing; whether there was one.
    pub fn cancel_drag(&mut self) -> bool {
        self.drag.take().is_some()
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

    /// The picked mask, when it keeps a subject.
    fn picked_subject(&self, masks: &[Mask]) -> Option<usize> {
        masks
            .iter()
            .position(|m| Some(m.id) == self.selected && m.picks().is_some())
    }

    /// What to paint for `shape`'s clip at the playhead.
    pub fn overlay(
        &self,
        project: &Project,
        shape: &Shape,
        playhead: Time,
        screen: &Screen,
        subjects: &Subjects,
    ) -> MaskOverlay {
        let mut out = MaskOverlay::default();
        let Some((_, clip)) = project.timeline.clip(shape.clip) else {
            return out;
        };
        let at = clip_time(clip, playhead);
        let aspect = aspect(shape);
        let seq = |line: &Vec<[f32; 2]>| line.iter().map(|uv| to_seq(shape, *uv)).collect();
        for mask in self.masks(project, shape.clip) {
            let state = mask.state_at(at);
            let picked = self.selected == Some(mask.id);
            let lines = outlines(&mask, &state, aspect, subjects);
            out.outlines
                .extend(lines.iter().map(|line| (seq(line), picked)));
            if !picked {
                continue;
            }
            out.handles = handles(&state, aspect)
                .into_iter()
                .map(|uv| to_seq(shape, uv))
                .collect();
            out.knob = knob(shape, &lines, state.feather, screen).map(|(k, top)| (top, k));
            if let Some(prompt) = subjects.prompt(&mask) {
                let dragged = self.drag.as_ref().and_then(|d| match d.edit {
                    Edit::SubjectPoint(k) if d.moved => Some((k, d.to)),
                    _ => None,
                });
                out.points = prompt
                    .points
                    .iter()
                    .enumerate()
                    .map(|(k, p)| {
                        let uv = match dragged {
                            Some((j, to)) if j == k => to,
                            _ => p.at,
                        };
                        (to_seq(shape, uv), p.include)
                    })
                    .collect();
                if let Some([u0, v0, u1, v1]) = prompt.rect {
                    out.boxes
                        .push(rect_corners([u0, v0], [u1, v1]).map(|uv| to_seq(shape, uv)));
                }
            }
        }
        if let Some(d) = self
            .drag
            .as_ref()
            .filter(|d| d.edit == Edit::Pick && d.moved)
        {
            out.boxes
                .push(rect_corners(d.from, d.to).map(|uv| to_seq(shape, uv)));
        }
        if let Some(h) = self.hover.as_ref().filter(|h| h.clip == shape.clip) {
            out.preview = h.outlines.iter().map(seq).collect();
        }
        if self.pen_clip == Some(shape.clip) {
            out.pen = self.pen.iter().map(|uv| to_seq(shape, *uv)).collect();
        }
        out
    }

    /// The prompt a click at `p` would make with the subject tool: the
    /// picked subject's on this frame plus the point (Alt: off the object;
    /// Shift: a new subject). `None` off the picture, while dragging, or
    /// when the clip is not media.
    pub fn hover_prompt(
        &self,
        p: Point<Pixels>,
        modifiers: Modifiers,
        screen: &Screen,
        project: &Project,
        shape: &Shape,
        subjects: &Subjects,
    ) -> Option<SubjectPrompt> {
        if self.tool != Some(Tool::Subject) || self.drag.is_some() {
            return None;
        }
        let (at, _) = subjects.frame?;
        let uv = to_uv(shape, screen.to_sequence(p))?;
        if !(0.0..=1.0).contains(&uv[0]) || !(0.0..=1.0).contains(&uv[1]) {
            return None;
        }
        let masks = self.masks(project, shape.clip);
        let base = self
            .picked_subject(&masks)
            .filter(|_| !modifiers.shift)
            .and_then(|i| subjects.prompt(&masks[i]).cloned());
        let mut prompt = base.unwrap_or(SubjectPrompt {
            at,
            ..Default::default()
        });
        prompt.points.push(SubjectPoint {
            at: uv,
            include: !modifiers.alt,
        });
        Some(prompt)
    }

    /// Press at `p` on `shape`'s clip: start a drag, add a pen point, or
    /// pick a mask. The commands to commit when the pen closes a path;
    /// `None` when nothing here takes the press.
    #[allow(clippy::too_many_arguments)]
    pub fn press(
        &mut self,
        p: Point<Pixels>,
        modifiers: Modifiers,
        screen: &Screen,
        project: &Project,
        shape: &Shape,
        playhead: Time,
        subjects: &Subjects,
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
        let frame = subjects.frame;
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
            to: uv,
            moved: false,
            alt: modifiers.alt,
            frame,
            now: masks.clone(),
        };
        // The picked subject's points on this frame come first.
        if let Some(i) = self.picked_subject(&masks)
            && let Some(prompt) = subjects.prompt(&masks[i])
            && let Some(k) = prompt
                .points
                .iter()
                .position(|q| near(screen.to_screen(to_seq(shape, q.at)), p, REACH))
        {
            let state = masks[i].state_at(at);
            self.drag = Some(begin(i, state, Edit::SubjectPoint(k)));
            return Some(Vec::new());
        }
        match self.tool {
            Some(Tool::Subject) => {
                frame?;
                let index = self
                    .picked_subject(&masks)
                    .filter(|_| !modifiers.shift)
                    .unwrap_or(masks.len());
                let state = masks
                    .get(index)
                    .map(|m| m.state_at(at))
                    .unwrap_or(MaskState {
                        shape: MaskShape::Subject,
                        feather: 0.0,
                        expansion: 0.0,
                        opacity: 1.0,
                    });
                self.drag = Some(begin(index, state, Edit::Pick));
                return Some(Vec::new());
            }
            Some(Tool::Pen) => {
                // Points placed on another clip don't carry over.
                if self.pen_clip != Some(clip.id) {
                    self.pen.clear();
                    self.pen_clip = Some(clip.id);
                }
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
            let lines = outlines(&masks[i], &state, aspect, subjects);
            if let Some((k, _)) = knob(shape, &lines, state.feather, screen)
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
            let hit = match state.shape {
                MaskShape::Subject => subjects.matte(mask.id).is_some_and(|m| m.contains(uv)),
                MaskShape::Person { .. } => {
                    subjects.person(mask.id).is_some_and(|m| m.contains(uv))
                }
                _ => {
                    let poly: Vec<Point<Pixels>> = state
                        .shape
                        .outline(aspect)
                        .into_iter()
                        .map(|uv| screen.to_screen(to_seq(shape, uv)))
                        .collect();
                    inside(&poly, p)
                }
            };
            if hit {
                self.selected = Some(mask.id);
                let edit = if matches!(state.shape, MaskShape::Subject | MaskShape::Person { .. }) {
                    Edit::Select
                } else {
                    Edit::Move
                };
                self.drag = Some(begin(i, state, edit));
                return Some(Vec::new());
            }
        }
        self.selected = None;
        None
    }

    /// Follow the mouse; the commands that show the edit live, if it moved
    /// and shows live (subject edits show their points and box until they
    /// commit).
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
        d.to = [m[0].clamp(0.0, 1.0), m[1].clamp(0.0, 1.0)];
        let mut state = d.start.clone();
        match d.edit {
            Edit::SubjectPoint(_) | Edit::Pick | Edit::Select => return None,
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
                    MaskShape::Path { .. }
                    | MaskShape::Subject
                    | MaskShape::Person { .. }
                    | MaskShape::Depth { .. } => {}
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
        match d.edit {
            Edit::SubjectPoint(k) => return self.end_point(&d, k),
            Edit::Pick => return self.end_pick(&d),
            Edit::Select => return Vec::new(),
            _ => {}
        }
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

    /// A subject point dragged (moved there) or, with the subject tool,
    /// clicked (taken away).
    fn end_point(&mut self, d: &Drag, k: usize) -> Vec<Command> {
        let Some((at, rate)) = d.frame else {
            return Vec::new();
        };
        let Some(mut prompt) = d
            .masks
            .get(d.index)
            .and_then(|m| m.picks())
            .and_then(|s| s.prompt_on(at.to_frame(rate), rate))
            .cloned()
        else {
            return Vec::new();
        };
        if k >= prompt.points.len() {
            return Vec::new();
        }
        if d.moved {
            prompt.points[k].at = d.to;
        } else if self.tool == Some(Tool::Subject) {
            prompt.points.remove(k);
        } else {
            return Vec::new();
        }
        self.commit_prompt(d, prompt)
    }

    /// A subject tool click (a point) or drag (a box), into the picked
    /// subject's prompt on this frame or a new subject.
    fn end_pick(&mut self, d: &Drag) -> Vec<Command> {
        let Some((at, rate)) = d.frame else {
            return Vec::new();
        };
        let old = d
            .masks
            .get(d.index)
            .and_then(|m| m.picks())
            .and_then(|s| s.prompt_on(at.to_frame(rate), rate))
            .cloned();
        let mut prompt = old.unwrap_or(SubjectPrompt {
            at,
            ..Default::default()
        });
        if d.moved {
            let (a, b) = (d.from, d.to);
            let rect = [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[0].max(b[0]),
                a[1].max(b[1]),
            ];
            if rect[2] - rect[0] < 1e-3 || rect[3] - rect[1] < 1e-3 {
                return Vec::new();
            }
            prompt.rect = Some(rect);
        } else {
            if !(0.0..=1.0).contains(&d.from[0]) || !(0.0..=1.0).contains(&d.from[1]) {
                return Vec::new();
            }
            prompt.points.push(SubjectPoint {
                at: d.from,
                include: !d.alt,
            });
        }
        self.commit_prompt(d, prompt)
    }

    /// The masks with `prompt` on its frame of the subject being edited,
    /// or of a new one (picked); a subject left with no prompt goes.
    fn commit_prompt(&mut self, d: &Drag, prompt: SubjectPrompt) -> Vec<Command> {
        let Some((_, rate)) = d.frame else {
            return Vec::new();
        };
        let mut masks = d.masks.clone();
        match masks.get_mut(d.index).filter(|m| m.picks().is_some()) {
            Some(mask) => {
                let subject = mask
                    .subject
                    .take()
                    .expect("picks a subject")
                    .with_prompt(prompt, rate);
                if subject.prompts.is_empty() {
                    masks.remove(d.index);
                    self.selected = None;
                } else {
                    mask.subject = Some(subject);
                }
            }
            None => {
                if prompt.is_empty() {
                    return Vec::new();
                }
                let mask = Mask::subject(Subject::new(prompt));
                self.selected = Some(mask.id);
                masks.push(mask);
            }
        }
        self.hover = None;
        vec![Command::SetMasks {
            clip: d.clip,
            masks,
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

/// Paint `overlay` at `screen`, in the accent.
pub(super) fn paint(overlay: &MaskOverlay, screen: &Screen, window: &mut Window) {
    let on_screen = |line: &[[f32; 2]]| -> Vec<Point<Pixels>> {
        line.iter().map(|p| screen.to_screen(*p)).collect()
    };
    for line in &overlay.preview {
        let pts = on_screen(line);
        let mut b = PathBuilder::fill();
        for (i, p) in pts.iter().enumerate() {
            if i == 0 {
                b.move_to(*p);
            } else {
                b.line_to(*p);
            }
        }
        b.close();
        if let Ok(path) = b.build() {
            window.paint_path(path, t::alpha(t::AC, 0.22));
        }
        draw(window, &pts, true, 1.5, None, t::AC);
    }
    for (outline, picked) in &overlay.outlines {
        let (width, color) = if *picked {
            (1.5, t::AC)
        } else {
            (1.0, t::alpha(t::AC, 0.6))
        };
        draw(window, &on_screen(outline), true, width, None, color);
    }
    for corners in &overlay.boxes {
        let dashed = t::alpha(t::AC, 0.67);
        draw(
            window,
            &on_screen(corners),
            true,
            1.0,
            Some([3.0, 3.0]),
            dashed,
        );
    }
    for h in &overlay.handles {
        square(screen.to_screen(*h), window);
    }
    for (at, include) in &overlay.points {
        let at = screen.to_screen(*at);
        let (ring, fill) = if *include {
            (t::ON_AC, t::AC)
        } else {
            (t::AC, t::ON_AC)
        };
        ringed(at, DOT, fill, ring, window);
        if !include {
            let r = px(DOT * 0.55);
            let minus = [point(at.x - r, at.y), point(at.x + r, at.y)];
            draw(window, &minus, false, 1.5, None, t::AC);
        }
    }
    if let Some((top, k)) = overlay.knob {
        let stem = [screen.to_screen(top), k];
        draw(window, &stem, false, 1.0, None, t::alpha(t::AC, 0.67));
        ringed(k, 3.5, t::ON_AC, t::AC, window);
    }
    if !overlay.pen.is_empty() {
        let pts = on_screen(&overlay.pen);
        draw(window, &pts, false, 1.5, None, t::AC);
        for p in pts {
            square(p, window);
        }
    }
}
