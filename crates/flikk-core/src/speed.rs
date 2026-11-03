//! Speed ramps and smooth slow motion.
//!
//! A media clip plays at one [`Clip::speed`] unless its [`Retime`] has speed
//! keys: then the speed changes over the clip, straight from key to key and
//! held before the first and after the last. Keys sit on clip time (from
//! the clip's start, on the timeline), like other keyframes, so trims and
//! splits keep the speed each frame had. Source time is the integral of the
//! speed: [`Clip::source_offset`] and its inverse [`Clip::timeline_offset`]
//! are what [`Clip::to_source`] and every other timeline → source mapping
//! (viewer, compositor, render, captions, waveforms) go through. A ramped
//! clip's `speed` is its mean speed, so `duration × speed` is still the
//! source it plays.
//!
//! Frames between source frames are drawn as [`Interpolation`] says:
//! repeated, blended, or made by an optical flow model
//! ([`crate::services::InterpolationService`]); without the model they are
//! blended. Under a ramp the clip's audio is stretched with the speed
//! (pitch kept) or muted ([`RampAudio`]).

use crate::model::{Clip, ClipContent};
use crate::services::{Between, Frame, InterpolationService};
use crate::time::Time;
use serde::{Deserialize, Serialize};

/// Slowest and fastest speed a clip plays at.
pub const MIN_SPEED: f64 = 0.01;
pub const MAX_SPEED: f64 = 100.0;

/// The speed at a clip time.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpeedKey {
    /// From the clip's start, on the timeline.
    pub at: Time,
    pub speed: f64,
}

/// How a clip shows the moments between two source frames, when it plays
/// slower than its source's frame rate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    /// The nearest earlier frame, repeated.
    #[default]
    Nearest,
    /// The two frames around it, mixed.
    Blend,
    /// A frame made between them along the motion (RIFE); blended until
    /// it is made, and without the model.
    OpticalFlow,
}

impl Interpolation {
    pub const ALL: [Interpolation; 3] = [Self::Nearest, Self::Blend, Self::OpticalFlow];

    pub fn label(self) -> &'static str {
        match self {
            Self::Nearest => "Frame repeat",
            Self::Blend => "Frame blend",
            Self::OpticalFlow => "Optical flow",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Nearest => "nearest",
            Self::Blend => "blend",
            Self::OpticalFlow => "optical_flow",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|i| i.key() == key)
    }
}

/// What a ramped clip's audio does (at one speed it is always stretched).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RampAudio {
    /// Follows the speed, pitch kept.
    #[default]
    Stretch,
    Mute,
}

/// How a clip's speed changes over it, and how it shows frames between
/// source frames.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Retime {
    /// By `at`; fewer than two: the clip plays at [`Clip::speed`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<SpeedKey>,
    #[serde(default, skip_serializing_if = "is_nearest")]
    pub interpolation: Interpolation,
    #[serde(default, skip_serializing_if = "is_stretch")]
    pub audio: RampAudio,
}

fn is_nearest(i: &Interpolation) -> bool {
    *i == Interpolation::Nearest
}

fn is_stretch(a: &RampAudio) -> bool {
    *a == RampAudio::Stretch
}

impl Retime {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether the speed changes over the clip.
    pub fn is_ramp(&self) -> bool {
        self.keys.len() >= 2
    }
}

/// Ready-made ramps: shapes over the clip (0..1) in multiples of its speed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeedPreset {
    /// Slow, speeding up.
    RampIn,
    /// Fast, slowing down.
    RampOut,
    /// Fast, slow in the middle, fast.
    Hero,
    /// Bullet time: a near-frozen middle.
    Bullet,
    /// Fast and slow by turns.
    Montage,
}

impl SpeedPreset {
    pub const ALL: [SpeedPreset; 5] = [
        Self::RampIn,
        Self::RampOut,
        Self::Hero,
        Self::Bullet,
        Self::Montage,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::RampIn => "Ramp in",
            Self::RampOut => "Ramp out",
            Self::Hero => "Hero",
            Self::Bullet => "Bullet",
            Self::Montage => "Montage",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::RampIn => "ramp_in",
            Self::RampOut => "ramp_out",
            Self::Hero => "hero",
            Self::Bullet => "bullet",
            Self::Montage => "montage",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }

    /// The shape: (position 0..1 over the clip, multiple of its speed).
    pub fn shape(self) -> &'static [[f64; 2]] {
        match self {
            Self::RampIn => &[[0.0, 0.25], [0.4, 0.4], [1.0, 2.0]],
            Self::RampOut => &[[0.0, 2.0], [0.6, 0.4], [1.0, 0.25]],
            Self::Hero => &[
                [0.0, 2.5],
                [0.3, 2.0],
                [0.42, 0.25],
                [0.58, 0.25],
                [0.7, 2.0],
                [1.0, 2.5],
            ],
            Self::Bullet => &[
                [0.0, 1.5],
                [0.25, 1.5],
                [0.38, 0.1],
                [0.62, 0.1],
                [0.75, 1.5],
                [1.0, 1.5],
            ],
            Self::Montage => &[
                [0.0, 2.0],
                [0.2, 2.0],
                [0.3, 0.5],
                [0.45, 0.5],
                [0.55, 2.0],
                [0.7, 2.0],
                [0.8, 0.5],
                [1.0, 0.5],
            ],
        }
    }

    /// The shape at `base` speed, as points for [`crate::command::Command::SetSpeedRamp`].
    pub fn points(self, base: f64) -> Vec<[f64; 2]> {
        self.shape()
            .iter()
            .map(|[u, s]| [*u, (s * base).clamp(MIN_SPEED, MAX_SPEED)])
            .collect()
    }
}

fn secs(t: Time) -> f64 {
    t.as_secs_f64()
}

/// Speed at clip time `t` on `keys` (sorted, at least one).
pub fn speed_at(keys: &[SpeedKey], t: Time) -> f64 {
    let (first, last) = (keys[0], keys[keys.len() - 1]);
    if t <= first.at {
        return first.speed;
    }
    if t >= last.at {
        return last.speed;
    }
    let i = keys.partition_point(|k| k.at <= t).max(1) - 1;
    let (a, b) = (keys[i], keys[i + 1]);
    let len = secs(b.at - a.at);
    if len <= 0.0 {
        return b.speed;
    }
    a.speed + (b.speed - a.speed) * secs(t - a.at) / len
}

/// Source seconds played from the first key to clip time `t` (negative
/// before it).
fn played_from_first(keys: &[SpeedKey], t: Time) -> f64 {
    let first = keys[0];
    if t <= first.at {
        return secs(t - first.at) * first.speed;
    }
    let mut sum = 0.0;
    for w in keys.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = secs(b.at - a.at);
        if t >= b.at {
            sum += len * (a.speed + b.speed) / 2.0;
            continue;
        }
        let x = secs(t - a.at);
        let at_t = if len > 0.0 {
            a.speed + (b.speed - a.speed) * x / len
        } else {
            b.speed
        };
        return sum + x * (a.speed + at_t) / 2.0;
    }
    let last = keys[keys.len() - 1];
    sum + secs(t - last.at) * last.speed
}

/// Source seconds played from clip time 0 to `t` on `keys`.
pub fn played(keys: &[SpeedKey], t: Time) -> f64 {
    played_from_first(keys, t) - played_from_first(keys, Time::ZERO)
}

/// The clip time at which `source` seconds from clip time 0 have played
/// (the inverse of [`played`]).
pub fn time_for(keys: &[SpeedKey], source: f64) -> Time {
    let first = keys[0];
    let mut target = source + played_from_first(keys, Time::ZERO);
    if target <= 0.0 {
        return first.at + Time::from_secs_f64(target / first.speed);
    }
    for w in keys.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = secs(b.at - a.at);
        let area = len * (a.speed + b.speed) / 2.0;
        if target > area {
            target -= area;
            continue;
        }
        if len <= 0.0 {
            return a.at;
        }
        // a.speed·x + (b.speed − a.speed)/(2·len)·x² = target
        let k = (b.speed - a.speed) / (2.0 * len);
        let disc = (a.speed * a.speed + 4.0 * k * target).max(0.0);
        let x = 2.0 * target / (a.speed + disc.sqrt()).max(1e-12);
        return a.at + Time::from_secs_f64(x.clamp(0.0, len));
    }
    let last = keys[keys.len() - 1];
    last.at + Time::from_secs_f64(target / last.speed)
}

/// `keys` between clip times `from` and `to`, moved to start at zero, with
/// a key at each end holding the speed there, so the speed of every frame
/// in between stays.
pub fn slice(keys: &[SpeedKey], from: Time, to: Time) -> Vec<SpeedKey> {
    let mut out = vec![SpeedKey {
        at: Time::ZERO,
        speed: speed_at(keys, from),
    }];
    out.extend(
        keys.iter()
            .filter(|k| k.at > from && k.at < to)
            .map(|k| SpeedKey {
                at: k.at - from,
                speed: k.speed,
            }),
    );
    out.push(SpeedKey {
        at: to - from,
        speed: speed_at(keys, to),
    });
    out
}

/// Mean speed over clip times `0..duration`.
pub fn mean(keys: &[SpeedKey], duration: Time) -> f64 {
    let d = secs(duration);
    if d <= 0.0 {
        return speed_at(keys, Time::ZERO);
    }
    played(keys, duration) / d
}

