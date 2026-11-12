//! The solids things are built from, placed in the scene: where a ray
//! meets one, the box that holds it, and its lowest point.

use super::space::{Place, Turn, Vec3};
use super::{Anchor, Shape};

/// How near a hit counts, so a surface doesn't meet itself.
pub(super) const NEAR: f32 = 1e-3;

impl Shape {
    /// Whether every side of it is flat.
    pub(super) fn is_flat(self) -> bool {
        matches!(self, Self::Box | Self::Wedge | Self::Roof | Self::Pyramid)
    }
}

/// One solid placed in the scene.
pub(super) struct Solid {
    pub shape: Shape,
    pub centre: Vec3,
    turn: Turn,
    half: Vec3,
    /// A sphere about `centre` holding it, for a quick miss.
    reach: f32,
    /// Its grey, and the thing a viewer would name it as (its place among
    /// the parts): what a picture of it needs. Nothing until given.
    pub shade: f32,
    pub owner: u16,
}

impl Solid {
    /// The solid of `shape` and `size` whose `anchor` stands at `place`.
    pub fn standing(shape: Shape, anchor: Anchor, place: Place, size: Vec3) -> Self {
        let half = size * 0.5;
        let above = match anchor {
            Anchor::Base => half.y,
            Anchor::Middle => 0.0,
            Anchor::Top => -half.y,
        };
        Self {
            shape,
            centre: place.point(Vec3::new(0.0, above, 0.0)),
            turn: place.turn,
            half,
            reach: half.length(),
            shade: 0.0,
            owner: 0,
        }
    }

