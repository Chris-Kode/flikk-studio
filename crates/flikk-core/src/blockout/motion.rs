//! Where things are at a time: the values keys move ([`Track`]), what a
//! cycle adds to them, and everything's place at one moment with what it
//! rides and faces ([`Moment`]), the camera's view with it.

use super::build::{Facing, Part};
use super::solid::Solid;
use super::space::{Place, Turn, Vec3, smoothstep};
use super::{
    Camera, Cycle, CycleKey, Ease, FPS, Frame, MID_GREY, NORMAL_FOV, Object, Scene, Shape, Side,
    Target, finite, wide,
};

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
    /// What tells its shake from any other's.
    seed: u32,
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
            seed: (object.name.bytes()).fold(0x811c_9dc5, |seed, byte| mixed(seed ^ byte as u32)),
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
    fn cycled(&self, index: usize, t: f64) -> Pose {
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

    /// How part `index` is at `t`: as its keys have it, with what its
    /// cycle and its shake add.
    fn pose(&self, index: usize, t: f64) -> Pose {
        let part = &self.parts[index];
        let mut pose = self.cycled(index, t);
        if let Some(shake) = &part.object.shake {
            let at = t / shake.seconds;
            for c in 0..3 {
                pose.at[c] += shake.at[c] * wander(at, part.moves.seed ^ c as u32);
                pose.rotate[c] += shake.rotate[c] * wander(at, part.moves.seed ^ (c as u32 + 3));
            }
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
    pub(super) fn stand(&self, t: f64) -> Standing {
        // Parts that face one another go round in a loop: there, the one
        // placed first turns to where the other is before anything faces.
        // And what stands on something is set down as its limbs are.
        let rough = self.rough.then(|| self.stand_as(t, Placing::Roughly));
        self.stand_as(t, Placing::Fully(rough.as_ref()))
    }

    /// Every part's place and size at `t`, each placed after what it
    /// rides.
    fn stand_as(&self, t: f64, placing: Placing) -> Standing {
        let count = self.parts.len();
        let mut places = vec![Place::ORIGIN; count];
        let mut sizes = vec![Vec3::ZERO; count];
        let mut placed = vec![false; count];
        for &index in &self.order {
            let part = &self.parts[index];
            let rides = part.parent.map_or(Place::ORIGIN, |parent| places[parent]);
            let pose = self.pose(index, t);
            let mut place = rides.then(Place {
                at: Vec3::of(pose.at),
                turn: Turn::of(pose.rotate),
            });
            if let Placing::Fully(rough) = placing {
                let stands = |other: usize| match (placed[other], rough) {
                    (false, Some(rough)) => rough.places[other],
                    _ => places[other],
                };
                let mut rotate = pose.rotate;
                rotate[1] += self.turn_to_face(index, t, place.at, rides, stands);
                place.turn = rides.turn.then(Turn::of(rotate));
                if let Some(rough) = rough {
                    place.at.y += self.lift(index, t, (place, pose.at[1] as f32), rough, stands);
                }
            }
            places[index] = place;
            sizes[index] = Vec3::of(pose.size);
            placed[index] = true;
        }
        Standing { places, sizes }
    }

    /// Degrees part `index`, at `at` on what stands at `rides`, turns
    /// about the vertical to what it faces at `t`; `stands` says where
    /// another part is.
    fn turn_to_face(
        &self,
        index: usize,
        t: f64,
        at: Vec3,
        rides: Place,
        stands: impl Fn(usize) -> Place,
    ) -> f64 {
        let part = &self.parts[index];
        // Toward a point of the scene, as a turn in the space it stands in.
        let toward = |point: Vec3| {
            let way = rides.turn.inward(point - at);
            (way.x.hypot(way.z) > 1e-5).then(|| (way.x.atan2(way.z) as f64).to_degrees())
        };
        let faced = match part.facing {
            Facing::Nothing => None,
            Facing::Point(point) => toward(Vec3::of(point)),
            Facing::Path => heading(&part.moves.places, t),
            Facing::Camera => {
                let rig = self.rig(t);
                let mount = rig.mount.map_or(Place::ORIGIN, &stands);
                toward(mount.point(rig.at(t)))
            }
            Facing::Part(faced) if faced == index => None,
            Facing::Part(faced) => toward(self.middle(faced, stands(faced))),
        };
        faced.unwrap_or(0.0)
    }

    /// How far up part `index` is to go at `t` to rest on what it stands
    /// on: nothing when it says nothing of that. `stood` is where it
    /// stands and the height it was written at, how far over the surface
    /// its lowest point then is. `rough` has its solids as its limbs hold
    /// them, and `stands` says where another part is.
    fn lift(
        &self,
        index: usize,
        t: f64,
        stood: (Place, f32),
        rough: &Standing,
        stands: impl Fn(usize) -> Place,
    ) -> f32 {
        let Some(rest) = &self.parts[index].rest else {
            return 0.0;
        };
        let (place, height) = stood;
        let solid = |of: usize, place: Place| {
            let object = &self.parts[of].object;
            let shape = object.shape.unwrap_or(Shape::Box);
            Solid::standing(shape, object.anchor, place, rough.sizes[of])
        };
        let there = |of: &&usize| !self.hidden(**of, t);
        // Its own solids ride it as they did in the rough.
        let own = rest.solids.iter().filter(there).map(|of| {
            let rode = rough.places[index].holds(rough.places[*of]);
            solid(*of, place.then(rode)).lowest()
        });
        let Some(lowest) = own.reduce(|a, b| if b.y < a.y { b } else { a }) else {
            return 0.0;
        };
        let surface = (rest.surface.iter().filter(there)).map(|of| solid(*of, stands(*of)));
        top_under(surface, lowest) + height - lowest.y
    }

    /// The middle of part `index` standing at `place`: what a camera
    /// looking at it looks at.
    fn middle(&self, index: usize, place: Place) -> Vec3 {
        let (low, high) = self.parts[index].bounds;
        place.point(Vec3::of(std::array::from_fn(|c| (low[c] + high[c]) * 0.5)))
    }

    /// The camera the picture is from at `t`.
    fn rig(&self, t: f64) -> &Rig {
        let cut_to = self.live.iter().take_while(|(at, _)| *at <= t).last();
        &self.rigs[cut_to.map_or(0, |(_, rig)| *rig)]
    }

    /// When the picture cuts: to another camera, or one camera to a new
    /// set-up.
    pub(super) fn cuts(&self) -> impl Iterator<Item = f64> + '_ {
        let own = self.rigs.iter().flat_map(|rig| rig.cuts.iter().copied());
        own.chain(self.live.iter().map(|(t, _)| *t))
    }

    /// The object the camera looks at at `t`, when it looks at one.
    pub(super) fn subject(&self, t: f64) -> Option<usize> {
        let rig = self.rig(t);
        let aimed = rig.aims.iter().take_while(|(at, ..)| *at <= t).last();
        match aimed.map_or(rig.aim, |(_, aim, _)| *aim) {
            Aim::Part(part) => Some(part),
            Aim::Held(framing) => Some(rig.framings[framing].subject),
            Aim::Point(_) => None,
        }
    }

    /// Everything at `t`: where each part stands, and what the camera
    /// sees.
    /// `shape` is the picture's, width to height: how far off a camera
    /// stands to hold what its `frame` says depends on it.
    pub(super) fn moment(&self, t: f64, shape: (u32, u32)) -> Moment {
        let standing = self.stand(t);
        let gone = (0..self.parts.len()).map(|i| self.hidden(i, t)).collect();
        let view = self.view(t, &standing, shape);
        Moment {
            places: standing.places,
            sizes: standing.sizes,
            gone,
            view,
        }
    }
}

/// How high the top of `surface` is under (or over) `point`; the ground
/// where none of it is.
fn top_under(surface: impl Iterator<Item = Solid>, point: Vec3) -> f32 {
    // From well over it, near enough to measure finely.
    let from = Vec3::new(point.x, point.y + 200.0, point.z);
    surface
        .filter_map(|solid| solid.hit(from, -Vec3::UP))
        .map(|(down, _)| from.y - down)
        .reduce(f32::max)
        .unwrap_or(0.0)
}

/// Where every part stands at a moment, and its size.
pub(super) struct Standing {
    pub places: Vec<Place>,
    pub sizes: Vec<Vec3>,
}

/// How parts are placed.
#[derive(Clone, Copy)]
enum Placing<'a> {
    /// Each where its keys, its cycle and what it rides have it: nothing
    /// faces or rests.
    Roughly,
    /// Each also turned to what it faces and set down on what it stands
    /// on: found where that stands already, else where the rough places,
    /// worked out first in a scene that needs them, have it.
    Fully(Option<&'a Standing>),
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

/// A camera as it is worked with: how its keys move it and what it
/// looks at and rides, names swapped for places among the parts, worked
/// out once when the scene is built.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Rig {
    places: Track,
    fovs: Track,
    rolls: Track,
    /// What it looks at, and what its keys have it look at later. Where
    /// that is, is found each frame: it may be moving.
    aim: Aim,
    aims: Vec<(f64, Aim, Ease)>,
    /// What its `frame`s hold; where it and its keys stand or frame, in
    /// order, when any of them frames; and which side of the picture what
    /// it holds sits on (-1 the left third, 1 the right).
    framings: Vec<Framing>,
    stops: Vec<Stop>,
    sides: Track,
    /// When its keys cut to a new set-up.
    cuts: Vec<f64>,
    /// How much a hand holds it, and what tells its shake from another's.
    shake: f32,
    seed: u32,
    /// The part it rides.
    pub mount: Option<usize>,
}

/// What a camera looks at: a point, in the space of what it rides, a
/// part, or what one of its framings holds.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Aim {
    Point([f64; 3]),
    Part(usize),
    Held(usize),
}

