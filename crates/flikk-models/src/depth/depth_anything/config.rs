//! Depth Anything V2 Small's file, and how it is run.

use crate::ModelFile;

/// onnx-community's export of `depth-anything/Depth-Anything-V2-Small`
/// (Apache-2.0) in full precision, pinned to a commit.
pub const MODEL_FILE: ModelFile = ModelFile {
    name: "depth-anything-v2-small.onnx",
    url: "https://huggingface.co/onnx-community/depth-anything-v2-small/resolve/4472b7362082ad9968fee890ca0f1e5aca36b93d/onnx/model.onnx",
    env: Some("FLIKK_DEPTH_MODEL"),
    what: "depth model",
    size: 99_060_839,
    sha256: "afb6a5c28f3b6bf1618c6e43f02073ef9dfdc70e937502d51603e57b0a1df10c",
};

/// The longer side of the picture the model sees. It was trained at 518 on
/// the shorter side; the longer side at 518 costs a third as much on the
/// CPU and is plenty for maps the effects blur and fog with.
pub const INPUT: usize = 518;
/// The model's patch: both sides are a multiple of it.
pub const PATCH: usize = 14;
/// The normalization it was trained with (ImageNet's).
pub const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
pub const STD: [f32; 3] = [0.229, 0.224, 0.225];
