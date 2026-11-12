//! Where things are at a time: the values keys move ([`Track`]), what a
//! cycle adds to them, and everything's place at one moment with what it
//! rides and faces ([`Moment`]), the camera's view with it.

use super::build::{Facing, Part};
use super::space::{Place, Turn, Vec3, smoothstep};
use super::{Camera, Cycle, CycleKey, Ease, FPS, MID_GREY, Object, Scene, Target};

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

/// One key of a track.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Knot {
    t: f64,
    value: [f64; 3],
    /// How fast the value passes through here, a second.
    slope: [f64; 3],
    /// How the move from the knot before arrives.
    ease: Ease,
}

/// A value its keys move: what it starts as, then its keys in order. A
/// key at the start replaces the start.
///
/// It follows a smooth curve through its keys that never goes past one:
/// where the value turns back or rests it slows to a stop, and between
/// such stops its speed changes without a jolt (how Blender's animation
/// curves set and clamp their handles). Two keys give a straight move, a
/// few around a subject an orbit. An ease times one stretch of that
/// curve, and a cut ends one curve and starts the next.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Track {
    knots: Vec<Knot>,
}

impl Track {
    pub fn new(base: [f64; 3], keys: impl Iterator<Item = (f64, [f64; 3], Ease)>) -> Self {
        let knot = |(t, value, ease)| Knot {
            t,
            value,
            slope: [0.0; 3],
            ease,
        };
        let mut knots = vec![knot((0.0, base, Ease::Linear))];
        for key in keys {
            if key.0 <= 0.0 {
                knots[0] = knot((0.0, key.1, Ease::Linear));
            } else {
                knots.push(knot(key));
            }
        }
        let mut start = 0;
        for end in 1..=knots.len() {
            if end == knots.len() || knots[end].ease == Ease::Cut {
                smooth(&mut knots[start..end]);
                start = end;
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

    /// When its last key is.
    fn end(&self) -> f64 {
        self.knots[self.knots.len() - 1].t
    }

    /// The value at `t`.
    pub fn at(&self, t: f64) -> [f64; 3] {
        let Some(next) = self.knots.iter().position(|knot| knot.t > t) else {
            return self.knots[self.knots.len() - 1].value;
        };
        let Some(from) = next.checked_sub(1) else {
            return self.knots[0].value;
        };
        let (a, b) = (&self.knots[from], &self.knots[next]);
        let span = (b.t - a.t).max(1e-9);
        // How far along the stretch of curve it is: past its end for an
        // ease that overshoots.
        let e = b.ease.at(((t - a.t) / span).clamp(0.0, 1.0));
        let (e2, e3) = (e * e, e * e * e);
        std::array::from_fn(|c| {
            (2.0 * e3 - 3.0 * e2 + 1.0) * a.value[c]
                + (e3 - 2.0 * e2 + e) * span * a.slope[c]
                + (-2.0 * e3 + 3.0 * e2) * b.value[c]
                + (e3 - e2) * span * b.slope[c]
        })
    }
}

/// Give each knot of one curve the pace the value passes it at.
fn smooth(run: &mut [Knot]) {
    let last = run.len() - 1;
    if last == 0 {
        return;
    }
    let spans: Vec<f64> = (run.windows(2))
        .map(|pair| (pair[1].t - pair[0].t).max(1e-9))
        .collect();
    for c in 0..3 {
        // The pace of each stretch, were it straight.
        let straight: Vec<f64> = (run.windows(2).zip(&spans))
            .map(|(pair, span)| (pair[1].value[c] - pair[0].value[c]) / span)
            .collect();
        // Where the value turns back or rests, it stops.
        let stops = |i: usize| i > 0 && i < last && straight[i - 1] * straight[i] <= 0.0;
        // Within three times the straight pace either side, the curve
        // stays between its keys.
        let held = |pace: f64, i: usize| {
            let beside = [i.checked_sub(1), (i < last).then_some(i)];
            (beside.into_iter().flatten()).fold(pace, |pace, stretch| {
                let most = 3.0 * straight[stretch].abs();
                match pace * straight[stretch] > 0.0 {
                    true => pace.clamp(-most, most),
                    false => 0.0,
                }
            })
        };
        let mut from = 0;
        for to in 1..=last {
            if to < last && !stops(to) {
                continue;
            }
            let ends = (stops(from), stops(to));
            let paces = flowing(&spans[from..to], &straight[from..to], ends);
            for (i, pace) in (from..=to).zip(paces) {
                run[i].slope[c] = held(pace, i);
            }
            from = to;
        }
    }
}

/// The paces through the knots of a stretch of curve that change the
/// least from knot to knot: its speed has no jolt anywhere. `spans` and
/// `straight` are the time and the straight pace between each two knots,
/// and `rests` says whether it stands still at its first and last knot;
/// where it doesn't, it bends no further there.
fn flowing(spans: &[f64], straight: &[f64], rests: (bool, bool)) -> Vec<f64> {
    let last = spans.len();
    // One row a knot of `below·before + on·pace + above·after = wanted`.
    let mut rows: Vec<[f64; 4]> = (0..=last)
        .map(|i| match i {
            0 if rests.0 => [0.0, 1.0, 0.0, 0.0],
            0 => [0.0, 2.0, 1.0, 3.0 * straight[0]],
            i if i == last && rests.1 => [0.0, 1.0, 0.0, 0.0],
            i if i == last => [1.0, 2.0, 0.0, 3.0 * straight[last - 1]],
            i => {
                let (before, after) = (spans[i - 1], spans[i]);
                let wanted = 3.0 * (after * straight[i - 1] + before * straight[i]);
                [after, 2.0 * (before + after), before, wanted]
            }
        })
        .collect();
    for i in 1..=last {
        let [_, on, above, wanted] = rows[i - 1];
        let share = rows[i][0] / on;
        rows[i][1] -= share * above;
        rows[i][3] -= share * wanted;
    }
    let mut paces = vec![0.0; last + 1];
    for i in (0..=last).rev() {
        let after = paces.get(i + 1).copied().unwrap_or(0.0);
        paces[i] = (rows[i][3] - rows[i][2] * after) / rows[i][1];
    }
    paces
}

/// When each of `keys` (the time it gives, and the place) is reached. A
/// key that gives no time is passed on the way: at an even pace over the
/// ground between the timed keys either side of it, the start and the end
/// of `duration` seconds standing in where there is none. `start` is
/// where the thing is before its keys.
pub(super) fn key_times(
    start: [f64; 3],
    keys: &[(Option<f64>, Option<[f64; 3]>)],
    duration: f64,
) -> Vec<f64> {
    let apart = |a: [f64; 3], b: [f64; 3]| {
        (0..3)
            .map(|c| (b[c] - a[c]) * (b[c] - a[c]))
            .sum::<f64>()
            .sqrt()
    };
    let mut times = vec![0.0; keys.len()];
    let (mut from, mut was) = (0.0, start);
    let mut i = 0;
    while i < keys.len() {
        if let (Some(t), place) = keys[i] {
            (times[i], from, was) = (t, t, place.unwrap_or(was));
            i += 1;
            continue;
        }
        // The keys passed on the way to the next timed one, or to the end.
        let next = (i..keys.len()).find(|k| keys[*k].0.is_some());
        let passed = i..next.unwrap_or(keys.len());
        let mut gone = Vec::with_capacity(passed.len());
        let mut far = 0.0;
        for k in passed.clone() {
            let place = keys[k].1.unwrap_or(was);
            far += apart(was, place);
            gone.push(far);
            was = place;
        }
        let (until, whole) = match next.map(|k| keys[k]) {
            Some((Some(t), place)) => (t, far + place.map_or(0.0, |place| apart(was, place))),
            _ => (duration, far),
        };
        let until = until.max(from);
        for (n, k) in passed.clone().enumerate() {
            let share = match whole > 1e-9 {
                true => gone[n] / whole,
                false => (n + 1) as f64 / passed.len() as f64,
            };
            times[k] = from + (until - from) * share;
        }
        from = times[passed.end - 1];
        i = passed.end;
    }
    times
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
    /// How `object`'s keys move it over a scene of `duration` seconds.
    pub fn of(object: &Object, duration: f64) -> Self {
        let keys = &object.keys;
        let given: Vec<_> = keys.iter().map(|key| (key.t, key.at)).collect();
        let times = key_times(object.at, &given, duration);
        let timed = || times.iter().copied().zip(keys);
        let places = timed().filter_map(|(t, k)| Some((t, k.at?, k.ease)));
        let turns = timed().filter_map(|(t, k)| Some((t, k.rotate?, k.ease)));
        let sizes = timed().filter_map(|(t, k)| Some((t, k.size?, k.ease)));
        let shades = timed().filter_map(|(t, k)| Some((t, k.shade?, k.ease)));
        Self {
            places: Track::new(object.at, places),
            turns: Track::new(object.rotate, turns),
            sizes: Track::new(object.size.unwrap_or([0.0; 3]), sizes),
            shades: Track::of_number(object.shade.unwrap_or(MID_GREY), shades),
            spin: object.spin,
            starts_hidden: object.hidden,
            hidden: timed().filter_map(|(t, k)| Some((t, k.hidden?))).collect(),
        }
    }

    /// How the keys have it at `t`, before any cycle.
    fn keyed(&self, t: f64) -> Pose {
        let turned = self.turns.at(t);
        Pose {
            at: self.places.at(t),
            rotate: std::array::from_fn(|c| turned[c] + self.spin[c] * t),
            // An ease that overshoots may take a size past nothing.
            size: self.sizes.at(t).map(|side| side.max(0.0)),
        }
    }

    /// The first turn its keys make that goes most of the way round and
    /// stops short: the times it is between, the degrees turned, and the
    /// angle to write at its end for the short way round. A turn written
    /// as 350 and then 10 goes back through 340 degrees, not on through 20.
    pub fn long_turn(&self) -> Option<(f64, f64, f64, f64)> {
        (self.turns.knots.windows(2)).find_map(|pair| {
            (0..3).find_map(|c| {
                let turned = pair[1].value[c] - pair[0].value[c];
                let short = shortest(turned);
                // A whole number of turns is a spin that was meant.
                (pair[1].ease != Ease::Cut && turned.abs() >= 270.0 && short.abs() > 1.0)
                    .then(|| (pair[0].t, pair[1].t, turned, pair[0].value[c] + short))
            })
        })
    }

    /// Its grey at `t`.
    pub fn shade(&self, t: f64) -> f64 {
        self.shades.at(t)[0].clamp(0.0, 1.0)
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
    let (a, b) = (places.at(t - window), places.at(t + window));
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
        let (a, b) = (places.at(t - step * 0.5), places.at(t + step * 0.5));
        let (dx, dz) = (b[0] - a[0], b[2] - a[2]);
        (dx.hypot(dz) > 1e-5).then(|| dx.atan2(dz).to_degrees())
    };
    if let Some(now) = going(t) {
        return Some(now);
    }
    let last = places.end();
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
        Vec3::of(self.camera_moves.places.at(t))
    }

    /// When the camera cuts to a new set-up.
    pub(super) fn cuts(&self) -> impl Iterator<Item = f64> + '_ {
        let keys = self.camera_moves.times.iter().zip(&self.camera.keys);
        keys.filter(|(_, key)| key.ease == Ease::Cut)
            .map(|(t, _)| *t)
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

/// How the camera's keys move it, worked out once when the scene is
/// built. What it looks at is found each frame: it may be moving.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct CameraMoves {
    places: Track,
    fovs: Track,
    rolls: Track,
    /// When each of its keys is reached.
    times: Vec<f64>,
}

impl CameraMoves {
    pub fn of(camera: &Camera, duration: f64) -> Self {
        let keys = &camera.keys;
        let given: Vec<_> = keys.iter().map(|key| (key.t, key.at)).collect();
        let times = key_times(camera.at, &given, duration);
        let timed = || times.iter().copied().zip(keys);
        let places = timed().filter_map(|(t, k)| Some((t, k.at?, k.ease)));
        let fovs = timed().filter_map(|(t, k)| Some((t, k.fov?, k.ease)));
        let rolls = timed().filter_map(|(t, k)| Some((t, k.roll?, k.ease)));
        Self {
            places: Track::new(camera.at, places),
            fovs: Track::of_number(camera.fov, fovs),
            rolls: Track::of_number(camera.roll, rolls),
            times,
        }
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
        let moves = &self.camera_moves;
        let targets = (moves.times.iter().zip(&camera.keys))
            .filter_map(|(at, k)| Some((*at, in_f64(point(k.look_at.as_ref()?)), k.ease)));
        let target = Track::new(in_f64(point(&camera.look_at)), targets).at(t);
        let fov = (moves.fovs.at(t)[0] as f32).clamp(5.0, 140.0);
        let shake = Shake::of(camera.shake as f32, t as f32);
        let roll = (moves.rolls.at(t)[0] as f32 + shake.roll).to_radians();

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
