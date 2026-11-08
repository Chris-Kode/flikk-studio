//! Where a picture lands on a canvas: built from a clip's transform
//! ([`placement`], [`placement_from`]) and mapped between the picture and
//! the canvas, corner pin included ([`Placement`]).

use crate::model::{BlendMode, Transform};

/// Steepest slant drawn, in degrees; beyond it a picture is nearly flat.
pub const MAX_SKEW: f32 = 80.0;

/// Where a `picture`-sized frame with transform `tf` lands on a `canvas`:
/// fitted to it by `tf.fit` and centred, then transformed
/// ([`placement_from`]), as in the export graph.
pub fn placement(tf: &Transform, picture: (u32, u32), canvas: (u32, u32)) -> Placement {
    let (pw, ph) = (picture.0.max(1) as f32, picture.1.max(1) as f32);
    let (cw, ch) = (canvas.0 as f32, canvas.1 as f32);
    let fit = tf.fit.scale(picture, canvas) as f32;
    placement_from(tf, [cw / 2.0, ch / 2.0], [pw * fit, ph * fit], canvas)
}

/// Where a picture resting at `center` with `size` (canvas pixels) lands
/// with transform `tf`: stretched and scaled, skewed and rotated clockwise
/// about its anchor, then moved by `position` × canvas size.
pub fn placement_from(
    tf: &Transform,
    center: [f32; 2],
    size: [f32; 2],
    canvas: (u32, u32),
) -> Placement {
    let (cw, ch) = (canvas.0 as f32, canvas.1 as f32);
    let s = tf.scale.max(0.0);
    let drawn = [
        size[0] * s * tf.stretch[0].max(0.0),
        size[1] * s * tf.stretch[1].max(0.0),
    ];
    let mut place = Placement {
        center: [0.0, 0.0],
        size: drawn,
        rotation_deg: tf.rotation_deg,
        skew_deg: tf.skew_deg.map(|d| d.clamp(-MAX_SKEW, MAX_SKEW)),
        crop: tf.crop.map(|c| c.clamp(0.0, 1.0)),
        flip: tf.flip,
        opacity: tf.opacity.clamp(0.0, 1.0),
        blend: tf.blend,
        pin: tf.corner_pin,
    };
    // The anchor stays where it rests (plus `position`); the centre is the
    // anchor's offset from it, scaled and turned with the picture.
    let [a, b, c, d] = place.linear();
    let rest = [tf.anchor[0] * size[0], tf.anchor[1] * size[1]];
    let (ax, ay) = (tf.anchor[0] * drawn[0], tf.anchor[1] * drawn[1]);
    place.center = [
        center[0] + rest[0] + tf.position[0] * cw - (a * ax + b * ay),
        center[1] + rest[1] + tf.position[1] * ch - (c * ax + d * ay),
    ];
    place
}

/// Where a picture lands on a canvas, in canvas pixels (y down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Centre of the picture.
    pub center: [f32; 2],
    /// Drawn size of the whole (uncropped) picture before skew and rotation.
    pub size: [f32; 2],
    /// Clockwise, around the centre.
    pub rotation_deg: f32,
    /// Slant before rotation, [x, y] degrees, as [`Transform::skew_deg`].
    ///
    /// [`Transform::skew_deg`]: crate::model::Transform::skew_deg
    pub skew_deg: [f32; 2],
    /// Fractions of the picture cut away, [left, top, right, bottom], in
    /// the picture's own (unflipped) coordinates.
    pub crop: [f32; 4],
    /// Mirror horizontally / vertically.
    pub flip: [bool; 2],
    pub opacity: f32,
    pub blend: BlendMode,
    /// How far each corner of the picture moves before the rest of the
    /// placement, [top-left, top-right, bottom-right, bottom-left], in its
    /// uv, as [`Transform::corner_pin`]; all zero = none. Pinned, the
    /// picture maps onto the canvas through a homography instead of an
    /// affine map.
    ///
    /// [`Transform::corner_pin`]: crate::model::Transform::corner_pin
    pub pin: [[f32; 2]; 4],
}

