//! Where things are at a time: the values keys move ([`Track`]), each
//! object's place with what it rides and faces, how it travels (what a
//! walk is paced by), and the camera's view.

use super::space::{Place, Turn, Vec3, smoothstep};
use super::{Action, CAMERA, Ease, FPS, Object, PATH, Scene, Target};

/// Seconds an action takes to become the next.
const TAKE_UP: f64 = 0.45;
/// Seconds before it sets off again that a resting object turns to the
/// way it will go.
const TURN: f64 = 0.6;
/// How deep facing is followed: one object facing another that faces it.
const FACING_DEPTH: u8 = 2;

impl Ease {
    /// How far a move is when `u` (0..1) of its time has passed: 0 at the
    /// start, 1 at the end, outside that on the way for some.
    fn at(self, u: f64) -> f64 {
        match self {
            Self::Linear => u,
            Self::In => u * u,
            Self::Out => 1.0 - (1.0 - u) * (1.0 - u),
            Self::InOut => u * u * (3.0 - 2.0 * u),
            Self::Cut => 0.0,
            Self::Back => {
                let v = u - 1.0;
                1.0 + 2.70158 * v * v * v + 1.70158 * v * v
            }
            Self::Bounce => {
                let (n, d) = (7.5625, 2.75);
                let fall = |u: f64, rest: f64| n * u * u + rest;
                match u {
                    u if u < 1.0 / d => fall(u, 0.0),
                    u if u < 2.0 / d => fall(u - 1.5 / d, 0.75),
                    u if u < 2.5 / d => fall(u - 2.25 / d, 0.9375),
                    u => fall(u - 2.625 / d, 0.984375),
                }
            }
        }
    }
}

type Knot = (f64, [f64; 3], Ease);

/// A value its keys move: what it starts as, then its keys in order. A
/// key at the start replaces the start.
pub(super) struct Track {
    knots: Vec<Knot>,
}

impl Track {
    pub fn new(base: [f64; 3], keys: impl Iterator<Item = Knot>) -> Self {
        let mut knots = vec![(0.0, base, Ease::Linear)];
        for key in keys {
            if key.0 <= 0.0 {
                knots[0] = (0.0, key.1, Ease::Linear);
            } else {
                knots.push(key);
            }
        }
        Self { knots }
    }

    /// One number its keys move.
    fn of_number(base: f64, keys: impl Iterator<Item = (f64, f64, Ease)>) -> Self {
        Self::new(
            [base, 0.0, 0.0],
            keys.map(|(t, v, e)| (t, [v, 0.0, 0.0], e)),
        )
    }

    /// The two knots `t` is between, and how far the move from one to the
    /// other is, eased; `None` before the first and after the last.
    fn span(&self, t: f64) -> Option<(usize, f64)> {
        let next = self.knots.iter().position(|k| k.0 > t)?;
        let from = next.checked_sub(1)?;
        let (start, end) = (self.knots[from].0, self.knots[next].0);
        let u = ((t - start) / (end - start).max(1e-9)).clamp(0.0, 1.0);
        Some((from, self.knots[next].2.at(u)))
    }

    /// What it is before its first knot and after its last.
    fn outside(&self, t: f64) -> [f64; 3] {
        if t <= 0.0 {
            self.knots[0].1
        } else {
            self.knots[self.knots.len() - 1].1
        }
    }

    /// The value at `t`, going straight from key to key.
    pub fn straight(&self, t: f64) -> [f64; 3] {
        match self.span(t) {
            Some((i, u)) => std::array::from_fn(|c| {
                let (a, b) = (self.knots[i].1[c], self.knots[i + 1].1[c]);
                a + (b - a) * u
            }),
            None => self.outside(t),
        }
    }

