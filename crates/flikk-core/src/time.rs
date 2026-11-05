//! Exact media time.
//!
//! [`Time`] counts flicks (1/705 600 000 s), which divide every common frame
//! rate (23.976, 24, 25, 29.97, 30, 48, 50, 59.94, 60, 120) and audio rate
//! exactly, so frame math never drifts.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, Neg, Sub};

pub const FLICKS_PER_SECOND: i64 = 705_600_000;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Time(pub i64);

impl Time {
    pub const ZERO: Time = Time(0);

    pub fn from_secs_f64(s: f64) -> Self {
        Time((s * FLICKS_PER_SECOND as f64).round() as i64)
    }

    pub fn from_millis(ms: i64) -> Self {
        Time(ms * (FLICKS_PER_SECOND / 1000))
    }

    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / FLICKS_PER_SECOND as f64
    }

    pub fn as_millis(self) -> i64 {
        self.0 / (FLICKS_PER_SECOND / 1000)
    }

    /// This duration multiplied by `factor`, rounded to the nearest flick.
    /// Timeline ↔ source conversions of retimed clips use it.
    pub fn scale(self, factor: f64) -> Self {
        if factor == 1.0 {
            return self;
        }
        Time((self.0 as f64 * factor).round() as i64)
    }

    /// Frame index containing this time (floor).
    pub fn to_frame(self, rate: Rate) -> i64 {
        self.0.div_euclid(rate.frame_duration().0)
    }

    pub fn from_frame(frame: i64, rate: Rate) -> Self {
        Time(frame * rate.frame_duration().0)
    }

    /// Snap down to the start of the frame containing this time.
    pub fn floor_to_frame(self, rate: Rate) -> Self {
        Self::from_frame(self.to_frame(rate), rate)
    }

    /// `HH:MM:SS:FF` at the given rate (non-drop-frame).
    pub fn timecode(self, rate: Rate) -> String {
        let sign = if self.0 < 0 { "-" } else { "" };
        let t = Time(self.0.abs());
        let fps = rate.rounded_fps() as i64;
        let frames = t.to_frame(rate);
        let (h, m, s, f) = (
            frames / (fps * 3600),
            frames / (fps * 60) % 60,
            frames / fps % 60,
            frames % fps,
        );
        format!("{sign}{h:02}:{m:02}:{s:02}:{f:02}")
    }

    /// Parse `SS`, `M:SS`, `H:MM:SS`, each optionally with `.fraction`,
    /// as typed in chat ("1:01–1:04").
    pub fn parse_clock(text: &str) -> Option<Self> {
        let mut secs = 0.0;
        for part in text.trim().split(':') {
            secs = secs * 60.0 + part.parse::<f64>().ok()?;
        }
        secs.is_finite().then(|| Self::from_secs_f64(secs))
    }
}

impl fmt::Display for Time {
    /// `M:SS.mmm`, the format the agent and UI exchange.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = self.as_millis();
        let sign = if ms < 0 { "-" } else { "" };
        let ms = ms.abs();
        write!(
            f,
            "{sign}{}:{:02}.{:03}",
            ms / 60_000,
            ms / 1000 % 60,
            ms % 1000
        )
    }
}

impl Add for Time {
    type Output = Time;
    fn add(self, rhs: Time) -> Time {
        Time(self.0 + rhs.0)
    }
}

impl Sub for Time {
    type Output = Time;
    fn sub(self, rhs: Time) -> Time {
        Time(self.0 - rhs.0)
    }
}

impl Neg for Time {
    type Output = Time;
    fn neg(self) -> Time {
        Time(-self.0)
    }
}

/// Half-open range `[start, end)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TimeRange {
    pub start: Time,
    pub end: Time,
}

impl TimeRange {
    pub fn new(start: Time, end: Time) -> Self {
        Self { start, end }
    }

    pub fn duration(&self) -> Time {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }

    pub fn contains(&self, t: Time) -> bool {
        self.start <= t && t < self.end
    }

    pub fn overlaps(&self, other: &TimeRange) -> bool {
        self.start < other.end && other.start < self.end
    }

    pub fn intersect(&self, other: &TimeRange) -> Option<TimeRange> {
        let r = TimeRange::new(self.start.max(other.start), self.end.min(other.end));
        (!r.is_empty()).then_some(r)
    }

    pub fn shift(&self, by: Time) -> TimeRange {
        TimeRange::new(self.start + by, self.end + by)
    }
}

impl fmt::Display for TimeRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}–{}", self.start, self.end)
    }
}

/// Frame rate as a rational, e.g. 30000/1001.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rate {
    pub num: u32,
    pub den: u32,
}

impl Rate {
    pub const FPS_23_976: Rate = Rate {
        num: 24_000,
        den: 1001,
    };
    pub const FPS_24: Rate = Rate { num: 24, den: 1 };
    pub const FPS_25: Rate = Rate { num: 25, den: 1 };
    pub const FPS_29_97: Rate = Rate {
        num: 30_000,
        den: 1001,
    };
    pub const FPS_30: Rate = Rate { num: 30, den: 1 };
    pub const FPS_50: Rate = Rate { num: 50, den: 1 };
    pub const FPS_60: Rate = Rate { num: 60, den: 1 };

    /// Rates offered for a sequence, with their names.
    pub const SEQUENCE: [(Rate, &'static str); 7] = [
        (Rate::FPS_23_976, "23.976"),
        (Rate::FPS_24, "24"),
        (Rate::FPS_25, "25"),
        (Rate::FPS_29_97, "29.97"),
        (Rate::FPS_30, "30"),
        (Rate::FPS_50, "50"),
        (Rate::FPS_60, "60"),
    ];

    pub fn fps(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    pub fn rounded_fps(self) -> u32 {
        self.fps().round().max(1.0) as u32
    }

    pub fn frame_duration(self) -> Time {
        Time(FLICKS_PER_SECOND * self.den as i64 / self.num.max(1) as i64)
    }
}

impl Default for Rate {
    fn default() -> Self {
        Rate::FPS_30
    }
}
