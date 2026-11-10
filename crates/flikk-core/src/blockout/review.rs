//! A blockout in words: what its camera sees at a few moments, and what
//! looks like a mistake. Whoever writes a scene as JSON can't watch it
//! play; this is read instead.

use super::render::{Detail, World};
use super::space::Vec3;
use super::{Ease, Scene, Shape, Target};

/// The most moments described, and the most objects named at each.
const MOMENTS: usize = 6;
const NAMED: usize = 8;
/// Where on an object it is looked for: across its width from its middle
/// and up its height, as shares of its size. Up its middle, and out to
/// each side where a tree's crown or a car's ends are.
const SPOTS: [(f32, f32); 5] = [
    (0.0, 0.2),
    (0.0, 0.5),
    (0.0, 0.8),
    (-0.4, 0.65),
    (0.4, 0.65),
];
/// Metres a second no person runs at.
const SPRINT: f32 = 11.0;

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
}

impl Scene {
    /// What the camera sees over the scene, in a picture `shape` (width
    /// to height).
    pub fn review(&self, shape: (u32, u32)) -> Review {
        let longer = shape.0.max(shape.1).max(1) as f32;
        let size = [shape.0, shape.1].map(|side| (side as f32 / longer * 320.0).max(8.0) as u32);
        let mut review = Review::default();

        // The start, the middle and the end, and each shot a cut starts.
        let mut moments = vec![0.0, self.duration * 0.5, self.duration];
        let cuts = (self.camera.keys.iter())
            .filter(|k| k.ease == Ease::Cut && k.t > 0.0 && k.t < self.duration);
        moments.extend(cuts.map(|k| k.t).take(MOMENTS - 3));
        moments.sort_by(f64::total_cmp);
        moments.dedup_by(|a, b| (*a - *b).abs() < 0.05);

        let mut ever = vec![false; self.objects.len()];
        let mut largest = vec![0f32; self.objects.len()];
        let mut note = |sights: &[Sight]| {
            for sight in sights.iter().filter(|s| s.clear > 0) {
                ever[sight.index] = true;
                largest[sight.index] = largest[sight.index].max(sight.share);
            }
        };
        for (i, t) in moments.iter().enumerate() {
            let world = World::of(self, *t, size[0], size[1], Detail::Draft);
            let sights = self.sights(&world, *t, size);
            note(&sights);
            review.seen.push(self.line(*t, &sights));
            if let Some(inside) = world.inside() {
                let name = &self.objects[inside as usize].name;
                review
                    .doubts
                    .push(format!("At {t:.1} s the camera is inside `{name}`."));
            } else if self.ground && world.eye().y < 0.02 {
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
            let world = World::of(self, t, size[0], size[1], Detail::Draft);
            note(&self.sights(&world, t, size));
        }
        self.doubts(&ever, &largest, &mut review);
        review
    }

    /// How each object that stands by itself shows at `t`.
    fn sights(&self, world: &World, t: f64, size: [u32; 2]) -> Vec<Sight> {
        let (wide, high) = (size[0] as f32, size[1] as f32);
        let mut sights = Vec::new();
        for (index, object) in self.objects.iter().enumerate() {
            // What rides something shows with it, but for a person.
            let rides = object.parent.is_some() && object.shape != Shape::Figure;
            if rides || object.hidden_now(t) {
                continue;
            }
            let place = self.place(index, t);
            let size = Vec3::of(object.size_now(t));
            // A spot on it: across its width from the middle, and up it.
            let spot =
                |across: f32, up: f32| place.point(Vec3::new(size.x * across, size.y * up, 0.0));
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
            if sight.clear > 0 || sight.behind.is_some() {
                sights.push(sight);
            }
        }
        sights.sort_by(|a, b| b.share.total_cmp(&a.share));
        sights
    }

    /// One line saying what `sights` show at `t`.
    fn line(&self, t: f64, sights: &[Sight]) -> String {
        let name = |index: usize| self.objects[index].name.as_str();
        let said: Vec<String> = (sights.iter().take(NAMED))
            .map(|sight| {
                let across = match sight.at.0 {
                    x if x < 0.36 => "left",
                    x if x > 0.64 => "right",
                    _ => "centre",
                };
                let hidden = match (sight.clear, sight.behind) {
                    (_, None) => String::new(),
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
        let Target::Object(name) = &self.camera.look_at else {
            return None;
        };
        let index = self.index(name)?;
        match sights.iter().find(|s| s.index == index) {
            Some(sight) if sight.clear > 0 => None,
            Some(Sight {
                behind: Some(front),
                ..
            }) => {
                let front = front.map_or("the ground", |i| self.objects[i as usize].name.as_str());
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
        let unseen: Vec<_> = (self.objects.iter().zip(ever))
            .filter(|(o, seen)| {
                !**seen
                    && o.parent.is_none()
                    && !o.hidden
                    && o.keys.iter().all(|k| k.hidden.is_none())
            })
            .map(|(o, _)| format!("`{}`", o.name))
            .collect();
        if !unseen.is_empty() {
            notes.push(format!("Never in the picture: {}.", unseen.join(", ")));
        }
        if let Target::Object(name) = &self.camera.look_at
            && let Some(index) = self.index(name)
            && ever[index]
            && largest[index] < 0.04
        {
            doubts.push(format!(
                "`{name}`, which the camera looks at, is never more than {:.0}% of the picture's \
                 height: bring the camera closer or narrow `fov`.",
                (largest[index] * 100.0).ceil()
            ));
        }
        for object in &self.objects {
            let name = &object.name;
            if let Some(late) = object.keys.iter().find(|k| k.t > self.duration + 1e-6) {
                notes.push(format!(
                    "`{name}` has a key at {:.1} s, after the end at {:.1} s: it is never reached.",
                    late.t, self.duration
                ));
            }
            if self.ground && object.parent.is_none() && object.at[1] < -0.05 {
                notes.push(format!("`{name}` starts below the ground."));
            }
            let goes = matches!(
                object.shape,
                Shape::Figure | Shape::Animal | Shape::Car | Shape::Bike
            );
            if goes
                && object
                    .off_course(self.duration)
                    .is_some_and(|off| off > 50.0)
            {
                doubts.push(format!(
                    "`{name}` travels sideways or backwards: give it `\"face\": \"path\"` unless \
                     that is meant."
                ));
            }
            if object.shape == Shape::Figure {
                let tall = object.size_now(0.0)[1] as f32;
                let speed = object.top_speed(self.duration);
                if speed * 1.75 / tall.max(0.01) > SPRINT {
                    doubts.push(format!(
                        "`{name}` goes at {speed:.0} m/s, faster than anyone runs."
                    ));
                }
            }
        }
        if let Some(late) = self.camera.keys.iter().find(|k| k.t > self.duration + 1e-6) {
            notes.push(format!(
                "The camera has a key at {:.1} s, after the end at {:.1} s: it is never reached.",
                late.t, self.duration
            ));
        }
    }
}
