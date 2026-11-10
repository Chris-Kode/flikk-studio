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

const SCENE: &str = r#"{
  "duration": 6,
  "sun": [120, 28],
  "objects": [
    {"name": "runner", "shape": "figure", "at": [-11, 0, 0.5], "face": "path", "shade": 0.8,
     "keys": [{"t": 6, "at": [11, 0, -0.5]}]},
    {"name": "car", "shape": "car", "at": [3.2, 0, -5.5], "rotate": [0, 90, 0], "shade": 0.4},
    {"name": "van", "shape": "box", "size": [2, 2.3, 5], "at": [-5.5, 0, -5.8], "shade": 0.55},
    {"name": "attendant", "shape": "figure", "at": [6.2, 0, -3.2], "face": "runner",
     "action": "wave"},
    {"name": "tree", "shape": "tree", "at": [-10, 0, -9], "shade": 0.5},
    {"name": "lamp", "shape": "cylinder", "size": [0.16, 5, 0.16], "at": [-1.5, 0, -4],
     "shade": 0.35},
    {"name": "lamp head", "shape": "box", "parent": "lamp", "size": [0.3, 0.12, 1.1],
     "at": [0, 5, 0.4], "shade": 0.9},
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
