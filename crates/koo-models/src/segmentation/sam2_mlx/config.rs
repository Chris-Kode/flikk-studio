//! SAM 2.1 tiny's weights for MLX, and its shape.

use crate::ModelFile;

/// Meta's SAM 2.1 tiny checkpoint (Apache-2.0) converted to MLX's layout
/// (convolutions as O×H×W×I) in half precision by avbiswas' `sam2-mlx`,
/// with Hiera's position embeddings baked into one table; pinned to a
/// commit.
pub const WEIGHTS: ModelFile = ModelFile {
    name: "sam2.1-tiny-mlx-fp16.safetensors",
    url: "https://huggingface.co/avbiswas/sam2.1-hiera-tiny-mlx-16bit/resolve/3cd20137d61efd06c9e0abaae709aad610925a6c/sam2.1_hiera_tiny_image_segmenter_fp16.safetensors",
    env: Some("KOOEDITOR_SUBJECTS_MLX_MODEL"),
    what: "subject mask model",
    size: 90_538_932,
};

/// Side of the square picture the model sees, of its coarsest feature
/// grid, and of its logits.
pub const INPUT: i32 = 1024;
pub const GRID: i32 = 64;
pub const LOGITS: i32 = 256;
/// Channels of the features the decoder and the memory attention read,
/// and of the memory.
pub const HIDDEN: i32 = 256;
pub const MEMORY: i32 = 64;

/// Hiera tiny: blocks per stage, the blocks attending globally, each
/// stage's window, channels and heads of the first stage (doubled at each
/// stage after).
pub const STAGES: [usize; 4] = [1, 2, 7, 2];
pub const GLOBAL_BLOCKS: [usize; 3] = [5, 7, 9];
pub const WINDOWS: [i32; 4] = [8, 4, 14, 7];
pub const EMBED: i32 = 96;
pub const HEADS: i32 = 1;
/// Stages whose first block pools its queries (halving the grid).
pub const POOLED_STAGES: usize = 3;
/// Layer norm epsilon of Hiera and of the channel norms; the transformer
/// layers use the default.
pub const EPS_FINE: f32 = 1e-6;
pub const EPS: f32 = 1e-5;
/// Heads of the mask decoder's attention.
pub const DECODER_HEADS: i32 = 8;
/// Memory attention layers, and the share of position added to the
/// current frame's features before them.
pub const MEMORY_LAYERS: usize = 4;
pub const POSITION_AT_INPUT: f32 = 0.1;
/// Decoder: masks 1..3 are the multimask answers; a single mask whose area
/// moves less than `STABILITY` when its threshold moves by
/// `STABILITY_DELTA` is kept over them.
pub const STABILITY_DELTA: f32 = 0.05;
pub const STABILITY: f32 = 0.98;
/// Logit of a mask where the object is not in the frame.
pub const NO_OBJECT: f32 = -1024.0;
/// A mask fed to the memory encoder is scaled to these logits.
pub const MEMORY_SCALE: f32 = 20.0;
pub const MEMORY_BIAS: f32 = -10.0;
/// Label of a click on the object, off it, a box's corners and padding.
pub const LABEL_ON: i32 = 1;
pub const LABEL_OFF: i32 = 0;
pub const LABEL_BOX: [i32; 2] = [2, 3];
pub const LABEL_PAD: i32 = -1;
