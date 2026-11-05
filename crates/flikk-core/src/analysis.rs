//! Results of local analysis (PRD M3). All times are *source* times; map to
//! the timeline with [`crate::model::Clip::to_timeline`].

use crate::ids::SpeakerId;
use crate::model::Fingerprint;
use crate::time::{FLICKS_PER_SECOND, Time, TimeRange};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Bumped when results change shape or meaning. Results of another version
/// are not read: the item is not analyzed until the user analyzes it again.
pub const ANALYSIS_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Word {
    /// Where the word sounds: Whisper's estimate, moved onto the audio so
    /// that its edges sit at the pauses around it, then onto the letters a
    /// forced aligner hears.
    pub range: TimeRange,
    pub text: String,
    /// 0..1 from the recognizer; the caption pass flags low values (PRD recipes).
    pub confidence: f32,
    /// The voice heard saying it, or `None` when that is unclear.
    pub speaker: Option<SpeakerId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Speaker {
    pub id: SpeakerId,
    /// "Speaker 1" until confirmed by a source fact.
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub fingerprint: Fingerprint,
    /// Position in `MediaInfo::audio` (ffmpeg `0:a:N`), not the container index.
    pub audio_stream: u32,
    pub language: String,
    pub words: Vec<Word>,
    pub speakers: Vec<Speaker>,
    /// What transcribed it: a local model's id ("whisper-small-q8"), a
    /// provider's model ("elevenlabs/scribe_v2"), or "script" for a
    /// voiceover, whose words are known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
}

impl Transcript {
    pub fn words_in(&self, range: TimeRange) -> impl Iterator<Item = &Word> {
        self.words.iter().filter(move |w| w.range.overlaps(&range))
    }
}

/// A sound other than speech or music: the AudioSet
/// classes the sound tagger heard, under the name the sound script uses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sound {
    /// Where it sounds, edges moved onto the audio levels.
    pub range: TimeRange,
    /// "door slam", "laughter".
    pub label: String,
    /// The tagger's highest probability over the sound, 0..1.
    pub confidence: f32,
}

/// A stretch of music, with its beat when one was found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Music {
    pub range: TimeRange,
    /// Genre, mood, vocals and lead instruments, most likely first
    /// ("electronic", "upbeat").
    pub tags: Vec<String>,
    pub beats: Vec<Time>,
    /// The beats that start a bar.
    pub downbeats: Vec<Time>,
}

impl Music {
    /// Beats per minute; `None` with fewer than four beats.
    pub fn bpm(&self) -> Option<f32> {
        let gap = typical_gap(&self.beats)?;
        Some((60.0 / gap) as f32)
    }

    /// Length of a bar; `None` with fewer than four downbeats.
    pub fn bar(&self) -> Option<Time> {
        typical_gap(&self.downbeats).map(Time::from_secs_f64)
    }
}

/// The usual interval of `times` in seconds: the mean of the intervals
/// within 10 % of the median (beats sit on 20 ms frames, so single
/// intervals are coarse), from at least three intervals.
fn typical_gap(times: &[Time]) -> Option<f64> {
    let mut gaps: Vec<f64> = times
        .windows(2)
        .map(|w| (w[1] - w[0]).as_secs_f64())
        .filter(|g| *g > 0.0)
        .collect();
    if gaps.len() < 3 {
        return None;
    }
    gaps.sort_by(f64::total_cmp);
    let median = gaps[gaps.len() / 2];
    let near: Vec<f64> = gaps
        .into_iter()
        .filter(|g| (g - median).abs() <= median * 0.1)
        .collect();
    Some(near.iter().sum::<f64>() / near.len() as f64)
}

/// What a source sounds like, as unit vectors a description of a sound can
/// be matched against: one for every `step`, each heard
/// over `window` from its start. Kept apart from [`Analysis`] (an hour is
/// 1.8 MB), as bytes of int8 with a scale per vector.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundVectors {
    pub step: Time,
    pub window: Time,
    scales: Vec<f32>,
    values: Vec<i8>,
}