/// A 3×3 matrix, row-major, acting on `(x, y, 1)`.
pub type Matrix3 = [f32; 9];

fn mul3(a: &Matrix3, b: &Matrix3) -> Matrix3 {
    let mut out = [0.0; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] = (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum();
        }
    }
    out
}

/// `m⁻¹`, if `m` is not singular.
fn invert3(m: &Matrix3) -> Option<Matrix3> {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let co = [
        e * i - f * h,
        c * h - b * i,
        b * f - c * e,
        f * g - d * i,
        a * i - c * g,
        c * d - a * f,
        d * h - e * g,
        b * g - a * h,
        a * e - b * d,
    ];
    let det = a * co[0] + b * co[3] + c * co[6];
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    Some(co.map(|v| v / det))
}

/// `m` applied to `p`, divided through; `None` behind the projection.
fn project(m: &Matrix3, p: [f32; 2]) -> Option<[f32; 2]> {
    let w = m[6] * p[0] + m[7] * p[1] + m[8];
    if w <= 1e-9 {
        return None;
    }
    Some([
        (m[0] * p[0] + m[1] * p[1] + m[2]) / w,
        (m[3] * p[0] + m[4] * p[1] + m[5]) / w,
    ])
}

/// The homography taking the unit square's corners (top-left, top-right,
/// bottom-right, bottom-left) to `quad`'s (Heckbert's square-to-quad).
fn square_to_quad(quad: [[f32; 2]; 4]) -> Option<Matrix3> {
    let [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] = quad;
    let (sx, sy) = (x0 - x1 + x2 - x3, y0 - y1 + y2 - y3);
    let (g, h) = if sx.abs() < 1e-9 && sy.abs() < 1e-9 {
        (0.0, 0.0)
    } else {
        let (dx1, dx2, dy1, dy2) = (x1 - x2, x3 - x2, y1 - y2, y3 - y2);
        let den = dx1 * dy2 - dx2 * dy1;
        if den.abs() < 1e-12 {
            return None;
        }
        ((sx * dy2 - dx2 * sy) / den, (dx1 * sy - sx * dy1) / den)
    };
    Some([
        x1 - x0 + g * x1,
        x3 - x0 + h * x3,
        x0,
        y1 - y0 + g * y1,
        y3 - y0 + h * y3,
        y0,
        g,
        h,
        1.0,
    ])
}

impl Placement {
    /// The picture stretched over a whole `width` × `height` canvas.
    pub fn fill(width: u32, height: u32) -> Self {
        Self {
            center: [width as f32 / 2.0, height as f32 / 2.0],
            size: [width as f32, height as f32],
            rotation_deg: 0.0,
            skew_deg: [0.0; 2],
            crop: [0.0; 4],
            flip: [false; 2],
            opacity: 1.0,
            blend: BlendMode::Normal,
            pin: [[0.0; 2]; 4],
        }
    }

    /// Whether any corner is pinned.
    pub fn is_pinned(&self) -> bool {
        self.pin.iter().flatten().any(|v| v.abs() > 1e-6)
    }

    /// Rotation after skew, `[a, b, c, d]`: a picture point `(x, y)` pixels
    /// from its centre lands at `center + (a·x + b·y, c·x + d·y)`.
    pub fn linear(&self) -> [f32; 4] {
        let (s, c) = self.rotation_deg.to_radians().sin_cos();
        let [kx, ky] = self.skew_deg.map(|d| d.to_radians().tan());
        [c - s * ky, c * kx - s, s + c * ky, s * kx + c]
    }

    /// Canvas point of `uv`, 0..1 across the whole picture.
    pub fn to_canvas(&self, uv: [f32; 2]) -> [f32; 2] {
        if self.is_pinned()
            && let Some(m) = self.homography()
            && let Some(p) = project(&m, uv)
        {
            return p;
        }
        self.unpinned_to_canvas(uv)
    }

