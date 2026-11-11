//! The aligner's files, and how it is run.

use crate::ModelFile;
use flikk_core::time::{FLICKS_PER_SECOND, Time};

/// The model as ONNX, with its Whisper-style features built in (16 kHz
/// samples and token ids in, a class per token out), weights quantized to
/// 4 bits. Qwen publishes no ONNX export; how this one is made and hosted
/// is in `docs/TECHNICAL_DESIGN.md`.
pub const FILE: ModelFile = ModelFile {
    name: "qwen3-forced-aligner-0.6b-q4.onnx",
    url: "https://huggingface.co/Chris-Kode/qwen3-forced-aligner-0.6b-onnx/resolve/c1386c6a094f575f2067cf82632c30238c1b1f9d/qwen3-forced-aligner-0.6b-q4.onnx",
    env: Some("FLIKK_ALIGNER_MODEL"),
    what: "alignment model",
    size: 589_447_679,
    sha256: "228f127baf68f9c71af16cdd3ada97d64122424caafee364b241d2c0f9ab49df",
};
/// Qwen's tokenizer, from the model's own repository.
pub const VOCAB: ModelFile = ModelFile {
    name: "qwen3-forced-aligner-vocab.json",
    url: "https://huggingface.co/Qwen/Qwen3-ForcedAligner-0.6B/resolve/c7cbfc2048c462b0d63a45797104fc9db3ad62b7/vocab.json",
    env: None,
    what: "alignment tokenizer",
    size: 2_776_833,
    sha256: "ca10d7e9fb3ed18575dd1e277a2579c16d108e32f27439684afa0e10b1440910",
};
pub const MERGES: ModelFile = ModelFile {
    name: "qwen3-forced-aligner-merges.txt",
    url: "https://huggingface.co/Qwen/Qwen3-ForcedAligner-0.6B/resolve/c7cbfc2048c462b0d63a45797104fc9db3ad62b7/merges.txt",
    env: None,
    what: "alignment tokenizer",
    size: 1_671_853,
    sha256: "8831e4f1a044471340f7c0a83d7bd71306a5b867e95fd870f74d0c5308a904d5",
};

/// The languages it knows, as Whisper's codes.
pub const LANGUAGES: &[&str] = &[
    "en", "es", "fr", "de", "it", "pt", "ru", "zh", "yue", "ja", "ko",
];
/// Follows each unit twice; the model answers with its start and its end.
pub const TIMESTAMP: i64 = 151705;
/// One step of those answers.
pub const STEP_MS: i64 = 80;
/// Longest run of words aligned at once.
pub const MAX_CHUNK: Time = Time(FLICKS_PER_SECOND * 30);
/// Audio either side of a run, so a word Whisper misplaced is still in it.
pub const SLACK: Time = Time(FLICKS_PER_SECOND * 5);
/// The model's starts come a little late; they move this much earlier,
/// never into the word before. Measured on hand-labelled English.
pub const EARLIER: Time = Time(FLICKS_PER_SECOND * 20 / 1000);
/// Next to a pause of at least [`PAUSE`] its starts come later still and
/// its ends early, clipping the onset of a phrase and the tail of its last
/// word: those edges move this much into the pause, never past its middle.
/// Measured against forced-aligned LibriSpeech.
pub const INTO_PAUSE: Time = Time(FLICKS_PER_SECOND * 20 / 1000);
pub const PAUSE: Time = Time(FLICKS_PER_SECOND / 10);