impl SoundVectors {
    pub const DIMS: usize = 512;
    const MAGIC: &'static [u8; 8] = b"FLKSNDV1";

    pub fn new(step: Time, window: Time) -> Self {
        Self {
            step,
            window,
            scales: Vec::new(),
            values: Vec::new(),
        }
    }

    /// Add the next window's vector ([`Self::DIMS`] numbers; zeros for
    /// silence).
    pub fn push(&mut self, v: &[f32]) {
        let max = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let scale = if max > 0.0 { max / 127.0 } else { 0.0 };
        self.scales.push(scale);
        self.values.extend(v.iter().take(Self::DIMS).map(|x| {
            if scale > 0.0 {
                (x / scale).round().clamp(-127.0, 127.0) as i8
            } else {
                0
            }
        }));
    }

    pub fn len(&self) -> usize {
        self.scales.len()
    }

    pub fn is_empty(&self) -> bool {
        self.scales.is_empty()
    }

    /// Where vector `i` was heard, in source time.
    pub fn range(&self, i: usize) -> TimeRange {
        let start = Time(self.step.0 * i as i64);
        TimeRange::new(start, start + self.window)
    }

    /// How well the unit vector `query` fits each window, -1..1; 0 for
    /// silence.
    pub fn scores(&self, query: &[f32]) -> Vec<f32> {
        self.scales
            .iter()
            .zip(self.values.chunks(Self::DIMS))
            .map(|(scale, v)| {
                let dot: f32 = v.iter().zip(query).map(|(&a, &b)| a as f32 * b).sum();
                dot * scale
            })
            .collect()
    }

    /// The unit-length mean of the vectors heard inside `range`, or of the
    /// one heard over most of it.
    pub fn mean(&self, range: TimeRange) -> Option<Vec<f32>> {
        let inside: Vec<usize> = (0..self.len())
            .filter(|&i| self.scales[i] > 0.0)
            .filter(|&i| {
                let r = self.range(i);
                r.start >= range.start && r.end <= range.end
            })
            .collect();
        let chosen = if inside.is_empty() {
            let best = (0..self.len())
                .filter(|&i| self.scales[i] > 0.0)
                .max_by_key(|&i| {
                    self.range(i)
                        .intersect(&range)
                        .map_or(0, |r| r.duration().0)
                })?;
            self.range(best).overlaps(&range).then_some(vec![best])?
        } else {
            inside
        };
        let mut sum = vec![0.0f32; Self::DIMS];
        for i in chosen {
            let scale = self.scales[i];
            for (s, &v) in sum.iter_mut().zip(&self.values[i * Self::DIMS..]) {
                *s += v as f32 * scale;
            }
        }
        let norm = sum.iter().map(|x| x * x).sum::<f32>().sqrt();
        (norm > 0.0).then(|| sum.iter().map(|x| x / norm).collect())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + self.len() * (4 + Self::DIMS));
        out.extend_from_slice(Self::MAGIC);
        out.extend_from_slice(&self.step.0.to_le_bytes());
        out.extend_from_slice(&self.window.0.to_le_bytes());
        out.extend_from_slice(&(self.len() as u32).to_le_bytes());
        for (i, scale) in self.scales.iter().enumerate() {
            out.extend_from_slice(&scale.to_le_bytes());
            out.extend(
                self.values[i * Self::DIMS..(i + 1) * Self::DIMS]
                    .iter()
                    .map(|&v| v as u8),
            );
        }
        out
    }

    /// `None` when `bytes` are not vectors this version wrote.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let rest = bytes.strip_prefix(Self::MAGIC)?;
        let i64_at = |at: usize| Some(i64::from_le_bytes(rest.get(at..at + 8)?.try_into().ok()?));
        let (step, window) = (Time(i64_at(0)?), Time(i64_at(8)?));
        let count = u32::from_le_bytes(rest.get(16..20)?.try_into().ok()?) as usize;
        let body = rest.get(20..)?;
        if body.len() != count * (4 + Self::DIMS) {
            return None;
        }
        let mut out = Self::new(step, window);
        for chunk in body.chunks(4 + Self::DIMS) {
            out.scales
                .push(f32::from_le_bytes(chunk[..4].try_into().ok()?));
            out.values.extend(chunk[4..].iter().map(|&b| b as i8));
        }
        Some(out)
    }
}

