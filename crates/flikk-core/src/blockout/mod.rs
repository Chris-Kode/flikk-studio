//! A blockout: a rough 3D scene of grey stand-ins and a moving camera,
//! drawn here in a few milliseconds a frame. It shows a shot's staging,
//! camera move and timing before a video model is paid to make it, and its
//! clip is what the model then follows.
//!
//! A [`Scene`] is plain data the agent writes as JSON ([`Scene::parse`])
//! and the user changes by asking. [`Scene::render`] draws it at a time,
//! [`Scene::write_clip`] writes it as a video file.
//!
//! Space is in metres: X to the right, Y up, the ground at `y = 0`. An
//! object's `at` is the middle of its base, so `y = 0` stands it on the
//! ground. Angles are degrees.

mod clip;
mod render;

use serde::{Deserialize, Serialize};

/// Frames a second a blockout's clip runs at.
pub const FPS: u32 = 24;
/// The longest a blockout runs, in seconds: what video models follow.
pub const MAX_SECONDS: f64 = 30.0;
/// The most objects a scene holds.
pub const MAX_OBJECTS: usize = 64;

/// A rough 3D scene over a few seconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    /// How long it runs, in seconds.
    pub duration: f64,
    #[serde(default)]
    pub objects: Vec<Object>,
    pub camera: Camera,
    /// A floor at `y = 0`; off for a scene in the air or in space.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub ground: bool,
}

/// One stand-in: a shape with a size, a place and a turn, which keys move.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Object {
    /// What it stands for ("runner", "car"); a camera can look at it by
    /// this name.
    pub name: String,
    pub shape: Shape,
    /// Width (X), height (Y) and depth (Z); the shape's own when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[f64; 3]>,
    /// The middle of its base.
    #[serde(default)]
    pub at: [f64; 3],
    /// Turns about X, Y (the vertical) and Z, about the middle of its base.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotate: [f64; 3],
    /// How light its grey is, 0 (dark) to 1 (light).
    #[serde(default = "mid_grey", skip_serializing_if = "is_mid_grey")]
    pub shade: f64,
    /// Where it is later; between keys it moves along a smooth path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<ObjectKey>,
}

/// What an object is at `t` seconds; what a key leaves out stays as it was.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectKey {
    pub t: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[f64; 3]>,
    /// How it arrives at this key.
    #[serde(default, skip_serializing_if = "Ease::is_linear")]
    pub ease: Ease,
}

/// The shapes stand-ins are made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    #[serde(alias = "cube")]
    Box,
    #[serde(alias = "ball")]
    Sphere,
    Cylinder,
    Cone,
    Capsule,
    /// A person: a body and a head with a visor on the side it faces (+Z).
    #[serde(alias = "person")]
    Figure,
}

impl Shape {
    /// The names a scene's JSON takes.
    pub const NAMES: [&'static str; 6] = ["box", "sphere", "cylinder", "cone", "capsule", "figure"];

    /// Its size when an object gives none.
    fn size(self) -> [f64; 3] {
        match self {
            Self::Capsule => [0.5, 1.8, 0.5],
            Self::Figure => [0.5, 1.75, 0.5],
            _ => [1.0, 1.0, 1.0],
        }
    }
}

/// The camera: where it is, what it looks at and how wide it sees.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    pub at: [f64; 3],
    pub look_at: Target,
    /// Degrees across the frame's longer side: 74 is a 24 mm lens, 54 a
    /// 35 mm, 40 a 50 mm, 24 an 85 mm.
    #[serde(default = "normal_fov")]
    pub fov: f64,
    /// Where it is later: a dolly, an orbit, a zoom.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<CameraKey>,
}

/// What the camera is at `t` seconds; what a key leaves out stays as it was.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraKey {
    pub t: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look_at: Option<Target>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fov: Option<f64>,
    #[serde(default, skip_serializing_if = "Ease::is_linear")]
    pub ease: Ease,
}

/// What a camera looks at: a point, or an object by name, which it then
/// follows as the object moves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Target {
    Point([f64; 3]),
    Object(String),
}

/// How a move arrives at a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    /// At one speed.
    #[default]
    Linear,
    /// Starting slowly.
    In,
    /// Ending slowly.
    Out,
    /// Starting and ending slowly.
    InOut,
}

impl Ease {
    fn is_linear(&self) -> bool {
        *self == Self::Linear
    }

