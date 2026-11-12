//! A blockout: a rough 3D scene of grey stand-ins and a moving camera,
//! drawn here in a few milliseconds a frame. It shows a shot's staging,
//! action, camera move and timing before a video model is paid to make it,
//! and its clip is what the model then follows.
//!
//! A [`Scene`] is plain data the agent writes as JSON ([`Scene::parse`])
//! and the user changes by asking. [`Scene::render`] draws it at a time,
//! [`Scene::write_clip`] writes it as a video file, [`Scene::review`]
//! says in words what its camera sees, for a writer that can't look, and
//! [`Scene::changed`] makes a change that says only what differs.
//!
//! Nothing in a scene is ready-made. Its writer builds every thing from
//! plain solids, hung from one another at their joints (`parent`,
//! `anchor`), and moves them with keys and with motion that repeats
//! ([`Cycle`]): by the clock, or by the ground covered, so that feet and
//! wheels keep up with it. A thing built once is a model, placed as many
//! times as the shot needs, each copy free to differ. `build` writes the
//! copies out into the parts that are drawn, and `motion` says where each
//! is at a time.
//!
//! Space is in metres: X to the right, Y up, the ground at `y = 0`. An
//! object's `at` is the middle of its base unless it says otherwise, so
//! `y = 0` stands it on the ground. Angles are degrees.

mod build;
mod change;
mod clip;
mod motion;
mod render;
mod review;
mod space;

pub use render::Detail;
pub use review::Review;

use build::Part;
use motion::CameraMoves;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Frames a second a blockout's clip runs at.
pub const FPS: u32 = 24;
/// The longest a blockout runs, in seconds: what video models follow.
pub const MAX_SECONDS: f64 = 30.0;
/// The most objects a scene is written with, and the most parts it has
/// once every copy of a model is written out.
pub const MAX_OBJECTS: usize = 256;
pub const MAX_PARTS: usize = 1200;

/// A rough 3D scene over a few seconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Written")]
pub struct Scene {
    /// How long it runs, in seconds.
    pub duration: f64,
    /// Things built once, by name, each a list of parts written as
    /// objects are; an object with `model` is a copy of one.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, Vec<Object>>,
    pub objects: Vec<Object>,
    pub camera: Camera,
    /// A floor at `y = 0`; off for a scene in the air or in space.
    #[serde(skip_serializing_if = "is_true")]
    pub ground: bool,
    /// Where the light comes from: degrees around the vertical (0 from
    /// +Z, 90 from +X) and degrees above the horizon.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sun: Option<[f64; 2]>,
    /// What is drawn: the objects, with every copy of a model written out.
    #[serde(skip)]
    parts: Vec<Part>,
    /// The order parts are placed in at a moment: each after what it
    /// rides, and after what it faces.
    #[serde(skip)]
    order: Vec<usize>,
    /// Whether parts face one another in a loop.
    #[serde(skip)]
    loops: bool,
    /// The part the camera rides, and how its keys move it.
    #[serde(skip)]
    mount: Option<usize>,
    #[serde(skip)]
    camera_moves: CameraMoves,
}

/// A scene as its JSON has it, before it is checked and built.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    duration: f64,
    #[serde(default)]
    models: BTreeMap<String, Vec<Object>>,
    #[serde(default)]
    objects: Vec<Object>,
    camera: Camera,
    #[serde(default = "yes")]
    ground: bool,
    #[serde(default)]
    sun: Option<[f64; 2]>,
}

