//! `FLIKK_DEMO_SHOT=card|blockout`: a video waiting on the user in the
//! chat, with a blockout, a first frame and a reference; `blockout` opens
//! the blockout's dialog on it.

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
  "objects": [
    {"name": "runner", "shape": "figure", "at": [-4, 0, 0], "rotate": [0, 90, 0],
     "keys": [{"t": 6, "at": [4, 0, 0]}]},
    {"name": "car", "shape": "box", "size": [1.8, 1.4, 4.2], "at": [3, 0, -6], "shade": 0.4},
    {"name": "trunk", "shape": "cylinder", "size": [0.5, 3, 0.5], "at": [-3, 0, -5]},
    {"name": "crown", "shape": "cone", "size": [2.6, 3.5, 2.6], "at": [-3, 3, -5], "shade": 0.5},
    {"name": "wall", "shape": "box", "size": [14, 3, 0.3], "at": [0, 0, -9], "shade": 0.7}
  ],
  "camera": {"at": [0, 1.6, 9], "look_at": "runner", "fov": 54,
    "keys": [{"t": 3, "at": [5, 2.2, 7]}, {"t": 6, "at": [8, 3, 2], "fov": 40, "ease": "out"}]}
}"#;

const PROMPT: &str = "A woman in a red tracksuit sprints down an empty car park at dawn, \
    breath showing in the cold air; the camera swings around her from the front to her side \
    as she passes a parked grey estate car. Handheld 35 mm, soft blue light, light fog.\n\n\
    @Video1 is a rough 3D blockout: follow its camera move, staging and timing. \
    Its grey shapes are stand-ins, not the look.\nStart on @Image1 as the first frame.";

/// Which state to seed, when one is asked for.
pub fn asked() -> Option<String> {
    super::enabled()
        .then(|| std::env::var("FLIKK_DEMO_SHOT").ok())
        .flatten()
        .map(|v| v.to_lowercase())
}

pub fn seed(state: &mut EditorState) {
    if asked().is_none() {
        return;
    }
    let Ok(scene) = serde_json::from_str(SCENE)
        .map_err(|_| ())
        .and_then(|json| Scene::parse(&json).map_err(|_| ()))
    else {
        return;
    };
    let blockout = Blockout::new(scene, 16.0 / 9.0);
    // Stills of the blockout stand in for the pictures a project would have.
    let picture = |at: f64| blockout.still(at).ok();
    let (first, reference) = (picture(0.0), picture(4.5));
    let shot = Shot {
        request: GenerateRequest::Video {
            prompt: PROMPT.into(),
            length: Some(Time::from_secs_f64(6.0)),
            aspect: Some("16:9".into()),
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
    state.seed_approval(ApprovalRequest {
        tool: "generate_video".into(),
        category: ToolCategory::Generate,
        args: video::request_json(&shot),
        summary: video::summary(&shot),
        allowed: false,
    });
}
