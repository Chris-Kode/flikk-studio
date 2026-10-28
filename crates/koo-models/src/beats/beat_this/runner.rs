//! The tracker run over a stretch in chunks as it decodes, keeping each
//! chunk's reliable middle, and its logits read into beats.

use super::config::{BORDER, CHUNK, FILE, FRAME, FRAME_TIME, HOP};
use crate::beats::{BeatTracker, Stream};
use crate::onnx;
use anyhow::{Result, anyhow, bail};
use koo_core::time::Time;
use ort::session::Session;
use ort::value::Tensor;
use std::sync::Mutex;

/// The model, loaded on first use and shared by every job; it tracks one
/// stretch at a time.
#[derive(Default)]
pub struct BeatThis {
    session: Mutex<Option<Session>>,
}

impl BeatTracker for BeatThis {
    fn track(&self, stream: Stream) -> Result<(Vec<Time>, Vec<Time>)> {
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if session.is_none() {
            *session = Some(onnx::session(&FILE.locate()?, FILE.what)?);
        }
        let mut tracker = Tracker::new(session.as_mut().expect("loaded above"));
        stream(&mut |samples| tracker.push(samples))?;
        tracker.finish()
    }
}

/// Beat and downbeat logits of each frame of `samples`.
fn logits(session: &mut Session, samples: &[f32]) -> Result<(Vec<f32>, Vec<f32>)> {
    let input =
        Tensor::from_array(([samples.len()], samples.to_vec())).map_err(|e| anyhow!("{e}"))?;
    let outputs = session
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

/// Beat tracking over one stretch of audio, fed to it as it decodes.
struct Tracker<'m> {
    session: &'m mut Session,
    /// Audio not yet past a chunk, starting `BORDER` frames before the
    /// first frame still to predict (silence before the stretch).
    buf: Vec<f32>,
    pushed: usize,
    beat: Vec<f32>,
    down: Vec<f32>,
}

impl<'m> Tracker<'m> {
    fn new(session: &'m mut Session) -> Self {
        Self {
            session,
            buf: vec![0.0; BORDER * FRAME],
            pushed: 0,
            beat: Vec::new(),
            down: Vec::new(),
        }
    }

    fn push(&mut self, samples: &[f32]) -> Result<()> {
        self.buf.extend_from_slice(samples);
        self.pushed += samples.len();
        while self.buf.len() >= CHUNK * FRAME {
            let (beat, down) = logits(self.session, &self.buf[..CHUNK * FRAME])?;
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
    fn finish(mut self) -> Result<(Vec<Time>, Vec<Time>)> {
        let total = self.pushed.div_ceil(FRAME);
        let left = total.saturating_sub(self.beat.len());
        if left > 0 {
            self.buf.resize(self.buf.len() + BORDER * FRAME, 0.0);
            let buf = std::mem::take(&mut self.buf);
            let (beat, down) = logits(self.session, &buf)?;
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