/// One thing in a scene: a solid, a joint that only carries others, or a
/// copy of a model. Keys and a cycle move it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Object {
    /// What it stands for ("runner", "left shin"); others ride, face and
    /// look at it by this name. A part of a copy is `copy/part`.
    pub name: String,
    /// The solid it is. None for a joint, and for a copy of a model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<Shape>,
    /// The model it is a copy of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// How large a copy is against its model.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale: f64,
    /// What a copy does its own way: for a part of the model by name,
    /// the fields that replace the model's.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parts: BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
    /// Width (X), height (Y) and depth (Z) of a solid; a metre each when
    /// left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[f64; 3]>,
    /// Where its anchor is.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub at: [f64; 3],
    /// Turns about X, Y (the vertical) and Z, about its anchor.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotate: [f64; 3],
    /// The point of a solid that `at` places and `rotate` turns about.
    #[serde(default, skip_serializing_if = "Anchor::is_base")]
    pub anchor: Anchor,
    /// How light its grey is, 0 (dark) to 1 (light). On a copy, how light
    /// the copy is against its model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shade: Option<f64>,
    /// The object it rides: `at` and `rotate` are then in that object's
    /// own space, and it moves and turns with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// What it turns to about the vertical, on top of `rotate`: `"path"`
    /// (the way it travels), `"camera"`, an object by name, or a point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face: Option<Target>,
    /// Degrees a second it keeps turning about X, Y and Z.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub spin: [f64; 3],
    /// Not there at the start; a key shows it. What rides it goes with it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// Motion that repeats, on top of what the keys make of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<Cycle>,
    /// Unsteady motion that never repeats, on top of both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shake: Option<Shake>,
    /// What it is later: its place, turn, size and grey each follow a
    /// smooth curve through their keys that never goes past one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<ObjectKey>,
}

/// What an object is at `t` seconds; what a key leaves out stays as it was.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectKey {
    /// When, in seconds. A key with a place and no time is passed on the
    /// way, at an even pace between the timed keys either side of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shade: Option<f64>,
    /// Gone (true) or there (false) from here on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// How the move to this key is timed.
    #[serde(default, skip_serializing_if = "Ease::is_linear")]
    pub ease: Ease,
}

/// Motion that repeats: a swing, a step, a turn of a wheel, a breath. Its
/// keys are added to where the object's own keys have it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cycle {
    /// How long one round takes, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
    /// Or the ground one round covers, in metres: the round then follows
    /// how far the thing it is part of has travelled, and comes to rest
    /// when that stands still.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metres: Option<f64>,
    /// How far into its round it starts, 0..1: half a round for the
    /// other leg.
    #[serde(default, skip_serializing_if = "is_nothing")]
    pub shift: f64,
    pub keys: Vec<CycleKey>,
}

/// What a cycle adds `t` of the way through a round (0..1). After the
/// last key it goes on to the first, unless a key at 1 ends the round.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleKey {
    pub t: f64,
    /// Metres added to its place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 3]>,
    /// Degrees added to its turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate: Option<[f64; 3]>,
    /// Metres added to its size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[f64; 3]>,
    /// How it arrives at this key: starting and ending slowly unless said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<Ease>,
}

/// Unsteady motion that never repeats: an engine's rumble, a boat on a
/// swell, a flag, someone who can't stand still. Each copy of a model
/// shakes its own way.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shake {
    /// The most it adds to its place, in metres either way.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub at: [f64; 3],
    /// The most it adds to its turn, in degrees either way.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotate: [f64; 3],
    /// About how long one wobble takes, in seconds.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub seconds: f64,
}

/// The solids things are built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    #[serde(alias = "cube")]
    Box,
    #[serde(alias = "ball", alias = "ellipsoid")]
    Sphere,
    Cylinder,
    /// Its point up.
    Cone,
    /// A rod with round ends, upright.
    Capsule,
    /// The upper half of a sphere, flat underneath.
    #[serde(alias = "hemisphere")]
    Dome,
    /// A ramp: low at +Z, as high as it is at -Z.
    #[serde(alias = "ramp")]
    Wedge,
    /// A gable roof or a tent: its ridge runs along Z.
    #[serde(alias = "prism")]
    Roof,
    Pyramid,
}

impl Shape {
    /// The names a scene's JSON takes.
    pub const NAMES: [&'static str; 9] = [
        "box", "sphere", "cylinder", "cone", "capsule", "dome", "wedge", "roof", "pyramid",
    ];
}