/// Mean of a shape of (position 0..1, speed) points, sorted by position,
/// held flat before the first and after the last.
pub fn shape_mean(points: &[[f64; 2]]) -> f64 {
    let (first, last) = (points[0], points[points.len() - 1]);
    let mut sum = first[0] * first[1] + (1.0 - last[0]) * last[1];
    for w in points.windows(2) {
        sum += (w[1][0] - w[0][0]) * (w[0][1] + w[1][1]) / 2.0;
    }
    sum
}

/// `points` (position 0..1, speed) sorted, clamped and checked: none is
/// outside 0..1 or [`MIN_SPEED`]..[`MAX_SPEED`]; positions repeated keep
/// the last.
pub fn clean_points(points: &[[f64; 2]]) -> Result<Vec<[f64; 2]>, String> {
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(points.len());
    for [u, s] in points {
        if !(u.is_finite() && (-1e-9..=1.0 + 1e-9).contains(u)) {
            return Err("ramp positions are 0..1 over the clip".into());
        }
        if !(s.is_finite() && (MIN_SPEED..=MAX_SPEED).contains(s)) {
            return Err(format!(
                "ramp speeds must be between {MIN_SPEED} and {MAX_SPEED}"
            ));
        }
        out.push([u.clamp(0.0, 1.0), *s]);
    }
    out.sort_by(|a, b| a[0].total_cmp(&b[0]));
    out.dedup_by(|b, a| {
        if (a[0] - b[0]).abs() < 1e-6 {
            a[1] = b[1];
            true
        } else {
            false
        }
    });
    Ok(out)
}

impl Clip {
    /// Whether this media clip's speed changes over it.
    pub fn is_ramped(&self) -> bool {
        self.retime.is_ramp() && matches!(self.content, ClipContent::Media { .. })
    }

    /// Speed at clip time `t`.
    pub fn speed_at(&self, t: Time) -> f64 {
        if self.is_ramped() {
            speed_at(&self.retime.keys, t)
        } else {
            self.speed
        }
    }

    /// Source played from the clip's start to clip time `t` (negative
    /// before it: the head handle).
    pub fn source_offset(&self, t: Time) -> Time {
        if self.is_ramped() {
            Time::from_secs_f64(played(&self.retime.keys, t))
        } else {
            t.scale(self.speed)
        }
    }

    /// The clip time at which `source` has played from its start (the
    /// inverse of [`Clip::source_offset`]).
    pub fn timeline_offset(&self, source: Time) -> Time {
        if self.is_ramped() {
            time_for(&self.retime.keys, secs(source))
        } else {
            source.scale(1.0 / self.speed)
        }
    }

    /// Source the clip plays.
    pub fn source_length(&self) -> Time {
        self.source_offset(self.duration)
    }

    /// Whether the clip shows source frames at another pace than the
    /// source's own: retimed, or ramped.
    pub fn is_retimed(&self) -> bool {
        self.is_ramped() || (self.speed - 1.0).abs() > 1e-9
    }

    /// After the keys or the duration changed: `speed` becomes the mean
    /// speed, keys past the end go (one stays at the end).
    pub fn settle_ramp(&mut self) {
        if !self.retime.is_ramp() {
            self.retime.keys.clear();
            return;
        }
        let end = self.duration;
        if self
            .retime
            .keys
            .iter()
            .any(|k| k.at > end || k.at < Time::ZERO)
        {
            self.retime.keys = slice(&self.retime.keys, Time::ZERO, end);
        }
        let keys = &self.retime.keys;
        if keys
            .windows(2)
            .all(|w| (w[0].speed - w[1].speed).abs() < 1e-9)
        {
            self.speed = keys[0].speed;
            self.retime.keys.clear();
            return;
        }
        self.speed = mean(keys, end).clamp(MIN_SPEED, MAX_SPEED);
    }
}

/// Moments between two source frames an optical flow frame is made at:
/// the moment is rounded to the nearest of these, so frames are reused as
/// the playhead moves.
pub const BETWEEN_STEPS: f32 = 32.0;

/// An [`InterpolationService`] that makes each frame when it is asked for
/// (renders), so nothing is blended in its place.
pub struct Waiting(pub std::sync::Arc<dyn InterpolationService>);

impl InterpolationService for Waiting {
    fn available(&self) -> Result<(), String> {
        self.0.available()
    }

    fn between(&self, request: &Between<'_>, _wait: bool) -> Option<Frame> {
        self.0.between(request, true)
    }

    fn version(&self) -> u64 {
        self.0.version()
    }
}