    /// Its lowest point: where it would touch a floor.
    pub fn lowest(&self) -> Vec3 {
        let h = self.half;
        // Straight down, in its own axes.
        let d = self.turn.inward(-Vec3::UP);
        let sign = |n: f32| if n < 0.0 { -1.0 } else { 1.0 };
        // The point of an ellipse of radii `x` and `z` furthest along `d`.
        let rim = |x: f32, z: f32| {
            let far = (x * d.x).hypot(z * d.z);
            match far > 1e-9 {
                true => (x * x * d.x / far, z * z * d.z / far),
                false => (0.0, 0.0),
            }
        };
        let furthest = |corners: &[Vec3]| {
            let further = |a: Vec3, b: Vec3| if b.dot(d) > a.dot(d) { b } else { a };
            corners
                .iter()
                .copied()
                .reduce(further)
                .unwrap_or(Vec3::ZERO)
        };
        let base = |y: f32| {
            [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)]
                .map(|(x, z)| Vec3::new(x * h.x, y, z * h.z))
        };
        let own = match self.shape {
            Shape::Box => Vec3::new(sign(d.x) * h.x, sign(d.y) * h.y, sign(d.z) * h.z),
            Shape::Sphere => {
                let far = Vec3::new(h.x * d.x, h.y * d.y, h.z * d.z)
                    .length()
                    .max(1e-9);
                Vec3::new(h.x * h.x * d.x, h.y * h.y * d.y, h.z * h.z * d.z) * (1.0 / far)
            }
            Shape::Cylinder => {
                let (x, z) = rim(h.x, h.z);
                Vec3::new(x, sign(d.y) * h.y, z)
            }
            Shape::Cone => {
                let (x, z) = rim(h.x, h.z);
                furthest(&[Vec3::new(0.0, h.y, 0.0), Vec3::new(x, -h.y, z)])
            }
            Shape::Capsule => {
                let radius = h.x.min(h.z).min(h.y);
                Vec3::new(0.0, sign(d.y) * (h.y - radius), 0.0) + d * radius
            }
            Shape::Dome => {
                let (x, z) = rim(h.x, h.z);
                let round = Vec3::new(h.x * d.x, 2.0 * h.y * d.y, h.z * d.z);
                let far = round.length().max(1e-9);
                let top = Vec3::new(
                    h.x * round.x,
                    2.0 * h.y * round.y - h.y * far,
                    h.z * round.z,
                );
                match d.y > 0.0 {
                    true => furthest(&[top * (1.0 / far), Vec3::new(x, -h.y, z)]),
                    false => Vec3::new(x, -h.y, z),
                }
            }
            Shape::Wedge => {
                let [a, b, c, e] = base(-h.y);
                let back = [1.0, -1.0].map(|x| Vec3::new(x * h.x, h.y, -h.z));
                furthest(&[a, b, c, e, back[0], back[1]])
            }
            Shape::Roof => {
                let [a, b, c, e] = base(-h.y);
                let ridge = [1.0, -1.0].map(|z| Vec3::new(0.0, h.y, z * h.z));
                furthest(&[a, b, c, e, ridge[0], ridge[1]])
            }
            Shape::Pyramid => {
                let [a, b, c, e] = base(-h.y);
                furthest(&[a, b, c, e, Vec3::new(0.0, h.y, 0.0)])
            }
        };
        self.centre + self.turn.outward(own)
    }

    /// The corners of a box along the scene's axes that holds it.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let (x, y, z) = (self.turn.x.abs(), self.turn.y.abs(), self.turn.z.abs());
        let reach = x * self.half.x + y * self.half.y + z * self.half.z;
        (self.centre - reach, self.centre + reach)
    }

    /// The eight corners of that box.
    pub fn corners(&self) -> [Vec3; 8] {
        let (low, high) = self.bounds();
        std::array::from_fn(|i| {
            let pick = |bit: usize, low: f32, high: f32| if i >> bit & 1 == 0 { low } else { high };
            Vec3::new(
                pick(0, low.x, high.x),
                pick(1, low.y, high.y),
                pick(2, low.z, high.z),
            )
        })
    }

    /// Where the ray from `origin` along `dir` (one unit long) first meets
    /// it: how far, and the surface's direction there.
    pub fn hit(&self, origin: Vec3, dir: Vec3) -> Option<(f32, Vec3)> {
        let to = self.centre - origin;
        let along = to.dot(dir);
        if along < -self.reach || to.dot(to) - along * along > self.reach * self.reach {
            return None;
        }
        let (o, d) = (
            self.turn.inward(origin - self.centre),
            self.turn.inward(dir),
        );
        let h = self.half;
        let (t, normal) = match self.shape {
            Shape::Box => hit_box(o, d, h),
            Shape::Sphere => hit_ellipsoid(o, d, h),
            Shape::Cylinder => hit_cylinder(o, d, h),
            Shape::Cone => hit_cone(o, d, h),
            Shape::Capsule => hit_capsule(o, d, h),
            Shape::Dome => hit_dome(o, d, h),
            Shape::Wedge => {
                let slope = (Vec3::new(0.0, h.z, h.y), 0.0);
                hit_hull(
                    o,
                    d,
                    &[base(h), back(h), side(h, 1.0), side(h, -1.0), slope],
                )
            }
            Shape::Roof => {
                let faces = [base(h), back(h), front(h), pitch(h, 1.0), pitch(h, -1.0)];
                hit_hull(o, d, &faces)
            }
            Shape::Pyramid => {
                let ahead = (Vec3::new(0.0, h.z, 2.0 * h.y), h.z * h.y);
                let behind = (Vec3::new(0.0, h.z, -2.0 * h.y), h.z * h.y);
                hit_hull(
                    o,
                    d,
                    &[base(h), pitch(h, 1.0), pitch(h, -1.0), ahead, behind],
                )
            }
        }?;
        Some((t, self.turn.outward(normal).unit_or(Vec3::UP)))
    }
}

/// The faces of a solid of half size `h`, each as a direction out of it
/// and how far along that direction the face is.
type Face = (Vec3, f32);

fn base(h: Vec3) -> Face {
    (Vec3::new(0.0, -1.0, 0.0), h.y)
}

fn back(h: Vec3) -> Face {
    (Vec3::new(0.0, 0.0, -1.0), h.z)
}

fn front(h: Vec3) -> Face {
    (Vec3::AHEAD, h.z)
}

