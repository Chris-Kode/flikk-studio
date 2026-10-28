//! Whisper's file, and how it is run.

use crate::ModelFile;

/// `$KOOEDITOR_WHISPER_MODEL` picks another: a path, or a ggml file name
/// in `paths::models()`.
pub const FILE: ModelFile = ModelFile {
    name: "ggml-base.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
    env: Some("KOOEDITOR_WHISPER_MODEL"),
    what: "Whisper model",
    size: 147_951_465,
};

/// Threads at most; more stop paying off, and leave none to the rest of
/// analysis.
pub const MAX_THREADS: usize = 8;
