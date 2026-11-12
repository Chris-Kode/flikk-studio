//! Many of one object: a row, a grid, a ring or a scatter of it, each one
//! its own a little where the scene says. A street of stalls, the windows
//! of a front, chairs round a table, a wood, a crowd.

use super::motion::mixed;
use super::{MAX_PARTS, MID_GREY, Object, finite};
use serde::{Deserialize, Serialize};

/// The candidates tried for each place in a scatter: the one furthest
/// from those already placed is kept.
const TRIED: u32 = 12;

/// Many of an object, where each stands, and how they differ. Each is
/// the object over again, called `name 1`, `name 2` and on; the name alone
/// is all of them. Give one of `step`, `ring` or `area`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repeat {
    /// How many; or how many along X, Y and Z, for a grid.
    pub count: Count,
    /// Metres from each to the next, the first at the object's `at`: a
    /// row along it. For a grid, the metres between them along X, Y and
    /// Z.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<[f64; 3]>,
    /// Or the radius of a ring around `at` they stand in, each turned to
    /// face its middle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ring: Option<f64>,
    /// Or the width (X) and depth (Z) of the ground around `at` they are
    /// scattered over, keeping apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<[f64; 2]>,
    /// How each differs from the others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vary: Option<Vary>,
    /// Another number scatters and varies them another way.
    #[serde(default, skip_serializing_if = "is_first")]
    pub seed: u32,
}

/// How many of a repeated object there are.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Count {
    One(u32),
    /// Along X, Y and Z.
    Each([u32; 3]),
}

/// How the ones of a repeat differ: each by its own share of these, the
/// same every time the scene is drawn.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vary {
    /// The most each is turned about the vertical, in degrees either way.
    #[serde(default, skip_serializing_if = "is_nothing")]
    pub turn: f64,
    /// The most each is larger or smaller: 0.2 for a fifth either way.
    #[serde(default, skip_serializing_if = "is_nothing")]
    pub scale: f64,
    /// The most each is lighter or darker.
    #[serde(default, skip_serializing_if = "is_nothing")]
    pub shade: f64,
    /// How far into their rounds each one's cycles and steps start, at
    /// most: 1 spreads them over a whole round, so a crowd is out of step.
    #[serde(default, skip_serializing_if = "is_nothing")]
    pub shift: f64,
}

fn is_first(seed: &u32) -> bool {
    *seed == 0
}

fn is_nothing(value: &f64) -> bool {
    *value == 0.0
}

/// Where one of a repeat stands off the object's `at`, and how much more
/// it is turned about the vertical, in degrees.
struct Spot {
    off: [f64; 3],
    turned: f64,
}

impl Repeat {
    /// Bring its numbers into range, and refuse what lays nothing out.
    pub(super) fn settle(&mut self, what: &str) -> Result<(), String> {
        let given = [
            self.step.is_some(),
            self.ring.is_some(),
            self.area.is_some(),
        ];
        if given.iter().filter(|given| **given).count() != 1 {
            return Err(format!(
                "{what}'s `repeat` needs where its copies stand: one of `step` (a row or a grid), \
                 `ring` or `area`"
            ));
        }
        if matches!(self.count, Count::Each(_)) && self.step.is_none() {
            return Err(format!(
                "{what}'s `repeat` counts along X, Y and Z, which is a grid: give it `step`, or \
                 one `count`"
            ));
        }
        finite(what, &self.step.unwrap_or_default())?;
        finite(what, &self.area.unwrap_or_default())?;
        finite(what, &[self.ring.unwrap_or(0.0)])?;
        if let Some(vary) = self.vary.as_mut() {
            finite(what, &[vary.turn, vary.scale, vary.shade, vary.shift])?;
            vary.scale = vary.scale.clamp(0.0, 0.9);
        }
        match self.total() {
            0 => Err(format!("{what}'s `repeat` has a `count` of none")),
            many if many > MAX_PARTS => Err(format!(
                "{what} repeats {many} times: a scene has at most {MAX_PARTS} parts"
            )),
            _ => Ok(()),
        }
    }

    fn total(&self) -> usize {
        match self.count {
            Count::One(count) => count as usize,
            Count::Each(each) => each
                .iter()
                .fold(1usize, |all, n| all.saturating_mul(*n as usize)),
        }
    }

