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
//! ([`Cycle`]). What a figure does is said rather than posed: feet
//! [`Step`], a hand is brought somewhere ([`Reach`]), a head looks
//! (`aim`), a key hands a thing to another. Cameras are framed as shots
//! are called ([`Frame`]) and cut between. A thing built once is a model, placed as many
//! times as the shot needs, each copy free to differ. `build` writes the
//! copies out into the parts that are drawn, `motion` says where each is
//! at a time, `world` is the scene at that time as rays meet it, and
//! `render` makes the picture.
//!
//! Space is in metres: X to the right, Y up, the ground at `y = 0`. An
//! object's `at` is the middle of its base unless it says otherwise, so
//! `y = 0` stands it on the ground; a thing with limbs says what it
//! stands `on`, and its lowest point is kept there. Angles are degrees.

mod build;
mod change;
mod clip;
mod motion;
mod render;
mod repeat;
mod review;
mod solid;
mod space;
mod world;

pub use render::Detail;
pub use repeat::{Count, Repeat, Vary};
pub use review::Review;

use build::Part;
use motion::Rig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Frames a second a blockout's clip runs at.
pub const FPS: u32 = 24;
/// The longest a blockout runs, in seconds: what video models follow.
pub const MAX_SECONDS: f64 = 30.0;
/// The most objects a scene is written with, and the most parts it has
/// once every copy of a model is written out.
pub const MAX_OBJECTS: usize = 256;
pub const MAX_PARTS: usize = 6000;

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
    /// The camera the shot opens on: `main` to a cut.
    pub camera: Camera,
    /// Other cameras, by name. Each goes on moving whether or not the
    /// picture is its own.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub cameras: BTreeMap<String, Camera>,
    /// When the picture goes over to another camera.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cuts: Vec<Cut>,
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
    /// Whether parts are placed roughly first: some face one another in
    /// a loop, or stand on something.
    #[serde(skip)]
    rough: bool,
    /// The cameras as they are worked with, the one the shot opens on
    /// first, and which of them the picture is from each cut on.
    #[serde(skip)]
    rigs: Vec<Rig>,
    #[serde(skip)]
    live: Vec<(f64, usize)>,
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
    #[serde(default)]
    cameras: BTreeMap<String, Camera>,
    #[serde(default)]
    cuts: Vec<Cut>,
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
    /// How wide the top of a box, a cylinder or a capsule is against its
    /// base: under 1 it narrows upward, over 1 downward, inside the same
    /// `size`. A limb, a trunk, a lampshade, a plinth.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub taper: f64,
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
    /// What it stands on: `"ground"`, or an object by name. Its lowest
    /// point, and that of all that rides it, then rests on that surface
    /// wherever it is and whatever its limbs do, and the height `at`
    /// gives is how far above that it is: nothing standing, more in a
    /// jump.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
    /// What it turns to about the vertical, on top of `rotate`: `"path"`
    /// (the way it travels), `"camera"`, an object by name, or a point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face: Option<Target>,
    /// What its front (+Z) turns to, up and down as well as round, as far
    /// as a neck goes: a head to who it looks at, a lamp to what it
    /// lights. `"camera"`, an object by name, a point, or `"ahead"`: not
    /// turned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim: Option<Target>,
    /// What it is brought to, as the end of a limb: the part it rides and
    /// the part that one rides bend, as an arm or a leg does, to put it
    /// there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<Reach>,
    /// That it is a foot that steps: it stays where it comes down while
    /// the thing it is part of passes over it, and the leg above it bends
    /// to keep it there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<Step>,
    /// Many of it, laid out: each is this object over again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<Repeat>,
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
    /// What its front turns to by then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim: Option<Target>,
    /// The object it goes with from here on, by name, from where it is at
    /// this moment: picked up, or set down on something that moves.
    /// `"none"` lets it go, and it stays where it is. Its own keys move it
    /// no more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
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
    /// Or the strides one round takes, of the feet that step on the thing
    /// it is part of: arms that swing in time with the legs, whatever the
    /// pace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strides: Option<f64>,
    /// The seconds it goes on from and until: the start and the end when
    /// left out. It comes in and dies away over a moment, and one by the
    /// clock starts its first round at `from`: a wave from 2 s until 4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<f64>,
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

/// The end of a limb brought to something: a hand to a door's handle, a
/// foot to a pedal. Out of the limb's reach, it goes as far as it can:
/// an arm held out, pointing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reach {
    /// What it is brought to: an object by name, `"camera"`, or a point.
    pub to: Target,
    /// The seconds it is there from and until: the start and the end
    /// when left out. It moves there, and back to how its limb is posed,
    /// over a moment before and after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<f64>,
}

/// A foot that steps as the thing it is part of travels: down where it
/// lands while the thing passes over it, then swung to where it lands
/// next. The part it rides and the part that one rides are its leg.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// The ground covered from one landing of this foot to its next: a
    /// stride. About one and a half times the leg's length when left out,
    /// a walk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metres: Option<f64>,
    /// How far into a stride it starts, 0..1. Left out, the feet of one
    /// thing take turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shift: Option<f64>,
    /// How high it is lifted as it swings, in metres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lift: Option<f64>,
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

