//! Whisper's files, one per size, and how they are run.

use crate::ModelFile;

/// `$KOOEDITOR_WHISPER_MODEL` stands in for whichever size is picked: a
/// path, or a ggml file name in `paths::models()`.
const ENV: Option<&str> = Some("KOOEDITOR_WHISPER_MODEL");

pub const BASE: ModelFile = ModelFile {
    name: "ggml-base.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.bin",
    env: ENV,
    what: "Whisper base model",
    size: 147_951_465,
};

pub const MEDIUM: ModelFile = ModelFile {
    name: "ggml-medium.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-medium.bin",
    env: ENV,
    what: "Whisper medium model",
    size: 1_533_763_059,
};

pub const LARGE: ModelFile = ModelFile {
    name: "ggml-large-v3.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3.bin",
    env: ENV,
    what: "Whisper large-v3 model",
    size: 3_095_033_483,
};

/// Threads at most; more stop paying off, and leave none to the rest of
/// analysis.
pub const MAX_THREADS: usize = 8;
