//! RIFE's file, and how it is run.

use crate::ModelFile;

pub const FILE: ModelFile = ModelFile {
    name: "rife-v4-fp32-timestep.onnx",
    url: "https://huggingface.co/walterlow/RIFE_fp32_timestep/resolve/ee09066f9822f8b28b8477a1b4cc30f19d607590/RIFE_fp32_timestep.onnx",
    env: Some("FLIKK_RIFE_MODEL"),
    what: "frame interpolation model",
    size: 21_604_567,
};

/// Widest picture the model sees; wider frames are made at this width
/// and scaled back up (memory grows with the picture).
pub const MAX_WIDTH: u32 = 1920;
