//! Where things are at a time: the values keys move ([`Track`]), what a
//! cycle adds to them, and everything's place at one moment with what it
//! rides and faces ([`Moment`]), the camera's view with it.

use super::build::{Facing, Part};
use super::space::{Place, Turn, Vec3, smoothstep};
use super::{Cycle, CycleKey, Ease, FPS, MID_GREY, Object, Scene, Target};

/// Seconds before it sets off again that a resting object turns to the
/// way it will go.
const TURN: f64 = 0.6;
/// Metres a second below which motion paced by the ground is at rest,
/// and above which it is in full swing.
const STILL: f32 = 0.05;
const MOVING: f32 = 0.45;

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
#[derive(Clone, Debug, PartialEq)]
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

/// How a part is at a moment, in the space it stands in.
pub(super) struct Pose {
    pub at: [f64; 3],
    pub rotate: [f64; 3],
    pub size: [f64; 3],
}

impl Cycle {
    /// Whether a round runs on into the next: no key at its end closes
    /// it. A swing does; a wheel's turn from 0 to 360 doesn't.
    fn runs_on(&self) -> bool {
        self.keys.last().is_none_or(|last| last.t < 1.0)
    }

    /// What it adds `rounds` rounds in, to a place, a turn and a size.
    fn added(&self, rounds: f64) -> Pose {
        let phase = (rounds + self.shift).rem_euclid(1.0);
        Pose {
            at: self.one(phase, |key| key.at),
            rotate: self.one(phase, |key| key.rotate),
            size: self.one(phase, |key| key.size),
        }
    }

    /// One of the things it adds, `phase` (0..1) of the way through a
    /// round.
    fn one(&self, phase: f64, of: fn(&CycleKey) -> Option<[f64; 3]>) -> [f64; 3] {
        let keys: Vec<_> = (self.keys.iter())
            .filter_map(|key| Some((key.t, of(key)?, key.ease.unwrap_or(Ease::InOut))))
            .collect();
        let (Some(first), Some(last)) = (keys.first().copied(), keys.last().copied()) else {
            return [0.0; 3];
        };
        // Running on, the last key leads to the first of the next round.
        let mut knots = Vec::with_capacity(keys.len() + 2);
        if self.runs_on() {
            knots.push((last.0 - 1.0, last.1, last.2));
            knots.extend(keys);
            knots.push((first.0 + 1.0, first.1, first.2));
        } else {
            knots = keys;
        }
        match knots.iter().position(|knot| knot.0 > phase) {
            None => last.1,
            Some(0) => first.1,
            Some(next) => {
                let (from, to) = (knots[next - 1], knots[next]);
                let u = to.2.at((phase - from.0) / (to.0 - from.0).max(1e-9));
                std::array::from_fn(|c| from.1[c] + (to.1[c] - from.1[c]) * u)
            }
        }
    }
}

/// How an object's keys move it, worked out once when the scene is built.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Moves {
    /// The path its keys move it along, in the space it stands in.
    pub places: Track,
    turns: Track,
    sizes: Track,
    shades: Track,
    /// Degrees a second it keeps turning.
    spin: [f64; 3],
    /// Whether it starts out of the scene, and when it goes and comes.
    starts_hidden: bool,
    hidden: Vec<(f64, bool)>,
}

impl Moves {
    pub fn of(object: &Object) -> Self {
        let keys = &object.keys;
        let places = keys.iter().filter_map(|k| Some((k.t, k.at?, k.ease)));
        let turns = keys.iter().filter_map(|k| Some((k.t, k.rotate?, k.ease)));
        let sizes = keys.iter().filter_map(|k| Some((k.t, k.size?, k.ease)));
        let shades = keys.iter().filter_map(|k| Some((k.t, k.shade?, k.ease)));
        Self {
            places: Track::new(object.at, places),
            turns: Track::new(object.rotate, turns),
            sizes: Track::new(object.size.unwrap_or([0.0; 3]), sizes),
            shades: Track::of_number(object.shade.unwrap_or(MID_GREY), shades),
            spin: object.spin,
            starts_hidden: object.hidden,
            hidden: (keys.iter())
                .filter_map(|k| Some((k.t, k.hidden?)))
                .collect(),
        }
    }

