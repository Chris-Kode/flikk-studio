//! `FLIKK_DEMO_SHOT=card|blockout|upright|plain`: a video waiting on the
//! user in the chat, with a blockout, a first frame and a reference;
//! `blockout` opens the blockout's dialog on it, `upright` is the same in
//! a 9:16 picture, and `plain` is an upright video from its prompt alone,
//! its tiles empty.

use crate::state::EditorState;
use flikk_agent::tools::ApprovalRequest;
use flikk_agent::tools::video::{self, Blockout, Shot};
use flikk_core::Time;
use flikk_core::blockout::Scene;
use flikk_core::cloud::{GenerateRequest, VideoReference};
use flikk_core::command::ToolCategory;
use flikk_core::model::MediaKind;

/// A shot blocked the way the agent blocks one: its people and its car
/// are models built here from plain solids, on feet that step.
const SCENE: &str = r#"{
  "duration": 6,
  "sun": [120, 28],
  "models": {
    "person": [
      {"name": "hips", "at": [0, 0.92, 0]},
      {"name": "pelvis", "shape": "sphere", "size": [0.34, 0.26, 0.24], "anchor": "middle", "parent": "hips"},
      {"name": "chest", "shape": "sphere", "size": [0.4, 0.56, 0.26], "parent": "hips", "at": [0, 0.04, 0], "rotate": [5, 0, 0]},
      {"name": "head", "shape": "sphere", "size": [0.2, 0.25, 0.22], "parent": "chest", "at": [0, 0.58, 0]},
      {"name": "visor", "shape": "box", "size": [0.15, 0.05, 0.05], "parent": "head", "at": [0, 0.12, 0.09], "shade": 0.25},
      {"name": "thigh l", "shape": "capsule", "size": [0.15, 0.48, 0.15], "anchor": "top", "parent": "hips", "at": [0.1, 0, 0]},
      {"name": "shin l", "shape": "capsule", "size": [0.11, 0.48, 0.11], "anchor": "top", "parent": "thigh l", "at": [0, -0.44, 0]},
      {"name": "foot l", "shape": "box", "size": [0.1, 0.07, 0.26], "parent": "shin l", "at": [0, -0.5, 0.07], "step": {}},
      {"name": "thigh r", "shape": "capsule", "size": [0.15, 0.48, 0.15], "anchor": "top", "parent": "hips", "at": [-0.1, 0, 0]},
      {"name": "shin r", "shape": "capsule", "size": [0.11, 0.48, 0.11], "anchor": "top", "parent": "thigh r", "at": [0, -0.44, 0]},
      {"name": "foot r", "shape": "box", "size": [0.1, 0.07, 0.26], "parent": "shin r", "at": [0, -0.5, 0.07], "step": {}},
      {"name": "arm l", "shape": "capsule", "size": [0.09, 0.34, 0.09], "anchor": "top", "parent": "chest", "at": [0.24, 0.48, 0], "rotate": [0, 0, 8],
       "cycle": {"strides": 1, "keys": [{"t": 0, "rotate": [30, 0, 0]}, {"t": 0.5, "rotate": [-30, 0, 0]}]}},
      {"name": "forearm l", "shape": "capsule", "size": [0.08, 0.32, 0.08], "anchor": "top", "parent": "arm l", "at": [0, -0.3, 0], "rotate": [-40, 0, 0]},
      {"name": "arm r", "shape": "capsule", "size": [0.09, 0.34, 0.09], "anchor": "top", "parent": "chest", "at": [-0.24, 0.48, 0], "rotate": [0, 0, -8],
       "cycle": {"strides": 1, "shift": 0.5, "keys": [{"t": 0, "rotate": [30, 0, 0]}, {"t": 0.5, "rotate": [-30, 0, 0]}]}},
      {"name": "forearm r", "shape": "capsule", "size": [0.08, 0.32, 0.08], "anchor": "top", "parent": "arm r", "at": [0, -0.3, 0], "rotate": [-40, 0, 0]}
    ],
    "car": [
      {"name": "body", "shape": "box", "size": [1.75, 0.55, 4.3], "at": [0, 0.28, 0]},
      {"name": "cabin", "shape": "box", "size": [1.6, 0.52, 1.9], "at": [0, 0.83, -0.4]},
      {"name": "windscreen", "shape": "wedge", "size": [1.6, 0.52, 0.7], "at": [0, 0.83, 0.9], "shade": 0.3},
      {"name": "rear glass", "shape": "wedge", "size": [1.6, 0.52, 0.5], "at": [0, 0.83, -1.6], "rotate": [0, 180, 0], "shade": 0.3},
      {"name": "wheel fl", "shape": "cylinder", "size": [0.66, 0.22, 0.66], "anchor": "middle", "at": [0.82, 0.33, 1.3], "rotate": [0, 0, 90], "shade": 0.3},
      {"name": "wheel fr", "shape": "cylinder", "size": [0.66, 0.22, 0.66], "anchor": "middle", "at": [-0.82, 0.33, 1.3], "rotate": [0, 0, 90], "shade": 0.3},
      {"name": "wheel rl", "shape": "cylinder", "size": [0.66, 0.22, 0.66], "anchor": "middle", "at": [0.82, 0.33, -1.3], "rotate": [0, 0, 90], "shade": 0.3},
      {"name": "wheel rr", "shape": "cylinder", "size": [0.66, 0.22, 0.66], "anchor": "middle", "at": [-0.82, 0.33, -1.3], "rotate": [0, 0, 90], "shade": 0.3}
    ]
  },
  "objects": [
    {"name": "runner", "model": "person", "at": [-11, 0, 0.5], "on": "ground", "face": "path", "shade": 0.8,
     "keys": [{"t": 6, "at": [11, 0, -0.5]}]},
    {"name": "attendant", "model": "person", "at": [6.2, 0, -3.2], "on": "ground", "face": "runner", "scale": 1.05,
     "parts": {"head": {"aim": "runner"},
               "arm r": {"rotate": [0, 0, -150], "cycle": null},
               "forearm r": {"rotate": [0, 0, 0], "cycle": {"seconds": 0.7, "keys": [{"t": 0, "rotate": [0, 0, -25]}, {"t": 0.5, "rotate": [0, 0, 25]}]}}}},
    {"name": "car", "model": "car", "at": [3.2, 0, -5.5], "rotate": [0, 90, 0], "shade": 0.42},
    {"name": "van", "shape": "box", "size": [2, 2.3, 5], "at": [-5.5, 0, -5.8], "shade": 0.55},
    {"name": "lamp", "shape": "cylinder", "size": [0.16, 5, 0.16], "at": [-1.5, 0, -4], "shade": 0.35},
    {"name": "lamp head", "shape": "box", "parent": "lamp", "size": [0.3, 0.12, 1.1], "at": [0, 5, 0.4], "shade": 0.9},
    {"name": "wall", "shape": "box", "size": [34, 3.2, 0.3], "at": [0, 0, -12], "shade": 0.7},
    {"name": "kerb", "shape": "box", "size": [34, 0.15, 0.4], "at": [0, 0, -3], "shade": 0.75}
  ],
  "camera": {"at": [-1, 1.5, 8], "look_at": "runner", "fov": 54, "shake": 0.35,
    "keys": [{"t": 3, "at": [4, 1.9, 7]}, {"t": 6, "at": [10, 2.4, 5], "fov": 40, "ease": "out"}]}
}"#;