/// What a `frame` holds, names swapped for places among the parts.
#[derive(Clone, Debug, PartialEq)]
struct Framing {
    /// The first object it names, and the solids of all of them.
    subject: usize,
    solids: Vec<usize>,
    /// The share of their height it holds, from the top down, and the
    /// share of the picture's height that fills.
    kept: f32,
    fills: f32,
    /// Whether they sit off to a side, and so have less room across.
    aside: bool,
}

/// Where a camera stands at a time: at a place, or as far off as a
/// framing takes, the way it looks from the place it would else be at.
#[derive(Clone, Debug, PartialEq)]
struct Stop {
    t: f64,
    at: Option<[f64; 3]>,
    framing: Option<usize>,
    ease: Ease,
}

impl Rig {
    /// `camera`, called `what`, in a scene of `duration` seconds; `part`
    /// says where an object is among the parts by its name, and `solids`
    /// which parts are its solids. Refused when it names an object there
    /// is none of.
    pub fn of(
        camera: &Camera,
        what: &str,
        duration: f64,
        part: impl Fn(&str) -> Option<usize>,
        solids: impl Fn(usize) -> Vec<usize>,
    ) -> Result<Self, String> {
        let keys = &camera.keys;
        let given: Vec<_> = keys.iter().map(|key| (key.t, key.at)).collect();
        let times = key_times(camera.at, &given, duration);
        let timed = || times.iter().copied().zip(keys);
        let places = timed().filter_map(|(t, k)| Some((t, k.at?, k.ease)));
        let fovs = timed().filter_map(|(t, k)| Some((t, wide(k.lens, k.fov)?, k.ease)));
        let rolls = timed().filter_map(|(t, k)| Some((t, k.roll?, k.ease)));
        let aim = |target: &Target| match target {
            Target::Point(point) => {
                finite(&format!("{what}'s `look_at`"), point)?;
                Ok(Aim::Point(*point))
            }
            Target::Object(name) => (part(name).map(Aim::Part))
                .ok_or_else(|| format!("{what} looks at `{name}`, which is no object's name")),
        };
        // The camera's own framing first, then its keys' in order.
        let mut framings = Vec::new();
        let mut framed = |frame: &Option<Frame>| -> Result<Option<usize>, String> {
            let Some(frame) = frame else {
                return Ok(None);
            };
            let named = |name: &String| {
                part(name)
                    .ok_or_else(|| format!("{what} frames `{name}`, which is no object's name"))
            };
            let subjects = (frame.of.names().iter())
                .map(named)
                .collect::<Result<Vec<_>, _>>()?;
            let Some(subject) = subjects.first().copied() else {
                return Err(format!(
                    "{what}'s `frame` holds nothing: name an object in `of`"
                ));
            };
            let (kept, fills) = frame.size.holds();
            framings.push(Framing {
                subject,
                solids: subjects
                    .iter()
                    .flat_map(|subject| solids(*subject))
                    .collect(),
                kept,
                fills,
                aside: frame.side != Side::Centre,
            });
            Ok(Some(framings.len() - 1))
        };
        let own = framed(&camera.frame)?;
        let theirs = (keys.iter())
            .map(|key| framed(&key.frame))
            .collect::<Result<Vec<_>, _>>()?;
        // A key looks at what it says to, else at what it frames.
        let aim_of = |target: &Option<Target>, framing: Option<usize>| match target {
            Some(target) => aim(target).map(Some),
            None => Ok(framing.map(Aim::Held)),
        };
        let aims = (timed().zip(&theirs))
            .map(|((t, k), framing)| Ok((t, aim_of(&k.look_at, *framing)?, k.ease)))
            .filter_map(|aimed: Result<_, String>| match aimed {
                Ok((t, aim, ease)) => Some(Ok((t, aim?, ease))),
                Err(e) => Some(Err(e)),
            })
            .collect::<Result<_, String>>()?;
        let Some(first_aim) = aim_of(&camera.look_at, own)? else {
            return Err(format!(
                "{what} needs a `look_at`, or a `frame` saying what its picture holds"
            ));
        };
        let stops = match framings.is_empty() {
            true => Vec::new(),
            false => {
                let first = Stop {
                    t: 0.0,
                    at: Some(camera.at),
                    framing: own,
                    ease: Ease::Linear,
                };
                let later = (timed().zip(&theirs))
                    .filter(|((_, k), framing)| k.at.is_some() || framing.is_some())
                    .map(|((t, k), framing)| Stop {
                        t,
                        at: k.at,
                        framing: *framing,
                        ease: k.ease,
                    });
                std::iter::once(first).chain(later).collect()
            }
        };
        let side = |frame: &Frame| match frame.side {
            Side::Left => -1.0,
            Side::Centre => 0.0,
            Side::Right => 1.0,
        };
        let sides = timed().filter_map(|(t, k)| Some((t, side(k.frame.as_ref()?), k.ease)));
        let mount = match &camera.parent {
            Some(parent) => Some(
                part(parent)
                    .ok_or_else(|| format!("{what} rides `{parent}`, which is no object's name"))?,
            ),
            None => None,
        };
        Ok(Self {
            places: Track::new(camera.at, places),
            fovs: Track::of_number(wide(camera.lens, camera.fov).unwrap_or(NORMAL_FOV), fovs),
            rolls: Track::of_number(camera.roll, rolls),
            aim: first_aim,
            aims,
            sides: Track::of_number(camera.frame.as_ref().map_or(0.0, side), sides),
            framings,
            stops,
            cuts: timed()
                .filter(|(_, k)| k.ease == Ease::Cut)
                .map(|(t, _)| t)
                .collect(),
            shake: camera.shake as f32,
            seed: what.bytes().fold(0, |seed, byte| mixed(seed ^ byte as u32)) & 0xfff0,
            mount,
        })
    }

