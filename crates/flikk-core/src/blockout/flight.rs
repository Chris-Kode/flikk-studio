//! Things let go: a cup dropped, a ball thrown. Once nothing holds it a
//! thing goes as its speed and gravity take it, which says where it is
//! at any later time, so its way through the air is worked out once, when
//! the scene is built, with where it comes to rest.

use super::build::AimAt;
use super::motion::Standing;
use super::space::{Place, Vec3};
use super::{FPS, Scene};

/// Metres a second faster that a thing falls, each second.
const GRAVITY: f32 = 9.81;
/// The fastest a thing leaves what let it go, in metres a second: the
/// last move of a hand is not a throw.
const LET_GO: f32 = 12.0;
/// How far above its lowest point a thing let go is taken as resting on
/// a surface it is pressed into, rather than under it.
const PRESSED: f32 = 0.3;
/// The shortest and the longest a throw is in the air for, in seconds.
const FLIGHT: (f32, f32) = (0.12, 8.0);

/// How a thing goes through the air from where it was let go.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Flight {
    from: Vec3,
    /// Metres a second, as it left.
    pub speed: Vec3,
    /// Seconds until it comes to rest, or is caught: never, for one
    /// still in the air when the scene ends.
    pub lasts: f64,
    /// Where it rests from then on.
    rest: Vec3,
}

impl Flight {
    /// Where it is `since` seconds after it was let go.
    pub fn at(&self, since: f64) -> Vec3 {
        if since >= self.lasts {
            return self.rest;
        }
        self.through(since.max(0.0) as f32)
    }

    /// Where its speed and gravity have it after `seconds` in the air.
    fn through(&self, seconds: f32) -> Vec3 {
        self.from + self.speed * seconds - Vec3::UP * (0.5 * GRAVITY * seconds * seconds)
    }
}

impl Scene {
    /// How part `part` goes from `t`, when its hold `hold` lets it go as
    /// things stand at `now`: thrown where that hold says, else with the
    /// speed it has, down to what is under it.
    pub(super) fn flown(&self, part: usize, hold: usize, t: f64, now: &Standing) -> Flight {
        let from = now.places[part].at;
        let held = &self.parts[part].held;
        let thrown = held[hold].thrown.as_ref();
        // What had it until now is not what it lands on.
        let let_go_by = held[..hold]
            .iter()
            .rev()
            .find_map(|before| before.at.and(before.by))
            .map(|by| self.parts[by].thing);
        let mut flight = Flight {
            from,
            speed: match thrown {
                Some((to, seconds)) => self.launch(from, t, to, *seconds).0,
                None => self.speed_of(part, t, from),
            },
            lasts: f64::INFINITY,
            rest: from,
        };
        // Thrown to a thing, it is caught: that has it from then on.
        if let Some((to @ AimAt::Part(_), seconds)) = thrown {
            flight.lasts = self.launch(from, t, to, *seconds).1 as f64;
            flight.rest = flight.through(flight.lasts as f32);
            return flight;
        }
        let own: Vec<usize> = (0..self.parts.len())
            .filter(|index| self.parts[*index].is_solid() && self.rides(*index, part))
            .collect();
        let lowest = (own.iter())
            .map(|of| self.solid(*of, now.places[*of], now.sizes[*of]).lowest().y)
            .fold(from.y, f32::min);
        let under = from.y - lowest;
        let lands_on = |index: usize| {
            let other = &self.parts[index];
            other.is_solid() && !self.rides(index, part) && Some(other.thing) != let_go_by
        };
        // Frame by frame, until its lowest point is down on something.
        let frame = 1.0 / FPS as f32;
        let mut above = None;
        for step in 0.. {
            let (seconds, time) = (step as f32 * frame, t + step as f64 / FPS as f64);
            if time > self.duration + frame as f64 {
                break;
            }
            let at = flight.through(seconds);
            let falling = flight.speed.y - GRAVITY * seconds <= 1e-3;
            let standing = self.stand(time.min(self.duration));
            let floor = self.floor_under(&standing, time, at - Vec3::UP * under, lands_on);
            let gap = floor.map(|floor| at.y - under - floor);
            if let Some(gap) = gap.filter(|gap| falling && *gap <= 0.0) {
                // Between the frame before and this one, where the gap
                // closed.
                let back = match above {
                    Some(above) if above > 0.0 => frame * gap / (gap - above),
                    _ => 0.0,
                };
                let landed = flight.through(seconds - back);
                let floor = floor.unwrap_or(landed.y);
                flight.lasts = (seconds - back) as f64;
                // Set down on what it was pressed into, not sunk in it.
                flight.rest = Vec3::new(landed.x, floor + under, landed.z);
                break;
            }
            above = gap;
        }
        flight
    }

