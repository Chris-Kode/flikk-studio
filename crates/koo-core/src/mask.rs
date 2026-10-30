//! Masks: shapes that keep or cut away part of a clip's picture, drawn in
//! the picture's own coordinates so they follow its transform.
//!
//! Shapes are measured in the picture's uv (0..1 across the whole picture,
//! before crop and flip), turned and rounded in its own proportions, so a
//! circle stays round on a wide picture. Feather and expansion are
//! fractions of the picture's height. Each mask becomes a coverage in 0..1
//! from its signed distance (soft over `feather`, at least a pixel),
//! inverted and faded by its opacity, and the masks combine in order: the
//! first adds to nothing (or, when it subtracts or intersects, cuts from
//! the whole picture); [`MaskMode::Add`] unites, [`MaskMode::Subtract`]
//! cuts away and [`MaskMode::Intersect`] keeps the overlap.
//!
//! A subject mask ([`MaskShape::Subject`], [`Mask::subject`]) keeps an
//! object a segmentation model finds instead of a shape: its [`Matte`] at
//! the time ([`SubjectMattes`]) is drawn on the raster as coverage, or as
//! signed distances when it feathers or expands (see
//! [`crate::subject`]). One without its matte yet is left out.
//!
//! The compositor multiplies the picture's alpha by [`raster`] before it
//! is placed and blended (`FxBatch::mask`), on the GPU and the CPU alike.
//! Captions and titles take no masks: the render burns them in with ASS,
//! which cannot feather or combine shapes.

use crate::ids::MaskId;
use crate::model::Ease;
use crate::services::Frame;
use crate::subject::{FieldKind, Matte, Subject};
use crate::time::Time;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Longest side of a mask raster, in pixels.
pub const MAX_RASTER: u32 = 1024;
/// Segments per span of a smooth path, and around an ellipse.
const SPAN_STEPS: usize = 8;
const ELLIPSE_STEPS: usize = 64;

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskMode {
    #[default]
    Add,
    Subtract,
    Intersect,
}

impl MaskMode {
    pub const ALL: [MaskMode; 3] = [Self::Add, Self::Subtract, Self::Intersect];

    pub fn label(self) -> &'static str {
        match self {
            Self::Add => "Add",
            Self::Subtract => "Subtract",
            Self::Intersect => "Intersect",
        }
    }
}

/// A mask's outline, in picture uv.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MaskShape {
    /// `roundness` rounds the corners, 0..1 of half the shorter side.
    Rectangle {
        center: [f32; 2],
        size: [f32; 2],
        #[serde(default)]
        rotation_deg: f32,
        #[serde(default)]
        roundness: f32,
    },
    Ellipse {
        center: [f32; 2],
        size: [f32; 2],
        #[serde(default)]
        rotation_deg: f32,
    },
    /// A closed path through `points`; `smooth` draws a curve through them
    /// instead of straight sides.
    Path {
        points: Vec<[f32; 2]>,
        #[serde(default)]
        smooth: bool,
    },
    /// The object [`Mask::subject`] picks, as the model finds it.
    Subject,
}

/// Everything about a mask that can be keyframed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskState {
    pub shape: MaskShape,
    /// Width of the soft edge, as a fraction of the picture's height.
    #[serde(default)]
    pub feather: f32,
    /// How far the edge moves out (or in, below 0), as a fraction of the
    /// picture's height.
    #[serde(default)]
    pub expansion: f32,
    #[serde(default = "one")]
    pub opacity: f32,
}

/// The whole state of a mask at a clip-relative time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskKeyframe {
    pub at: Time,
    #[serde(flatten)]
    pub state: MaskState,
    /// Easing into the next keyframe.
    #[serde(default)]
    pub ease: Ease,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    pub id: MaskId,
    #[serde(default)]
    pub mode: MaskMode,
    #[serde(default)]
    pub invert: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// The state without keyframes.
    #[serde(flatten)]
    pub state: MaskState,
    /// Sorted by time; between two, the shape moves when both have the
    /// same kind of shape (and paths the same number of points), else it
    /// holds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keyframes: Vec<MaskKeyframe>,
    /// What a [`MaskShape::Subject`] keeps. Not keyframed: the model
    /// follows the object instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<Subject>,
}

