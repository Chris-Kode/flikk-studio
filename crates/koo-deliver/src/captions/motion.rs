//! How a caption changes while it is on screen: its intro and outro, and the
//! word being spoken. The burned-in ASS and the viewer both draw from here,
//! so they show the same thing at the same moment.

use koo_core::Time;
use koo_core::memory::{
    CaptionHighlight, CaptionMotion, CaptionPosition, CaptionStyle, MotionKind,
};

/// Distance from the frame edge to a top or bottom caption, as a fraction of
/// the frame height.
pub const MARGIN: f32 = 0.07;
/// How far a sliding caption travels, as a fraction of the frame height.
const SLIDE: f32 = 0.04;
/// The size a popping caption grows from.
const POP_FROM: f32 = 0.6;

/// Where a caption's block of lines sits, in frame pixels: its `edge`
/// (bottom, middle or top) at `y`, centred on `x`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pin {
    pub x: f32,
    pub y: f32,
    pub edge: CaptionPosition,
}

pub fn pin(style: &CaptionStyle, width: f32, height: f32) -> Pin {
    let x = width * (0.5 + style.offset_x / 100.0);
    let up = height * style.offset_y / 100.0;
    let y = match style.position {
        CaptionPosition::Bottom => height * (1.0 - MARGIN),
        CaptionPosition::Middle => height / 2.0,
        CaptionPosition::Top => height * MARGIN,
    };
    Pin {
        x,
        y: y - up,
        edge: style.position,
    }
}

/// When each word of `text` starts, in seconds into a caption `duration`
/// long: the saved word times when there is one per word, else spread by
/// word length. Never decreasing, and every word shows before the end.
pub fn word_starts(text: &str, times: &[Time], duration: f64) -> Vec<f64> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut starts: Vec<f64> = if times.len() == words.len() {
        times.iter().map(|t| t.as_secs_f64()).collect()
    } else {
        let total: usize = words.iter().map(|w| w.chars().count() + 1).sum();
        let mut at = 0;
        words
            .iter()
            .map(|w| {
                let start = duration * at as f64 / total.max(1) as f64;
                at += w.chars().count() + 1;
                start
            })
            .collect()
    };
    let last = duration * 0.95;
    let mut floor = 0.0;
    for s in &mut starts {
        *s = s.clamp(floor, last);
        floor = *s;
    }
    starts
}

/// The word being spoken `t` seconds into the caption.
pub fn spoken(starts: &[f64], t: f64) -> Option<usize> {
    starts.iter().rposition(|&s| s <= t)
}

/// How a word is drawn under the style's highlight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WordLook {
    Plain,
    /// Text in the highlight color.
    Lit,
    /// On a box in the highlight color.
    Boxed,
    Hidden,
}

pub fn word_look(highlight: CaptionHighlight, spoken: Option<usize>, word: usize) -> WordLook {
    let now = spoken == Some(word);
    let said = spoken.is_some_and(|s| word <= s);
    match highlight {
        CaptionHighlight::Color if now => WordLook::Lit,
        CaptionHighlight::Box if now => WordLook::Boxed,
        CaptionHighlight::Karaoke if said => WordLook::Lit,
        CaptionHighlight::Reveal if !said => WordLook::Hidden,
        _ => WordLook::Plain,
    }
}

/// What the intro or outro does to a caption at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub opacity: f32,
    pub scale: f32,
    /// Moved up, as a fraction of the frame height.
    pub rise: f32,
}

impl Pose {
    pub const REST: Pose = Pose {
        opacity: 1.0,
        scale: 1.0,
        rise: 0.0,
    };

    /// `kind` at `p`, from 0 (out of sight) to 1 (at rest).
    fn of(kind: MotionKind, p: f32) -> Pose {
        let p = p.clamp(0.0, 1.0);
        match kind {
            MotionKind::None => Self::REST,
            MotionKind::Fade => Pose {
                opacity: p,
                ..Self::REST
            },
            MotionKind::Pop => Pose {
                opacity: p,
                scale: POP_FROM + (1.0 - POP_FROM) * p,
                rise: 0.0,
            },
            MotionKind::Slide => Pose {
                opacity: p,
                scale: 1.0,
                rise: -SLIDE * (1.0 - p),
            },
        }
    }
}

/// Intro and outro lengths in seconds, shortened alike when a caption is
/// too short for both.
fn motion_lengths(style: &CaptionStyle, duration: f64) -> (f64, f64) {
    let len = |m: &CaptionMotion| match m.kind {
        MotionKind::None => 0.0,
        _ => m.duration.max(0.0) as f64,
    };
    let (a, b) = (len(&style.intro), len(&style.outro));
    let k = if a + b > duration {
        duration / (a + b)
    } else {
        1.0
    };
    (a * k, b * k)
}

/// The pose `t` seconds into a caption `duration` long.
pub fn pose_at(style: &CaptionStyle, duration: f64, t: f64) -> Pose {
    let (a, b) = motion_lengths(style, duration);
    if t < a {
        Pose::of(style.intro.kind, (t / a) as f32)
    } else if t > duration - b {
        Pose::of(style.outro.kind, ((duration - t) / b) as f32)
    } else {
        Pose::REST
    }
}

/// Seconds into the caption, from 0 to `duration`, between which only the
/// pose changes, and linearly: where words start under a highlight, and
/// where the intro ends and the outro begins.
pub fn spans(style: &CaptionStyle, starts: &[f64], duration: f64) -> Vec<f64> {
    let (a, b) = motion_lengths(style, duration);
    let mut cuts = vec![0.0, a, duration - b, duration];
    if style.highlight != CaptionHighlight::None {
        cuts.extend_from_slice(starts);
    }
    cuts.retain(|t| (0.0..=duration).contains(t));
    cuts.sort_by(f64::total_cmp);
    // ASS times are in centiseconds.
    cuts.dedup_by(|b, a| *b - *a < 0.005);
    cuts
}