fn side(h: Vec3, sign: f32) -> Face {
    (Vec3::new(sign, 0.0, 0.0), h.x)
}

/// A face sloping from the top's middle down to the side at `sign`.
fn pitch(h: Vec3, sign: f32) -> Face {
    (Vec3::new(sign * 2.0 * h.y, h.x, 0.0), h.x * h.y)
}

/// Where a ray enters the solid inside all of `faces`.
fn hit_hull(o: Vec3, d: Vec3, faces: &[Face]) -> Option<(f32, Vec3)> {
    let (mut near, mut far) = (f32::NEG_INFINITY, f32::INFINITY);
    let mut normal = Vec3::ZERO;
    for (out, offset) in faces {
        let (toward, inside) = (out.dot(d), offset - out.dot(o));
        if toward.abs() < 1e-9 {
            if inside < 0.0 {
                return None;
            }
            continue;
        }
        let t = inside / toward;
        if toward < 0.0 {
            if t > near {
                (near, normal) = (t, *out);
            }
        } else {
            far = far.min(t);
        }
    }
    (near <= far && near > NEAR).then_some((near, normal))
}

/// The nearer root of `a·t² + 2·b·t + c = 0` past [`NEAR`], and the
/// farther one.
fn roots(a: f32, b: f32, c: f32) -> Option<(f32, f32)> {
    if a.abs() < 1e-9 {
        return None;
    }
    let disc = b * b - a * c;
    if disc < 0.0 {
        return None;
    }
    let root = disc.sqrt();
    let (t0, t1) = ((-b - root) / a, (-b + root) / a);
    Some(if t0 <= t1 { (t0, t1) } else { (t1, t0) })
}

fn hit_box(o: Vec3, d: Vec3, half: Vec3) -> Option<(f32, Vec3)> {
    let (mut near, mut far) = (f32::NEG_INFINITY, f32::INFINITY);
    let mut normal = Vec3::ZERO;
    let axes = [
        (o.x, d.x, half.x, Vec3::new(1.0, 0.0, 0.0)),
        (o.y, d.y, half.y, Vec3::new(0.0, 1.0, 0.0)),
        (o.z, d.z, half.z, Vec3::new(0.0, 0.0, 1.0)),
    ];
    for (o, d, half, axis) in axes {
        if d.abs() < 1e-9 {
            if o.abs() > half {
                return None;
            }
            continue;
        }
        let (a, b) = ((-half - o) / d, (half - o) / d);
        let (enter, leave) = if a < b { (a, b) } else { (b, a) };
        if enter > near {
            near = enter;
            normal = axis * -d.signum();
        }
        far = far.min(leave);
    }
    (near <= far && near > NEAR).then_some((near, normal))
}

fn hit_ellipsoid(o: Vec3, d: Vec3, half: Vec3) -> Option<(f32, Vec3)> {
    let (o, d) = (o.over(half), d.over(half));
    let (t, _) = roots(d.dot(d), o.dot(d), o.dot(o) - 1.0)?;
    (t > NEAR).then(|| (t, (o + d * t).over(half)))
}

/// The upper half of an ellipsoid twice as high as `half`, standing on
/// the underside of its box.
fn hit_dome(o: Vec3, d: Vec3, half: Vec3) -> Option<(f32, Vec3)> {
    let o = Vec3::new(o.x, o.y + half.y, o.z);
    let radii = Vec3::new(half.x, half.y * 2.0, half.z);
    let (os, ds) = (o.over(radii), d.over(radii));
    let (enter, leave) = roots(ds.dot(ds), os.dot(ds), os.dot(os) - 1.0)?;
    if enter > NEAR && o.y + d.y * enter >= 0.0 {
        let at = os + ds * enter;
        return Some((enter, at.over(radii)));
    }
    // Under the round part: in through the flat underside, if at all.
    let under = -o.y / d.y;
    (d.y > 1e-9 && under > NEAR && under >= enter && under <= leave)
        .then_some((under, Vec3::new(0.0, -1.0, 0.0)))
}

