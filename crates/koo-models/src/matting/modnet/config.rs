//! MODNet's file, and how it is run.

use crate::ModelFile;

/// Xenova's ONNX export of the official photographic portrait matting
/// checkpoint (`modnet_photographic_portrait_matting.ckpt`, Apache-2.0),
/// in full precision; pinned to a commit. Dynamic height and width.
pub const GRAPH: ModelFile = ModelFile {
    name: "modnet-photographic.onnx",
    url: "https://huggingface.co/Xenova/modnet/resolve/fa2fa546052fba4c08921230a26cc69a333fca12/onnx/model.onnx",
    env: Some("KOOEDITOR_MODNET"),
    what: "person mask model",
    size: 25_888_640,
};

/// Short side of the picture the model sees (its training size); the long
/// side keeps the picture's proportions.
pub const SHORT_SIDE: usize = 512;
/// Both sides are a multiple of this (the network's stride).
pub const STRIDE: usize = 32;
/// Long side at most, for very wide pictures.
pub const LONG_SIDE_MAX: usize = 1024;
/// Pixels are scaled to -1..1.
pub const MEAN: f32 = 0.5;
pub const STD: f32 = 0.5;
