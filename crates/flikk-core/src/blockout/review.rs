//! A blockout in words: what its camera sees at a few moments, and what
//! looks like a mistake. Whoever writes a scene as JSON can't watch it
//! play; this is read instead.

use super::Scene;
use super::space::Vec3;
use super::world::World;
use std::collections::HashMap;

/// The most moments described, and the most objects named at each.
const MOMENTS: usize = 6;
const NAMED: usize = 8;
/// Where on a thing it is looked for: across its width from its middle
/// and up its height, as shares of the box it fills. Up its middle, and
/// out to each side where a tree's crown or a car's ends are.
const SPOTS: [(f32, f32); 5] = [
    (0.0, 0.2),
    (0.0, 0.5),
    (0.0, 0.8),
    (-0.4, 0.65),
    (0.4, 0.65),
];
/// How many times a second a scene is watched, and the metres a second
/// nothing on feet goes faster than.
const WATCHED: f64 = 12.0;
const SPRINT: f32 = 12.5;
/// Metres a walker's lowest point may be off the ground and go unsaid.
const OFF_GROUND: f32 = 0.06;
/// What a blockout's camera sees, said for its writer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Review {
    /// What is in the picture at a few moments, one line each.
    pub seen: Vec<String>,
    /// What looks like a mistake a viewer would see. Nothing here stops
    /// the scene being used: a reveal hides its subject on purpose.
    pub doubts: Vec<String>,
    /// What could be tidied and no viewer would notice: an object never
    /// in the picture, a key after the end.
    pub notes: Vec<String>,
}

impl Review {
    /// All of it as text, what to check last.
    pub fn text(&self) -> String {
        let mut text = self.seen.join("\n");
        let check: Vec<&str> = (self.doubts.iter().chain(&self.notes))
            .map(String::as_str)
            .collect();
        if !check.is_empty() {
            text.push_str("\nCheck:\n- ");
            text.push_str(&check.join("\n- "));
        }
        text
    }
}

/// How one object shows at a moment.
struct Sight {
    index: usize,
    /// Its middle in the picture, 0..1 across and down.
    at: (f32, f32),
    /// Its height as a share of the picture's.
    share: f32,
    /// How many of the [`SPOTS`] looked for on it are in the picture with
    /// nothing in front, and what is in front of the others.
    clear: usize,
    behind: Option<Option<u16>>,
    /// Whether some of it shows somewhere in the picture.
    glimpsed: bool,
    /// For a repeat, said as one: how many of it show. The rest is then
    /// of the one that shows largest.
    showing: usize,
}

/// What is found watching a scene all through, moment by moment.
struct Watch {
    /// Whether each object was ever in the picture, and how tall it got
    /// there as a share of the picture's height.
    ever: Vec<bool>,
    largest: Vec<f32>,
    /// How far off the ground each walker's lowest point got, and when.
    off_ground: Vec<Option<(f32, f64)>>,
    /// The objects the camera was inside, and when it first was; when it
    /// was first under the ground.
    inside: Vec<(usize, f64)>,
    under_ground: Option<f64>,
    /// The stretches the camera's subject was not to be seen for: which
    /// object, from when until when, and behind what (nothing when it was
    /// out of the picture).
    lost: Vec<(usize, f64, f64, Option<Option<u16>>)>,
    /// Where each thing was at the last moment, and when that was.
    was: Vec<Option<Vec3>>,
    then: Option<f64>,
    /// The fastest each thing went, and when.
    fastest: Vec<(f32, f64)>,
    /// Pairs of things of which one went through the other, and when it
    /// first did.
    through: Vec<(usize, usize, f64)>,
    /// The ends of limbs that were short of what they reach for: what
    /// that is, by how much at the worst, and when.
    short: Vec<(usize, usize, f32, f64)>,
}