    /// Where each one stands; `seed` tells this repeat from others.
    fn spots(&self, seed: u32) -> Vec<Spot> {
        let total = self.total();
        let plain = |off: [f64; 3]| Spot { off, turned: 0.0 };
        if let Some(radius) = self.ring {
            return (0..total)
                .map(|one| {
                    let round = one as f64 / total as f64 * std::f64::consts::TAU;
                    Spot {
                        off: [radius * round.sin(), 0.0, radius * round.cos()],
                        turned: round.to_degrees() + 180.0,
                    }
                })
                .collect();
        }
        if let Some([wide, deep]) = self.area {
            return scattered(total, seed)
                .into_iter()
                .map(|(x, z)| plain([(x - 0.5) * wide, 0.0, (z - 0.5) * deep]))
                .collect();
        }
        let step = self.step.unwrap_or_default();
        match self.count {
            Count::One(count) => (0..count)
                .map(|one| plain(step.map(|metres| metres * one as f64)))
                .collect(),
            Count::Each([across, up, deep]) => (0..up)
                .flat_map(|y| (0..deep).flat_map(move |z| (0..across).map(move |x| [x, y, z])))
                .map(|along| plain(std::array::from_fn(|axis| step[axis] * along[axis] as f64)))
                .collect(),
        }
    }
}

/// `count` places in a square one unit along each side, spread so that
/// none crowds another, and the first of them the same however many
/// follow: each is the candidate, of a few, furthest from those before it.
fn scattered(count: usize, seed: u32) -> Vec<(f64, f64)> {
    let mut placed: Vec<(f64, f64)> = Vec::with_capacity(count);
    for one in 0..count as u32 {
        let candidate = |tried: u32| {
            let of = |what: u32| chance(seed, one, tried * 2 + what);
            (of(100), of(101))
        };
        let room = |spot: &(f64, f64)| {
            (placed.iter())
                .map(|other| (spot.0 - other.0).hypot(spot.1 - other.1))
                .fold(f64::MAX, f64::min)
        };
        let best = (0..TRIED)
            .map(candidate)
            .max_by(|a, b| room(a).total_cmp(&room(b)));
        placed.extend(best);
    }
    placed
}

/// A number from 0 to 1 for the `one`th of the repeat seeded `seed`: its
/// own for each `what`, and the same every time.
fn chance(seed: u32, one: u32, what: u32) -> f64 {
    let own = mixed(one.wrapping_mul(0x9e37_79b9) ^ mixed(what.wrapping_add(0x85eb_ca6b)));
    mixed(seed ^ own) as f64 / u32::MAX as f64
}

impl Object {
    /// The objects this stands for when it repeats, each with how far
    /// into their rounds its cycles and steps start; `None` when it is
    /// one object. Each rides the object's name, which a joint then has.
    pub(super) fn repeated(&self) -> Option<Vec<(Object, f64)>> {
        let repeat = self.repeat.as_ref()?;
        let named = (self.name.bytes()).fold(repeat.seed, |seed, byte| mixed(seed ^ byte as u32));
        let vary = repeat.vary.clone().unwrap_or_default();
        let copies = repeat.spots(named).into_iter().enumerate();
        Some(
            copies
                .map(|(index, spot)| {
                    let one = index as u32;
                    // From one less to one more, its own for each `what`.
                    let either_way = |what: u32| 2.0 * chance(named, one, what) - 1.0;
                    let mut copy = self.clone();
                    copy.repeat = None;
                    copy.name = format!("{} {}", self.name, index + 1);
                    copy.parent = Some(self.name.clone());
                    copy.stand(&spot, vary.turn * either_way(0));
                    copy.grow(1.0 + vary.scale * either_way(1));
                    if vary.shade != 0.0 {
                        let shade = self.shade.unwrap_or(MID_GREY) + vary.shade * either_way(2);
                        copy.shade = Some(shade.clamp(0.0, 1.0));
                    }
                    (copy, vary.shift * chance(named, one, 3))
                })
                .collect(),
        )
    }

    /// Move it to `spot` of a repeat, turned `more` degrees on top, now
    /// and at every key.
    fn stand(&mut self, spot: &Spot, more: f64) {
        let places =
            std::iter::once(&mut self.at).chain(self.keys.iter_mut().flat_map(|key| &mut key.at));
        for at in places {
            (0..3).for_each(|axis| at[axis] += spot.off[axis]);
        }
        let turns = std::iter::once(&mut self.rotate)
            .chain(self.keys.iter_mut().flat_map(|key| &mut key.rotate));
        turns.for_each(|rotate| rotate[1] += spot.turned + more);
    }

    /// Make it `scale` times as large: a copy of a model by its `scale`,
    /// a solid by its size.
    fn grow(&mut self, scale: f64) {
        if self.model.is_some() {
            self.scale *= scale;
        } else if self.shape.is_some() {
            let size = self.size.get_or_insert([1.0; 3]);
            let sizes =
                std::iter::once(size).chain(self.keys.iter_mut().flat_map(|key| &mut key.size));
            sizes.for_each(|size| size.iter_mut().for_each(|side| *side *= scale));
        }
    }
}
