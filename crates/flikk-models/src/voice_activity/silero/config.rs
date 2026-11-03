//! Silero's file, and how it is run.

use crate::{ModelFile, SPEECH_RATE};

/// Silero VAD as ggml, which whisper.cpp runs.
pub const FILE: ModelFile = ModelFile {
    name: "ggml-silero-v6.2.0.bin",
    url: "https://huggingface.co/ggml-org/whisper-vad/resolve/9ffd54a1e1ee413ddf265af9913beaf518d1639b/ggml-silero-v6.2.0.bin",
    env: Some("FLIKK_VAD_MODEL"),
    what: "voice activity model",
    size: 885_098,
};

/// Silero hears 16 kHz audio in windows of this many samples.
pub const WINDOW: usize = 512;
/// Music holds Silero down for seconds after it ends, missing the first
/// words said, so it also hears the audio in 4 s blocks, each after the
/// second before it. A block that starts during a sentence doubts the rest
/// of it, so a window is speech when either hearing finds it.
pub const BLOCK: usize = 125 * WINDOW;
pub const WARM_UP: usize = 32 * WINDOW;
/// Speech starts where Silero is sure of it and lasts until it doubts it.
pub const THRESHOLD: f32 = 0.5;
pub const DOUBT: f32 = THRESHOLD - 0.15;
/// faster-whisper's VAD filter for the stretches Whisper hears: they end
/// once Silero has doubted the speech for 2 s, and take 400 ms either side.
pub const MIN_SILENCE: usize = 2 * SPEECH_RATE as usize;
pub const SPEECH_PAD: usize = SPEECH_RATE as usize * 400 / 1000;
/// Too small a model to gain from threads: one runs three times faster
/// than four.
pub const THREADS: i32 = 1;
