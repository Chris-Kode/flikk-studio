//! A blockout in words: what its camera sees at a few moments, and what
//! looks like a mistake. Whoever writes a scene as JSON can't watch it
//! play; this is read instead.

use super::Scene;
use super::space::Vec3;
use super::world::World;

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

        // The start, the middle and the end, and each shot a cut starts.
        let mut moments = vec![0.0, self.duration * 0.5, self.duration];
        let cuts = self.cuts().filter(|t| *t > 0.0 && *t < self.duration);
        moments.extend(cuts.take(MOMENTS - 3));
        moments.sort_by(f64::total_cmp);
        moments.dedup_by(|a, b| (*a - *b).abs() < 0.05);

        let mut ever = vec![false; self.parts.len()];
        let mut largest = vec![0f32; self.parts.len()];
        // How far off the ground each walker's lowest point got, and when.
        let mut off_ground: Vec<Option<(f32, f64)>> = vec![None; self.parts.len()];
        let mut tread = |world: &World, t: f64| {
            for index in self.walkers() {
                let off = world
                    .lowest(index as u16)
                    .filter(|off| off.abs() > OFF_GROUND);
                let worst = off_ground[index].map_or(0.0, |(worst, _)| worst.abs());
                if let Some(off) = off.filter(|off| off.abs() > worst) {
                    off_ground[index] = Some((off, t));
                }
            }
        };
        let mut note = |sights: &[Sight]| {
            for sight in sights.iter().filter(|s| s.clear > 0 || s.glimpsed) {
                ever[sight.index] = true;
                largest[sight.index] = largest[sight.index].max(sight.share);
            }
        };
        for (i, t) in moments.iter().enumerate() {
            let world = World::of(self, *t, size[0], size[1]);
            let sights = self.sights(&world, *t, size);
            note(&sights);
            tread(&world, *t);
            review.seen.push(self.line(*t, &sights));
            if let Some(inside) = world.inside() {
                let name = &self.parts[inside as usize].object.name;
                review
                    .doubts
                    .push(format!("At {t:.1} s the camera is inside `{name}`."));
            } else if self.ground && world.eye.y < 0.02 {
                review
                    .doubts
                    .push(format!("At {t:.1} s the camera is under the ground."));
            }
            if i == 0 {
                review.doubts.extend(self.start_doubt(&sights));
            }
        }
        // Between those moments, only whether each is in the picture.
        for step in 1..8 {
            let t = self.duration * step as f64 / 8.0;
            let world = World::of(self, t, size[0], size[1]);
            note(&self.sights(&world, t, size));
            tread(&world, t);
        }
        for (index, off) in off_ground.iter().enumerate() {
            if let Some((off, t)) = off {
                let name = &self.parts[index].object.name;
                let (far, side) = (off.abs(), if *off < 0.0 { "under" } else { "over" });
                review.doubts.push(format!(
                    "At {t:.1} s the lowest point of `{name}` is {far:.2} m {side} the ground: \
                     give it `\"on\": \"ground\"` and its feet stay on it."
                ));
            }
        }
        self.doubts(&ever, &largest, &mut review);
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

    /// How each thing a viewer would name shows at `t`: what stands by
    /// itself, and each copy of a model, not their parts.
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
        let mut sights = Vec::new();
        for (index, part) in self.parts.iter().enumerate() {
            if part.thing != index || world.moment.gone[index] {
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
            if sight.clear > 0 || sight.behind.is_some() || sight.glimpsed {
                sights.push(sight);
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
        let name = |index: usize| self.parts[index].object.name.as_str();
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
                format!(
                    "`{}` {across}, {share:.0}% of the height{hidden}",
                    name(sight.index)
                )
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
                    self.parts[i as usize].object.name.as_str()
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
        let unseen: Vec<_> = things()
            .filter(|(index, part)| {
                !ever[*index]
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

/// The first of the times keys give that is after the end of `duration`
/// seconds.
fn late(times: impl Iterator<Item = Option<f64>>, duration: f64) -> Option<f64> {
    times.flatten().find(|t| *t > duration + 1e-6)
}
