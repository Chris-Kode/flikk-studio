//! Every model file the editor can run, pinned to the commit it was
//! published at, and the models they make up. How the exports were made and
//! measured is in `docs/TECHNICAL_DESIGN.md`.

/// The files, one constant each, so the code that runs a model names the
/// file it opens.
pub mod files {
    use crate::ModelFile;

    /// Silero VAD as ggml, which whisper.cpp runs.
    pub const SILERO: ModelFile = ModelFile {
        name: "ggml-silero-v6.2.0.bin",
        url: "https://huggingface.co/ggml-org/whisper-vad/resolve/9ffd54a1e1ee413ddf265af9913beaf518d1639b/ggml-silero-v6.2.0.bin",
        env: Some("KOOEDITOR_VAD_MODEL"),
        what: "voice activity model",
        size: 885_098,
    };

    pub const WHISPER_BASE: ModelFile = ModelFile {
        name: "ggml-base.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        env: Some("KOOEDITOR_WHISPER_MODEL"),
        what: "Whisper model",
        size: 147_951_465,
    };

    /// Qwen3-ForcedAligner-0.6B as ONNX, with its Whisper-style features
    /// built in, weights quantized to 4 bits. Qwen publishes no ONNX
    /// export; this one is ours.
    pub const ALIGNER: ModelFile = ModelFile {
        name: "qwen3-forced-aligner-0.6b-q4.onnx",
        url: "https://huggingface.co/Chris-Kode/qwen3-forced-aligner-0.6b-onnx/resolve/c1386c6a094f575f2067cf82632c30238c1b1f9d/qwen3-forced-aligner-0.6b-q4.onnx",
        env: Some("KOOEDITOR_ALIGNER_MODEL"),
        what: "alignment model",
        size: 589_447_679,
    };
    /// Qwen's tokenizer, from the model's own repository.
    pub const ALIGNER_VOCAB: ModelFile = ModelFile {
        name: "qwen3-forced-aligner-vocab.json",
        url: "https://huggingface.co/Qwen/Qwen3-ForcedAligner-0.6B/resolve/c7cbfc2048c462b0d63a45797104fc9db3ad62b7/vocab.json",
        env: None,
        what: "alignment tokenizer",
        size: 2_776_833,
    };
    pub const ALIGNER_MERGES: ModelFile = ModelFile {
        name: "qwen3-forced-aligner-merges.txt",
        url: "https://huggingface.co/Qwen/Qwen3-ForcedAligner-0.6B/resolve/c7cbfc2048c462b0d63a45797104fc9db3ad62b7/merges.txt",
        env: None,
        what: "alignment tokenizer",
        size: 1_671_853,
    };

    /// pyannote segmentation-3.0 as ONNX.
    pub const SEGMENTATION: ModelFile = ModelFile {
        name: "pyannote-segmentation-3.0.onnx",
        url: "https://huggingface.co/onnx-community/pyannote-segmentation-3.0/resolve/733a93b6473d019a773298e08cefa686894b1854/onnx/model.onnx",
        env: None,
        what: "speaker segmentation model",
        size: 5_986_908,
    };
    /// WeSpeaker's own ONNX export of ResNet34-LM.
    pub const EMBEDDING: ModelFile = ModelFile {
        name: "wespeaker-voxceleb-resnet34-lm.onnx",
        url: "https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM/resolve/f0c48c298fd835726c27956a5d617bad7115627e/voxceleb_resnet34_LM.onnx",
        env: None,
        what: "speaker embedding model",
        size: 26_530_309,
    };

    pub const TAGGER: ModelFile = ModelFile {
        name: "efficientat-mn10-as.onnx",
        url: "https://huggingface.co/Chris-Kode/efficientat-mn10-as-onnx/resolve/81f0e383972c52b354de924778fec25b2a4a29d3/efficientat-mn10-as.onnx",
        env: Some("KOOEDITOR_TAGGER_MODEL"),
        what: "sound tagger",
        size: 20_200_590,
    };

