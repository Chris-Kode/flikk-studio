//! SAM 2.1's files, and how it is run.

use crate::ModelFile;

/// A file of square-zero-labs' export of `facebook/sam2.1-hiera-tiny`
/// (Apache-2.0) with the memory modules, which its authors checked against
/// `transformers`' `Sam2VideoModel`; pinned to a commit.
macro_rules! published {
    ($file:literal) => {
        concat!(
            "https://huggingface.co/square-zero-labs/sam2.1-tiny-video-onnx/resolve/3b2984dd865f6e9d2cc6aed0be6a5a5c2eb352ce/",
            $file
        )
    };
}

/// A 1024 × 1024 picture → its features at three scales, and their
/// positions.
pub const ENCODER: ModelFile = ModelFile {
    name: "sam2.1-tiny-vision-encoder.onnx",
    url: published!("onnx/vision_encoder.onnx"),
    env: None,
    what: "subject mask encoder",
    size: 134_335_567,
};
/// Features and points → the object's logits, a score and its pointer.
pub const DECODER: ModelFile = ModelFile {
    name: "sam2.1-tiny-mask-decoder.onnx",
    url: published!("onnx/mask_decoder.onnx"),
    env: None,
    what: "subject mask decoder",
    size: 17_794_355,
};
/// A frame's features and mask → the memory later frames look back at.
pub const MEMORY_ENCODER: ModelFile = ModelFile {
    name: "sam2.1-tiny-memory-encoder.onnx",
    url: published!("onnx/memory_encoder.onnx"),
    env: None,
    what: "subject tracking memory",
    size: 5_616_496,
};
/// A frame's features conditioned on the memory of other frames.
pub const MEMORY_ATTENTION: ModelFile = ModelFile {
    name: "sam2.1-tiny-memory-attention.onnx",
    url: published!("onnx/memory_attention.onnx"),
    env: None,
    what: "subject tracking attention",
    size: 32_259_165,
};
/// Frame distances of object pointers → their positions.
pub const POINTER_POSITIONS: ModelFile = ModelFile {
    name: "sam2.1-tiny-pointer-tpos.onnx",
    url: published!("onnx/pointer_tpos.onnx"),
    env: None,
    what: "subject tracking pointers",
    size: 67_289,
};
/// The memory's temporal position table and the picture normalization.
pub const CONSTANTS: ModelFile = ModelFile {
    name: "sam2.1-tiny-constants.json",
    url: published!("constants.json"),
    env: None,
    what: "subject mask constants",
    size: 9_922,
};

/// Side of the square picture the model sees.
pub const INPUT: usize = 1024;
/// Side of its feature grid at the coarsest scale, and of its memory.
pub const GRID: usize = 64;
pub const TOKENS: usize = GRID * GRID;
/// Channels of the coarsest features, and of the memory.
pub const HIDDEN: usize = 256;
pub const MEMORY: usize = 64;
/// Channels of the two finer feature scales (256² and 128²).
pub const FINE: [(usize, usize); 2] = [(32, 256), (64, 128)];
/// Frames of memory the attention sees: the prompted frames nearest and
/// the latest tracked ones, padded by repeating the latest.
pub const MEMORY_FRAMES: usize = 7;
/// Object pointers it sees, each split into `HIDDEN / MEMORY` tokens.
pub const POINTERS: usize = 16;
/// Prompted frames among the memory frames at most: the nearest before
/// and after.
pub const PROMPTED_FRAMES: usize = 2;
/// Label of a click on the object, off it, a box's corners and padding.
pub const LABEL_ON: i32 = 1;
pub const LABEL_OFF: i32 = 0;
pub const LABEL_BOX: [i32; 2] = [2, 3];
pub const LABEL_PAD: i32 = -1;