impl Mask {
    pub fn new(shape: MaskShape) -> Self {
        Self {
            id: MaskId::new(),
            mode: MaskMode::Add,
            invert: false,
            enabled: true,
            state: MaskState {
                shape,
                feather: 0.0,
                expansion: 0.0,
                opacity: 1.0,
            },
            keyframes: Vec::new(),
            subject: None,
        }
    }

    /// A mask keeping the object `subject` picks.
    pub fn subject(subject: Subject) -> Self {
        Self {
            subject: Some(subject),
            ..Self::new(MaskShape::Subject)
        }
    }

    /// Its subject, when it keeps one.
    pub fn picks(&self) -> Option<&Subject> {
        self.subject
            .as_ref()
            .filter(|_| self.state.shape == MaskShape::Subject)
    }

    /// The state at clip-relative time `t`: [`Mask::state`] without
    /// keyframes, the nearest one outside them, eased in between.
    pub fn state_at(&self, t: Time) -> MaskState {
        let (mut prev, mut next): (Option<&MaskKeyframe>, Option<&MaskKeyframe>) = (None, None);
        for k in &self.keyframes {
            if k.at <= t {
                if prev.is_none_or(|p| k.at >= p.at) {
                    prev = Some(k);
                }
            } else if next.is_none_or(|n| k.at < n.at) {
                next = Some(k);
            }
        }
        match (prev, next) {
            (None, None) => self.state.clone(),
            (Some(k), None) | (None, Some(k)) => k.state.clone(),
            (Some(a), Some(b)) => {
                let u = (t - a.at).0 as f64 / (b.at - a.at).0 as f64;
                a.state.lerp(&b.state, a.ease.apply(u as f32))
            }
        }
    }

    /// `self` showing `state` at clip-relative time `t`: a keyframe there
    /// when it has keyframes (keeping one there's easing), else its static
    /// state.
    pub fn with_state_at(&self, t: Time, state: MaskState) -> Mask {
        let mut out = self.clone();
        if self.keyframes.is_empty() {
            out.state = state;
            return out;
        }
        match out.keyframes.iter_mut().find(|k| k.at == t) {
            Some(k) => k.state = state,
            None => {
                out.keyframes.push(MaskKeyframe {
                    at: t,
                    state,
                    ease: Ease::default(),
                });
                out.keyframes.sort_by_key(|k| k.at);
            }
        }
        out
    }

    /// Keyframes at or before clip time `t`, plus one at `t` when there
    /// were any after it, so the shape at a new end does not jump.
    pub fn keep_until(&mut self, t: Time) {
        if self.keyframes.iter().any(|k| k.at > t) && !self.keyframes.iter().any(|k| k.at == t) {
            let ease = self
                .keyframes
                .iter()
                .filter(|k| k.at < t)
                .max_by_key(|k| k.at)
                .map(|k| k.ease);
            self.keyframes.push(MaskKeyframe {
                at: t,
                state: self.state_at(t),
                ease: ease.unwrap_or_default(),
            });
        }
        self.keyframes.retain(|k| k.at <= t);
        self.keyframes.sort_by_key(|k| k.at);
    }

    /// Keyframes from clip time `t` on, moved to start at zero, plus one at
    /// zero when there were any before it.
    pub fn keep_from(&mut self, t: Time) {
        if self.keyframes.iter().any(|k| k.at < t) && !self.keyframes.iter().any(|k| k.at == t) {
            let ease = self
                .keyframes
                .iter()
                .filter(|k| k.at < t)
                .max_by_key(|k| k.at)
                .map(|k| k.ease);
            self.keyframes.push(MaskKeyframe {
                at: t,
                state: self.state_at(t),
                ease: ease.unwrap_or_default(),
            });
        }
        self.keyframes.retain(|k| k.at >= t);
        for k in &mut self.keyframes {
            k.at = k.at - t;
        }
        self.keyframes.sort_by_key(|k| k.at);
    }

