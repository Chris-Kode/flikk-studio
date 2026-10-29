//! Whisper's files, one per size, and how they are run.

use crate::ModelFile;

/// `$KOOEDITOR_WHISPER_MODEL` stands in for whichever size is picked: a
/// path, or a ggml file name in `paths::models()`.
const ENV: Option<&str> = Some("KOOEDITOR_WHISPER_MODEL");

/// Small, its weights stored as 8-bit blocks.
pub const SMALL: ModelFile = ModelFile {
    name: "ggml-small-q8_0.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small-q8_0.bin",
    env: ENV,
    what: "Whisper small model",
    size: 264_464_607,
};

/// Large-v3-turbo, its weights stored as 8-bit blocks.
pub const LARGE: ModelFile = ModelFile {
    name: "ggml-large-v3-turbo-q8_0.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo-q8_0.bin",
    env: ENV,
    what: "Whisper large model",
    size: 874_188_075,
};

/// Whether whisper.cpp was built for a GPU. It still runs on the CPU when
/// the machine has none.
pub const GPU: bool = cfg!(any(feature = "metal", feature = "vulkan", feature = "cuda"));

/// Threads at most; more stop paying off, and leave none to the rest of
/// analysis.
pub const MAX_THREADS: usize = 8;
