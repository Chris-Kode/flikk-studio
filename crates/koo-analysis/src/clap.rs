//! Vectors for finding a sound by description, from LAION's
//! larger_clap_general (Apache-2.0): its audio tower turns 4 s windows
//! every 2 s into unit vectors, its text tower turns a description into
//! one, and the closer two are the better the description fits.

use crate::bpe::Bpe;
use crate::ffmpeg::SOUND_RATE;
use anyhow::{Result, anyhow, bail};
use koo_core::analysis::SoundVectors;
use koo_core::time::{FLICKS_PER_SECOND, Time};
use ort::session::Session;
use ort::value::Tensor;
use std::path::PathBuf;

/// Samples in a window, and between the starts of windows.
pub const WINDOW: usize = 4 * SOUND_RATE as usize;
pub const STEP: usize = 2 * SOUND_RATE as usize;
pub const WINDOW_TIME: Time = Time(FLICKS_PER_SECOND * 4);
pub const STEP_TIME: Time = Time(FLICKS_PER_SECOND * 2);
/// Windows run at once.
pub const BATCH: usize = 8;
/// What the audio tower hears: a window repeated to fill 10 s.
const INPUT: usize = 10 * SOUND_RATE as usize;
/// Windows quieter than this are left out (their vector stays zero).
const SILENT_DB: f32 = -60.0;
/// RoBERTa's `<s>` and `</s>`, and the longest text the tower reads.
const BOS: i64 = 0;
const EOS: i64 = 2;
const MAX_TOKENS: usize = 77;

pub struct ClapAudio {
    session: Session,
}

impl ClapAudio {
    pub fn load(model: &std::path::Path) -> Result<Self> {
        Ok(Self {
            session: crate::onnx::session(model, koo_models::files::CLAP_AUDIO.what)?,
        })
    }

    /// The vector of each of `windows`, [`WINDOW`] samples each (the last
    /// may be shorter); silent windows get zeros without running the model.
    pub fn embed(&mut self, windows: &[&[f32]]) -> Result<Vec<Vec<f32>>> {
        let mut out = vec![vec![0.0; SoundVectors::DIMS]; windows.len()];
        let heard: Vec<usize> = (0..windows.len())
            .filter(|&i| !windows[i].is_empty() && rms_db(windows[i]) > SILENT_DB)
            .collect();
        if heard.is_empty() {
            return Ok(out);
        }
        let mut input = Vec::with_capacity(heard.len() * INPUT);
        for &i in &heard {
            input.extend(repeat_pad(windows[i]));
        }
        let input =
            Tensor::from_array(([heard.len(), INPUT], input)).map_err(|e| anyhow!("{e}"))?;
        let outputs = self
            .session
            .run(ort::inputs!["samples" => input])
            .map_err(|e| anyhow!("sound search model failed: {e}"))?;
        let (_, v) = outputs["embedding"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        if v.len() != heard.len() * SoundVectors::DIMS {
            bail!(
                "sound search model gave {} numbers for {} windows",
                v.len(),
                heard.len()
            );
        }
        for (k, &i) in heard.iter().enumerate() {
            out[i] = v[k * SoundVectors::DIMS..(k + 1) * SoundVectors::DIMS].to_vec();
        }
        Ok(out)
    }
}

/// The window repeated as many whole times as fit in 10 s, then silence,
/// as CLAP's feature extractor pads ("repeatpad").
fn repeat_pad(window: &[f32]) -> Vec<f32> {
    let w = &window[..window.len().min(INPUT)];
    let mut out = Vec::with_capacity(INPUT);
    for _ in 0..INPUT / w.len() {
        out.extend_from_slice(w);
    }
    out.resize(INPUT, 0.0);
    out
}

fn rms_db(samples: &[f32]) -> f32 {
    let energy = samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32;
    10.0 * energy.max(1e-12).log10()
}

/// The text tower and its tokenizer.
pub struct ClapText {
    session: Session,
    bpe: Bpe,
}

impl ClapText {
    pub fn load(files: &TextFiles) -> Result<Self> {
        Ok(Self {
            session: crate::onnx::session(&files.model, koo_models::files::CLAP_TEXT.what)?,
            bpe: Bpe::load(&files.vocab, &files.merges)?,
        })
    }

    /// The unit vector of `text`.
    pub fn embed(&mut self, text: &str) -> Result<Vec<f32>> {
        let mut ids = vec![BOS];
        ids.extend(self.bpe.encode_text(text.trim()));
        ids.truncate(MAX_TOKENS - 1);
        ids.push(EOS);
        let n = ids.len();
        let ids = Tensor::from_array(([1, n], ids)).map_err(|e| anyhow!("{e}"))?;
        let mask = Tensor::from_array(([1, n], vec![1i64; n])).map_err(|e| anyhow!("{e}"))?;
        let outputs = self
            .session
            .run(ort::inputs!["input_ids" => ids, "attention_mask" => mask])
            .map_err(|e| anyhow!("sound search text model failed: {e}"))?;
        let (_, v) = outputs["embedding"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        if v.len() != SoundVectors::DIMS {
            bail!("sound search text model gave {} numbers", v.len());
        }
        Ok(v.to_vec())
    }
}

/// The text tower's files on disk.
pub struct TextFiles {
    model: PathBuf,
    vocab: PathBuf,
    merges: PathBuf,
}

/// The text tower's files, if they are downloaded.
pub fn text_files() -> Result<TextFiles> {
    use koo_models::files::{CLAP_MERGES, CLAP_TEXT, CLAP_VOCAB};
    Ok(TextFiles {
        model: CLAP_TEXT.locate()?,
        vocab: CLAP_VOCAB.locate()?,
        merges: CLAP_MERGES.locate()?,
    })
}