    /// The place at `t` on a smooth path through its keys: a curve through
    /// three or more, so an orbit takes a few keys, and a straight line
    /// between two. A key held (the same place twice) is left and reached
    /// without overshoot, and a cut ends one path and starts the next.
    pub fn path(&self, t: f64) -> [f64; 3] {
        let Some((i, u)) = self.span(t) else {
            return self.outside(t);
        };
        let knots = &self.knots;
        let place = |k: usize| knots[k].1;
        let cut_to = |k: usize| knots[k].2 == Ease::Cut;
        // The direction it passes knot `k` in: toward the next from the one
        // before, none where it rests.
        let tangent = |k: usize, c: usize| {
            let before = k.checked_sub(1).filter(|_| !cut_to(k)).map(place);
            let after = (k + 1 < knots.len() && !cut_to(k + 1)).then(|| place(k + 1));
            match (before, after) {
                (Some(b), Some(a)) if b == place(k) || a == place(k) => 0.0,
                (Some(b), Some(a)) => (a[c] - b[c]) * 0.5,
                (None, Some(a)) => a[c] - place(k)[c],
                (Some(b), None) => place(k)[c] - b[c],
                (None, None) => 0.0,
            }
        };
        let (u2, u3) = (u * u, u * u * u);
        std::array::from_fn(|c| {
            let (p0, p1) = (place(i)[c], place(i + 1)[c]);
            (2.0 * u3 - 3.0 * u2 + 1.0) * p0
                + (u3 - 2.0 * u2 + u) * tangent(i, c)
                + (-2.0 * u3 + 3.0 * u2) * p1
                + (u3 - u2) * tangent(i + 1, c)
        })
    }
}

impl Object {
    pub(super) fn size_now(&self, t: f64) -> [f64; 3] {
        let base = self.size.unwrap_or_else(|| self.shape.size());
        let keys = (self.keys.iter()).filter_map(|k| Some((k.t, k.size?, k.ease)));
        Track::new(base, keys).straight(t)
    }

    pub(super) fn shade_now(&self, t: f64) -> f64 {
        let keys = (self.keys.iter()).filter_map(|k| Some((k.t, k.shade?, k.ease)));
        Track::of_number(self.shade, keys).straight(t)[0]
    }

    /// Whether it is out of the scene at `t`.
    pub(super) fn hidden_now(&self, t: f64) -> bool {
        let said = (self.keys.iter()).filter_map(|k| Some((k.t, k.hidden?)));
        said.take_while(|(at, _)| *at <= t)
            .last()
            .map_or(self.hidden, |(_, hidden)| hidden)
    }

    /// What it does at `t`: an action, the one it is becoming, and how
    /// far that has come (0..1).
    pub(super) fn action_now(&self, t: f64) -> (Action, Action, f32) {
        let mut now = (0.0, self.action);
        let mut before = self.action;
        for (at, action) in (self.keys.iter()).filter_map(|k| Some((k.t, k.action?))) {
            if at - TAKE_UP * 0.5 > t {
                break;
            }
            (before, now) = (now.1, (at, action));
        }
        let begun = (t - (now.0 - TAKE_UP * 0.5)) / TAKE_UP;
        (before, now.1, smoothstep(0.0, 1.0, begun as f32))
    }

    /// The path its keys move it along, in the space it stands in.
    fn places(&self) -> Track {
        let keys = self.keys.iter().filter_map(|k| Some((k.t, k.at?, k.ease)));
        Track::new(self.at, keys)
    }

    fn rotate_now(&self, t: f64) -> [f64; 3] {
        let keys = (self.keys.iter()).filter_map(|k| Some((k.t, k.rotate?, k.ease)));
        let turned = Track::new(self.rotate, keys).straight(t);
        std::array::from_fn(|c| turned[c] + self.spin[c] * t)
    }

    /// Metres a second it goes over the ground at `t`.
    pub(super) fn speed(&self, t: f64) -> f32 {
        speed(&self.places(), t, 0.12)
    }

    /// The fastest it goes over the ground, in metres a second.
    pub(super) fn top_speed(&self, duration: f64) -> f32 {
        let places = self.places();
        let steps = (duration * 6.0).ceil() as u32;
        (0..=steps)
            .map(|i| speed(&places, i as f64 / 6.0, 0.12))
            .fold(0.0, f32::max)
    }

    /// The most its travel is off the way `rotate` turns it, in degrees
    /// (90 is sideways, 180 backwards), while it goes at more than a
    /// stroll. `None` when it faces something, or never goes that fast.
    pub(super) fn off_course(&self, duration: f64) -> Option<f64> {
        if self.face.is_some() {
            return None;
        }
        let places = self.places();
        let steps = (duration * 6.0).ceil() as u32;
        (0..=steps)
            .map(|i| i as f64 / 6.0)
            .filter(|t| speed(&places, *t, 0.12) > 0.4)
            .filter_map(|t| Some(shortest(heading(&places, t)? - self.rotate_now(t)[1]).abs()))
            .reduce(f64::max)
    }