/// A camera: where it is, what it looks at and how wide it sees.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    /// Where it is; with a `frame`, the way it looks from.
    pub at: [f64; 3],
    /// What it looks at. Left out, it looks at what its `frame` holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look_at: Option<Target>,
    /// What its picture holds: it then stands as far off, along the way
    /// from `at`, as that takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<Frame>,
    /// How wide it sees, as the lens of a film camera in millimetres (on
    /// a frame 36 mm along its longer side): 24 is wide, 35 and 50 as an
    /// eye sees, 85 and up long. Or give `fov`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens: Option<f64>,
    /// Or in degrees across the frame's longer side: 74 is a 24 mm lens,
    /// 54 a 35 mm, 40 a 50 mm, 24 an 85 mm. A 35 mm lens when neither is
    /// given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fov: Option<f64>,
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
    /// What its picture holds when it gets here, as the things are then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<Frame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fov: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roll: Option<f64>,
    /// How the move to this key is timed; [`Ease::Cut`] is a cut to it.
    #[serde(default, skip_serializing_if = "Ease::is_linear")]
    pub ease: Ease,
}

/// What a camera's picture holds, as a shot is called on a set: "a
/// medium close-up of the runner, on the left".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    /// The objects it holds, by name.
    pub of: Subjects,
    /// How much of them.
    #[serde(default, skip_serializing_if = "ShotSize::is_medium")]
    pub size: ShotSize,
    /// Where across the picture they sit.
    #[serde(default, skip_serializing_if = "Side::is_centre")]
    pub side: Side,
}

/// One object by name, or several.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Subjects {
    One(String),
    Many(Vec<String>),
}

impl Subjects {
    fn names(&self) -> &[String] {
        match self {
            Self::One(name) => std::slice::from_ref(name),
            Self::Many(names) => names,
        }
    }
}

/// How much of its subjects a picture holds. On a standing person: the
/// part of them from the top of the head down, and how much of the
/// picture's height that fills.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShotSize {
    /// The head.
    #[serde(alias = "closeup")]
    CloseUp,
    /// Head and shoulders.
    MediumCloseUp,
    /// From the waist up.
    #[default]
    Medium,
    /// From the knees up.
    #[serde(alias = "cowboy")]
    MediumWide,
    /// All of them, filling the picture.
    Full,
    /// All of them, half the picture high.
    #[serde(alias = "long")]
    Wide,
    /// All of them, small in their surroundings.
    #[serde(alias = "extreme_wide")]
    VeryWide,
}

impl ShotSize {
    /// The names a scene's JSON takes.
    pub const NAMES: [&'static str; 7] = [
        "close_up",
        "medium_close_up",
        "medium",
        "medium_wide",
        "full",
        "wide",
        "very_wide",
    ];

    fn is_medium(&self) -> bool {
        *self == Self::Medium
    }

    /// The share of its subjects' height it holds, from their top down,
    /// and the share of the picture's height that fills.
    fn holds(self) -> (f32, f32) {
        match self {
            Self::CloseUp => (0.17, 0.75),
            Self::MediumCloseUp => (0.3, 0.8),
            Self::Medium => (0.5, 0.85),
            Self::MediumWide => (0.72, 0.9),
            Self::Full => (1.0, 0.85),
            Self::Wide => (1.0, 0.5),
            Self::VeryWide => (1.0, 0.25),
        }
    }
}

/// Where across the picture a frame's subjects sit: in the middle, or on
/// the line a third of the way in from a side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    #[default]
    #[serde(alias = "center", alias = "middle")]
    Centre,
    Right,
}