    /// Where its keys have it at `t`, in the space of what it rides.
    fn at(&self, t: f64) -> Vec3 {
        Vec3::of(self.places.at(t))
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
    /// What the camera sees at `t` in a picture of `shape` (width to
    /// height), the parts standing as `standing` has them.
    fn view(&self, t: f64, standing: &Standing, shape: (u32, u32)) -> View {
        let rig = self.rig(t);
        let places = &standing.places;
        let mount = rig.mount.map_or(Place::ORIGIN, |part| places[part]);
        // Each target is where it is now, so a camera between two objects
        // follows both as they move.
        let point = |aim: &Aim| match aim {
            Aim::Point(point) => mount.point(Vec3::of(*point)),
            Aim::Part(part) => self.middle(*part, places[*part]),
            Aim::Held(framing) => {
                let framing = &rig.framings[*framing];
                let held = self.held(framing, standing, t);
                held.map_or(places[framing.subject].at, |(low, high)| (low + high) * 0.5)
            }
        };
        let in_f64 = |v: Vec3| [v.x as f64, v.y as f64, v.z as f64];
        let targets = (rig.aims.iter()).map(|(at, aim, ease)| (*at, in_f64(point(aim)), *ease));
        let target = Track::new(in_f64(point(&rig.aim)), targets).at(t);
        let fov = (rig.fovs.at(t)[0] as f32).clamp(5.0, 140.0);
        let shake = Handheld::of(rig.shake, t as f32, rig.seed);
        let roll = (rig.rolls.at(t)[0] as f32 + shake.roll).to_radians();

        let eye = mount.point(self.stood(rig, t, shape));
        let level = mount.turn.y;
        let ahead = (Vec3::of(target) - eye).unit_or(-Vec3::AHEAD);
        // Looking straight up or down, any level direction is to the right.
        let right = ahead.cross(level).unit_or(mount.turn.x);
        // What it holds on a third of the picture: it looks a third of
        // half the picture's width to the other side.
        let across = half_width(fov, shape).0 * rig.sides.at(t)[0] as f32 / 3.0;
        let ahead = (ahead - right * across).unit_or(ahead);
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

    /// Where `rig` is at `t`, in the space of what it rides: where its
    /// keys have it, or on the curve through where its framings put it,
    /// each worked out as things stand when it gets there.
    fn stood(&self, rig: &Rig, t: f64, shape: (u32, u32)) -> Vec3 {
        if rig.stops.is_empty() {
            return rig.at(t);
        }
        let place = |stop: &Stop| {
            let written = stop.at.unwrap_or_else(|| rig.places.at(stop.t));
            let Some(framing) = stop.framing else {
                return written;
            };
            let then = self.stand(stop.t);
            let mount = rig.mount.map_or(Place::ORIGIN, |part| then.places[part]);
            let framing = &rig.framings[framing];
            let Some(held) = self.held(framing, &then, stop.t) else {
                return written;
            };
            // Narrower across where what it holds sits to a side.
            let (across, up) = half_width(rig.fovs.at(stop.t)[0] as f32, shape);
            let room = (
                across * if framing.aside { 0.6 } else { 0.9 },
                up * framing.fills,
            );
            let eye = framed_from(mount.point(Vec3::of(written)), held, room);
            let within = mount.turn.inward(eye - mount.at);
            [within.x as f64, within.y as f64, within.z as f64]
        };
        let first = place(&rig.stops[0]);
        let later = rig.stops[1..]
            .iter()
            .map(|stop| (stop.t, place(stop), stop.ease));
        Vec3::of(Track::new(first, later).at(t))
    }

    /// The box, low corner and high, that holds what `framing` keeps of
    /// its subjects as they stand at `t`: the solids that reach into the
    /// share of their height it keeps, from there up. `None` when none of
    /// them is in the scene.
    fn held(&self, framing: &Framing, standing: &Standing, t: f64) -> Option<(Vec3, Vec3)> {
        let boxes: Vec<(Vec3, Vec3)> = (framing.solids.iter())
            .filter(|of| !self.hidden(**of, t))
            .map(|of| {
                let object = &self.parts[*of].object;
                let shape = object.shape.unwrap_or(Shape::Box);
                Solid::standing(
                    shape,
                    object.anchor,
                    standing.places[*of],
                    standing.sizes[*of],
                )
                .bounds()
            })
            .collect();
        let together = |boxes: &mut dyn Iterator<Item = (Vec3, Vec3)>| {
            boxes.reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
        };
        let (low, high) = together(&mut boxes.iter().copied())?;
        let from = high.y - (high.y - low.y) * framing.kept;
        let (low, high) = together(&mut boxes.iter().copied().filter(|held| held.1.y > from))?;
        Some((Vec3::new(low.x, from.max(low.y), low.z), high))
    }
}

/// Half of what a camera seeing `fov` degrees across the longer side of a
/// picture of `shape` sees across it and up it, one unit ahead.
fn half_width(fov: f32, shape: (u32, u32)) -> (f32, f32) {
    let longer = (fov.to_radians() * 0.5).tan();
    let (wide, high) = (shape.0.max(1) as f32, shape.1.max(1) as f32);
    match wide >= high {
        true => (longer, longer * high / wide),
        false => (longer * wide / high, longer),
    }
}

/// Where a camera looking from `from` at the middle of the box `held`
/// stands for the box to just fit `room`: half of what it may take
/// across and up the picture, one unit ahead (the fit Blender's
/// camera-to-selected makes, the way it looks kept).
fn framed_from(from: Vec3, held: (Vec3, Vec3), room: (f32, f32)) -> Vec3 {
    let (low, high) = held;
    let middle = (low + high) * 0.5;
    let ahead = (middle - from).unit_or(-Vec3::AHEAD);
    let right = ahead.cross(Vec3::UP).unit_or(Vec3::new(1.0, 0.0, 0.0));
    let up = right.cross(ahead);
    let corners = (0..8).map(|i| {
        let pick = |bit: usize, low: f32, high: f32| if i >> bit & 1 == 0 { low } else { high };
        Vec3::new(
            pick(0, low.x, high.x),
            pick(1, low.y, high.y),
            pick(2, low.z, high.z),
        ) - middle
    });
    // Each corner says how far back keeps it in: the furthest is it.
    let back = corners
        .map(|corner| {
            let nearer = corner.dot(ahead);
            let across = corner.dot(right).abs() / room.0.max(1e-3);
            let over = corner.dot(up).abs() / room.1.max(1e-3);
            across.max(over) - nearer
        })
        .fold(0.3, f32::max);
    middle - ahead * back
}

/// What a hand does to a camera at a moment.
struct Handheld {
    /// Turns to the right and up, as slopes.
    pan: f32,
    tilt: f32,
    /// Degrees.
    roll: f32,
    /// Metres to the right and up.
    sway: f32,
    bob: f32,
}

impl Handheld {
    /// What a hand holding it `amount` does at `t`; `seed` tells one
    /// camera's from another's.
    fn of(amount: f32, t: f32, seed: u32) -> Self {
        // The drift of a held camera: a wobble every second and a half
        // or so, with quicker ones in it.
        let moved = |channel: u32, most: f32| {
            amount * most * wander(t as f64 * 0.65, seed + channel) as f32
        };
        Self {
            pan: moved(1, 0.036),
            tilt: moved(2, 0.03),
            roll: moved(3, 1.5),
            sway: moved(4, 0.065),
            bob: moved(5, 0.065),
        }
    }
}

/// A value that wanders between -1 and 1 and never repeats, `at` counted
/// in wobbles; `seed` tells one wanderer from another. Smooth noise at
/// three rates at once, each twice as quick and half as strong as the
/// last: the fractal noise Blender's noise modifier adds to a curve.
fn wander(at: f64, seed: u32) -> f64 {
    let (mut sum, mut strength, mut rate) = (0.0, 1.0, 1.0);
    for octave in 0..3 {
        // Each starts somewhere of its own, so they never fall in step.
        let seed = mixed(seed.wrapping_add(octave));
        sum += strength * noise(at * rate + (seed & 0xffff) as f64 / 256.0, seed);
        strength *= 0.5;
        rate *= 2.0;
    }
    // Three rates seldom pull the same way: this far it does reach.
    sum / 1.2
}

/// Smooth noise between -1 and 1: nothing at each whole `x`, a slope of
/// its own there, eased from one to the next (Perlin's, along a line).
fn noise(x: f64, seed: u32) -> f64 {
    let cell = x.floor();
    let within = x - cell;
    let slope = |cell: f64| {
        let mixed = mixed(seed ^ (cell as i64 as u32).wrapping_mul(0x9e37_79b9));
        mixed as f64 / u32::MAX as f64 * 2.0 - 1.0
    };
    let (from, to) = (slope(cell) * within, slope(cell + 1.0) * (within - 1.0));
    let eased = within * within * within * (within * (within * 6.0 - 15.0) + 10.0);
    2.0 * (from + (to - from) * eased)
}

/// `n` with its bits well mixed.
fn mixed(mut n: u32) -> u32 {
    n ^= n >> 16;
    n = n.wrapping_mul(0x7feb_352d);
    n ^= n >> 15;
    n = n.wrapping_mul(0x846c_a68b);
    n ^ (n >> 16)
}