    /// How far a move is, 0..1, when `u` of its time has passed.
    fn at(self, u: f64) -> f64 {
        match self {
            Self::Linear => u,
            Self::In => u * u,
            Self::Out => 1.0 - (1.0 - u) * (1.0 - u),
            Self::InOut => u * u * (3.0 - 2.0 * u),
        }
    }
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_zero(v: &[f64; 3]) -> bool {
    *v == [0.0; 3]
}

fn mid_grey() -> f64 {
    0.62
}

fn is_mid_grey(shade: &f64) -> bool {
    *shade == mid_grey()
}

fn normal_fov() -> f64 {
    54.0
}

impl Scene {
    /// The scene `json` describes, or what is wrong with it, said for
    /// whoever wrote it. Sizes, shades, the field of view and the length
    /// are brought into range rather than refused.
    pub fn parse(json: &serde_json::Value) -> Result<Self, String> {
        let mut scene: Self =
            serde_json::from_value(json.clone()).map_err(|e| format!("blockout: {e}"))?;
        scene.settle()?;
        Ok(scene)
    }

    /// Bring values into range and keys into order; refuse what can't be
    /// drawn.
    fn settle(&mut self) -> Result<(), String> {
        if !self.duration.is_finite() || self.duration <= 0.0 {
            return Err("blockout: `duration` is in seconds, more than 0".into());
        }
        self.duration = self.duration.min(MAX_SECONDS);
        if self.objects.len() > MAX_OBJECTS {
            return Err(format!(
                "blockout: at most {MAX_OBJECTS} objects; this has {}. Keep it rough",
                self.objects.len()
            ));
        }
        let finite = |what: &str, v: &[f64]| match v.iter().all(|n| n.is_finite()) {
            true => Ok(()),
            false => Err(format!("blockout: {what} has a number that is not finite")),
        };
        let size = |s: &mut [f64; 3]| s.iter_mut().for_each(|n| *n = n.clamp(0.01, 1000.0));
        for (i, object) in self.objects.iter_mut().enumerate() {
            let what = format!("object `{}`", object.name);
            if object.name.trim().is_empty() {
                return Err(format!("blockout: objects[{i}] needs a `name`"));
            }
            finite(&what, &object.at)?;
            finite(&what, &object.rotate)?;
            finite(&what, &[object.shade])?;
            object.shade = object.shade.clamp(0.0, 1.0);
            if let Some(s) = object.size.as_mut() {
                finite(&what, s)?;
                size(s);
            }
            for key in &mut object.keys {
                finite(&what, &[key.t])?;
                for v in [&key.at, &key.rotate, &key.size].into_iter().flatten() {
                    finite(&what, v)?;
                }
                if let Some(s) = key.size.as_mut() {
                    size(s);
                }
            }
            object.keys.sort_by(|a, b| a.t.total_cmp(&b.t));
        }
        let camera = &mut self.camera;
        finite("the camera", &camera.at)?;
        finite("the camera", &[camera.fov])?;
        camera.fov = camera.fov.clamp(5.0, 140.0);
        for key in &mut camera.keys {
            finite("the camera", &[key.t, key.fov.unwrap_or(0.0)])?;
            finite("the camera", &key.at.unwrap_or_default())?;
            key.fov = key.fov.map(|f| f.clamp(5.0, 140.0));
        }
        camera.keys.sort_by(|a, b| a.t.total_cmp(&b.t));
        let targets = std::iter::once(&self.camera.look_at)
            .chain(self.camera.keys.iter().filter_map(|k| k.look_at.as_ref()));
        for target in targets {
            match target {
                Target::Point(p) => finite("the camera's `look_at`", p)?,
                Target::Object(name) if self.object(name).is_none() => {
                    return Err(format!(
                        "blockout: the camera looks at `{name}`, which is no object's name"
                    ));
                }
                Target::Object(_) => {}
            }
        }
        Ok(())
    }

    fn object(&self, name: &str) -> Option<&Object> {
        self.objects.iter().find(|o| o.name == name)
    }

    /// How many frames its clip has.
    pub fn frames(&self) -> u32 {
        ((self.duration * FPS as f64).round() as u32).max(1)
    }
}

impl Object {
    fn size_now(&self, t: f64) -> [f64; 3] {
        let base = self.size.unwrap_or_else(|| self.shape.size());
        let keys = self
            .keys
            .iter()
            .filter_map(|k| Some((k.t, k.size?, k.ease)));
        straight(base, keys, t)
    }

