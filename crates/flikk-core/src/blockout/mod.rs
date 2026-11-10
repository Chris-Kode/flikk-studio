//! A blockout: a rough 3D scene of grey stand-ins and a moving camera,
//! drawn here in a few milliseconds a frame. It shows a shot's staging,
//! action, camera move and timing before a video model is paid to make it,
//! and its clip is what the model then follows.
//!
//! A [`Scene`] is plain data the agent writes as JSON ([`Scene::parse`])
//! and the user changes by asking. [`Scene::render`] draws it at a time,
//! [`Scene::write_clip`] writes it as a video file, and [`Scene::review`]
//! says in words what its camera sees, for a writer that can't look.
//!
//! Stand-ins are plain solids, props made of a few (`kit`), and jointed
//! people and animals that walk, run and act (`figure`). Where each is at
//! a time, and what the camera sees, is worked out in `motion`.
//!
//! Space is in metres: X to the right, Y up, the ground at `y = 0`. An
//! object's `at` is the middle of its base, so `y = 0` stands it on the
//! ground. Angles are degrees.

mod clip;
mod figure;
mod kit;
mod motion;
mod render;
mod review;
mod space;

pub use render::Detail;
pub use review::Review;

use serde::{Deserialize, Serialize};

/// Frames a second a blockout's clip runs at.
pub const FPS: u32 = 24;
/// The longest a blockout runs, in seconds: what video models follow.
pub const MAX_SECONDS: f64 = 30.0;
/// The most objects a scene holds.
pub const MAX_OBJECTS: usize = 128;

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
    /// Where the light comes from: degrees around the vertical (0 from
    /// +Z, 90 from +X) and degrees above the horizon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sun: Option<[f64; 2]>,
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
    /// The object it rides: `at` and `rotate` are then in that object's
    /// own space, and it moves and turns with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// What it turns to about the vertical, on top of `rotate`: `"path"`
    /// (the way it travels), `"camera"`, an object by name, or a point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face: Option<Target>,
    /// Degrees a second it keeps turning about X, Y and Z: a rotor, a
    /// turntable.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub spin: [f64; 3],
    /// What a person does (animals only walk and run).
    #[serde(default, skip_serializing_if = "Action::is_auto")]
    pub action: Action,
    /// Not there at the start; a key shows it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// What it is later; between keys it moves along a smooth path.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shade: Option<f64>,
    /// What it does from here on, taken up over a moment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
    /// Gone (true) or there (false) from here on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// How it arrives at this key.
    #[serde(default, skip_serializing_if = "Ease::is_linear")]
    pub ease: Ease,
}

/// What stand-ins are: plain solids, props made of a few, and jointed
/// bodies. All face +Z until turned.
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
    /// A ramp: low at +Z, as high as it is at -Z.
    #[serde(alias = "ramp")]
    Wedge,
    /// A gable roof or a tent: its ridge runs along Z.
    #[serde(alias = "prism")]
    Roof,
    Pyramid,
    /// A person with joints, who walks, runs and acts ([`Action`]).
    #[serde(alias = "person")]
    Figure,
    /// A four-legged animal, a horse until sized otherwise, which walks
    /// and gallops as fast as it moves.
    #[serde(alias = "horse", alias = "dog")]
    Animal,
    Car,
    /// A bicycle or a motorbike.
    #[serde(alias = "motorbike", alias = "bicycle")]
    Bike,
    Tree,
    #[serde(alias = "building")]
    House,
    Chair,
    Table,
}

impl Shape {
    /// The names a scene's JSON takes.
    pub const NAMES: [&'static str; 16] = [
        "box", "sphere", "cylinder", "cone", "capsule", "wedge", "roof", "pyramid", "figure",
        "animal", "car", "bike", "tree", "house", "chair", "table",
    ];

    /// Its size when an object gives none.
    fn size(self) -> [f64; 3] {
        match self {
            Self::Capsule => [0.5, 1.8, 0.5],
            Self::Figure => [0.5, 1.75, 0.5],
            Self::Animal => [0.55, 1.6, 2.3],
            Self::Car => [1.8, 1.45, 4.3],
            Self::Bike => [0.6, 1.1, 1.9],
            Self::Tree => [3.5, 6.0, 3.5],
            Self::House => [7.0, 6.0, 9.0],
            Self::Chair => [0.45, 0.9, 0.48],
            Self::Table => [1.5, 0.75, 0.85],
            _ => [1.0, 1.0, 1.0],
        }
    }
}