    /// `rate` (cycles a second, from its speed over the ground and the
    /// time) summed from the start to `t`: how far through its strides a
    /// walker is.
    pub(super) fn paced(&self, t: f64, rate: impl Fn(f32, f64) -> f32) -> f32 {
        let places = self.places();
        let step = 1.0 / FPS as f64;
        let mut cycles = 0.0;
        let mut from = 0.0;
        let mut was = places.path(0.0);
        while from < t {
            let to = (from + step).min(t);
            let is = places.path(to);
            let metres = (is[0] - was[0]).hypot(is[2] - was[2]);
            let speed = (metres / (to - from).max(1e-6)) as f32;
            cycles += rate(speed, from) * (to - from) as f32;
            (from, was) = (to, is);
        }
        cycles
    }
}

/// Metres a second over the ground at `t`, measured across `window`
/// seconds either side.
fn speed(places: &Track, t: f64, window: f64) -> f32 {
    let (a, b) = (places.path(t - window), places.path(t + window));
    ((b[0] - a[0]).hypot(b[2] - a[2]) / (2.0 * window)) as f32
}

/// `degrees` as the short way round: -180..180.
fn shortest(degrees: f64) -> f64 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

/// The way a path goes over the ground at `t`, in degrees about the
/// vertical from +Z. While it rests it is the way it last went, turning
/// to the way it goes next just before it sets off. `None` when it never
/// moves.
fn heading(places: &Track, t: f64) -> Option<f64> {
    let step = 1.0 / FPS as f64;
    let going = |t: f64| {
        let (a, b) = (places.path(t - step * 0.5), places.path(t + step * 0.5));
        let (dx, dz) = (b[0] - a[0], b[2] - a[2]);
        (dx.hypot(dz) > 1e-5).then(|| dx.atan2(dz).to_degrees())
    };
    if let Some(now) = going(t) {
        return Some(now);
    }
    let last = places.knots[places.knots.len() - 1].0;
    // The nearest moments it moves, before and after this rest.
    let from = t.clamp(0.0, last);
    let seek = |forward: bool| {
        (1..)
            .map(|i| from + i as f64 * if forward { step } else { -step })
            .take_while(|at| (-step..=last + step).contains(at))
            .find_map(|at| Some((at, going(at)?)))
    };
    match (seek(false), seek(true)) {
        (Some((_, was)), Some((sets_off, next))) => {
            let turned = smoothstep(0.0, 1.0, ((t - (sets_off - TURN)) / TURN) as f32);
            Some(was + shortest(next - was) * turned as f64)
        }
        (Some((_, way)), None) | (None, Some((_, way))) => Some(way),
        (None, None) => None,
    }
}

impl Scene {
    /// Where object `index` stands in the scene at `t`.
    pub(super) fn place(&self, index: usize, t: f64) -> Place {
        self.place_facing(index, t, 0)
    }

    /// The middle of object `index` at `t`: what a camera looking at it
    /// looks at.
    pub(super) fn middle(&self, index: usize, t: f64) -> Vec3 {
        self.middle_facing(index, t, 0)
    }

    fn middle_facing(&self, index: usize, t: f64, depth: u8) -> Vec3 {
        let half_height = self.objects[index].size_now(t)[1] as f32 * 0.5;
        self.place_facing(index, t, depth)
            .point(Vec3::new(0.0, half_height, 0.0))
    }

    /// [`Scene::place`], `depth` objects into following what faces what.
    fn place_facing(&self, index: usize, t: f64, depth: u8) -> Place {
        let object = &self.objects[index];
        let rides = (object.parent.as_deref())
            .and_then(|name| self.index(name))
            .map_or(Place::ORIGIN, |parent| self.place_facing(parent, t, depth));
        let places = object.places();
        let at = Vec3::of(places.path(t));
        let mut rotate = object.rotate_now(t);
        // Toward a point of the scene, as a turn in the space it stands in.
        let toward = |point: Vec3| {
            let way = rides.turn.inward(point - rides.point(at));
            (way.x.hypot(way.z) > 1e-5).then(|| (way.x.atan2(way.z) as f64).to_degrees())
        };
        let faced = match &object.face {
            _ if depth > FACING_DEPTH => None,
            None => None,
            Some(Target::Point(point)) => toward(Vec3::of(*point)),
            Some(Target::Object(name)) if name == PATH => heading(&places, t),
            Some(Target::Object(name)) if name == CAMERA => toward(self.eye(t, depth + 1)),
            Some(Target::Object(name)) => (self.index(name))
                .filter(|faced| *faced != index)
                .and_then(|faced| toward(self.middle_facing(faced, t, depth + 1))),
        };
        rotate[1] += faced.unwrap_or(0.0);
        rides.then(Place {
            at,
            turn: Turn::of(rotate),
        })
    }

