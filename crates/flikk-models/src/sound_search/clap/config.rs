//! CLAP's files, and how its towers are run.

use crate::{ModelFile, SOUND_RATE};

pub const AUDIO: ModelFile = ModelFile {
    name: "larger-clap-general-audio.onnx",
    url: "https://huggingface.co/Chris-Kode/larger-clap-general-onnx/resolve/ccfced4f8df016a4123b6d938821f600a5be7a00/larger-clap-general-audio.onnx",
    env: Some("FLIKK_CLAP_AUDIO_MODEL"),
    what: "sound search model",
    size: 78_682_146,
};
pub const TEXT: ModelFile = ModelFile {
    name: "larger-clap-general-text.onnx",
    url: "https://huggingface.co/Chris-Kode/larger-clap-general-onnx/resolve/ccfced4f8df016a4123b6d938821f600a5be7a00/larger-clap-general-text.onnx",
    env: Some("FLIKK_CLAP_TEXT_MODEL"),
    what: "sound search text model",
    size: 141_210_922,
};
/// RoBERTa's tokenizer, from the model's own repository.
pub const VOCAB: ModelFile = ModelFile {
    name: "larger-clap-general-vocab.json",
    url: "https://huggingface.co/laion/larger_clap_general/resolve/ada0c23a36c4e8582805bb38fec3905903f18b41/vocab.json",
    env: Some("FLIKK_CLAP_VOCAB"),
    what: "sound search vocabulary",
    size: 798_293,
};
pub const MERGES: ModelFile = ModelFile {
    name: "larger-clap-general-merges.txt",
    url: "https://huggingface.co/laion/larger_clap_general/resolve/ada0c23a36c4e8582805bb38fec3905903f18b41/merges.txt",
    env: Some("FLIKK_CLAP_MERGES"),
    what: "sound search vocabulary",
    size: 456_318,
};

/// Samples in a window, and between the starts of windows.
pub const WINDOW: usize = 4 * SOUND_RATE as usize;
pub const STEP: usize = 2 * SOUND_RATE as usize;
/// Windows run at once.
pub const BATCH: usize = 8;
/// What the audio tower hears: a window repeated to fill 10 s.
pub const INPUT: usize = 10 * SOUND_RATE as usize;
/// Windows quieter than this are left out (their vector stays zero).
pub const SILENT_DB: f32 = -60.0;
/// RoBERTa's `<s>` and `</s>`, and the longest text the tower reads.
pub const BOS: i64 = 0;
pub const EOS: i64 = 2;
pub const MAX_TOKENS: usize = 77;