    /// Stretch keyframe times by `factor` (speed changes).
    pub fn scale_keyframes(&mut self, factor: f64) {
        for k in &mut self.keyframes {
            k.at = k.at.scale(factor);
        }
    }
}

fn lerp(a: f32, b: f32, k: f32) -> f32 {
    a + (b - a) * k
}

fn lerp2(a: [f32; 2], b: [f32; 2], k: f32) -> [f32; 2] {
    [lerp(a[0], b[0], k), lerp(a[1], b[1], k)]
}

impl MaskState {
    /// `self` → `other` at `k` (0..1); the shape holds when it can't move
    /// between the two.
    pub fn lerp(&self, other: &MaskState, k: f32) -> MaskState {
        use MaskShape::*;
        let shape = match (&self.shape, &other.shape) {
            (
                Rectangle {
                    center: c0,
                    size: s0,
                    rotation_deg: r0,
                    roundness: o0,
                },
                Rectangle {
                    center: c1,
                    size: s1,
                    rotation_deg: r1,
                    roundness: o1,
                },
            ) => Rectangle {
                center: lerp2(*c0, *c1, k),
                size: lerp2(*s0, *s1, k),
                rotation_deg: lerp(*r0, *r1, k),
                roundness: lerp(*o0, *o1, k),
            },
            (
                Ellipse {
                    center: c0,
                    size: s0,
                    rotation_deg: r0,
                },
                Ellipse {
                    center: c1,
                    size: s1,
                    rotation_deg: r1,
                },
            ) => Ellipse {
                center: lerp2(*c0, *c1, k),
                size: lerp2(*s0, *s1, k),
                rotation_deg: lerp(*r0, *r1, k),
            },
            (Path { points: p0, smooth }, Path { points: p1, .. }) if p0.len() == p1.len() => {
                Path {
                    points: p0.iter().zip(p1).map(|(a, b)| lerp2(*a, *b, k)).collect(),
                    smooth: *smooth,
                }
            }
            (a, _) => a.clone(),
        };
        MaskState {
            shape,
            feather: lerp(self.feather, other.feather, k),
            expansion: lerp(self.expansion, other.expansion, k),
            opacity: lerp(self.opacity, other.opacity, k),
        }
    }
}

impl MaskShape {
    /// The shape moved by `delta` (uv).
    pub fn moved(&self, delta: [f32; 2]) -> MaskShape {
        let add = |p: [f32; 2]| [p[0] + delta[0], p[1] + delta[1]];
        let mut out = self.clone();
        match &mut out {
            MaskShape::Rectangle { center, .. } | MaskShape::Ellipse { center, .. } => {
                *center = add(*center)
            }
            MaskShape::Path { points, .. } => {
                for p in points {
                    *p = add(*p);
                }
            }
            MaskShape::Subject => {}
        }
        out
    }

    /// Its outline as a closed polyline in uv, for a picture `aspect`
    /// (width / height) wide; none for a subject (see [`Matte::outlines`]).
    pub fn outline(&self, aspect: f32) -> Vec<[f32; 2]> {
        let aspect = aspect.max(1e-3);
        let to_uv = |p: [f32; 2]| [p[0] / aspect, p[1]];
        self.square_outline(aspect).into_iter().map(to_uv).collect()
    }

