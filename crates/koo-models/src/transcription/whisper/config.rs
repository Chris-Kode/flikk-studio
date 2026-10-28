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

pub const SMALL: ModelFile = ModelFile {
    name: "ggml-small.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small.bin",
    env: ENV,
    what: "Whisper small model",
    size: 487_601_967,
};

pub const MEDIUM: ModelFile = ModelFile {
    name: "ggml-medium.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-medium.bin",
    env: ENV,
    what: "Whisper medium model",
    size: 1_533_763_059,
};

pub const TURBO: ModelFile = ModelFile {
    name: "ggml-large-v3-turbo.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo.bin",
    env: ENV,
    what: "Whisper large-v3-turbo model",
    size: 1_624_555_275,
};

/// Threads at most; more stop paying off, and leave none to the rest of
/// analysis.
pub const MAX_THREADS: usize = 8;