    /// Where `uv` lands without the corner pin: the affine part of the
    /// placement, in which the pin's corners are measured.
    pub fn unpinned_to_canvas(&self, uv: [f32; 2]) -> [f32; 2] {
        let [a, b, c, d] = self.linear();
        let x = (uv[0] - 0.5) * self.size[0];
        let y = (uv[1] - 0.5) * self.size[1];
        [
            self.center[0] + a * x + b * y,
            self.center[1] + c * x + d * y,
        ]
    }

    /// Picture `uv` → canvas, the affine part only.
    fn affine(&self) -> Matrix3 {
        let [a, b, c, d] = self.linear();
        let [w, h] = self.size;
        let [cx, cy] = self.center;
        [
            a * w,
            b * h,
            cx - 0.5 * (a * w + b * h),
            c * w,
            d * h,
            cy - 0.5 * (c * w + d * h),
            0.0,
            0.0,
            1.0,
        ]
    }

    /// Picture `uv` → canvas, corner pin included. `None` when the pinned
    /// corners make no quad.
    pub fn homography(&self) -> Option<Matrix3> {
        let affine = self.affine();
        if !self.is_pinned() {
            return Some(affine);
        }
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let quad = std::array::from_fn(|i| {
            [
                corners[i][0] + self.pin[i][0],
                corners[i][1] + self.pin[i][1],
            ]
        });
        Some(mul3(&affine, &square_to_quad(quad)?))
    }

    /// Canvas → picture `uv`, corner pin included, as a 3×3 matrix: `uv =
    /// (r₀·q, r₁·q) / (r₂·q)` for `q = (x, y, 1)`, outside the picture
    /// where `r₂·q ≤ 0`. Unpinned, the last row is `(0, 0, 1)` and this is
    /// [`Self::inverse`]. `None` when the picture is flat.
    pub fn uv_map(&self) -> Option<Matrix3> {
        if !self.is_pinned() {
            let [a, b, c, d] = self.inverse()?;
            let [cx, cy] = self.center;
            return Some([
                a,
                b,
                0.5 - a * cx - b * cy,
                c,
                d,
                0.5 - c * cx - d * cy,
                0.0,
                0.0,
                1.0,
            ]);
        }
        self.inverse()?;
        invert3(&self.homography()?)
    }

    /// Canvas → picture `uv` (0..1 across the whole picture), as `[a, b, c,
    /// d]`: `uv = (a·dx + b·dy, c·dx + d·dy) + 0.5` for a canvas point `d`
    /// from `center`. `None` when the picture is flat.
    pub fn inverse(&self) -> Option<[f32; 4]> {
        let [a, b, c, d] = self.linear();
        let det = a * d - b * c;
        let [w, h] = self.size;
        if det.abs() < 1e-6 || w.abs() < 1e-6 || h.abs() < 1e-6 {
            return None;
        }
        Some([d / det / w, -b / det / w, -c / det / h, a / det / h])
    }

    /// Picture `uv` under canvas point `p`, if the picture is not flat.
    pub fn to_picture(&self, p: [f32; 2]) -> Option<[f32; 2]> {
        if self.is_pinned() {
            return project(&self.uv_map()?, p);
        }
        self.unpinned_to_picture(p)
    }

    /// [`Self::to_picture`] without the corner pin.
    pub fn unpinned_to_picture(&self, p: [f32; 2]) -> Option<[f32; 2]> {
        let [a, b, c, d] = self.inverse()?;
        let (dx, dy) = (p[0] - self.center[0], p[1] - self.center[1]);
        Some([a * dx + b * dy + 0.5, c * dx + d * dy + 0.5])
    }

    /// The whole picture's corners on the canvas: top-left, top-right,
    /// bottom-right, bottom-left.
    pub fn corners(&self) -> [[f32; 2]; 4] {
        [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|uv| self.to_canvas(uv))
    }
}
