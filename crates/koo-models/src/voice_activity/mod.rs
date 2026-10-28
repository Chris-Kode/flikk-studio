//! Voice activity: where someone talks in an item's audio. Its speech
//! ranges are what silences and cuts go by; its looser stretches are what
//! a transcriber hears.

pub mod silero;

use crate::{Model, SPEECH_RATE};
use anyhow::Result;
use koo_core::time::{Time, TimeRange};
use std::ops::Range;

/// The models that can do it, the default first.
pub const MODELS: &[&Model] = &[&silero::MODEL];

/// Hears where someone talks.
pub trait VoiceActivity: Send + Sync {
    /// The speech in [`SPEECH_RATE`] mono `audio`; `progress` takes 0..1.
    fn hear(&self, audio: &[f32], progress: &dyn Fn(f32)) -> Result<Speech>;
}

/// Where someone talks in a stretch of audio.
pub struct Speech {
    /// Tight ranges, shaped by [`ranges`].
    pub ranges: Vec<TimeRange>,
    /// The looser stretches a transcriber hears, in samples.
    pub stretches: Vec<Range<usize>>,
}

/// The runner of `model`, one of [`MODELS`].
#[cfg(feature = "whisper")]
pub fn runner(model: &Model) -> Box<dyn VoiceActivity> {
    match model.id {
        silero::ID => Box::<silero::Silero>::default(),
        id => unreachable!("{id} is not a voice activity model"),
    }
}

/// Pauses shorter than this stay inside one speech range.
pub const BRIDGE_MS: i64 = 250;
/// Bursts shorter than this are clicks, not speech.
pub const MIN_SPEECH_MS: i64 = 120;
/// Padding before each range so word onsets are not clipped.
pub const PAD_MS: i64 = 60;

/// Speech in [`SPEECH_RATE`] audio `len` samples long, from whether each
/// frame of `frame` samples is voiced: pauses under [`BRIDGE_MS`] kept
/// inside, bursts under [`MIN_SPEECH_MS`] dropped, and [`PAD_MS`] before
/// each range and `tail` after it. Every way of hearing speech, a model's
/// or not, shapes its ranges so.
pub fn ranges(voiced: &[bool], frame: usize, len: usize, tail: Time) -> Vec<TimeRange> {
    let frame_time = |i: usize| Time::from_secs_f64((i * frame) as f64 / SPEECH_RATE as f64);
    let end = Time::from_secs_f64(len as f64 / SPEECH_RATE as f64);
    let mut runs: Vec<TimeRange> = Vec::new();
    let mut start = None;
    for (i, v) in voiced.iter().chain([&false]).enumerate() {
        match (start, *v) {
            (None, true) => start = Some(i),
            (Some(s), false) => {
                runs.push(TimeRange::new(frame_time(s), frame_time(i).min(end)));
                start = None;
            }
            _ => {}
        }
    }

    let bridge = Time::from_millis(BRIDGE_MS);
    let mut merged: Vec<TimeRange> = Vec::new();
    for r in runs {
        match merged.last_mut() {
            Some(last) if r.start - last.end < bridge => last.end = r.end,
            _ => merged.push(r),
        }
    }
    merged.retain(|r| r.duration() >= Time::from_millis(MIN_SPEECH_MS));

    let pad = Time::from_millis(PAD_MS);
    let mut padded: Vec<TimeRange> = Vec::new();
    for r in merged {
        let r = TimeRange::new((r.start - pad).max(Time::ZERO), (r.end + tail).min(end));
        match padded.last_mut() {
            Some(last) if r.start <= last.end => last.end = r.end,
            _ => padded.push(r),
        }
    }
    padded
}