impl Watch {
    fn over(parts: usize) -> Self {
        Self {
            ever: vec![false; parts],
            largest: vec![0.0; parts],
            off_ground: vec![None; parts],
            inside: Vec::new(),
            under_ground: None,
            lost: Vec::new(),
            was: vec![None; parts],
            then: None,
            fastest: vec![(0.0, 0.0); parts],
            through: Vec::new(),
            short: Vec::new(),
        }
    }

    /// Take in `scene` at `t`: `world` as it stands then, and how each
    /// thing shows.
    fn see(&mut self, scene: &Scene, world: &World, t: f64, sights: &[Sight]) {
        for sight in sights.iter().filter(|s| s.clear > 0 || s.glimpsed) {
            self.ever[sight.index] = true;
            self.largest[sight.index] = self.largest[sight.index].max(sight.share);
        }
        for index in scene.walkers() {
            let off = (world.lowest(index as u16)).filter(|off| off.abs() > OFF_GROUND);
            let worst = self.off_ground[index].map_or(0.0, |(worst, _)| worst.abs());
            if let Some(off) = off.filter(|off| off.abs() > worst) {
                self.off_ground[index] = Some((off, t));
            }
        }
        match world.inside() {
            Some(inside)
                if self
                    .inside
                    .iter()
                    .all(|(object, _)| *object != inside as usize) =>
            {
                self.inside.push((inside as usize, t));
            }
            None if scene.ground && world.eye.y < 0.02 => {
                self.under_ground.get_or_insert(t);
            }
            _ => {}
        }
        self.see_subject(scene, world, t, sights);
        self.see_moving(scene, world, t);
        for (end, to, short) in scene.short_reaches(t, &world.moment.places) {
            match self.short.iter_mut().find(|(other, ..)| *other == end) {
                Some(worst) if worst.2 >= short => {}
                Some(worst) => *worst = (end, to, short, t),
                None => self.short.push((end, to, short, t)),
            }
        }
        self.then = Some(t);
    }

    /// Whether what the camera looks at at `t` is to be seen.
    fn see_subject(&mut self, scene: &Scene, world: &World, t: f64, sights: &[Sight]) {
        let Some(subject) = scene.subject(t).map(|part| scene.parts[part].thing) else {
            return;
        };
        if world.moment.gone[subject] {
            return;
        }
        let behind = match sights.iter().find(|sight| sight.index == subject) {
            Some(sight) if sight.clear > 0 => return,
            Some(sight) if sight.behind.is_some() => sight.behind,
            _ => None,
        };
        // It goes on from the moment before, or starts now.
        match self.lost.last_mut() {
            Some(lost) if lost.0 == subject && Some(lost.2) == self.then && lost.3 == behind => {
                lost.2 = t;
            }
            _ => self.lost.push((subject, t, t, behind)),
        }
    }

    /// How fast each thing goes at `t`, and what goes through what.
    fn see_moving(&mut self, scene: &Scene, world: &World, t: f64) {
        let moment = &world.moment;
        let things = (scene.parts.iter().enumerate())
            .filter(|(index, part)| part.thing == *index && scene.stands_alone(*index));
        let now: Vec<Option<Vec3>> = (0..scene.parts.len())
            .map(|index| (!moment.gone[index]).then(|| moment.places[index].at))
            .collect();
        let since = self.then.map_or(0.0, |then| (t - then) as f32);
        // How far a thing went since the last moment, when it was there.
        let moved = |index: usize| Some(now[index]? - self.was[index]?);
        if since > 0.0 {
            for (index, _) in things {
                let speed = moved(index).map_or(0.0, |moved| moved.length() / since);
                if speed > self.fastest[index].0 {
                    self.fastest[index] = (speed, t);
                }
            }
            for (inner, outer) in world.through() {
                let (inner, outer) = (inner as usize, outer as usize);
                let apart = |a: usize, b: usize| {
                    let on = scene.parts[a].rest.as_ref();
                    !on.is_some_and(|rest| {
                        rest.surface.iter().any(|of| scene.parts[*of].thing == b)
                    })
                };
                // The ones of a repeat are said as one.
                let said = (scene.said_as(inner), scene.said_as(outer));
                let known = |pair: &(usize, usize, f64)| {
                    let pair = (scene.said_as(pair.0), scene.said_as(pair.1));
                    pair == said || pair == (said.1, said.0)
                };
                // One at rest in the other is in it, not going through it.
                let passing = match (moved(inner), moved(outer)) {
                    (Some(a), Some(b)) => (a - b).length() / since > 0.5,
                    _ => false,
                };
                let top = |index: usize| scene.stands_alone(index);
                if passing
                    && said.0 != said.1
                    && top(inner)
                    && top(outer)
                    && apart(inner, outer)
                    && apart(outer, inner)
                    && !self.through.iter().any(known)
                {
                    self.through.push((inner, outer, t));
                }
            }
        }
        self.was = now;
    }