    /// The outline in square units: x = u × aspect, y = v.
    fn square_outline(&self, aspect: f32) -> Vec<[f32; 2]> {
        match self {
            MaskShape::Rectangle {
                center,
                size,
                rotation_deg,
                roundness,
            } => {
                let (hw, hh) = (size[0].abs() * aspect / 2.0, size[1].abs() / 2.0);
                let r = roundness.clamp(0.0, 1.0) * hw.min(hh);
                let mut local = Vec::new();
                let corners = [
                    (hw - r, -hh + r, -90.0f32),
                    (hw - r, hh - r, 0.0),
                    (-hw + r, hh - r, 90.0),
                    (-hw + r, -hh + r, 180.0),
                ];
                for (cx, cy, from) in corners {
                    if r <= 1e-6 {
                        local.push([cx, cy]);
                        continue;
                    }
                    for i in 0..=SPAN_STEPS {
                        let a = (from + 90.0 * i as f32 / SPAN_STEPS as f32).to_radians();
                        local.push([cx + r * a.cos(), cy + r * a.sin()]);
                    }
                }
                // Clockwise from the top-left corner, like the box handles.
                let n = local.len();
                local.rotate_left(n - n / 4);
                turned(&local, [center[0] * aspect, center[1]], *rotation_deg)
            }
            MaskShape::Ellipse {
                center,
                size,
                rotation_deg,
            } => {
                let (a, b) = (size[0].abs() * aspect / 2.0, size[1].abs() / 2.0);
                let local: Vec<[f32; 2]> = (0..ELLIPSE_STEPS)
                    .map(|i| {
                        let t = std::f32::consts::TAU * i as f32 / ELLIPSE_STEPS as f32;
                        [a * t.cos(), b * t.sin()]
                    })
                    .collect();
                turned(&local, [center[0] * aspect, center[1]], *rotation_deg)
            }
            MaskShape::Path { points, smooth } => {
                let pts: Vec<[f32; 2]> = points.iter().map(|p| [p[0] * aspect, p[1]]).collect();
                if *smooth && pts.len() >= 3 {
                    catmull_rom(&pts)
                } else {
                    pts
                }
            }
            MaskShape::Subject => Vec::new(),
        }
    }
}

/// `local` points turned clockwise by `deg` and moved to `center`.
fn turned(local: &[[f32; 2]], center: [f32; 2], deg: f32) -> Vec<[f32; 2]> {
    let (s, c) = deg.to_radians().sin_cos();
    local
        .iter()
        .map(|p| {
            [
                center[0] + c * p[0] - s * p[1],
                center[1] + s * p[0] + c * p[1],
            ]
        })
        .collect()
}

/// A closed Catmull-Rom curve through `pts`.
fn catmull_rom(pts: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let n = pts.len();
    let mut out = Vec::with_capacity(n * SPAN_STEPS);
    for i in 0..n {
        let p0 = pts[(i + n - 1) % n];
        let p1 = pts[i];
        let p2 = pts[(i + 1) % n];
        let p3 = pts[(i + 2) % n];
        for s in 0..SPAN_STEPS {
            let t = s as f32 / SPAN_STEPS as f32;
            let (t2, t3) = (t * t, t * t * t);
            let f = |a: f32, b: f32, c: f32, d: f32| {
                0.5 * (2.0 * b
                    + (-a + c) * t
                    + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2
                    + (-a + 3.0 * b - 3.0 * c + d) * t3)
            };
            out.push([f(p0[0], p1[0], p2[0], p3[0]), f(p0[1], p1[1], p2[1], p3[1])]);
        }
    }
    out
}

/// A mask ready to measure distances to, in square units.
struct Prepared {
    shape: Distance,
    /// Where the distance can be under `reach`: x0, y0, x1, y1.
    bounds: [f32; 4],
    expansion: f32,
    feather: f32,
    invert: bool,
    opacity: f32,
    mode: MaskMode,
}

enum Distance {
    /// Centre, half size, corner radius, clockwise turn as (sin, cos).
    Rect([f32; 2], [f32; 2], f32, (f32, f32)),
    /// Centre, semi-axes, turn.
    Ellipse([f32; 2], [f32; 2], (f32, f32)),
    Polygon(Vec<[f32; 2]>),
    /// A subject's matte drawn on the raster, a value per pixel (empty
    /// when only its bounds are wanted).
    Field(Vec<f32>, FieldKind),
}

