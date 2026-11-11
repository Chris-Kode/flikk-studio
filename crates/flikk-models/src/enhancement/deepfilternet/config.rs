//! DeepFilterNet 3's file, and every number it is run with: those of its
//! `config.ini` (`[df]` and `[deepfilternet]`), and how long a stretch it
//! runs on.

use crate::ModelFile;

/// The authors' ONNX export (`enc.onnx`, `erb_dec.onnx`, `df_dec.onnx` and
/// `config.ini` in a gzipped tar), from their repository at the commit
/// that states the weights are MIT or Apache-2.0 like the code.
pub const ARCHIVE: ModelFile = ModelFile {
    name: "deepfilternet3-onnx.tar.gz",
    url: "https://raw.githubusercontent.com/Rikorose/DeepFilterNet/7b14b895fe7fc42cab72b4a4c1e49c982bd6404b/models/DeepFilterNet3_onnx.tar.gz",
    env: Some("FLIKK_DEEPFILTERNET"),
    what: "Clean voice model",
    size: 7_983_136,
};

/// FFT size and hop, samples: 20 ms windows every 10 ms at 48 kHz.
pub const FFT: usize = 960;
pub const HOP: usize = 480;
/// Frequency bins of a frame.
pub const BINS: usize = FFT / 2 + 1;
/// ERB bands the gains are given in, and the fewest bins one spans.
pub const ERB_BANDS: usize = 32;
pub const ERB_MIN_BINS: usize = 2;
/// Bins the deep filter covers, from the lowest (4.8 kHz).
pub const DF_BINS: usize = 96;
/// Frames the deep filter spans, and how many of them are ahead.
pub const DF_ORDER: usize = 5;
pub const DF_LOOKAHEAD: usize = 2;
/// Frames the encoder looks ahead: its features are shifted this far.
pub const CONV_LOOKAHEAD: usize = 2;
/// Smoothing of the feature normalization, per frame: `exp(-hop / sr /
/// norm_tau)` with `norm_tau` 1 s, rounded as `libDF` rounds it.
pub const NORM_ALPHA: f32 = 0.99;
/// Where the normalizations start: the mean (dB) from the lowest ERB band
/// to the highest, the unit norm from the lowest deep-filter bin up.
pub const MEAN_NORM_INIT: [f32; 2] = [-60.0, -90.0];
pub const UNIT_NORM_INIT: [f32; 2] = [0.001, 0.0001];
/// Frames run at once, and frames before them run again first so the
/// recurrent layers settle: 30 s in stretches, with 5 s before each. On
/// the authors' sample (`noisy_snr0.wav`) stretches of 3 s or more with
/// 3 s or more before them score within 0.05 dB SI-SDR of the whole file
/// at once; 1 s before costs 0.8 dB.
pub const CHUNK: usize = 3_000;
pub const WARM_UP: usize = 500;