    /// Add what was found that a viewer would take for a mistake to
    /// `doubts`.
    fn say(&self, scene: &Scene, doubts: &mut Vec<String>) {
        let name = |index: usize| scene.parts[scene.said_as(index)].object.name.as_str();
        for (inside, t) in &self.inside {
            doubts.push(format!(
                "At {t:.1} s the camera is inside `{}`.",
                name(*inside)
            ));
        }
        if let Some(t) = self.under_ground {
            doubts.push(format!("At {t:.1} s the camera is under the ground."));
        }
        // A subject lost from the start is said of the opening picture.
        let lost = (self.lost.iter()).filter(|(_, from, to, _)| *from > 0.0 && to - from >= 0.45);
        for (subject, from, to, behind) in lost {
            let how = match behind {
                Some(front) => format!(
                    "hidden behind {}",
                    front.map_or("the ground".to_owned(), |i| format!(
                        "`{}`",
                        name(i as usize)
                    ))
                ),
                None => "out of the picture".to_owned(),
            };
            doubts.push(format!(
                "From {from:.1} to {to:.1} s `{}`, which the camera looks at, is {how}.",
                name(*subject)
            ));
        }
        for (index, off) in self.off_ground.iter().enumerate() {
            if let Some((off, t)) = off {
                let (far, side) = (off.abs(), if *off < 0.0 { "under" } else { "over" });
                doubts.push(format!(
                    "At {t:.1} s the lowest point of `{}` is {far:.2} m {side} the ground: give \
                     it `\"on\": \"ground\"` and its feet stay on it.",
                    name(index)
                ));
            }
        }
        for (index, (speed, t)) in self.fastest.iter().enumerate() {
            if *speed > SPRINT && !scene.parts[index].strides.is_empty() {
                doubts.push(format!(
                    "`{}` goes at {speed:.0} m/s around {t:.1} s, faster than anyone runs: give \
                     its keys more time or less ground.",
                    name(index)
                ));
            }
        }
        for (inner, outer, t) in &self.through {
            doubts.push(format!(
                "At {t:.1} s `{}` goes through `{}`.",
                name(*inner),
                name(*outer)
            ));
        }
        for (end, to, short, t) in &self.short {
            doubts.push(format!(
                "At {t:.1} s `{}` is {short:.2} m short of `{}`, as far as its limb goes: bring \
                 them nearer.",
                name(*end),
                name(*to)
            ));
        }
    }
}

/// How many rays across and down the picture look for what shows in it.
const LOOKS: (usize, usize) = (32, 18);