/// What a person does. Legs carry it by themselves: under [`Action::Auto`]
/// and under the actions of the arms, it stands while it is still and
/// walks or runs as fast as its keys move it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Stands, walks or runs as fast as it moves.
    #[default]
    #[serde(alias = "idle", alias = "move")]
    Auto,
    /// Stands still even while it moves: on a board, a lift, a walkway.
    Stand,
    /// Walks, on the spot when it is not moved.
    Walk,
    /// Runs, on the spot when it is not moved.
    #[serde(alias = "sprint", alias = "jog")]
    Run,
    /// Seated; the seat is 0.26 of its height above `at`.
    Sit,
    /// Astride a bike or a horse, or at a wheel: seated, hands forward.
    #[serde(alias = "drive")]
    Ride,
    Crouch,
    /// On its back on the ground.
    Lie,
    /// In the air, legs tucked, arms up; its keys carry it up and down.
    Jump,
    Dance,
    /// Both arms up, bouncing.
    Cheer,
    Wave,
    /// One arm out ahead.
    Point,
    /// Hands moving as it speaks.
    Talk,
    /// Arms holding something in front of it.
    Carry,
    /// Leaning on something with both hands.
    Push,
    /// One arm up high.
    Reach,
    /// Guard up, jabbing.
    #[serde(alias = "punch", alias = "box")]
    Fight,
}

impl Action {
    /// The names a scene's JSON takes.
    pub const NAMES: [&'static str; 18] = [
        "auto", "stand", "walk", "run", "sit", "ride", "crouch", "lie", "jump", "dance", "cheer",
        "wave", "point", "talk", "carry", "push", "reach", "fight",
    ];

    fn is_auto(&self) -> bool {
        *self == Self::Auto
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
    /// Degrees it is rolled clockwise: a Dutch angle.
    #[serde(default, skip_serializing_if = "is_nothing")]
    pub roll: f64,
    /// How much a hand holds it, 0 (locked off) to 1 (running with it).
    #[serde(default, skip_serializing_if = "is_nothing")]
    pub shake: f64,
    /// The object it rides: `at`, and a point it looks at, are then in
    /// that object's own space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Where it is later: a dolly, an orbit, a zoom, a cut.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roll: Option<f64>,
    /// How it arrives at this key; [`Ease::Cut`] is a cut to it.
    #[serde(default, skip_serializing_if = "Ease::is_linear")]
    pub ease: Ease,
}

/// What a camera looks at, or an object faces: a point, or an object by
/// name, which it then follows as the object moves.
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
    /// Not at all: it stays as it was, then is there at once. A camera's
    /// cut.
    #[serde(alias = "hold", alias = "step")]
    Cut,
    /// Past the key and back to it: a settle.
    #[serde(alias = "overshoot")]
    Back,
    /// Landing on the key and bouncing to rest.
    Bounce,
}

