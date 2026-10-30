//! Segmentation: the object that clicks and a box pick in a picture, and
//! the same object followed through the frames around it.

pub mod sam2;

use crate::Model;
use anyhow::Result;
use koo_core::services::Frame;

/// The models that can do it, the default first.
pub const MODELS: &[&Model] = &[&sam2::MODEL];

/// What picks the object in one frame, in the picture's uv (0..1 across
/// it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prompt {
    /// Clicks, and whether each is on the object.
    pub points: Vec<([f32; 2], bool)>,
    /// A box around it: left, top, right, bottom.
    pub rect: Option<[f32; 4]>,
}

impl Prompt {
    pub fn is_empty(&self) -> bool {
        self.points.is_empty() && self.rect.is_none()
    }
}

/// The object in one frame.
#[derive(Clone, Debug)]
pub struct Segment {
    /// `side` × `side` logits over the whole picture (stretched to a
    /// square), row by row; positive on the object.
    pub logits: Vec<f32>,
    pub side: usize,
    /// How well the model thinks the logits fit the prompt, 0..1.
    pub score: f32,
    /// Whether the object is in the frame at all (it may be hidden).
    pub present: bool,
}

/// A frame as the model sees it: the slow part of segmenting, kept to
/// answer several prompts on the same frame.
pub struct Encoded {
    pub(crate) features: Vec<Vec<f32>>,
}

/// Finds and follows objects in pictures.
pub trait Segmenter: Send + Sync {
    /// `picture` (any size; stretched to the model's square) ready to
    /// segment.
    fn encode(&self, picture: &Frame) -> Result<Encoded>;

    /// The object `prompt` picks in an encoded frame, by itself.
    fn segment(&self, frame: &Encoded, prompt: &Prompt) -> Result<Segment>;

    /// Follow the object through frames `0..count`: each of `prompts`
    /// (frame, prompt) picks it on its frame by itself; the frames after
    /// the first prompted one are tracked forward from it and the frames
    /// before it backward, each looking back at the prompted frames and
    /// the ones just tracked. `frame` hands over frame `i` when asked (the
    /// prompted ones, then in tracking order); `done` takes each frame's
    /// object as it is found and stops the tracking when it fails.
    fn track(
        &self,
        count: usize,
        prompts: &[(usize, Prompt)],
        frame: &mut dyn FnMut(usize) -> Result<Frame>,
        done: &mut dyn FnMut(usize, Segment) -> Result<()>,
    ) -> Result<()>;
}

/// The runner of `model`, one of [`MODELS`].
#[cfg(feature = "segment")]
pub fn runner(model: &Model) -> Box<dyn Segmenter> {
    match model.id {
        sam2::ID => Box::<sam2::Sam2>::default(),
        id => unreachable!("{id} is not a segmentation model"),
    }
}