impl Distance {
    /// Signed distance from `p`: negative inside.
    fn at(&self, p: [f32; 2]) -> f32 {
        let local = |c: [f32; 2], (s, co): (f32, f32)| {
            let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
            [co * dx + s * dy, -s * dx + co * dy]
        };
        match self {
            Distance::Rect(c, half, r, turn) => {
                let q = local(*c, *turn);
                let qx = q[0].abs() - half[0] + r;
                let qy = q[1].abs() - half[1] + r;
                qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
            }
            Distance::Ellipse(c, ab, turn) => {
                let q = local(*c, *turn);
                let (a, b) = (ab[0].max(1e-6), ab[1].max(1e-6));
                let k1 = (q[0] / a).hypot(q[1] / b);
                let k2 = (q[0] / (a * a)).hypot(q[1] / (b * b));
                if k2 < 1e-9 {
                    -a.min(b)
                } else {
                    k1 * (k1 - 1.0) / k2
                }
            }
            Distance::Polygon(v) => polygon_distance(v, p),
            Distance::Field(..) => f32::MAX,
        }
    }
}

/// Signed distance from `p` to the polygon `v` (even-odd inside).
fn polygon_distance(v: &[[f32; 2]], p: [f32; 2]) -> f32 {
    let n = v.len();
    if n < 3 {
        return f32::MAX;
    }
    let mut d = f32::MAX;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (v[i], v[j]);
        let e = [b[0] - a[0], b[1] - a[1]];
        let w = [p[0] - a[0], p[1] - a[1]];
        let len = e[0] * e[0] + e[1] * e[1];
        let t = if len > 0.0 {
            ((w[0] * e[0] + w[1] * e[1]) / len).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = [w[0] - e[0] * t, w[1] - e[1] * t];
        d = d.min(q[0] * q[0] + q[1] * q[1]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) * e[0] / e[1] {
            inside = !inside;
        }
        j = i;
    }
    let d = d.sqrt();
    if inside { -d } else { d }
}

/// What subject masks are drawn from at a time: each one's matte, and the
/// picture whose edges they refine onto.
#[derive(Clone, Copy, Debug, Default)]
pub struct SubjectMattes<'a> {
    pub mattes: &'a [(MaskId, Matte)],
    pub picture: Option<&'a Frame>,
}

impl SubjectMattes<'_> {
    fn get(&self, mask: MaskId) -> Option<&Matte> {
        self.mattes
            .iter()
            .find(|(id, _)| *id == mask)
            .map(|(_, m)| m)
    }
}

/// `mask` at `t` ready to measure; drawn on a `raster` (width, height)
/// when given, else only its bounds are wanted. `None` for a subject
/// without its matte.
fn prepare(
    mask: &Mask,
    t: Time,
    aspect: f32,
    mattes: &SubjectMattes,
    raster: Option<(u32, u32)>,
) -> Option<Prepared> {
    let state = mask.state_at(t);
    let turn = |deg: f32| deg.to_radians().sin_cos();
    let feather = state.feather.max(0.0);
    let reach = state.expansion.max(0.0) + feather;
    if state.shape == MaskShape::Subject {
        let subject = mask.subject.as_ref()?;
        let matte = mattes.get(mask.id)?;
        let [u0, v0, u1, v1] = matte.bounds().unwrap_or([0.5, 0.5, 0.5, 0.5]);
        let kind = if feather > 0.0 || state.expansion != 0.0 {
            FieldKind::Distance
        } else {
            FieldKind::Coverage
        };
        let picture = mattes.picture.filter(|_| subject.refine);
        let field = raster.map_or_else(Vec::new, |(w, h)| matte.field(w, h, kind, picture));
        return Some(Prepared {
            shape: Distance::Field(field, kind),
            bounds: [
                u0 * aspect - reach,
                v0 - reach,
                u1 * aspect + reach,
                v1 + reach,
            ],
            expansion: state.expansion,
            feather,
            invert: mask.invert,
            opacity: state.opacity.clamp(0.0, 1.0),
            mode: mask.mode,
        });
    }
    let outline = state.shape.square_outline(aspect);
    let shape = match &state.shape {
        MaskShape::Rectangle {
            center,
            size,
            rotation_deg,
            roundness,
        } => {
            let half = [size[0].abs() * aspect / 2.0, size[1].abs() / 2.0];
            let r = roundness.clamp(0.0, 1.0) * half[0].min(half[1]);
            Distance::Rect(
                [center[0] * aspect, center[1]],
                half,
                r,
                turn(*rotation_deg),
            )
        }
        MaskShape::Ellipse {
            center,
            size,
            rotation_deg,
        } => Distance::Ellipse(
            [center[0] * aspect, center[1]],
            [size[0].abs() * aspect / 2.0, size[1].abs() / 2.0],
            turn(*rotation_deg),
        ),
        MaskShape::Path { .. } | MaskShape::Subject => Distance::Polygon(outline.clone()),
    };
    let bounds = outline.iter().fold(
        [f32::MAX, f32::MAX, f32::MIN, f32::MIN],
        |[x0, y0, x1, y1], p| [x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1])],
    );
    Some(Prepared {
        shape,
        bounds: [
            bounds[0] - reach,
            bounds[1] - reach,
            bounds[2] + reach,
            bounds[3] + reach,
        ],
        expansion: state.expansion,
        feather,
        invert: mask.invert,
        opacity: state.opacity.clamp(0.0, 1.0),
        mode: mask.mode,
    })
}

