//! Whisper's files, one per size, and how they are run.

use crate::{ModelFile, SPEECH_RATE};

/// `$FLIKK_WHISPER_MODEL` stands in for whichever size is picked: a
/// path, or a ggml file name in `paths::models()`.
const ENV: Option<&str> = Some("FLIKK_WHISPER_MODEL");

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
pub const GPU: bool = cfg!(any(feature = "metal", feature = "vulkan"));

/// Threads at most; more stop paying off, and leave none to the rest of
/// analysis.
pub const MAX_THREADS: usize = 8;

/// Whisper hears 30 s at once.
pub const WINDOW: usize = 30 * SPEECH_RATE as usize;
/// Silence a stretch that fits one window is heard after. Speech from the
/// very start of what Whisper hears is taken for a phrase cut off and left
/// out (the first words of an item that starts talking), and Whisper that
/// starts on speech more often leaves the last phrase of a stretch out
/// too. A longer stretch goes on in windows whisper.cpp starts where
/// Whisper finished a segment; silence before it moved them, and in
/// conversation a window then ended at its first sentence.
pub const LEAD: usize = SPEECH_RATE as usize / 2;
/// The speech fades in from that silence over 10 ms.
pub const FADE: usize = SPEECH_RATE as usize / 100;
/// Beams searched, as OpenAI's transcribe does; one guess at a time drops
/// and swaps more words.
pub const BEAM_SIZE: i32 = 5;
/// Whisper's odds that a stretch holds no speech, read where OpenAI reads
/// them, before its first word (whisper.cpp reads a row of logits it never
/// fills). Above this it is taken at its word and not heard out: given
/// silence, hum or noise Silero let through, Whisper small wrote a
/// confident line ("Subtítulos en español de…") at 0.4 to 0.85, and speech
/// was at most 0.22.
pub const NO_SPEECH: f32 = 0.6;
/// Above this, the words are kept only if Whisper was sure of them: those
/// lines came at -0.53 to -0.57 a token, speech at -0.05 to -0.5.
pub const DOUBT: f32 = 0.35;
pub const MIN_LOGPROB: f32 = -0.4;
/// The most times a run of tokens is said in a row: twice, or three times
/// for one or two ("no, no, no"). Past that, Whisper is in a loop, which
/// whisper.cpp falls back from but keeps when every temperature loops (220
/// tokens of one character, over noise).
pub const fn repeats(tokens: usize) -> usize {
    if tokens <= 2 { 3 } else { 2 }
}
