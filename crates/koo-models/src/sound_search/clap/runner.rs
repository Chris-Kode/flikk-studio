//! The audio tower run on batches of windows, each repeated to fill
//! 10 s, and the text tower on a tokenized description.

use super::config::{
    AUDIO, BATCH, BOS, EOS, INPUT, MAX_TOKENS, MERGES, SILENT_DB, STEP, TEXT, VOCAB, WINDOW,
};
use crate::bpe::Bpe;
use crate::sound_search::SoundSearch;
use crate::{Windows, onnx};
use anyhow::{Result, anyhow, bail};
use koo_core::analysis::SoundVectors;
use ort::session::Session;
use ort::value::Tensor;
use std::sync::Mutex;

/// The towers, each loaded on first use and shared by every job; each
/// runs one batch at a time.
#[derive(Default)]
pub struct Clap {
    audio: Mutex<Option<Session>>,
    text: Mutex<Option<(Session, Bpe)>>,
}

impl SoundSearch for Clap {
    fn windows(&self) -> Windows {
        Windows {
            len: WINDOW,
            step: STEP,
            batch: BATCH,
        }
    }

    fn embed(&self, windows: &[&[f32]]) -> Result<Vec<Vec<f32>>> {
        let mut session = self.audio.lock().unwrap_or_else(|e| e.into_inner());
        if session.is_none() {
            *session = Some(onnx::session(&AUDIO.locate()?, AUDIO.what)?);
        }
        embed(session.as_mut().expect("loaded above"), windows)
    }

    fn describe(&self, text: &str) -> Result<Vec<f32>> {
        let mut loaded = self.text.lock().unwrap_or_else(|e| e.into_inner());
        if loaded.is_none() {
            *loaded = Some((
                onnx::session(&TEXT.locate()?, TEXT.what)?,
                Bpe::load(&VOCAB.locate()?, &MERGES.locate()?)?,
            ));
        }
        let (session, bpe) = loaded.as_mut().expect("loaded above");
        describe(session, bpe, text)
    }
}

/// The vector of each of `windows`, [`WINDOW`] samples each (the last may
/// be shorter); silent windows get zeros without running the model.
fn embed(session: &mut Session, windows: &[&[f32]]) -> Result<Vec<Vec<f32>>> {
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
    let input = Tensor::from_array(([heard.len(), INPUT], input)).map_err(|e| anyhow!("{e}"))?;
    let outputs = session
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

/// The unit vector of `text`.
fn describe(session: &mut Session, bpe: &Bpe, text: &str) -> Result<Vec<f32>> {
    let mut ids = vec![BOS];
    ids.extend(bpe.encode_text(text.trim()));
    ids.truncate(MAX_TOKENS - 1);
    ids.push(EOS);
    let n = ids.len();
    let ids = Tensor::from_array(([1, n], ids)).map_err(|e| anyhow!("{e}"))?;
    let mask = Tensor::from_array(([1, n], vec![1i64; n])).map_err(|e| anyhow!("{e}"))?;
    let outputs = session
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