impl Scene {
    /// What the camera sees over the scene, in a picture `shape` (width
    /// to height).
    pub fn review(&self, shape: (u32, u32)) -> Review {
        let longer = shape.0.max(shape.1).max(1) as f32;
        let size = [shape.0, shape.1].map(|side| (side as f32 / longer * 320.0).max(8.0) as u32);
        let mut review = Review::default();

        // Said in words: the start, the middle and the end, and each shot
        // a cut starts.
        let mut moments = vec![0.0, self.duration * 0.5, self.duration];
        let cuts = self.cuts().filter(|t| *t > 0.0 && *t < self.duration);
        moments.extend(cuts.take(MOMENTS - 3));
        moments.sort_by(f64::total_cmp);
        moments.dedup_by(|a, b| (*a - *b).abs() < 0.05);
        for (i, t) in moments.iter().enumerate() {
            let world = World::of(self, *t, size[0], size[1]);
            let sights = self.sights(&world, *t, size);
            review.seen.push(self.line(*t, &sights));
            if i == 0 {
                review.doubts.extend(self.start_doubt(&sights));
            }
        }
        // Watched all through, a few times a second, for what goes wrong
        // between those.
        let mut watch = Watch::over(self.parts.len());
        let looks = ((self.duration * WATCHED).ceil() as usize).clamp(8, 360);
        let moments: Vec<f64> = (0..=looks)
            .map(|look| self.duration * look as f64 / looks as f64)
            .collect();
        // Each moment is set up on a thread of its own, a few at a time,
        // and taken in as they come in order.
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
        for together in moments.chunks(threads) {
            let seen: Vec<_> = std::thread::scope(|scope| {
                let looking: Vec<_> = (together.iter())
                    .map(|t| {
                        scope.spawn(move || {
                            let world = World::of(self, *t, size[0], size[1]);
                            let sights = self.sights(&world, *t, size);
                            (world, sights)
                        })
                    })
                    .collect();
                (looking.into_iter()).map(|look| look.join().ok()).collect()
            });
            // A moment that could not be set up is not watched.
            for (t, (world, sights)) in
                (together.iter().zip(&seen)).filter_map(|(t, seen)| Some((t, seen.as_ref()?)))
            {
                watch.see(self, world, *t, sights);
            }
        }
        watch.say(self, &mut review.doubts);
        self.doubts(&watch.ever, &watch.largest, &mut review);
        review
    }