impl Side {
    /// The names a scene's JSON takes.
    pub const NAMES: [&'static str; 3] = ["left", "centre", "right"];

    fn is_centre(&self) -> bool {
        *self == Self::Centre
    }
}

/// The picture going over to another camera.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cut {
    /// When, in seconds.
    pub t: f64,
    /// The camera it goes to, by name: `main` is the one the shot opens
    /// on.
    pub camera: String,
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

/// The field of view of a camera that gives none: a 35 mm lens.
const NORMAL_FOV: f64 = 54.4;
/// The name of the camera the shot opens on.
const MAIN: &str = "main";

/// What an object can face that is no object: the way it travels, and
/// the camera.
const PATH: &str = "path";
const CAMERA: &str = "camera";
/// What a key can have an object go with that is no object: nothing.
const NONE: &str = "none";
/// What an object can aim at that is no object: where it faces anyway.
const AHEAD: &str = "ahead";
/// What an object can stand on that is no object: the floor at `y = 0`.
const GROUND: &str = "ground";

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
            cameras,
            cuts,
            ground,
            sun,
        } = written;
        let mut scene = Self {
            duration,
            models,
            objects,
            camera,
            cameras,
            cuts,
            ground,
            sun,
            parts: Vec::new(),
            order: Vec::new(),
            rough: false,
            rigs: Vec::new(),
            live: Vec::new(),
        };
        if !scene.duration.is_finite() || scene.duration <= 0.0 {
            return Err("`duration` is in seconds, more than 0".into());
        }
        scene.duration = scene.duration.min(MAX_SECONDS);
        if scene.objects.len() > MAX_OBJECTS {
            return Err(format!(
                "at most {MAX_OBJECTS} objects; this has {}. Build what repeats as a model, and \
                 lay many of one thing out with `repeat`",
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
        scene.camera.settle("the camera")?;
        for (name, camera) in &mut scene.cameras {
            if name == MAIN {
                return Err(format!(
                    "`{MAIN}` is the camera the shot opens on: give the others names of their own"
                ));
            }
            camera.settle(&format!("camera `{name}`"))?;
        }
        for cut in &scene.cuts {
            finite("a cut", &[cut.t])?;
        }
        scene.cuts.sort_by(|a, b| a.t.total_cmp(&b.t));
        scene.build()?;
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
    /// A joint called `name` riding `parent`: nothing is drawn of it.
    fn joint(name: String, parent: Option<String>) -> Self {
        Self {
            name,
            parent,
            scale: 1.0,
            taper: 1.0,
            ..Self::default()
        }
    }

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
        finite(&what, &[self.taper])?;
        let narrows = matches!(
            self.shape,
            Some(Shape::Box | Shape::Cylinder | Shape::Capsule)
        );
        if self.taper != 1.0 && !narrows {
            return Err(format!(
                "{what} has a `taper`, which narrows a box, a cylinder or a capsule"
            ));
        }
        self.taper = self.taper.clamp(0.02, 50.0);
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
        if let Some(step) = self.step.as_mut() {
            if self.reach.is_some() {
                return Err(format!(
                    "{what} has a `step` and a `reach`: its limb can do one"
                ));
            }
            let given = [step.metres, step.shift, step.lift].map(|n| n.unwrap_or(0.0));
            finite(&what, &given)?;
            step.metres = step.metres.map(|metres| metres.clamp(0.02, 100.0));
            step.lift = step.lift.map(|lift| lift.clamp(0.0, 10.0));
        }
        if let Some(reach) = &self.reach {
            finite(
                &what,
                &[reach.from.unwrap_or(0.0), reach.until.unwrap_or(0.0)],
            )?;
            if let Target::Point(point) = &reach.to {
                finite(&what, point)?;
            }
        }
        if let Some(repeat) = self.repeat.as_mut() {
            repeat.settle(&what)?;
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
        let round = match (self.seconds, self.metres, self.strides) {
            (Some(round), None, None) | (None, Some(round), None) | (None, None, Some(round)) => {
                round
            }
            _ => {
                return Err(format!(
                    "{what}'s `cycle` needs how long a round is: `seconds`, `metres` travelled, \
                     or `strides` taken"
                ));
            }
        };
        if !round.is_finite() || round <= 0.0 {
            return Err(format!("{what}'s `cycle` has a round of no length"));
        }
        finite(
            what,
            &[
                self.shift,
                self.from.unwrap_or(0.0),
                self.until.unwrap_or(0.0),
            ],
        )?;
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
    fn settle(&mut self, what: &str) -> Result<(), String> {
        finite(what, &self.at)?;
        finite(what, &[self.roll, self.shake])?;
        self.shake = self.shake.clamp(0.0, 1.0);
        in_range_wide(what, &mut self.lens, &mut self.fov)?;
        for key in &mut self.keys {
            let numbers = [key.t, key.roll].map(|n| n.unwrap_or(0.0));
            finite(what, &numbers)?;
            finite(what, &key.at.unwrap_or_default())?;
            in_range_wide(what, &mut key.lens, &mut key.fov)?;
        }
        in_order(&mut self.keys, what, |key| (key.t, key.at.is_some()))?;
        Ok(())
    }
}

/// Bring the `lens` or the `fov` a camera gives into range; it gives one
/// or neither.
fn in_range_wide(what: &str, lens: &mut Option<f64>, fov: &mut Option<f64>) -> Result<(), String> {
    finite(what, &[lens.unwrap_or(0.0), fov.unwrap_or(0.0)])?;
    if lens.is_some() && fov.is_some() {
        return Err(format!(
            "{what} gives a `lens` and a `fov`: one says the other"
        ));
    }
    *lens = lens.map(|lens| lens.clamp(6.0, 400.0));
    *fov = fov.map(|fov| fov.clamp(5.0, 140.0));
    Ok(())
}

/// Degrees a camera sees across the frame's longer side, from the `lens`
/// or the `fov` it gives; `None` when it gives neither.
fn wide(lens: Option<f64>, fov: Option<f64>) -> Option<f64> {
    // A frame 36 mm along its longer side.
    let through = lens.map(|lens| 2.0 * (18.0 / lens).atan().to_degrees());
    through.or(fov).map(|fov| fov.clamp(5.0, 140.0))
}