    /// How the keys have it at `t`, before any cycle.
    fn keyed(&self, t: f64) -> Pose {
        let turned = self.turns.straight(t);
        Pose {
            at: self.places.path(t),
            rotate: std::array::from_fn(|c| turned[c] + self.spin[c] * t),
            size: self.sizes.straight(t),
        }
    }

    /// Its grey at `t`.
    pub fn shade(&self, t: f64) -> f64 {
        self.shades.straight(t)[0]
    }

    /// Whether it says it is out of the scene at `t`.
    fn hidden(&self, t: f64) -> bool {
        (self.hidden.iter())
            .take_while(|(at, _)| *at <= t)
            .last()
            .map_or(self.starts_hidden, |(_, hidden)| *hidden)
    }
}

impl Part {
    /// The most its travel is off the way `rotate` turns it, in degrees
    /// (90 is sideways, 180 backwards), while it goes at more than a
    /// stroll. `None` when it faces something, or never goes that fast.
    pub(super) fn off_course(&self, duration: f64) -> Option<f64> {
        if self.facing != Facing::Nothing {
            return None;
        }
        let places = &self.moves.places;
        let steps = (duration * 6.0).ceil() as u32;
        (0..=steps)
            .map(|i| i as f64 / 6.0)
            .filter(|t| speed(places, *t, 0.12) > 0.4)
            .filter_map(|t| {
                Some(shortest(heading(places, t)? - self.moves.keyed(t).rotate[1]).abs())
            })
            .reduce(f64::max)
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
    /// How part `index` is at `t`: as its keys have it, with what its
    /// cycle adds.
    fn pose(&self, index: usize, t: f64) -> Pose {
        let part = &self.parts[index];
        let mut pose = part.moves.keyed(t);
        let Some(cycle) = &part.object.cycle else {
            return pose;
        };
        // By the clock, or by the ground its whole has covered: then it
        // comes to rest as the whole does, unless a round never returns.
        let (rounds, swing) = match (cycle.seconds, cycle.metres) {
            (Some(seconds), _) => (t / seconds, 1.0),
            (None, Some(metres)) => {
                let (gone, speed) = self.travel(part.whole, t);
                let swing = match cycle.runs_on() {
                    true => smoothstep(STILL, MOVING, speed) as f64,
                    false => 1.0,
                };
                (gone / metres, swing)
            }
            (None, None) => return pose,
        };
        let added = cycle.added(rounds);
        for c in 0..3 {
            pose.at[c] += added.at[c] * swing;
            pose.rotate[c] += added.rotate[c] * swing;
            pose.size[c] = (pose.size[c] + added.size[c] * swing).max(0.005);
        }
        pose
    }

    /// Metres part `whole` has travelled over the ground by `t`, and its
    /// speed then in metres a second.
    fn travel(&self, whole: usize, t: f64) -> (f64, f32) {
        let gone = &self.parts[whole].travelled;
        let at = |t: f64| {
            let frame = (t * FPS as f64).clamp(0.0, gone.len().saturating_sub(1) as f64);
            let (from, to) = (frame.floor() as usize, frame.ceil() as usize);
            let (Some(a), Some(b)) = (gone.get(from), gone.get(to)) else {
                return 0.0;
            };
            (a + (b - a) * frame.fract() as f32) as f64
        };
        (at(t), ((at(t + 0.1) - at(t - 0.1)) / 0.2) as f32)
    }

    /// Whether part `index`, or what it rides, is out of the scene at `t`.
    pub(super) fn hidden(&self, index: usize, t: f64) -> bool {
        let mut part = Some(index);
        while let Some(index) = part {
            if self.parts[index].moves.hidden(t) {
                return true;
            }
            part = self.parts[index].parent;
        }
        false
    }

    /// Where every part stands at `t`, and its size then.
    pub(super) fn stand(&self, t: f64) -> (Vec<Place>, Vec<Vec3>) {
        // Parts that face one another go round in a loop: there, the one
        // placed first turns to where the other is before anything faces.
        let rough = match self.loops {
            true => self.stand_facing(t, None).0,
            false => Vec::new(),
        };
        self.stand_facing(t, Some(&rough))
    }

    /// Every part's place and size at `t`, each placed after what it
    /// rides. With `rough`, each also turns to what it faces: where that
    /// stands already, else where `rough` has it. Without, nothing faces.
    fn stand_facing(&self, t: f64, rough: Option<&[Place]>) -> (Vec<Place>, Vec<Vec3>) {
        let count = self.parts.len();
        let mut places = vec![Place::ORIGIN; count];
        let mut sizes = vec![Vec3::ZERO; count];
        let mut placed = vec![false; count];
        for &index in &self.order {
            let part = &self.parts[index];
            let rides = part.parent.map_or(Place::ORIGIN, |parent| places[parent]);
            let pose = self.pose(index, t);
            let at = Vec3::of(pose.at);
            let mut rotate = pose.rotate;
            if let Some(rough) = rough {
                let stands = |other: usize| match placed[other] {
                    true => places[other],
                    false => rough.get(other).copied().unwrap_or(Place::ORIGIN),
                };
                // Toward a point of the scene, as a turn in the space it
                // stands in.
                let toward = |point: Vec3| {
                    let way = rides.turn.inward(point - rides.point(at));
                    (way.x.hypot(way.z) > 1e-5).then(|| (way.x.atan2(way.z) as f64).to_degrees())
                };
                let faced = match part.facing {
                    Facing::Nothing => None,
                    Facing::Point(point) => toward(Vec3::of(point)),
                    Facing::Path => heading(&part.moves.places, t),
                    Facing::Camera => {
                        let mount = self.mount.map_or(Place::ORIGIN, stands);
                        toward(mount.point(self.camera_at(t)))
                    }
                    Facing::Part(faced) if faced == index => None,
                    Facing::Part(faced) => toward(self.middle(faced, stands(faced))),
                };
                rotate[1] += faced.unwrap_or(0.0);
            }
            places[index] = rides.then(Place {
                at,
                turn: Turn::of(rotate),
            });
            sizes[index] = Vec3::of(pose.size);
            placed[index] = true;
        }
        (places, sizes)
    }

    /// The middle of part `index` standing at `place`: what a camera
    /// looking at it looks at.
    fn middle(&self, index: usize, place: Place) -> Vec3 {
        let (low, high) = self.parts[index].bounds;
        place.point(Vec3::of(std::array::from_fn(|c| (low[c] + high[c]) * 0.5)))
    }

    /// Where the camera's keys have it at `t`, in the space of what it
    /// rides.
    fn camera_at(&self, t: f64) -> Vec3 {
        let camera = &self.camera;
        let at = (camera.keys.iter()).filter_map(|k| Some((k.t, k.at?, k.ease)));
        Vec3::of(Track::new(camera.at, at).path(t))
    }

    /// Everything at `t`: where each part stands, and what the camera
    /// sees.
    pub(super) fn moment(&self, t: f64) -> Moment {
        let (places, sizes) = self.stand(t);
        let gone = (0..self.parts.len()).map(|i| self.hidden(i, t)).collect();
        let view = self.view(t, &places);
        Moment {
            places,
            sizes,
            gone,
            view,
        }
    }
}

/// A scene at one moment.
pub(super) struct Moment {
    /// Where each part stands, and its size.
    pub places: Vec<Place>,
    pub sizes: Vec<Vec3>,
    /// Whether each part, or what it rides, is out of the scene.
    pub gone: Vec<bool>,
    pub view: View,
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
    /// What the camera sees at `t`, the parts standing at `places`.
    fn view(&self, t: f64, places: &[Place]) -> View {
        let camera = &self.camera;
        let mount = self.mount.map_or(Place::ORIGIN, |part| places[part]);
        // Each target is where it is now, so a camera between two objects
        // follows both as they move.
        let point = |target: &Target| match target {
            Target::Point(p) => mount.point(Vec3::of(*p)),
            Target::Object(name) => {
                (self.find(name)).map_or(Vec3::ZERO, |i| self.middle(i, places[i]))
            }
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

        let eye = mount.point(self.camera_at(t));
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