impl Prepared {
    /// Coverage at square point `p`, raster pixel `index`, softened over
    /// at least `pixel`.
    fn coverage(&self, p: [f32; 2], index: usize, pixel: f32) -> f32 {
        let soft = self.feather.max(pixel);
        let smooth = |d: f32| {
            let c = (0.5 - (d - self.expansion) / soft).clamp(0.0, 1.0);
            c * c * (3.0 - 2.0 * c)
        };
        let [x0, y0, x1, y1] = self.bounds;
        let outside = p[0] < x0 - soft || p[0] > x1 + soft || p[1] < y0 - soft || p[1] > y1 + soft;
        let c = match &self.shape {
            Distance::Field(v, FieldKind::Coverage) => v.get(index).copied().unwrap_or(0.0),
            Distance::Field(v, FieldKind::Distance) => v.get(index).map_or(0.0, |d| smooth(*d)),
            _ if outside => 0.0,
            shape => smooth(shape.at(p)),
        };
        let c = if self.invert { 1.0 - c } else { c };
        c * self.opacity
    }
}

/// Whether `masks` change anything: at least one is enabled.
pub fn any_enabled(masks: &[Mask]) -> bool {
    masks.iter().any(|m| m.enabled)
}

/// How much of the picture `masks` keep at clip-relative time `t`, as a
/// `width` × `height` straight RGBA8 frame of white with that alpha, for a
/// picture `aspect` (width / height) wide, subjects drawn from `mattes`.
/// `None` when no mask is enabled (or has its matte).
pub fn raster(
    masks: &[Mask],
    t: Time,
    aspect: f32,
    width: u32,
    height: u32,
    mattes: &SubjectMattes,
) -> Option<Frame> {
    let aspect = aspect.max(1e-3);
    let (w, h) = (width.max(1), height.max(1));
    let prepared: Vec<Prepared> = masks
        .iter()
        .filter(|m| m.enabled)
        .filter_map(|m| prepare(m, t, aspect, mattes, Some((w, h))))
        .collect();
    let first = prepared.first()?;
    let start = if first.mode == MaskMode::Add {
        0.0
    } else {
        1.0
    };
    let (w, h) = (w as usize, h as usize);
    let pixel = 1.0 / h as f32;
    let mut rgba = vec![255u8; w * h * 4];
    let draw = |y0: usize, rows: &mut [u8]| {
        for (i, line) in rows.chunks_mut(w * 4).enumerate() {
            let v = (y0 + i) as f32 / h as f32 + 0.5 / h as f32;
            let row = (y0 + i) * w;
            for (x, px) in line.chunks_mut(4).enumerate() {
                let u = (x as f32 + 0.5) / w as f32;
                let p = [u * aspect, v];
                let mut acc = start;
                for m in &prepared {
                    let c = m.coverage(p, row + x, pixel);
                    acc = match m.mode {
                        MaskMode::Add => acc + c * (1.0 - acc),
                        MaskMode::Subtract => acc * (1.0 - c),
                        MaskMode::Intersect => acc * c,
                    };
                }
                px[3] = (acc.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    };
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(16);
    if threads == 1 || w * h < 65_536 {
        draw(0, &mut rgba);
    } else {
        let rows_per = h.div_ceil(threads);
        std::thread::scope(|scope| {
            for (i, rows) in rgba.chunks_mut(rows_per * w * 4).enumerate() {
                let draw = &draw;
                scope.spawn(move || draw(i * rows_per, rows));
            }
        });
    }
    Some(Frame {
        width: w as u32,
        height: h as u32,
        rgba: Arc::from(rgba),
    })
}

/// The raster size for a `picture`-sized picture: its own, at most
/// [`MAX_RASTER`] on the longer side.
pub fn raster_size(picture: (u32, u32)) -> (u32, u32) {
    let (w, h) = (picture.0.max(1), picture.1.max(1));
    let k = (MAX_RASTER as f32 / w.max(h) as f32).min(1.0);
    (
        ((w as f32 * k).round() as u32).max(1),
        ((h as f32 * k).round() as u32).max(1),
    )
}

/// The part of the picture `masks` can leave visible at `t`, as `[u0, v0,
/// u1, v1]`, when they only add uninverted shapes (subjects by their
/// `mattes`); `None` when they may leave anything (or there are none).
pub fn kept_bounds(
    masks: &[Mask],
    t: Time,
    aspect: f32,
    mattes: &SubjectMattes,
) -> Option<[f32; 4]> {
    let aspect = aspect.max(1e-3);
    let enabled: Vec<&Mask> = masks.iter().filter(|m| m.enabled).collect();
    if enabled.is_empty() || enabled.iter().any(|m| m.invert || m.mode != MaskMode::Add) {
        return None;
    }
    enabled
        .iter()
        .map(|m| {
            let [x0, y0, x1, y1] = prepare(m, t, aspect, mattes, None)?.bounds;
            Some([x0 / aspect, y0, x1 / aspect, y1])
        })
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .reduce(|a, b| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        })
}

/// `picture`'s alpha multiplied by `mask`'s, sampled bilinearly at the
/// same uv (the CPU reference for `FxBatch::mask`; `koo-gpu` matches it).
pub fn apply(picture: &Frame, mask: &Frame) -> Frame {
    let (pw, ph) = (picture.width as usize, picture.height as usize);
    let (mw, mh) = (mask.width.max(1) as i32, mask.height.max(1) as i32);
    let alpha = |x: i32, y: i32| -> f32 {
        let (x, y) = (x.clamp(0, mw - 1) as usize, y.clamp(0, mh - 1) as usize);
        mask.rgba[(y * mw as usize + x) * 4 + 3] as f32 / 255.0
    };
    let mut rgba = picture.rgba.to_vec();
    for y in 0..ph {
        let ty = (y as f32 + 0.5) / ph as f32 * mh as f32 - 0.5;
        let by = ty.floor();
        let ay = ty - by;
        for x in 0..pw {
            let tx = (x as f32 + 0.5) / pw as f32 * mw as f32 - 0.5;
            let bx = tx.floor();
            let ax = tx - bx;
            let (bx, by) = (bx as i32, by as i32);
            let top = lerp(alpha(bx, by), alpha(bx + 1, by), ax);
            let bottom = lerp(alpha(bx, by + 1), alpha(bx + 1, by + 1), ax);
            let m = lerp(top, bottom, ay);
            let i = (y * pw + x) * 4 + 3;
            rgba[i] = ((rgba[i] as f32 / 255.0 * m).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    Frame {
        rgba: Arc::from(rgba),
        ..*picture
    }
}