    /// The speed part `part`, at `from` at `t`, has as it is let go: what
    /// it was moved at over the frame before.
    fn speed_of(&self, part: usize, t: f64, from: Vec3) -> Vec3 {
        let frame = 1.0 / FPS as f64;
        if t < frame {
            return Vec3::ZERO;
        }
        let speed = (from - self.stand(t - frame).places[part].at) * FPS as f32;
        match speed.length() {
            fast if fast > LET_GO => speed * (LET_GO / fast),
            _ => speed,
        }
    }

    /// The speed a thing leaves `from` with at `t` to come to `to`, and
    /// the seconds it is in the air: `seconds` when that is said, else
    /// what the easiest throw takes. What it is thrown to is aimed at
    /// where it will be by then.
    fn launch(&self, from: Vec3, t: f64, to: &AimAt, seconds: Option<f64>) -> (Vec3, f32) {
        let aimed = |after: f32| {
            let time = (t + after as f64).min(self.duration);
            match to {
                AimAt::Point(point) => self.over_land(Vec3::of(*point)),
                AimAt::Part(target) => self.middle(*target, self.stand(time).places[*target]),
                AimAt::Camera => {
                    let rig = self.rig(time);
                    let mount = rig
                        .mount
                        .map_or(Place::ORIGIN, |m| self.stand(time).places[m]);
                    mount.point(rig.at(time))
                }
                AimAt::Ahead => from,
            }
        };
        let flies = |off: Vec3| match seconds {
            Some(seconds) => seconds as f32,
            None => easiest(off),
        };
        let mut lasts = flies(aimed(0.0) - from);
        for _ in 0..3 {
            lasts = flies(aimed(lasts) - from);
        }
        let lasts = lasts.clamp(FLIGHT.0, FLIGHT.1);
        let off = aimed(lasts) - from;
        let up = (off.y + 0.5 * GRAVITY * lasts * lasts) / lasts;
        (Vec3::new(off.x / lasts, up, off.z / lasts), lasts)
    }

    /// How high the top of what is under `point` is at `time`, the parts
    /// standing as `standing` has them: the highest of the solids
    /// `lands_on` allows that is no more than a little above it, else
    /// the ground. `None` where there is neither.
    fn floor_under(
        &self,
        standing: &Standing,
        time: f64,
        point: Vec3,
        lands_on: impl Fn(usize) -> bool,
    ) -> Option<f32> {
        let from = point + Vec3::UP * PRESSED;
        let tops = (0..self.parts.len())
            .filter(|index| lands_on(*index) && !self.hidden(*index, time))
            .filter_map(|index| {
                let solid = self.solid(index, standing.places[index], standing.sizes[index]);
                solid.hit(from, -Vec3::UP).map(|(down, _)| from.y - down)
            });
        let ground = self.ground.then(|| self.ground_at(point.x, point.z));
        tops.chain(ground).reduce(f32::max)
    }
}

/// The seconds in the air of the throw that takes the least speed to
/// come to a place `off` from where it starts: half way between straight
/// at it and straight up.
fn easiest(off: Vec3) -> f32 {
    let (level, up) = (off.x.hypot(off.z), off.y);
    if level < 0.05 {
        // Straight up to where it stops rising, or straight down.
        return (2.0 * up.abs() / GRAVITY).sqrt();
    }
    let angle = std::f32::consts::FRAC_PI_4 + up.atan2(level) * 0.5;
    let speed_squared =
        GRAVITY * level * level / (2.0 * angle.cos().powi(2) * (level * angle.tan() - up));
    level / (speed_squared.max(1e-6).sqrt() * angle.cos())
}