/// The point of a solid that its `at` places and its `rotate` turns
/// about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    /// The middle of its underside: it stands there.
    #[default]
    #[serde(alias = "bottom")]
    Base,
    #[serde(alias = "centre", alias = "center")]
    Middle,
    /// The middle of its top: it hangs from there, as a limb from its
    /// joint.
    Top,
}

impl Anchor {
    /// The names a scene's JSON takes.
    pub const NAMES: [&'static str; 3] = ["base", "middle", "top"];

    fn is_base(&self) -> bool {
        *self == Self::Base
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
    /// How much a hand holds it, 0 (locked off) to 1 (running with it):
    /// it drifts and wobbles without ever repeating.
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
    /// When, in seconds; a key with a place and no time is passed on the
    /// way, as an object's is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look_at: Option<Target>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fov: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roll: Option<f64>,
    /// How the move to this key is timed; [`Ease::Cut`] is a cut to it.
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

/// How the move to a key is timed along the curve through the keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    /// At the pace the keys give: through a key without stopping, slowing
    /// only where the value turns back or rests.
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

fn one() -> f64 {
    1.0
}

fn is_one(value: &f64) -> bool {
    *value == 1.0
}

/// The grey of a solid that gives none.
const MID_GREY: f64 = 0.62;

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
        serde_json::from_value(json.clone()).map_err(|e| format!("blockout: {e}"))
    }

    /// How many frames its clip has.
    pub fn frames(&self) -> u32 {
        ((self.duration * FPS as f64).round() as u32).max(1)
    }

    /// Where the part called `name` is among the parts drawn.
    fn find(&self, name: &str) -> Option<usize> {
        self.parts.iter().position(|part| part.object.name == name)
    }
}

impl TryFrom<Written> for Scene {
    type Error = String;

