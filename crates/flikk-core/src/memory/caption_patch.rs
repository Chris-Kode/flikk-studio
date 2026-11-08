//! Changing a caption style a few parameters at a time, and the ranges its
//! numbers keep to, which the style panel and the agent share.

use super::{CaptionHighlight, CaptionPosition, CaptionStyle, Color, MotionKind};
use serde::{Deserialize, Serialize};
use std::ops::RangeInclusive;

/// The ranges a caption style's numbers keep to. Sizes are pixels at
/// 1080p, offsets percent of the frame, durations seconds.
pub mod limits {
    use std::ops::RangeInclusive;

    pub const SIZE: RangeInclusive<f32> = 12.0..=160.0;
    pub const SPACING: RangeInclusive<f32> = -5.0..=30.0;
    pub const OUTLINE: RangeInclusive<f32> = 0.0..=12.0;
    pub const SHADOW: RangeInclusive<f32> = 0.0..=20.0;
    /// Degrees.
    pub const SHADOW_ANGLE: RangeInclusive<f32> = 0.0..=360.0;
    pub const PADDING: RangeInclusive<f32> = 0.0..=40.0;
    pub const OFFSET: RangeInclusive<f32> = -50.0..=50.0;
    /// Characters.
    pub const LINE_LENGTH: RangeInclusive<f32> = 12.0..=60.0;
    pub const MOTION: RangeInclusive<f32> = 0.05..=2.0;
    pub const MAX_LINES: RangeInclusive<u32> = 1..=3;
}

fn clamp(v: &mut f32, range: RangeInclusive<f32>) {
    *v = v.clamp(*range.start(), *range.end());
}

impl CaptionStyle {
    /// Keep every number inside [`limits`].
    pub fn clamp(&mut self) {
        clamp(&mut self.size, limits::SIZE);
        clamp(&mut self.spacing, limits::SPACING);
        clamp(&mut self.outline, limits::OUTLINE);
        clamp(&mut self.shadow, limits::SHADOW);
        clamp(&mut self.shadow_angle, limits::SHADOW_ANGLE);
        clamp(&mut self.background.padding, limits::PADDING);
        clamp(&mut self.offset_x, limits::OFFSET);
        clamp(&mut self.offset_y, limits::OFFSET);
        clamp(&mut self.intro.duration, limits::MOTION);
        clamp(&mut self.outro.duration, limits::MOTION);
        let line = limits::LINE_LENGTH;
        self.max_chars_per_line = self
            .max_chars_per_line
            .clamp(*line.start() as u32, *line.end() as u32);
        let lines = limits::MAX_LINES;
        self.max_lines = self.max_lines.clamp(*lines.start(), *lines.end());
    }

    /// The ready-made look named `name`, whatever its case.
    pub fn look(name: &str) -> Option<CaptionStyle> {
        Self::looks()
            .into_iter()
            .find(|l| l.name.eq_ignore_ascii_case(name.trim()))
    }
}

/// Some of a caption style's parameters, to change and leave the rest:
/// what the agent sends. Unknown keys are refused, so a misspelt one is an
/// error rather than ignored. Its name is not part of it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CaptionStylePatch {
    pub font: Option<String>,
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub uppercase: Option<bool>,
    pub color: Option<Color>,
    pub spacing: Option<f32>,
    pub outline: Option<f32>,
    pub outline_color: Option<Color>,
    pub shadow: Option<f32>,
    pub shadow_color: Option<Color>,
    pub shadow_angle: Option<f32>,
    pub background: Option<BackgroundPatch>,
    pub position: Option<CaptionPosition>,
    pub offset_x: Option<f32>,
    pub offset_y: Option<f32>,
    pub highlight: Option<CaptionHighlight>,
    pub highlight_color: Option<Color>,
    pub intro: Option<MotionPatch>,
    pub outro: Option<MotionPatch>,
    /// Rounded to whole characters.
    pub max_chars_per_line: Option<f32>,
    /// Rounded to whole lines.
    pub max_lines: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BackgroundPatch {
    pub enabled: Option<bool>,
    pub color: Option<Color>,
    pub padding: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MotionPatch {
    pub kind: Option<MotionKind>,
    pub duration: Option<f32>,
}

fn put<T: Clone>(to: &mut T, from: &Option<T>) {
    if let Some(v) = from {
        *to = v.clone();
    }
}

impl MotionPatch {
    fn apply(&self, motion: &mut super::CaptionMotion) {
        put(&mut motion.kind, &self.kind);
        put(&mut motion.duration, &self.duration);
    }
}

impl CaptionStylePatch {
    /// Whether it changes nothing.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Change `style` by the parameters given, keeping it inside
    /// [`limits`].
    pub fn apply(&self, style: &mut CaptionStyle) {
        put(&mut style.font, &self.font);
        put(&mut style.size, &self.size);
        put(&mut style.bold, &self.bold);
        put(&mut style.italic, &self.italic);
        put(&mut style.uppercase, &self.uppercase);
        put(&mut style.color, &self.color);
        put(&mut style.spacing, &self.spacing);
        put(&mut style.outline, &self.outline);
        put(&mut style.outline_color, &self.outline_color);
        put(&mut style.shadow, &self.shadow);
        put(&mut style.shadow_color, &self.shadow_color);
        put(&mut style.shadow_angle, &self.shadow_angle);
        if let Some(b) = &self.background {
            put(&mut style.background.enabled, &b.enabled);
            put(&mut style.background.color, &b.color);
            put(&mut style.background.padding, &b.padding);
        }
        put(&mut style.position, &self.position);
        put(&mut style.offset_x, &self.offset_x);
        put(&mut style.offset_y, &self.offset_y);
        put(&mut style.highlight, &self.highlight);
        put(&mut style.highlight_color, &self.highlight_color);
        if let Some(m) = &self.intro {
            m.apply(&mut style.intro);
        }
        if let Some(m) = &self.outro {
            m.apply(&mut style.outro);
        }
        if let Some(n) = self.max_chars_per_line {
            style.max_chars_per_line = n.round().max(0.0) as u32;
        }
        if let Some(n) = self.max_lines {
            style.max_lines = n.round().max(0.0) as u32;
        }
        style.clamp();
    }
}