    fn at_now(&self, t: f64) -> [f64; 3] {
        let keys = self.keys.iter().filter_map(|k| Some((k.t, k.at?, k.ease)));
        path(self.at, keys, t)
    }

    fn rotate_now(&self, t: f64) -> [f64; 3] {
        let keys = (self.keys.iter()).filter_map(|k| Some((k.t, k.rotate?, k.ease)));
        straight(self.rotate, keys, t)
    }

    /// The middle of it at `t`, which a camera looking at it looks at.
    fn middle(&self, t: f64) -> [f64; 3] {
        let (at, size) = (self.at_now(t), self.size_now(t));
        [at[0], at[1] + size[1] * 0.5, at[2]]
    }
}

/// Where the camera is and what it sees at a time.
pub(super) struct View {
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub fov: f64,
}

impl Scene {
    pub(super) fn view(&self, t: f64) -> View {
        let camera = &self.camera;
        let at = camera
            .keys
            .iter()
            .filter_map(|k| Some((k.t, k.at?, k.ease)));
        let fov = (camera.keys.iter()).filter_map(|k| Some((k.t, [k.fov?, 0.0, 0.0], k.ease)));
        // Each target is where it is now, so a camera between two objects
        // follows both as they move.
        let point = |target: &Target| match target {
            Target::Point(p) => *p,
            Target::Object(name) => self.object(name).map_or([0.0; 3], |o| o.middle(t)),
        };
        let targets =
            (camera.keys.iter()).filter_map(|k| Some((k.t, point(k.look_at.as_ref()?), k.ease)));
        View {
            eye: path(camera.at, at, t),
            target: straight(point(&camera.look_at), targets, t),
            fov: straight([camera.fov, 0.0, 0.0], fov, t)[0],
        }
    }
}

type Knot = (f64, [f64; 3], Ease);

/// The knots a value passes: what it starts as, then its keys in order. A
/// key at the start replaces the start.
fn knots(base: [f64; 3], keys: impl Iterator<Item = Knot>) -> Vec<Knot> {
    let mut knots = vec![(0.0, base, Ease::Linear)];
    for key in keys {
        if key.0 <= 0.0 {
            knots[0] = (0.0, key.1, Ease::Linear);
        } else {
            knots.push(key);
        }
    }
    knots
}

/// The two knots `t` is between, and how far the move from one to the
/// other is (0..1), eased; `None` before the first and after the last.
fn span(knots: &[Knot], t: f64) -> Option<(usize, f64)> {
    let next = knots.iter().position(|k| k.0 > t)?;
    let from = next.checked_sub(1)?;
    let (start, end) = (knots[from].0, knots[next].0);
    let u = ((t - start) / (end - start).max(1e-9)).clamp(0.0, 1.0);
    Some((from, knots[next].2.at(u)))
}

/// A value at `t` that goes straight from key to key.
fn straight(base: [f64; 3], keys: impl Iterator<Item = Knot>, t: f64) -> [f64; 3] {
    let knots = knots(base, keys);
    match span(&knots, t) {
        Some((i, u)) => std::array::from_fn(|c| {
            let (a, b) = (knots[i].1[c], knots[i + 1].1[c]);
            a + (b - a) * u
        }),
        None if t <= 0.0 => knots[0].1,
        None => knots[knots.len() - 1].1,
    }
}

/// A place at `t` on a smooth path through its keys: a curve through
/// three or more, so an orbit takes a few keys, and a straight line
/// between two. A key held (the same place twice) is left and reached
/// without overshoot.
fn path(base: [f64; 3], keys: impl Iterator<Item = Knot>, t: f64) -> [f64; 3] {
    let knots = knots(base, keys);
    let Some((i, u)) = span(&knots, t) else {
        return if t <= 0.0 {
            knots[0].1
        } else {
            knots[knots.len() - 1].1
        };
    };
    let place = |k: usize| knots[k].1;
    // The direction it passes knot `k` in: toward the next from the one
    // before, none where it rests.
    let tangent = |k: usize, c: usize| {
        let before = k.checked_sub(1).map(place);
        let after = (k + 1 < knots.len()).then(|| place(k + 1));
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
