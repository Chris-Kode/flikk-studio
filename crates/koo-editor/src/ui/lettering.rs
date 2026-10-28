//! Captions and titles drawn over a picture as the render burns them in
//! (`koo_deliver::captions::to_ass`): a box behind each line, the spoken
//! word's highlight, shadow, stroke and fill, moved by the intro and outro.
//! The viewer and the caption style previews both paint with this.

use gpui_kit::*;
use koo_core::memory::{CaptionPosition, CaptionStyle, Color};
use koo_deliver::captions::{Pose, WordLook, pin, word_look};
use std::ops::Range;

/// Text to letter, in one style, at one moment.
pub struct Lettering {
    /// Wrapped and cased.
    pub lines: Vec<SharedString>,
    pub style: CaptionStyle,
    pub place: Place,
    pub pose: Pose,
    /// The word being spoken, counted across lines.
    pub spoken: Option<usize>,
}

/// Where the block of lines goes in the frame.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// Where the style puts captions.
    Caption,
    /// Centred, ignoring the style's placement.
    Center,
    /// Left-aligned, 6% in and 12% up.
    LowerThird,
}

impl Lettering {
    /// A title as the render draws it: bold white Inter with a black stroke.
    pub fn title(text: &str, lower: bool) -> Self {
        let style = CaptionStyle {
            font: "Inter".into(),
            size: if lower { 54. } else { 96. },
            outline: if lower { 2. } else { 3. },
            position: CaptionPosition::Middle,
            ..CaptionStyle::default()
        };
        Self {
            lines: text.lines().map(|l| l.to_string().into()).collect(),
            style,
            place: if lower {
                Place::LowerThird
            } else {
                Place::Center
            },
            pose: Pose::REST,
            spoken: None,
        }
    }
}

/// One shaped line and where its words are.
struct Line {
    shaped: ShapedLine,
    /// Byte range of each word in the line, with its index in the caption.
    words: Vec<(Range<usize>, usize)>,
    /// Width with letter spacing.
    width: Pixels,
    glyphs: usize,
}

impl Line {
    fn word_at(&self, byte: usize) -> Option<usize> {
        self.words
            .iter()
            .find(|(r, _)| r.contains(&byte))
            .map(|(_, j)| *j)
    }
}

pub fn hsla(c: Color, opacity: f32) -> Hsla {
    Rgba {
        r: c.r as f32 / 255.,
        g: c.g as f32 / 255.,
        b: c.b as f32 / 255.,
        a: c.a as f32 / 255. * opacity,
    }
    .into()
}

fn font(style: &CaptionStyle, window: &Window) -> Font {
    let mut font = window.text_style().font();
    font.family = style.font.clone().into();
    font.weight = if style.bold {
        FontWeight::BOLD
    } else {
        FontWeight::NORMAL
    };
    font.style = if style.italic {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    };
    font
}