/// The steps analysis runs: all of them, each when its model is on disk
/// (a sound-only pass leaves speech and shots out). Voice activity and
/// levels always run; a step left out stays empty in the results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalysisSteps {
    pub transcript: bool,
    /// Needs `transcript`.
    pub speakers: bool,
    /// Sounds and music.
    pub sounds: bool,
    /// Needs `sounds`, which finds the music.
    pub beats: bool,
    /// Needs `sounds`, which hears the audio for it.
    pub sound_search: bool,
    pub shots: bool,
}

impl Default for AnalysisSteps {
    fn default() -> Self {
        Self {
            transcript: true,
            speakers: true,
            sounds: true,
            beats: true,
            sound_search: true,
            shots: true,
        }
    }
}

impl AnalysisSteps {
    /// Speakers without a transcript, or beats without the music, don't run.
    pub fn speakers(&self) -> bool {
        self.transcript && self.speakers
    }

    pub fn beats(&self) -> bool {
        self.sounds && self.beats
    }

    /// Sound vectors are made in the sound pass.
    pub fn sound_search(&self) -> bool {
        self.sounds && self.sound_search
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    pub transcript: Option<Transcript>,
    /// Speech regions from voice activity detection: Silero's, which leaves
    /// music out, or in a build without it an energy VAD's, which takes
    /// music for speech. When a provider transcribed, where its words are.
    pub speech: Vec<TimeRange>,
    pub shot_changes: Vec<Time>,
    /// Loudness of the analyzed audio stream, for placing cuts in pauses.
    #[serde(default)]
    pub levels: Option<Levels>,
    #[serde(default)]
    pub sounds: Vec<Sound>,
    #[serde(default)]
    pub music: Vec<Music>,
    /// The [`ANALYSIS_VERSION`] that made it.
    #[serde(default)]
    pub version: u32,
}

/// Audio level in 10 ms frames from the start of the source, in whole dB
/// below full scale. Stored as hex, one byte per frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Levels {
    /// −dBFS per frame, 0..=100.
    below_full_scale: Vec<u8>,
}

impl Levels {
    pub const FRAME: Time = Time(FLICKS_PER_SECOND / 100);

    pub fn from_db(db: impl IntoIterator<Item = f32>) -> Self {
        Self {
            below_full_scale: db
                .into_iter()
                .map(|d| (-d).round().clamp(0.0, 100.0) as u8)
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.below_full_scale.len()
    }

    pub fn is_empty(&self) -> bool {
        self.below_full_scale.is_empty()
    }

    /// dBFS of frame `i`.
    pub fn db(&self, i: usize) -> f32 {
        -(self.below_full_scale[i] as f32)
    }

    /// dBFS of the frame playing at source time `t`, if the audio reaches it.
    pub fn at(&self, t: Time) -> Option<f32> {
        let i = usize::try_from(t.0.div_euclid(Self::FRAME.0)).ok()?;
        (i < self.len()).then(|| self.db(i))
    }
}

impl Serialize for Levels {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use std::fmt::Write;
        let mut hex = String::with_capacity(self.len() * 2);
        for b in &self.below_full_scale {
            let _ = write!(hex, "{b:02x}");
        }
        s.serialize_str(&hex)
    }
}

impl<'de> Deserialize<'de> for Levels {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let hex = String::deserialize(d)?;
        let bytes = hex
            .as_bytes()
            .chunks(2)
            .map(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|p| u8::from_str_radix(p, 16).ok())
                    .ok_or_else(|| serde::de::Error::custom("levels are not hex"))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            below_full_scale: bytes,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Loudness {
    pub integrated_lufs: f32,
    pub true_peak_db: f32,
    pub range_lu: f32,
}