    /// Bring values into range and keys into order, write the copies of
    /// models out, and refuse what can't be drawn.
    fn try_from(written: Written) -> Result<Self, String> {
        let Written {
            duration,
            models,
            objects,
            camera,
            ground,
            sun,
        } = written;
        let mut scene = Self {
            duration,
            models,
            objects,
            camera,
            ground,
            sun,
            parts: Vec::new(),
            order: Vec::new(),
            loops: false,
            mount: None,
            camera_moves: CameraMoves::default(),
        };
        if !scene.duration.is_finite() || scene.duration <= 0.0 {
            return Err("`duration` is in seconds, more than 0".into());
        }
        scene.duration = scene.duration.min(MAX_SECONDS);
        if scene.objects.len() > MAX_OBJECTS {
            return Err(format!(
                "at most {MAX_OBJECTS} objects; this has {}. Build what repeats as a model",
                scene.objects.len()
            ));
        }
        if let Some(sun) = scene.sun.as_mut() {
            finite("`sun`", sun)?;
            sun[1] = sun[1].clamp(5.0, 90.0);
        }
        let written = (scene.objects.iter_mut()).chain(scene.models.values_mut().flatten());
        for object in written {
            object.settle()?;
        }
        scene.camera.settle()?;
        scene.build()?;
        let targets = std::iter::once(&scene.camera.look_at)
            .chain(scene.camera.keys.iter().filter_map(|k| k.look_at.as_ref()));
        for target in targets {
            match target {
                Target::Point(p) => finite("the camera's `look_at`", p)?,
                Target::Object(name) if scene.find(name).is_none() => {
                    return Err(format!(
                        "the camera looks at `{name}`, which is no object's name"
                    ));
                }
                Target::Object(_) => {}
            }
        }
        Ok(scene)
    }
}

fn finite(what: &str, v: &[f64]) -> Result<(), String> {
    match v.iter().all(|n| n.is_finite()) {
        true => Ok(()),
        false => Err(format!("{what} has a number that is not finite")),
    }
}

/// Put `keys` in the order they are reached. When each gives its time
/// (`of` says a key's time, and whether it gives a place) that is by time.
/// A key with no time is passed between those written either side of it,
/// so then the order written stands, and has to make sense.
fn in_order<K>(
    keys: &mut [K],
    what: &str,
    of: impl Fn(&K) -> (Option<f64>, bool),
) -> Result<(), String> {
    if keys.iter().all(|key| of(key).0.is_some()) {
        keys.sort_by(|a, b| of(a).0.unwrap_or(0.0).total_cmp(&of(b).0.unwrap_or(0.0)));
        return Ok(());
    }
    let mut latest = 0.0;
    for key in keys.iter() {
        match of(key) {
            (None, false) => {
                return Err(format!(
                    "{what} has a key with no `t` and no `at`: a key without a time is a place \
                     passed on the way"
                ));
            }
            (Some(t), _) if t < latest => {
                return Err(format!(
                    "{what} has keys without `t`, which are passed in the order written, so its \
                     timed keys have to be in order too: {t} comes after {latest}"
                ));
            }
            (Some(t), _) => latest = t,
            (None, true) => {}
        }
    }
    Ok(())
}

/// A size no smaller or larger than can be drawn.
fn in_range(size: &mut [f64; 3]) {
    size.iter_mut().for_each(|n| *n = n.clamp(0.005, 1000.0));
}

impl Object {
    fn settle(&mut self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("every object needs a `name`".into());
        }
        let what = format!("object `{}`", self.name);
        if self.shape.is_some() && self.model.is_some() {
            return Err(format!(
                "{what} has a `shape` and a `model`: it is a solid or a copy, not both"
            ));
        }
        finite(&what, &self.at)?;
        finite(&what, &self.rotate)?;
        finite(&what, &self.spin)?;
        finite(&what, &[self.scale, self.shade.unwrap_or(0.0)])?;
        self.scale = self.scale.clamp(0.01, 100.0);
        self.shade = self.shade.map(|s| s.clamp(0.0, 1.0));
        if let Some(size) = self.size.as_mut() {
            finite(&what, size)?;
            in_range(size);
        }
        for key in &mut self.keys {
            finite(&what, &[key.t.unwrap_or(0.0), key.shade.unwrap_or(0.0)])?;
            for v in [&key.at, &key.rotate, &key.size].into_iter().flatten() {
                finite(&what, v)?;
            }
            key.shade = key.shade.map(|s| s.clamp(0.0, 1.0));
            if let Some(size) = key.size.as_mut() {
                in_range(size);
            }
        }
        in_order(&mut self.keys, &what, |key| (key.t, key.at.is_some()))?;
        if let Some(cycle) = self.cycle.as_mut() {
            cycle.settle(&what)?;
        }
        if let Some(shake) = self.shake.as_mut() {
            finite(&what, &shake.at)?;
            finite(&what, &shake.rotate)?;
            finite(&what, &[shake.seconds])?;
            shake.seconds = shake.seconds.clamp(0.02, 60.0);
        }
        Ok(())
    }
}

impl Cycle {
    fn settle(&mut self, what: &str) -> Result<(), String> {
        let round = match (self.seconds, self.metres) {
            (Some(round), None) | (None, Some(round)) => round,
            _ => {
                return Err(format!(
                    "{what}'s `cycle` needs how long a round is: `seconds`, or `metres` travelled"
                ));
            }
        };
        if !round.is_finite() || round <= 0.0 {
            return Err(format!("{what}'s `cycle` has a round of no length"));
        }
        finite(what, &[self.shift])?;
        for key in &mut self.keys {
            finite(what, &[key.t])?;
            key.t = key.t.clamp(0.0, 1.0);
            for v in [&key.at, &key.rotate, &key.size].into_iter().flatten() {
                finite(what, v)?;
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
            let numbers = [key.t, key.fov, key.roll].map(|n| n.unwrap_or(0.0));
            finite(what, &numbers)?;
            finite(what, &key.at.unwrap_or_default())?;
            key.fov = key.fov.map(|f| f.clamp(5.0, 140.0));
        }
        in_order(&mut self.keys, what, |key| (key.t, key.at.is_some()))?;
        Ok(())
    }
}