const PROMPT: &str = "A woman in a red tracksuit sprints down an empty car park at dawn, \
    breath showing in the cold air; the camera swings around her from the front to her side \
    as she passes a parked grey estate car. Handheld 35 mm, soft blue light, light fog.\n\n\
    @Video1 is a rough 3D blockout: follow its camera move, staging, action and timing. \
    Its grey shapes are stand-ins, not the look.\nStart on @Image1 as the first frame.";

/// Which state to seed, when one is asked for.
pub fn asked() -> Option<String> {
    super::enabled()
        .then(|| std::env::var("FLIKK_DEMO_SHOT").ok())
        .flatten()
        .map(|v| v.to_lowercase())
}

pub fn seed(state: &mut EditorState) {
    let Some(asked) = asked() else {
        return;
    };
    if asked == "plain" {
        return seed_shot(state, plain());
    }
    let Ok(scene) = serde_json::from_str(SCENE)
        .map_err(|_| ())
        .and_then(|json| Scene::parse(&json).map_err(|_| ()))
    else {
        return;
    };
    // `upright` is the same shot in a 9:16 picture.
    let (aspect, shape) = match asked.as_str() {
        "upright" => ("9:16", 9.0 / 16.0),
        _ => ("16:9", 16.0 / 9.0),
    };
    let blockout = Blockout::new(scene, shape);
    // Stills of the blockout stand in for the pictures a project would have.
    let picture = |at: f64| blockout.still(at).ok();
    let (first, reference) = (picture(0.0), picture(4.5));
    let shot = Shot {
        request: GenerateRequest::Video {
            prompt: PROMPT.into(),
            length: Some(Time::from_secs_f64(6.0)),
            aspect: Some(aspect.into()),
            resolution: Some("720p".into()),
            first_frame: first,
            last_frame: None,
            references: (reference.into_iter())
                .map(|path| VideoReference {
                    kind: MediaKind::Image,
                    path,
                    length: None,
                })
                .collect(),
            audio: true,
        },
        grabs: Vec::new(),
        placement: None,
        blockout: Some(blockout),
    };
    seed_shot(state, shot);
}

/// An upright video from its prompt alone.
fn plain() -> Shot {
    let prompt = PROMPT.split("\n\n").next().unwrap_or(PROMPT);
    Shot {
        request: GenerateRequest::Video {
            prompt: prompt.into(),
            length: Some(Time::from_secs_f64(5.0)),
            aspect: Some("9:16".into()),
            resolution: None,
            first_frame: None,
            last_frame: None,
            references: Vec::new(),
            audio: true,
        },
        grabs: Vec::new(),
        placement: None,
        blockout: None,
    }
}

fn seed_shot(state: &mut EditorState, shot: Shot) {
    state.seed_approval(ApprovalRequest {
        tool: "generate_video".into(),
        category: ToolCategory::Generate,
        args: video::request_json(&shot),
        summary: video::summary(&shot),
        allowed: false,
    });
}