fn shape(l: &Lettering, font_size: Pixels, spacing: Pixels, window: &mut Window) -> Vec<Line> {
    let font = font(&l.style, window);
    let mut first = 0;
    l.lines
        .iter()
        .map(|text| {
            let run = TextRun {
                len: text.len(),
                font: font.clone(),
                color: white(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = window
                .text_system()
                .shape_line(text.clone(), font_size, &[run], None);
            let mut words = Vec::new();
            let mut at = 0;
            for word in text.split_whitespace() {
                let start = at + text[at..].find(word).unwrap_or(0);
                at = start + word.len();
                words.push((start..at, first + words.len()));
            }
            first += words.len();
            let glyphs = shaped.runs.iter().map(|r| r.glyphs.len()).sum::<usize>();
            Line {
                width: shaped.width + spacing * glyphs as f32,
                shaped,
                words,
                glyphs,
            }
        })
        .collect()
}

/// The block's size at `k` pixels per 1080p pixel, before motion.
pub fn measure(l: &Lettering, k: f32, window: &mut Window) -> Size<Pixels> {
    let s = &l.style;
    let lines = shape(l, px((s.size * k).max(1.)), px(s.spacing * k), window);
    let line_h = lines
        .first()
        .map_or(px(0.), |line| line.shaped.ascent + line.shaped.descent);
    let width = lines.iter().map(|l| l.width).fold(px(0.), Pixels::max);
    let pad = if s.background.enabled {
        s.background.padding * k
    } else {
        0.
    };
    let edge = px(2. * (s.outline.max(pad) + s.shadow) * k);
    size(width + edge, line_h * lines.len() as f32 + edge)
}

/// Paint `l` into `frame` at `k` pixels per 1080p pixel.
pub fn paint(l: &Lettering, frame: Bounds<Pixels>, k: f32, window: &mut Window, _: &mut App) {
    let s = &l.style;
    let pose = l.pose;
    if pose.opacity <= 0. || l.lines.is_empty() {
        return;
    }
    let k = k * pose.scale;
    let font_size = px((s.size * k).max(1.));
    let spacing = px(s.spacing * k);
    let lines = shape(l, font_size, spacing, window);
    let (ascent, descent) = lines
        .first()
        .map_or((px(0.), px(0.)), |l| (l.shaped.ascent, l.shaped.descent));
    let line_h = ascent + descent;
    let block = line_h * lines.len() as f32;
    let (w, h) = (frame.size.width, frame.size.height);
    let (x, top) = match l.place {
        Place::Caption => {
            let p = pin(s, f32::from(w), f32::from(h));
            let y = frame.top() + px(p.y) - h * pose.rise;
            let top = match p.edge {
                CaptionPosition::Bottom => y - block,
                CaptionPosition::Middle => y - block / 2.,
                CaptionPosition::Top => y,
            };
            (frame.left() + px(p.x), top)
        }
        Place::Center => (frame.center().x, frame.center().y - block / 2.),
        Place::LowerThird => (frame.left() + w * 0.06, frame.bottom() - h * 0.12 - block),
    };
    let left = |line: &Line| match l.place {
        Place::LowerThird => x,
        _ => x - line.width / 2.,
    };
    let look = |j: usize| word_look(s.highlight, l.spoken, j);
    let alpha = pose.opacity;

    // Glyph x positions (with spacing) for each line, and the right edge of
    // each glyph: where the next one starts.
    let placed: Vec<Vec<(Pixels, Pixels, usize)>> = lines
        .iter()
        .map(|line| {
            let mut xs: Vec<(Pixels, usize)> = Vec::with_capacity(line.glyphs);
            for run in &line.shaped.runs {
                for g in &run.glyphs {
                    xs.push((g.position.x + spacing * xs.len() as f32, g.index));
                }
            }
            (0..xs.len())
                .map(|i| {
                    let end = xs.get(i + 1).map_or(line.width, |n| n.0);
                    (xs[i].0, end, xs[i].1)
                })
                .collect()
        })
        .collect();
    // The horizontal extent of the glyphs whose word passes `keep`.
    let extent = |i: usize, keep: &dyn Fn(usize) -> bool| -> Option<(Pixels, Pixels)> {
        let line = &lines[i];
        let mut span: Option<(Pixels, Pixels)> = None;
        for &(x0, x1, byte) in &placed[i] {
            if line.word_at(byte).is_some_and(keep) {
                span = Some(span.map_or((x0, x1), |(a, _)| (a, x1)));
            }
        }
        span
    };
    let pad = px(s.background.padding.max(0.) * k);
    let boxed = |i: usize, (x0, x1): (Pixels, Pixels)| {
        let line_top = top + line_h * i as f32;
        let origin = left(&lines[i]);
        Bounds::from_corners(
            point(origin + x0 - pad, line_top - pad),
            point(origin + x1 + pad, line_top + line_h + pad),
        )
    };
    if s.background.enabled {
        let color = hsla(s.background.color, alpha);
        for i in 0..lines.len() {
            // Hidden words come last; the box stops at the last shown one.
            if let Some((x0, _)) = extent(i, &|_| true)
                && let Some((_, x1)) = extent(i, &|j| look(j) != WordLook::Hidden)
            {
                window.paint_quad(fill(boxed(i, (x0, x1)), color));
            }
        }
    }
    if l.spoken.is_some() {
        let color = hsla(s.highlight_color, alpha);
        for i in 0..lines.len() {
            if let Some(span) = extent(i, &|j| look(j) == WordLook::Boxed) {
                window.paint_quad(fill(boxed(i, span), color));
            }
        }
    }

    let outline = s.outline * k;
    let ring: Vec<Point<Pixels>> = if outline > 0. {
        let steps = if outline > 2. { 16 } else { 8 };
        let mut ring: Vec<Point<Pixels>> = (0..steps)
            .map(|i| {
                let a = i as f32 / steps as f32 * std::f32::consts::TAU;
                point(px(a.cos() * outline), px(a.sin() * outline))
            })
            .collect();
        // Thick strokes: fill in between the copies.
        if outline > 3. {
            ring.extend(ring.clone().into_iter().map(|p| p * 0.5));
        }
        ring
    } else {
        Vec::new()
    };
    let shadow = s.shadow * k;
    let rad = s.shadow_angle.to_radians();
    let cast = point(px(rad.cos() * shadow), px(rad.sin() * shadow));
    let (fill_color, lit, stroke, shade) = (
        hsla(s.color, alpha),
        hsla(s.highlight_color, alpha),
        hsla(s.outline_color, alpha),
        hsla(s.shadow_color, alpha),
    );
    // Shadow, stroke, then fill, each over every line, as the render does.
    for pass in 0..3 {
        if (pass == 0 && shadow <= 0.) || (pass == 1 && ring.is_empty()) {
            continue;
        }
        for (i, line) in lines.iter().enumerate() {
            let baseline = point(left(line), top + line_h * i as f32 + ascent);
            let mut n = 0;
            for run in &line.shaped.runs {
                for g in &run.glyphs {
                    let at = baseline + point(g.position.x + spacing * n as f32, px(0.));
                    n += 1;
                    let word = line.word_at(g.index);
                    let look = word.map_or(WordLook::Plain, look);
                    if look == WordLook::Hidden {
                        continue;
                    }
                    let size = line.shaped.font_size;
                    if g.is_emoji {
                        if pass == 2 {
                            let _ = window.paint_emoji(at, run.font_id, g.id, size);
                        }
                        continue;
                    }
                    let mut glyph = |at: Point<Pixels>, color: Hsla| {
                        let _ = window.paint_glyph(at, run.font_id, g.id, size, color);
                    };
                    match pass {
                        0 => {
                            for offset in &ring {
                                glyph(at + cast + *offset, shade);
                            }
                            glyph(at + cast, shade);
                        }
                        1 => {
                            for offset in &ring {
                                glyph(at + *offset, stroke);
                            }
                        }
                        _ => glyph(
                            at,
                            if look == WordLook::Lit {
                                lit
                            } else {
                                fill_color
                            },
                        ),
                    }
                }
            }
        }
    }
}
