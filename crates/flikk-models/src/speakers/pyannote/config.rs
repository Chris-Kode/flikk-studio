//! The two models' files, and how they are run.

use crate::{ModelFile, SPEECH_RATE};
use flikk_core::time::{FLICKS_PER_SECOND, Time};

/// pyannote segmentation-3.0 as ONNX: 16 kHz samples in, a powerset class
/// per frame out (nobody, one of three voices, or two of them).
pub const SEGMENTATION: ModelFile = ModelFile {
    name: "pyannote-segmentation-3.0.onnx",
    url: "https://huggingface.co/onnx-community/pyannote-segmentation-3.0/resolve/733a93b6473d019a773298e08cefa686894b1854/onnx/model.onnx",
    env: None,
    what: "speaker segmentation model",
    size: 5_986_908,
    sha256: "057ee564753071c0b09b5b611648b50ac188d50846bff5f01e9f7bbf1591ea25",
};
/// WeSpeaker's own ONNX export of ResNet34-LM: Kaldi filterbank features
/// in, a 256-number voiceprint out.
pub const EMBEDDING: ModelFile = ModelFile {
    name: "wespeaker-voxceleb-resnet34-lm.onnx",
    url: "https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM/resolve/f0c48c298fd835726c27956a5d617bad7115627e/voxceleb_resnet34_LM.onnx",
    env: None,
    what: "speaker embedding model",
    size: 26_530_309,
    sha256: "7bb2f06e9df17cdf1ef14ee8a15ab08ed28e8d0ef5054ee135741560df2ec068",
};

/// Samples the segmentation model hears at once.
pub const WINDOW: usize = 10 * SPEECH_RATE as usize;
/// Samples between windows; a multiple of the filterbank's 10 ms shift.
/// pyannote uses 1 s; 2.5 s told voices apart as well on AMI meetings in a
/// third of the time, 5 s merged voices.
pub const STEP: usize = SPEECH_RATE as usize * 5 / 2;
/// Segmentation frames per window, and samples between them.
pub const FRAMES: usize = 589;
pub const FRAME_STEP: usize = 270;
/// Samples each segmentation frame hears.
pub const FRAME_SPAN: usize = 991;
/// Voices a window tells apart, one bit each per frame.
pub const LOCAL: usize = 3;
/// Bits for each powerset class.
pub const CLASSES: [u8; 7] = [0b000, 0b001, 0b010, 0b100, 0b011, 0b101, 0b110];
/// Windows segmented per model run.
pub const BATCH: usize = 4;
/// Features a voiceprint needs; fewer give none.
pub const MIN_FEATURES: usize = 10;
/// Windows whose features are computed together.
pub const BLOCK: usize = 60;
/// pyannote 3.1's clustering: centroids further apart than this (unit
/// voiceprints, Euclidean) are different voices…
pub const THRESHOLD: f32 = 0.704_565_5;
/// …and clusters of fewer voiceprints join the nearest larger one.
pub const MIN_CLUSTER: usize = 12;
/// Voiceprints the clustering is fitted on; longer items use an even
/// sample and match the rest to its clusters.
pub const MAX_FITTED: usize = 2000;
/// How far around a word to look when nobody speaks in it.
pub const REACH: Time = Time(FLICKS_PER_SECOND / 2);