    /// The things that walk on the ground and could be off it: what is
    /// written at the ground's height, moves limbs by the ground it
    /// covers, and doesn't say what it stands on.
    fn walkers(&self) -> impl Iterator<Item = usize> + '_ {
        let paced = |index: usize| self.paced(index);
        (self.parts.iter().enumerate())
            .filter(move |(index, part)| {
                self.ground
                    && self.hills.is_none()
                    && part.thing == *index
                    && part.parent.is_none()
                    && part.rest.is_none()
                    && part.object.at[1].abs() < 0.01
                    && part
                        .object
                        .keys
                        .iter()
                        .all(|key| key.at.is_none_or(|at| at[1].abs() < 0.01))
                    && paced(*index)
            })
            .map(|(index, _)| index)
    }

    /// Whether part `index` moves over the ground as what walks or rolls
    /// does: its feet step, or a part of it moves by the ground covered.
    fn paced(&self, index: usize) -> bool {
        let by_ground = |part: &super::build::Part| {
            let cycle = part.object.cycle.as_ref();
            part.whole == index && cycle.is_some_and(|cycle| cycle.metres.is_some())
        };
        !self.parts[index].strides.is_empty() || self.parts.iter().any(by_ground)
    }

    /// The part `index` is said as: the repeat it is one of, else itself.
    fn said_as(&self, index: usize) -> usize {
        self.parts[index].one_of.unwrap_or(index)
    }

    /// Whether part `index` rides nothing: it has no parent, or is one of
    /// a repeat that has none.
    pub(super) fn stands_alone(&self, index: usize) -> bool {
        let part = &self.parts[index];
        part.lands || part.parent.is_none()
    }

    /// How each thing a viewer would name shows at `t`: what stands by
    /// itself, and each copy of a model, not their parts. The ones of a
    /// repeat are said as one.
    fn sights(&self, world: &World, t: f64, size: [u32; 2]) -> Vec<Sight> {
        let (wide, high) = (size[0] as f32, size[1] as f32);
        // What a spread of rays through the picture meets: a thing too
        // large or too far to one side for its spots still shows.
        let mut glimpsed = vec![false; self.parts.len()];
        for look in 0..LOOKS.0 * LOOKS.1 {
            let across = (look % LOOKS.0) as f32 + 0.5;
            let down = (look / LOOKS.0) as f32 + 0.5;
            let met = world.thing_at(across * wide / LOOKS.0 as f32, down * high / LOOKS.1 as f32);
            if let Some(thing) = met {
                glimpsed[thing as usize] = true;
            }
        }
        let mut repeats = vec![false; self.parts.len()];
        for all in self.parts.iter().filter_map(|part| part.one_of) {
            repeats[all] = true;
        }
        let mut sights: Vec<Sight> = Vec::new();
        let mut said: HashMap<usize, usize> = HashMap::new();
        for (index, part) in self.parts.iter().enumerate() {
            let hole = part.object.cut.is_some();
            if part.thing != index || world.moment.gone[index] || repeats[index] || hole {
                continue;
            }
            let place = world.moment.places[index];
            let (low, high_corner) = (Vec3::of(part.bounds.0), Vec3::of(part.bounds.1));
            let reach = high_corner - low;
            // A spot on it: across its width from the middle, and up it.
            let spot = |across: f32, up: f32| {
                let middle = (low + high_corner) * 0.5;
                place.point(Vec3::new(
                    middle.x + reach.x * across,
                    low.y + reach.y * up,
                    middle.z,
                ))
            };
            let ends = (
                world.pixel_of(spot(0.0, 0.0)),
                world.pixel_of(spot(0.0, 1.0)),
            );
            let (Some(foot), Some(head)) = ends else {
                continue;
            };
            let mut sight = Sight {
                index,
                at: (
                    (foot.0 + head.0) * 0.5 / wide,
                    (foot.1 + head.1) * 0.5 / high,
                ),
                share: (foot.0 - head.0).hypot(foot.1 - head.1) / high,
                clear: 0,
                behind: None,
                glimpsed: glimpsed[index],
                showing: 1,
            };
            for (across, up) in SPOTS {
                let point = spot(across, up);
                let inside = world
                    .pixel_of(point)
                    .is_some_and(|(x, y, _)| (0.0..wide).contains(&x) && (0.0..high).contains(&y));
                match world.in_front(point, index as u16) {
                    _ if !inside => {}
                    None => sight.clear += 1,
                    front => sight.behind = front,
                }
            }
            if sight.clear == 0 && sight.behind.is_none() && !sight.glimpsed {
                continue;
            }
            let Some(all) = part.one_of else {
                sights.push(sight);
                continue;
            };
            sight.index = all;
            sight.showing = usize::from(sight.clear > 0 || sight.glimpsed);
            match said.get(&all) {
                Some(at) => {
                    let one = &mut sights[*at];
                    let showing = one.showing + sight.showing;
                    let shows = |sight: &Sight| (sight.showing > 0, sight.share);
                    if shows(&sight) > shows(one) {
                        *one = sight;
                    }
                    one.showing = showing;
                }
                None => {
                    said.insert(all, sights.len());
                    sights.push(sight);
                }
            }
        }
        // What the camera looks at first, then what was built of parts
        // (the shot's characters and things), then the plain set; the
        // larger first among each.
        let looked_at = self.subject(t).map(|found| self.parts[found].thing);
        let rank = |sight: &Sight| match self.parts[sight.index].object.shape {
            _ if Some(sight.index) == looked_at => 2,
            None => 1,
            Some(_) => 0,
        };
        sights.sort_by(|a, b| (rank(b).cmp(&rank(a))).then(b.share.total_cmp(&a.share)));
        sights
    }

    /// One line saying what `sights` show at `t`.
    fn line(&self, t: f64, sights: &[Sight]) -> String {
        let name = |index: usize| self.parts[self.said_as(index)].object.name.as_str();
        let of = |all: usize| {
            (self.parts.iter())
                .filter(|part| part.one_of == Some(all))
                .count()
        };
        let subject = self.subject(t).map(|part| self.parts[part].thing);
        let said: Vec<String> = (sights.iter().take(NAMED))
            .map(|sight| {
                let across = match sight.at.0 {
                    x if x < 0.36 => "left",
                    x if x > 0.64 => "right",
                    _ => "centre",
                };
                let hidden = match (sight.clear, sight.behind) {
                    (0, None) => ", at the edge".to_owned(),
                    (_, None) => String::new(),
                    (0, Some(_)) if sight.glimpsed => ", partly hidden".to_owned(),
                    (clear, Some(front)) => format!(
                        ", {} behind {}",
                        if clear == 0 { "hidden" } else { "partly" },
                        front.map_or("the ground", |i| name(i as usize))
                    ),
                };
                let share = (sight.share * 100.0).round();
                // What the camera looks at is said as a shot is called.
                let shot = match Some(sight.index) == subject {
                    true => format!(" ({})", shot_called(sight.share)),
                    false => String::new(),
                };
                match of(sight.index) {
                    0 => format!(
                        "`{}` {across}, {share:.0}% of the height{shot}{hidden}",
                        name(sight.index)
                    ),
                    all if sight.showing == 0 => {
                        format!("`{}` (none of its {all} to be seen)", name(sight.index))
                    }
                    all => format!(
                        "`{}` ({} of its {all} in the picture) {across}, up to {share:.0}% of \
                         the height{shot}",
                        name(sight.index),
                        sight.showing
                    ),
                }
            })
            .collect();
        let more = match sights.len().saturating_sub(NAMED) {
            0 => String::new(),
            more => format!("; and {more} more"),
        };
        match said.is_empty() {
            true => format!("{t:.1} s: nothing in the picture"),
            false => format!("{t:.1} s: {}{more}", said.join("; ")),
        }
    }

    /// What is wrong with the opening picture: the object the camera
    /// looks at should be in it.
    fn start_doubt(&self, sights: &[Sight]) -> Option<String> {
        let subject = self.subject(0.0)?;
        let name = &self.parts[subject].object.name;
        let index = self.parts[subject].thing;
        match sights.iter().find(|s| s.index == index) {
            Some(sight) if sight.clear > 0 => None,
            Some(Sight {
                behind: Some(front),
                ..
            }) => {
                let front = front.map_or("the ground", |i| {
                    self.parts[self.said_as(i as usize)].object.name.as_str()
                });
                Some(format!(
                    "At the start `{name}`, which the camera looks at, is hidden behind {front}."
                ))
            }
            _ => Some(format!(
                "At the start `{name}`, which the camera looks at, is not in the picture."
            )),
        }
    }

    /// What looks like a mistake over the whole scene, and what could be
    /// tidied. `ever` says which objects were in the picture at some
    /// moment, `largest` how tall each got there.
    fn doubts(&self, ever: &[bool], largest: &[f32], review: &mut Review) {
        let Review { doubts, notes, .. } = review;
        let things =
            || (self.parts.iter().enumerate()).filter(|(index, part)| part.thing == *index);
        let shown_later = |index: usize| {
            (self.parts.iter())
                .any(|p| p.thing == index && p.object.keys.iter().any(|k| k.hidden.is_some()))
        };
        // Only what has something to draw could have been.
        let mut drawn = vec![false; self.parts.len()];
        for part in self.parts.iter().filter(|part| part.is_solid()) {
            drawn[self.said_as(part.thing)] = true;
        }
        let unseen: Vec<_> = things()
            .filter(|(index, part)| {
                !ever[*index]
                    && drawn[*index]
                    && part.parent.is_none()
                    && !self.hidden(*index, 0.0)
                    && !shown_later(*index)
            })
            .map(|(_, part)| format!("`{}`", part.object.name))
            .collect();
        if !unseen.is_empty() {
            notes.push(format!("Never in the picture: {}.", unseen.join(", ")));
        }
        if let Some(subject) = self.subject(0.0)
            && let (name, index) = (&self.parts[subject].object.name, self.parts[subject].thing)
            && ever[index]
            && largest[index] < 0.04
        {
            doubts.push(format!(
                "`{name}`, which the camera looks at, is never more than {:.0}% of the picture's \
                 height: bring the camera closer or narrow `fov`.",
                (largest[index] * 100.0).ceil()
            ));
        }
        for (index, part) in self.parts.iter().enumerate() {
            let (name, object) = (&part.object.name, &part.object);
            if let Some(late) = late(object.keys.iter().map(|k| k.t), self.duration) {
                notes.push(format!(
                    "`{name}` has a key at {late:.1} s, after the end at {:.1} s: it is never \
                     reached.",
                    self.duration
                ));
            }
            if let Some((from, to, turned, short)) = part.moves.long_turn() {
                doubts.push(format!(
                    "`{name}` turns {:.0}° between {from:.1} s and {to:.1} s, the long way round: \
                     angles are taken as written, so write {short:.0} there for the short way.",
                    turned.abs()
                ));
            }
            if part.thing != index {
                continue;
            }
            if self.ground && part.parent.is_none() && object.at[1] + part.bounds.1[1] < -0.05 {
                notes.push(format!("`{name}` starts below the ground."));
            }
            // What steps or rolls with the ground it covers has a front.
            if self.paced(index) && part.off_course(self.duration).is_some_and(|off| off > 50.0) {
                doubts.push(format!(
                    "`{name}` travels sideways or backwards: give it `\"face\": \"path\"` unless \
                     that is meant."
                ));
            }
        }
        let main = std::iter::once(("The camera".to_owned(), &self.camera));
        let others =
            (self.cameras.iter()).map(|(name, camera)| (format!("Camera `{name}`"), camera));
        for (what, camera) in main.chain(others) {
            if let Some(late) = late(camera.keys.iter().map(|k| k.t), self.duration) {
                notes.push(format!(
                    "{what} has a key at {late:.1} s, after the end at {:.1} s: it is never \
                     reached.",
                    self.duration
                ));
            }
        }
        if let Some(late) = self.cuts.iter().find(|cut| cut.t > self.duration + 1e-6) {
            notes.push(format!(
                "The cut to `{}` at {:.1} s is after the end at {:.1} s: it is never reached.",
                late.camera, late.t, self.duration
            ));
        }
        let cut_to: Vec<&str> = self.cuts.iter().map(|cut| cut.camera.as_str()).collect();
        for name in self
            .cameras
            .keys()
            .filter(|name| !cut_to.contains(&name.as_str()))
        {
            notes.push(format!(
                "Camera `{name}` is never cut to: give it a place in `cuts`."
            ));
        }
    }
}

/// What a shot of a standing figure `share` of the picture's height tall
/// is called.
fn shot_called(share: f32) -> &'static str {
    match share {
        s if s < 0.35 => "a very wide shot",
        s if s < 0.65 => "a wide shot",
        s if s < 1.03 => "a full shot",
        s if s < 1.46 => "a medium wide shot",
        s if s < 2.13 => "a medium shot",
        s if s < 3.43 => "a medium close-up",
        _ => "a close-up",
    }
}

/// The first of the times keys give that is after the end of `duration`
/// seconds.
fn late(times: impl Iterator<Item = Option<f64>>, duration: f64) -> Option<f64> {
    times.flatten().find(|t| *t > duration + 1e-6)
}
