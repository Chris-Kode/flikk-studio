//! A scene seen from outside its shot: from any side or from above, with
//! the shot's camera and the ways things go drawn in. For whoever stages
//! the shot; no video model is sent it.

use super::Scene;
use super::motion::View;
use super::space::Vec3;

/// The field of view a scene is looked at with from outside, in degrees.
const FOV: f32 = 40.0;
/// Seconds between the places a path is drawn through.
const PATH_STEP: f64 = 0.1;

/// Where a scene is looked at from, outside its shot: turned toward its
/// middle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outside {
    /// Degrees around the vertical: 0 from +Z, 90 from +X.
    pub around: f64,
    /// Degrees above the horizon: 90 looks straight down, a plan.
    pub above: f64,
    /// How much nearer than where all of the scene is in the picture: 2
    /// is twice as near.
    pub closer: f64,
}

impl Default for Outside {
    fn default() -> Self {
        Self {
            around: 35.0,
            above: 30.0,
            closer: 1.0,
        }
    }
}

/// A line drawn into a view from outside: light for the camera, dark
/// for the way a thing goes.
pub(super) struct Guide {
    pub from: Vec3,
    pub to: Vec3,
    pub light: bool,
}

impl Scene {
    /// Work out the ball that holds all of the scene, over all of its
    /// length: what a view from outside turns about.
    pub(super) fn measure(&mut self) {
        let mut held: Option<(Vec3, Vec3)> = None;
        let mut hold = |low: Vec3, high: Vec3| {
            held = Some(held.map_or((low, high), |(l, h)| (l.min(low), h.max(high))));
        };
        for step in 0..=4 {
            let t = self.duration * step as f64 / 4.0;
            let standing = self.stand(t);
            for (index, part) in self.parts.iter().enumerate() {
                if part.is_solid() && !self.hidden(index, t) {
                    let solid = self.solid(index, standing.places[index], standing.sizes[index]);
                    let (low, high) = solid.bounds();
                    hold(low, high);
                }
            }
            for rig in self.rigs.iter().filter(|rig| rig.mount.is_none()) {
                hold(rig.at(t), rig.at(t));
            }
        }
        let (low, high) = held.unwrap_or((Vec3::ZERO, Vec3::ZERO));
        self.extent = ((low + high) * 0.5, ((high - low).length() * 0.5).max(1.0));
    }

    /// How far off the things of a scene are, about: what a picture of
    /// its depth is scaled by.
    pub(super) fn reach(&self) -> f32 {
        self.extent.1
    }

    /// The view of the scene from `from`.
    pub(super) fn outside_view(&self, from: &Outside) -> View {
        let (middle, radius) = self.extent;
        let around = from.around.to_radians() as f32;
        let above = (from.above.clamp(2.0, 90.0)).to_radians() as f32;
        let back = Vec3::new(
            around.sin() * above.cos(),
            above.sin(),
            around.cos() * above.cos(),
        );
        let right = Vec3::new(around.cos(), 0.0, -around.sin());
        let off = radius / (FOV.to_radians() * 0.5).sin() / from.closer.clamp(0.2, 20.0) as f32;
        View {
            eye: middle + back * off,
            ahead: -back,
            right,
            up: right.cross(-back),
            fov: FOV,
        }
    }

    /// What is drawn into a view from outside at `t`: the camera of the
    /// shot, seeing as `shot` in a picture of `shape`, and the way each
    /// thing that travels goes, marked at every second.
    pub(super) fn guides(&self, t: f64, shot: &View, shape: (u32, u32)) -> Vec<Guide> {
        let mut guides = Vec::new();
        let mut line = |from: Vec3, to: Vec3, light: bool| guides.push(Guide { from, to, light });

        // The camera: its four edges out to a frame, and which way is up.
        let deep = (self.extent.1 * 0.2).clamp(0.8, 5.0);
        let long = (shot.fov.to_radians() * 0.5).tan() * deep;
        let (wide, high) = (shape.0.max(1) as f32, shape.1.max(1) as f32);
        let (across, up) = match wide >= high {
            true => (shot.right * long, shot.up * (long * high / wide)),
            false => (shot.right * (long * wide / high), shot.up * long),
        };
        let middle = shot.eye + shot.ahead * deep;
        let corners = [
            middle - across + up,
            middle + across + up,
            middle + across - up,
            middle - across - up,
        ];
        for (corner, next) in corners.iter().zip(corners.iter().cycle().skip(1)) {
            line(shot.eye, *corner, true);
            line(*corner, *next, true);
        }
        let top = middle + up * 1.35;
        line(middle - across * 0.3 + up, top, true);
        line(middle + across * 0.3 + up, top, true);

        // The camera's own way, where its keys alone say it.
        let steps = (self.duration / PATH_STEP).ceil() as usize;
        let at = |step: usize| (step as f64 * PATH_STEP).min(self.duration);
        if let Some(track) = self.rig(t).track() {
            for step in 0..steps {
                let ends = [at(step), at(step + 1)].map(|t| self.over_land(Vec3::of(track.at(t))));
                line(ends[0], ends[1], true);
            }
        }
        // The way each thing goes that rides nothing.
        let travels = |index: &usize| {
            let part = &self.parts[*index];
            self.stands_alone(*index)
                && !self.hidden(*index, t)
                && part.object.keys.iter().any(|key| key.at.is_some())
        };
        for index in (0..self.parts.len()).filter(travels) {
            let places = &self.parts[index].moves.places;
            let lifted = |t: f64| self.over_land(Vec3::of(places.at(t))) + Vec3::UP * 0.03;
            let mut second = 1.0;
            for step in 0..steps {
                let (from, to) = (lifted(at(step)), lifted(at(step + 1)));
                line(from, to, false);
                if at(step + 1) >= second {
                    // A tick across the way at each second: close
                    // together where it is slow.
                    second += 1.0;
                    let along = (to - from).unit_or(Vec3::AHEAD);
                    let across = Vec3::UP.cross(along).unit_or(Vec3::AHEAD) * 0.18;
                    line(to - across, to + across, false);
                }
            }
        }
        guides
    }
}