impl Ease {
    /// The names a scene's JSON takes.
    pub const NAMES: [&'static str; 7] = ["linear", "in", "out", "in_out", "cut", "back", "bounce"];

    fn is_linear(&self) -> bool {
        *self == Self::Linear
    }
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(v: &[f64; 3]) -> bool {
    *v == [0.0; 3]
}

fn is_nothing(value: &f64) -> bool {
    *value == 0.0
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

/// What an object can face that is no object: the way it travels, and
/// the camera.
const PATH: &str = "path";
const CAMERA: &str = "camera";

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
        if let Some(sun) = self.sun.as_mut() {
            finite("`sun`", sun)?;
            sun[1] = sun[1].clamp(5.0, 90.0);
        }
        for (i, object) in self.objects.iter_mut().enumerate() {
            if object.name.trim().is_empty() {
                return Err(format!("blockout: objects[{i}] needs a `name`"));
            }
            object.settle()?;
        }
        for object in &self.objects {
            self.check_ties(object)?;
        }
        self.camera.settle()?;
        if let Some(parent) = &self.camera.parent
            && self.index(parent).is_none()
        {
            return Err(format!(
                "blockout: the camera rides `{parent}`, which is no object's name"
            ));
        }
        let targets = std::iter::once(&self.camera.look_at)
            .chain(self.camera.keys.iter().filter_map(|k| k.look_at.as_ref()));
        for target in targets {
            match target {
                Target::Point(p) => finite("the camera's `look_at`", p)?,
                Target::Object(name) if self.index(name).is_none() => {
                    return Err(format!(
                        "blockout: the camera looks at `{name}`, which is no object's name"
                    ));
                }
                Target::Object(_) => {}
            }
        }
        Ok(())
    }

    /// Refuse an object that rides or faces what isn't there, or rides
    /// itself by way of others.
    fn check_ties(&self, object: &Object) -> Result<(), String> {
        let name = &object.name;
        match &object.face {
            Some(Target::Point(p)) => finite(&format!("object `{name}`'s `face`"), p)?,
            Some(Target::Object(faced))
                if faced != PATH && faced != CAMERA && self.index(faced).is_none() =>
            {
                return Err(format!(
                    "blockout: `{name}` faces `{faced}`, which is no object's name, \
                     `{PATH}` or `{CAMERA}`"
                ));
            }
            _ => {}
        }
        let mut above = object.parent.as_deref();
        for _ in 0..=self.objects.len() {
            let Some(parent) = above else {
                return Ok(());
            };
            let Some(index) = self.index(parent) else {
                return Err(format!(
                    "blockout: `{name}` rides `{parent}`, which is no object's name"
                ));
            };
            above = self.objects[index].parent.as_deref();
        }
        Err(format!(
            "blockout: `{name}` rides itself by way of its `parent`"
        ))
    }

    /// Where the object called `name` is in `objects`.
    fn index(&self, name: &str) -> Option<usize> {
        self.objects.iter().position(|o| o.name == name)
    }

    /// How many frames its clip has.
    pub fn frames(&self) -> u32 {
        ((self.duration * FPS as f64).round() as u32).max(1)
    }
}

fn finite(what: &str, v: &[f64]) -> Result<(), String> {
    match v.iter().all(|n| n.is_finite()) {
        true => Ok(()),
        false => Err(format!("blockout: {what} has a number that is not finite")),
    }
}

/// A size no smaller or larger than can be drawn.
fn in_range(size: &mut [f64; 3]) {
    size.iter_mut().for_each(|n| *n = n.clamp(0.01, 1000.0));
}

impl Object {
    fn settle(&mut self) -> Result<(), String> {
        let what = format!("object `{}`", self.name);
        finite(&what, &self.at)?;
        finite(&what, &self.rotate)?;
        finite(&what, &self.spin)?;
        finite(&what, &[self.shade])?;
        self.shade = self.shade.clamp(0.0, 1.0);
        if let Some(size) = self.size.as_mut() {
            finite(&what, size)?;
            in_range(size);
        }
        for key in &mut self.keys {
            finite(&what, &[key.t, key.shade.unwrap_or(0.0)])?;
            for v in [&key.at, &key.rotate, &key.size].into_iter().flatten() {
                finite(&what, v)?;
            }
            key.shade = key.shade.map(|s| s.clamp(0.0, 1.0));
            if let Some(size) = key.size.as_mut() {
                in_range(size);
            }
        }
        self.keys.sort_by(|a, b| a.t.total_cmp(&b.t));
        Ok(())
    }
}

impl Camera {
    fn settle(&mut self) -> Result<(), String> {
        let what = "the camera";
        finite(what, &self.at)?;
        finite(what, &[self.fov, self.roll, self.shake])?;
        self.fov = self.fov.clamp(5.0, 140.0);
        self.shake = self.shake.clamp(0.0, 1.0);
        for key in &mut self.keys {
            finite(
                what,
                &[key.t, key.fov.unwrap_or(0.0), key.roll.unwrap_or(0.0)],
            )?;
            finite(what, &key.at.unwrap_or_default())?;
            key.fov = key.fov.map(|f| f.clamp(5.0, 140.0));
        }
        self.keys.sort_by(|a, b| a.t.total_cmp(&b.t));
        Ok(())
    }
}
