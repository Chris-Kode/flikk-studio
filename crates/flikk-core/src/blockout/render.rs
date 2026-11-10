//! Drawing a scene: one ray a pixel against a handful of solids, lit by
//! one light with a shadow, so depth, contact and the camera's move read
//! at a glance. Greys only: the look is the video model's to make.

use super::{Object, Scene, Shape};
use crate::services::Frame;
use std::ops::{Add, Mul, Sub};

/// Pictures smaller than this are drawn on the calling thread.
const THREADED_PIXELS: usize = 32_768;
/// How near a hit counts, so a surface doesn't shadow itself.
const NEAR: f32 = 1e-3;
/// Where the light comes from: above, to the right, toward the viewer.
const LIGHT: Vec3 = Vec3::new(0.42, 0.82, 0.39);
const GROUND_GREY: f32 = 0.5;
const SKY_HORIZON: f32 = 0.76;
const SKY_ZENITH: f32 = 0.9;
/// Metres at which the haze has taken about two thirds of a surface.
const HAZE: f32 = 140.0;

impl Scene {
    /// The scene at `t` seconds, `width` × `height` pixels, opaque.
    pub fn render(&self, t: f64, width: u32, height: u32) -> Frame {
        let (width, height) = (width.max(1), height.max(1));
        let world = World::of(self, t.clamp(0.0, self.duration), width, height);
        let row = width as usize * 4;
        let mut rgba = vec![0u8; row * height as usize];
        let draw = |first: usize, rows: &mut [u8]| {
            for (y, line) in rows.chunks_exact_mut(row).enumerate() {
                for (x, pixel) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let grey = world.grey(x as f32 + 0.5, (first + y) as f32 + 0.5);
                    let value = (grey.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    *pixel = [value, value, value, 255];
                }
            }
        };
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
        if threads == 1 || rgba.len() / 4 < THREADED_PIXELS {
            draw(0, &mut rgba);
        } else {
            let band = (height as usize).div_ceil(threads);
            std::thread::scope(|scope| {
                for (i, rows) in rgba.chunks_mut(band * row).enumerate() {
                    let draw = &draw;
                    scope.spawn(move || draw(i * band, rows));
                }
            });
        }
        Frame {
            width,
            height,
            rgba: rgba.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

impl Vec3 {
    const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    fn of(v: [f64; 3]) -> Self {
        Self::new(v[0] as f32, v[1] as f32, v[2] as f32)
    }

    fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    /// One unit long; `fallback` when it has no length.
    fn unit_or(self, fallback: Self) -> Self {
        let length = self.length();
        if length > 1e-6 {
            self * (1.0 / length)
        } else {
            fallback
        }
    }

    /// Each part over the same part of `o`.
    fn over(self, o: Self) -> Self {
        Self::new(self.x / o.x, self.y / o.y, self.z / o.z)
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}

/// How a solid is turned: its own axes, in the world.
#[derive(Clone, Copy)]
struct Turn {
    x: Vec3,
    y: Vec3,
    z: Vec3,
}

impl Turn {
    /// From turns in degrees about X, Y and Z: rolled about Z, tipped
    /// about X, then turned about the vertical.
    fn of(rotate: [f64; 3]) -> Self {
        let [(sx, cx), (sy, cy), (sz, cz)] = rotate.map(|d| (d.to_radians() as f32).sin_cos());
        Self {
            x: Vec3::new(cy * cz + sy * sx * sz, cx * sz, -sy * cz + cy * sx * sz),
            y: Vec3::new(-cy * sz + sy * sx * cz, cx * cz, sy * sz + cy * sx * cz),
            z: Vec3::new(sy * cx, -sx, cy * cx),
        }
    }

    /// A direction of the solid's own, in the world.
    fn outward(&self, v: Vec3) -> Vec3 {
        self.x * v.x + self.y * v.y + self.z * v.z
    }

    /// A direction of the world, in the solid's own axes.
    fn inward(&self, v: Vec3) -> Vec3 {
        Vec3::new(self.x.dot(v), self.y.dot(v), self.z.dot(v))
    }
}

#[derive(Clone, Copy)]
enum Form {
    Box,
    /// A sphere stretched to its half size.
    Ellipsoid,
    Cylinder,
    /// Its point up.
    Cone,
    /// Upright, as thick as its width.
    Capsule,
}

/// One convex piece of an object, placed in the world.
struct Solid {
    form: Form,
    centre: Vec3,
    turn: Turn,
    /// Half its width, height and depth.
    half: Vec3,
    /// A sphere about `centre` holding it, for a quick miss.
    reach: f32,
    shade: f32,
}

impl Solid {
    fn new(form: Form, centre: Vec3, turn: Turn, half: Vec3, shade: f32) -> Self {
        Self {
            form,
            centre,
            turn,
            half,
            reach: half.length(),
            shade,
        }
    }

    /// Where the ray from `origin` along `dir` (one unit long) first meets
    /// it: how far, and the surface's direction there.
    fn hit(&self, origin: Vec3, dir: Vec3) -> Option<(f32, Vec3)> {
        let to = self.centre - origin;
        let along = to.dot(dir);
        if along < -self.reach || to.dot(to) - along * along > self.reach * self.reach {
            return None;
        }
        let (o, d) = (
            self.turn.inward(origin - self.centre),
            self.turn.inward(dir),
        );
        let (t, normal) = match self.form {
            Form::Box => hit_box(o, d, self.half),
            Form::Ellipsoid => hit_ellipsoid(o, d, self.half),
            Form::Cylinder => hit_cylinder(o, d, self.half),
            Form::Cone => hit_cone(o, d, self.half),
            Form::Capsule => hit_capsule(o, d, self.half),
        }?;
        Some((
            t,
            self.turn.outward(normal).unit_or(Vec3::new(0.0, 1.0, 0.0)),
        ))
    }
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

/// The pieces `object` is made of at `t`.
fn add_pieces(object: &Object, t: f64, out: &mut Vec<Solid>) {
    let size = Vec3::of(object.size_now(t));
    let base = Vec3::of(object.at_now(t));
    let turn = Turn::of(object.rotate_now(t));
    let shade = object.shade as f32;
    // A piece `half` its size, its middle at `up` above the base and
    // `forward` of it.
    let mut piece = |form, half: Vec3, up: f32, forward: f32, shade: f32| {
        let centre = base + turn.outward(Vec3::new(0.0, up, forward));
        out.push(Solid::new(form, centre, turn, half, shade));
    };
    let half = size * 0.5;
    match object.shape {
        Shape::Box => piece(Form::Box, half, half.y, 0.0, shade),
        Shape::Sphere => piece(Form::Ellipsoid, half, half.y, 0.0, shade),
        Shape::Cylinder => piece(Form::Cylinder, half, half.y, 0.0, shade),
        Shape::Cone => piece(Form::Cone, half, half.y, 0.0, shade),
        Shape::Capsule => piece(Form::Capsule, half, half.y, 0.0, shade),
        Shape::Figure => {
            let height = size.y;
            let head = height * 0.11;
            let body = Vec3::new(half.x * 0.8, height * 0.42, half.z * 0.8);
            piece(Form::Capsule, body, body.y, 0.0, shade);
            piece(
                Form::Ellipsoid,
                Vec3::new(head, head, head),
                height - head,
                0.0,
                shade,
            );
            // A visor, so the side it faces reads without limbs.
            let visor = Vec3::new(head * 0.7, head * 0.22, head * 0.2);
            piece(Form::Box, visor, height - head, head * 0.9, shade * 0.45);
        }
    }
}

/// What a ray met.
struct Hit {
    distance: f32,
    normal: Vec3,
    shade: f32,
}

/// A scene at one moment, seen through its camera.
struct World {
    solids: Vec<Solid>,
    ground: bool,
    eye: Vec3,
    forward: Vec3,
    /// Across the picture and down it, a pixel long at one unit ahead.
    right: Vec3,
    down: Vec3,
    /// The picture's middle, in pixels.
    middle: (f32, f32),
}

impl World {
    fn of(scene: &Scene, t: f64, width: u32, height: u32) -> Self {
        let mut solids = Vec::new();
        for object in &scene.objects {
            add_pieces(object, t, &mut solids);
        }
        let view = scene.view(t);
        let eye = Vec3::of(view.eye);
        let up = Vec3::new(0.0, 1.0, 0.0);
        let forward = (Vec3::of(view.target) - eye).unit_or(Vec3::new(0.0, 0.0, -1.0));
        // Looking straight up or down, any level direction is to the right.
        let right = forward.cross(up).unit_or(Vec3::new(1.0, 0.0, 0.0));
        let above = right.cross(forward);
        let longer = width.max(height) as f32;
        let pixel = 2.0 * (view.fov.to_radians() as f32 * 0.5).tan() / longer;
        Self {
            solids,
            ground: scene.ground,
            eye,
            forward,
            right: right * pixel,
            down: above * -pixel,
            middle: (width as f32 * 0.5, height as f32 * 0.5),
        }
    }

    /// The first thing the ray meets: a solid, else the ground.
    fn first(&self, origin: Vec3, dir: Vec3) -> Option<Hit> {
        let mut first: Option<Hit> = None;
        for solid in &self.solids {
            if let Some((distance, normal)) = solid.hit(origin, dir)
                && first.as_ref().is_none_or(|f| distance < f.distance)
            {
                first = Some(Hit {
                    distance,
                    normal,
                    shade: solid.shade,
                });
            }
        }
        let floor = (self.ground && dir.y < -1e-6 && origin.y > 0.0).then(|| -origin.y / dir.y);
        match floor {
            Some(distance) if first.as_ref().is_none_or(|f| distance < f.distance) => {
                let at = origin + dir * distance;
                Some(Hit {
                    distance,
                    normal: Vec3::new(0.0, 1.0, 0.0),
                    shade: floor_grey(at, distance),
                })
            }
            _ => first,
        }
    }

    fn shadowed(&self, at: Vec3) -> bool {
        self.solids.iter().any(|s| s.hit(at, LIGHT).is_some())
    }

    /// How light the picture is at pixel (`x`, `y`), 0..1.
    fn grey(&self, x: f32, y: f32) -> f32 {
        let dir = self.forward + self.right * (x - self.middle.0) + self.down * (y - self.middle.1);
        let dir = dir.unit_or(self.forward);
        let sky = match dir.y {
            up if up >= 0.0 => SKY_HORIZON + (SKY_ZENITH - SKY_HORIZON) * up.sqrt(),
            down => SKY_HORIZON - 0.12 * (-down).sqrt(),
        };
        let Some(hit) = self.first(self.eye, dir) else {
            return sky;
        };
        let at = self.eye + dir * hit.distance;
        let facing = hit.normal.dot(LIGHT).max(0.0);
        let lit = if facing > 0.0 && self.shadowed(at + hit.normal * (NEAR * 4.0)) {
            0.0
        } else {
            facing
        };
        // Its own grey under the light, the sky's fill from above, and a
        // darker rim where it turns away from the camera.
        let fill = 0.5 + 0.5 * hit.normal.y;
        let rim = 0.86 + 0.14 * hit.normal.dot(dir).abs();
        let surface = hit.shade * (0.34 + 0.56 * lit + 0.14 * fill) * rim;
        let haze = 1.0 - (-hit.distance / HAZE).exp();
        surface + (SKY_HORIZON - surface) * haze
    }
}

/// The ground's grey at `at`: metre tiles a touch lighter and darker, so
/// a camera move reads against it, fading out with `distance` before they
/// shimmer.
fn floor_grey(at: Vec3, distance: f32) -> f32 {
    let odd = ((at.x.floor() as i64 + at.z.floor() as i64) & 1) as f32;
    let fade = (-distance / 30.0).exp();
    GROUND_GREY + (odd - 0.5) * 0.07 * fade
}