    /// What the camera rides at `t`, and where it is in the scene.
    fn eye(&self, t: f64, depth: u8) -> Vec3 {
        let camera = &self.camera;
        let at = (camera.keys.iter()).filter_map(|k| Some((k.t, k.at?, k.ease)));
        self.mount(t, depth)
            .point(Vec3::of(Track::new(camera.at, at).path(t)))
    }

    fn mount(&self, t: f64, depth: u8) -> Place {
        (self.camera.parent.as_deref())
            .and_then(|name| self.index(name))
            .map_or(Place::ORIGIN, |parent| self.place_facing(parent, t, depth))
    }
}

/// Where the camera is and what it sees at a time.
pub(super) struct View {
    pub eye: Vec3,
    /// The way it looks, and the picture's right and top: one unit each.
    pub ahead: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    /// Degrees across the picture's longer side.
    pub fov: f32,
}

impl Scene {
    pub(super) fn view(&self, t: f64) -> View {
        let camera = &self.camera;
        let mount = self.mount(t, 0);
        // Each target is where it is now, so a camera between two objects
        // follows both as they move.
        let point = |target: &Target| match target {
            Target::Point(p) => mount.point(Vec3::of(*p)),
            Target::Object(name) => (self.index(name)).map_or(Vec3::ZERO, |i| self.middle(i, t)),
        };
        let in_f64 = |v: Vec3| [v.x as f64, v.y as f64, v.z as f64];
        let targets = (camera.keys.iter())
            .filter_map(|k| Some((k.t, in_f64(point(k.look_at.as_ref()?)), k.ease)));
        let target = Track::new(in_f64(point(&camera.look_at)), targets).straight(t);
        let number = |base: f64, of: fn(&super::CameraKey) -> Option<f64>| {
            let keys = (camera.keys.iter()).filter_map(|k| Some((k.t, of(k)?, k.ease)));
            Track::of_number(base, keys).straight(t)[0] as f32
        };
        let fov = number(camera.fov, |k| k.fov);
        let shake = Shake::of(camera.shake as f32, t as f32);
        let roll = (number(camera.roll, |k| k.roll) + shake.roll).to_radians();

        let eye = self.eye(t, 0);
        let level = mount.turn.y;
        let ahead = (Vec3::of(target) - eye).unit_or(-Vec3::AHEAD);
        // Looking straight up or down, any level direction is to the right.
        let right = ahead.cross(level).unit_or(mount.turn.x);
        let up = right.cross(ahead);
        // A hand turns it a little and moves it a little.
        let ahead = (ahead + right * shake.pan + up * shake.tilt).unit_or(ahead);
        let right = ahead.cross(up).unit_or(right);
        let up = right.cross(ahead);
        let (sin, cos) = roll.sin_cos();
        View {
            eye: eye + right * shake.sway + up * shake.bob,
            ahead,
            right: right * cos - up * sin,
            up: up * cos + right * sin,
            fov,
        }
    }
}

/// What a hand does to a camera at a moment.
struct Shake {
    /// Turns to the right and up, as slopes.
    pan: f32,
    tilt: f32,
    /// Degrees.
    roll: f32,
    /// Metres to the right and up.
    sway: f32,
    bob: f32,
}

impl Shake {
    fn of(amount: f32, t: f32) -> Self {
        // A few slow waves that never line up: the drift of a held camera.
        let wave = |seed: f32| {
            let part = |rate: f32, weight: f32| (t * rate + seed * 12.9).sin() * weight;
            part(1.9 + seed, 0.55) + part(4.3 + seed * 1.7, 0.3) + part(9.7 - seed, 0.15)
        };
        Self {
            pan: amount * 0.022 * wave(0.3),
            tilt: amount * 0.018 * wave(1.1),
            roll: amount * 0.9 * wave(2.3),
            sway: amount * 0.04 * wave(3.7),
            bob: amount * 0.04 * wave(5.2),
        }
    }
}