fn hit_cylinder(o: Vec3, d: Vec3, half: Vec3) -> Option<(f32, Vec3)> {
    let (ox, oz, dx, dz) = (o.x / half.x, o.z / half.z, d.x / half.x, d.z / half.z);
    let side = roots(
        dx * dx + dz * dz,
        ox * dx + oz * dz,
        ox * ox + oz * oz - 1.0,
    )
    .map(|(t, _)| t)
    .filter(|t| *t > NEAR && (o.y + d.y * t).abs() <= half.y)
    .map(|t| {
        let normal = Vec3::new((ox + dx * t) / half.x, 0.0, (oz + dz * t) / half.z);
        (t, normal)
    });
    // The end it comes in through, if it comes in through one.
    let end = (d.y.abs() > 1e-9)
        .then(|| {
            let y = -half.y * d.y.signum();
            ((y - o.y) / d.y, Vec3::new(0.0, y.signum(), 0.0))
        })
        .filter(|(t, _)| {
            let (x, z) = (ox + dx * t, oz + dz * t);
            *t > NEAR && x * x + z * z <= 1.0
        });
    nearer(side, end)
}

fn hit_cone(o: Vec3, d: Vec3, half: Vec3) -> Option<(f32, Vec3)> {
    let (ox, oz, dx, dz) = (o.x / half.x, o.z / half.z, d.x / half.x, d.z / half.z);
    // Its radius is 1 at the base and 0 at the point: `k` a metre down.
    let k = 1.0 / (2.0 * half.y);
    let k2 = k * k;
    let below = half.y - o.y;
    let (a, b, c) = (
        dx * dx + dz * dz - k2 * d.y * d.y,
        ox * dx + oz * dz + k2 * below * d.y,
        ox * ox + oz * oz - k2 * below * below,
    );
    let on_side = |t: &f32| *t > NEAR && (o.y + d.y * t).abs() <= half.y;
    let side = match roots(a, b, c) {
        Some((t0, t1)) => [t0, t1].into_iter().find(on_side),
        // Along its slope: it meets the side once.
        None if a.abs() < 1e-9 && b.abs() > 1e-9 => Some(-c / (2.0 * b)).filter(on_side),
        None => None,
    }
    .map(|t| {
        let (x, z) = (ox + dx * t, oz + dz * t);
        let down = half.y - (o.y + d.y * t);
        (t, Vec3::new(x / half.x, k2 * down, z / half.z))
    });
    let base = (d.y > 1e-9)
        .then(|| ((-half.y - o.y) / d.y, Vec3::new(0.0, -1.0, 0.0)))
        .filter(|(t, _)| {
            let (x, z) = (ox + dx * t, oz + dz * t);
            *t > NEAR && x * x + z * z <= 1.0
        });
    nearer(side, base)
}

fn hit_capsule(o: Vec3, d: Vec3, half: Vec3) -> Option<(f32, Vec3)> {
    let radius = half.x.min(half.z).min(half.y);
    // Its spine runs from one ball's middle to the other's.
    let reach = half.y - radius;
    let side = roots(d.x * d.x + d.z * d.z, o.x * d.x + o.z * d.z, {
        o.x * o.x + o.z * o.z - radius * radius
    })
    .map(|(t, _)| t)
    .filter(|t| *t > NEAR && (o.y + d.y * t).abs() <= reach)
    .map(|t| (t, Vec3::new(o.x + d.x * t, 0.0, o.z + d.z * t)));
    let ball = |y: f32| {
        let from = Vec3::new(o.x, o.y - y, o.z);
        let (t, _) = roots(1.0, from.dot(d), from.dot(from) - radius * radius)?;
        (t > NEAR).then(|| (t, from + d * t))
    };
    nearer(side, nearer(ball(reach), ball(-reach)))
}

fn nearer<T>(a: Option<(f32, T)>, b: Option<(f32, T)>) -> Option<(f32, T)> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}