    pub const BEATS: ModelFile = ModelFile {
        name: "beat-this-final0.onnx",
        url: "https://huggingface.co/Chris-Kode/beat-this-final0-onnx/resolve/49876475cff9b3abe3eec5303fa539cff06843fd/beat-this-final0.onnx",
        env: Some("KOOEDITOR_BEATS_MODEL"),
        what: "beat tracker",
        size: 84_973_959,
    };

    pub const CLAP_AUDIO: ModelFile = ModelFile {
        name: "larger-clap-general-audio.onnx",
        url: "https://huggingface.co/Chris-Kode/larger-clap-general-onnx/resolve/ccfced4f8df016a4123b6d938821f600a5be7a00/larger-clap-general-audio.onnx",
        env: Some("KOOEDITOR_CLAP_AUDIO_MODEL"),
        what: "sound search model",
        size: 78_682_146,
    };
    pub const CLAP_TEXT: ModelFile = ModelFile {
        name: "larger-clap-general-text.onnx",
        url: "https://huggingface.co/Chris-Kode/larger-clap-general-onnx/resolve/ccfced4f8df016a4123b6d938821f600a5be7a00/larger-clap-general-text.onnx",
        env: Some("KOOEDITOR_CLAP_TEXT_MODEL"),
        what: "sound search text model",
        size: 141_210_922,
    };
    /// RoBERTa's tokenizer, from the model's own repository.
    pub const CLAP_VOCAB: ModelFile = ModelFile {
        name: "larger-clap-general-vocab.json",
        url: "https://huggingface.co/laion/larger_clap_general/resolve/ada0c23a36c4e8582805bb38fec3905903f18b41/vocab.json",
        env: Some("KOOEDITOR_CLAP_VOCAB"),
        what: "sound search vocabulary",
        size: 798_293,
    };
    pub const CLAP_MERGES: ModelFile = ModelFile {
        name: "larger-clap-general-merges.txt",
        url: "https://huggingface.co/laion/larger_clap_general/resolve/ada0c23a36c4e8582805bb38fec3905903f18b41/merges.txt",
        env: Some("KOOEDITOR_CLAP_MERGES"),
        what: "sound search vocabulary",
        size: 456_318,
    };
}

/// The models, each the files one task needs.
pub mod models {
    use super::files;
    use crate::Model;

    pub const SILERO_VAD: Model = Model {
        id: "silero-vad-6.2",
        name: "Silero VAD 6.2",
        files: &[files::SILERO],
    };

    pub const WHISPER_BASE: Model = Model {
        id: "whisper-base",
        name: "Whisper base",
        files: &[files::WHISPER_BASE],
    };

    pub const QWEN3_ALIGNER: Model = Model {
        id: "qwen3-forced-aligner-0.6b",
        name: "Qwen3 Aligner 0.6B",
        files: &[files::ALIGNER, files::ALIGNER_VOCAB, files::ALIGNER_MERGES],
    };

    pub const PYANNOTE: Model = Model {
        id: "pyannote-3.1",
        name: "pyannote + WeSpeaker",
        files: &[files::SEGMENTATION, files::EMBEDDING],
    };

    pub const EFFICIENTAT: Model = Model {
        id: "efficientat-mn10-as",
        name: "EfficientAT mn10",
        files: &[files::TAGGER],
    };

    pub const BEAT_THIS: Model = Model {
        id: "beat-this-final0",
        name: "Beat This!",
        files: &[files::BEATS],
    };

    /// The audio tower runs in analysis; the text tower and its tokenizer
    /// when a sound is looked for by description.
    pub const CLAP: Model = Model {
        id: "larger-clap-general",
        name: "LAION CLAP",
        files: &[
            files::CLAP_AUDIO,
            files::CLAP_TEXT,
            files::CLAP_VOCAB,
            files::CLAP_MERGES,
        ],
    };
}
