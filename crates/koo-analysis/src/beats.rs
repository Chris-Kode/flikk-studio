//! Beats and downbeats in music with Beat This! (MIT, CPJKU), its `final0`
//! checkpoint: logits for every 20 ms frame of up to 30 s at a time, read
//! as Beat This!'s own "minimal" postprocessor does (peaks above 0.5
//! within 70 ms, neighbours averaged, downbeats moved onto beats).

use crate::ffmpeg::SOUND_RATE;
use anyhow::{Result, anyhow, bail};
use koo_core::time::{FLICKS_PER_SECOND, Time};
use ort::session::Session;
use ort::value::Tensor;

/// Samples per 20 ms frame.
const FRAME: usize = SOUND_RATE as usize / 50;
/// Frames the model hears at once, as it was trained.
const CHUNK: usize = 1500;
/// Frames at each end of a chunk whose predictions are unreliable.
const BORDER: usize = 6;
const HOP: usize = CHUNK - 2 * BORDER;
const FRAME_TIME: Time = Time(FLICKS_PER_SECOND / 50);

pub struct BeatModel {
    session: Session,
}

impl BeatModel {
    pub fn load(model: &std::path::Path) -> Result<Self> {
        Ok(Self {
            session: crate::onnx::session(model, koo_models::files::BEATS.what)?,
        })
    }

    /// Beat and downbeat logits of each frame of `samples`.
    fn logits(&mut self, samples: &[f32]) -> Result<(Vec<f32>, Vec<f32>)> {
        let input =
            Tensor::from_array(([samples.len()], samples.to_vec())).map_err(|e| anyhow!("{e}"))?;
        let outputs = self
            .session
            .run(ort::inputs!["samples" => input])
            .map_err(|e| anyhow!("beat tracker failed: {e}"))?;
        let get = |name: &str| -> Result<Vec<f32>> {
            let (_, v) = outputs[name]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow!("{e}"))?;
            Ok(v.to_vec())
        };
        let (beat, down) = (get("beat")?, get("downbeat")?);
        if beat.len() != down.len() {
            bail!(
                "beat tracker gave {} beats and {} downbeats",
                beat.len(),
                down.len()
            );
        }
        Ok((beat, down))
    }
}

/// Beat tracking over one stretch of audio, fed to it as it decodes.
pub struct Tracker<'m> {
    model: &'m mut BeatModel,
    /// Audio not yet past a chunk, starting `BORDER` frames before the
    /// first frame still to predict (silence before the stretch).
    buf: Vec<f32>,
    pushed: usize,
    beat: Vec<f32>,
    down: Vec<f32>,
}

impl<'m> Tracker<'m> {
    pub fn new(model: &'m mut BeatModel) -> Self {
        Self {
            model,
            buf: vec![0.0; BORDER * FRAME],
            pushed: 0,
            beat: Vec::new(),
            down: Vec::new(),
        }
    }

    pub fn push(&mut self, samples: &[f32]) -> Result<()> {
        self.buf.extend_from_slice(samples);
        self.pushed += samples.len();
        while self.buf.len() >= CHUNK * FRAME {
            let (beat, down) = self.model.logits(&self.buf[..CHUNK * FRAME])?;
            self.keep(&beat, &down, HOP);
            self.buf.drain(..HOP * FRAME);
        }
        Ok(())
    }

    fn keep(&mut self, beat: &[f32], down: &[f32], frames: usize) {
        let end = (BORDER + frames).min(beat.len());
        if BORDER < end {
            self.beat.extend_from_slice(&beat[BORDER..end]);
            self.down.extend_from_slice(&down[BORDER..end]);
        }
    }

    /// Beats and downbeats from the start of the stretch.
    pub fn finish(mut self) -> Result<(Vec<Time>, Vec<Time>)> {
        let total = self.pushed.div_ceil(FRAME);
        let left = total.saturating_sub(self.beat.len());
        if left > 0 {
            self.buf.resize(self.buf.len() + BORDER * FRAME, 0.0);
            let buf = std::mem::take(&mut self.buf);
            let (beat, down) = self.model.logits(&buf)?;
            self.keep(&beat, &down, left);
        }
        let beats = peaks(&self.beat);
        let mut downbeats: Vec<Time> = peaks(&self.down)
            .into_iter()
            .filter_map(|d| beats.iter().min_by_key(|b| (b.0 - d.0).abs()).copied())
            .collect();
        downbeats.dedup();
        Ok((beats, downbeats))
    }
}

/// Frames whose logit is above 0 and the highest within 3 frames either
/// side; runs of neighbouring frames become their mean.
fn peaks(logits: &[f32]) -> Vec<Time> {
    let n = logits.len();
    let frames: Vec<usize> = (0..n)
        .filter(|&i| {
            let around = i.saturating_sub(3)..(i + 4).min(n);
            logits[i] > 0.0 && logits[around].iter().all(|&v| v <= logits[i])
        })
        .collect();
    let mut out: Vec<Time> = Vec::new();
    let mut group: Vec<usize> = Vec::new();
    let flush = |group: &mut Vec<usize>, out: &mut Vec<Time>| {
        if !group.is_empty() {
            let mean = group.iter().sum::<usize>() as f64 / group.len() as f64;
            out.push(Time((mean * FRAME_TIME.0 as f64).round() as i64));
            group.clear();
        }
    };
    for f in frames {
        if group.last().is_some_and(|&g| f - g > 1) {
            flush(&mut group, &mut out);
        }
        group.push(f);
    }
    flush(&mut group, &mut out);
    out
}
