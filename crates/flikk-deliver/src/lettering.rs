//! Captions and titles as pictures, for the program the viewer and the
//! agent see ([`Services::lettering`]). They are lettered as the render
//! burns them in ([`crate::captions::to_ass`]): a box behind each line, the
//! spoken word's highlight, shadow, stroke and fill, moved by the intro and
//! outro. Each is drawn with cosmic-text into a picture tight around it,
//! which the compositor places with the clip's transform.
//!
//! [`Services::lettering`]: flikk_core::services::Services::lettering

use crate::captions::{self, Pose, WordLook, pin, word_look};
use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent,
    Weight, Wrap,
};
use flikk_core::Time;
use flikk_core::memory::{CaptionHighlight, CaptionPosition, CaptionStyle, Color};
use flikk_core::model::{Clip, ClipContent, Track, TrackKind};
use flikk_core::services::{Frame, Lettered, LetteringService};
use std::collections::VecDeque;
use std::ops::Range;
use std::sync::{Arc, LazyLock, Mutex};

/// Pictures kept for reuse: a caption shows the same picture for most
/// frames.
const CACHED: usize = 24;
/// Finest drawing, as a multiple of the resting size.
const MAX_ZOOM: f32 = 3.0;

/// System fonts, loaded once, and rasterized glyphs.
struct Fonts {
    system: FontSystem,
    glyphs: SwashCache,
}

static FONTS: LazyLock<Mutex<Fonts>> = LazyLock::new(|| {
    Mutex::new(Fonts {
        system: FontSystem::new(),
        glyphs: SwashCache::new(),
    })
});

/// Where the block of lines goes in the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    /// Where the style puts captions.
    Caption,
    /// Centred, ignoring the style's placement.
    Center,
    /// Left-aligned, 6% in and 12% up.
    LowerThird,
}

/// Text to letter, in one style, at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Letters {
    /// Wrapped and cased.
    pub lines: Vec<String>,
    pub style: CaptionStyle,
    pub place: Place,
    pub pose: Pose,
    /// The word being spoken, counted across lines.
    pub spoken: Option<usize>,
}

impl Letters {
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
            lines: text.lines().map(str::to_owned).collect(),
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

    /// What `clip` on `track` letters `t` into it, in `styles` (by track),
    /// or `None` if it is not a title on a title track or a caption with
    /// text on a caption track.
    pub fn of(track: &Track, clip: &Clip, t: Time, styles: &[CaptionStyle]) -> Option<Self> {
        match (&clip.content, track.kind) {
            (ClipContent::Title { text, template }, TrackKind::Title) => {
                Some(Self::title(text, template.to_lowercase().contains("lower")))
            }
            (ClipContent::Caption { text, words, .. }, TrackKind::Caption)
                if !text.trim().is_empty() =>
            {
                let style = captions::style_for(track, styles);
                let duration = clip.duration.as_secs_f64();
                let at = t.as_secs_f64();
                let starts = captions::word_starts(text, words, duration);
                Some(Self {
                    lines: captions::wrap(text, &style),
                    pose: captions::pose_at(&style, duration, at),
                    spoken: captions::spoken(&starts, at),
                    style: style.into_owned(),
                    place: Place::Caption,
                })
            }
            _ => None,
        }
    }
}

/// [`LetteringService`] in a fixed set of caption styles.
pub struct Letterer {
    styles: Vec<CaptionStyle>,
    cache: Mutex<VecDeque<(Key, Lettered)>>,
}

/// What a picture was drawn from.
#[derive(PartialEq)]
struct Key {
    letters: Letters,
    canvas: (u32, u32),
    zoom: u32,
}

impl Letterer {
    pub fn new(styles: Vec<CaptionStyle>) -> Self {
        Self {
            styles,
            cache: Mutex::new(VecDeque::new()),
        }
    }
}

impl LetteringService for Letterer {
    fn letter(
        &self,
        track: &Track,
        clip: &Clip,
        t: Time,
        canvas: (u32, u32),
        zoom: f32,
    ) -> Option<Lettered> {
        let letters = Letters::of(track, clip, t, &self.styles)?;
        let key = Key {
            letters,
            canvas,
            zoom: (zoom.clamp(0.05, MAX_ZOOM) * 100.0).round() as u32,
        };
        if let Ok(cache) = self.cache.lock()
            && let Some((_, hit)) = cache.iter().find(|(k, _)| *k == key)
        {
            return Some(hit.clone());
        }
        let drawn = draw(&key.letters, canvas, key.zoom as f32 / 100.0)?;
        if let Ok(mut cache) = self.cache.lock() {
            if cache.len() >= CACHED {
                cache.pop_front();
            }
            cache.push_back((key, drawn.clone()));
        }
        Some(drawn)
    }
}

/// One shaped glyph of a line.
struct Glyph {
    /// From the line's left edge, letter spacing included.
    x: f32,
    /// Where the next glyph starts.
    end: f32,
    /// Byte offset in the line.
    byte: usize,
    layout: cosmic_text::LayoutGlyph,
}

/// One shaped line and where its words are.
struct Line {
    glyphs: Vec<Glyph>,
    /// Byte range of each word in the line, with its index in the caption.
    words: Vec<(Range<usize>, usize)>,
    /// Width with letter spacing.
    width: f32,
    ascent: f32,
    descent: f32,
}

impl Line {
    fn word_at(&self, byte: usize) -> Option<usize> {
        self.words
            .iter()
            .find(|(r, _)| r.contains(&byte))
            .map(|(_, j)| *j)
    }
}

fn shape(fonts: &mut Fonts, l: &Letters, font_size: f32, spacing: f32) -> Vec<Line> {
    let s = &l.style;
    let attrs = Attrs::new()
        .family(Family::Name(&s.font))
        .weight(if s.bold { Weight::BOLD } else { Weight::NORMAL })
        .style(if s.italic {
            Style::Italic
        } else {
            Style::Normal
        });
    let mut first = 0;
    l.lines
        .iter()
        .map(|text| {
            let mut buffer =
                Buffer::new(&mut fonts.system, Metrics::new(font_size, font_size * 1.25));
            buffer.set_wrap(Wrap::None);
            buffer.set_size(None, None);
            buffer.set_text(text, &attrs, Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut fonts.system, false);
            let mut glyphs: Vec<Glyph> = Vec::new();
            for run in buffer.layout_runs() {
                for g in run.glyphs {
                    let n = glyphs.len() as f32;
                    glyphs.push(Glyph {
                        x: g.x + spacing * n,
                        end: g.x + g.w + spacing * (n + 1.0),
                        byte: g.start,
                        layout: g.clone(),
                    });
                }
            }
            let width = glyphs.last().map_or(0.0, |g| g.end);
            let (mut ascent, mut descent) = (0.0f32, 0.0f32);
            for g in &glyphs {
                if let Some(font) = fonts
                    .system
                    .get_font(g.layout.font_id, g.layout.font_weight)
                {
                    let m = font.metrics();
                    let k = g.layout.font_size / m.units_per_em.max(1) as f32;
                    ascent = ascent.max(m.ascent * k);
                    descent = descent.max(-m.descent * k);
                }
            }
            if ascent + descent <= 0.0 {
                (ascent, descent) = (font_size * 0.8, font_size * 0.2);
            }
            let mut words = Vec::new();
            let mut at = 0;
            for word in text.split_whitespace() {
                let start = at + text[at..].find(word).unwrap_or(0);
                at = start + word.len();
                words.push((start..at, first + words.len()));
            }
            first += words.len();
            Line {
                glyphs,
                words,
                width,
                ascent,
                descent,
            }
        })
        .collect()
}

/// Straight sRGB 0..1 with `opacity` folded into alpha.
fn rgba(c: Color, opacity: f32) -> [f32; 4] {
    [
        c.r as f32 / 255.0,
        c.g as f32 / 255.0,
        c.b as f32 / 255.0,
        c.a as f32 / 255.0 * opacity,
    ]
}

/// Something to draw, in drawing pixels from the canvas origin.
enum Mark {
    Rect {
        rect: [f32; 4],
        color: [f32; 4],
    },
    /// A rasterized glyph at whole pixel `(x, y)`: a mask in `color`, or a
    /// color glyph in its own colors faded by `color`'s alpha.
    Glyph {
        key: CacheKey,
        x: i32,
        y: i32,
        color: [f32; 4],
    },
}

/// What lettering draws, in drawing pixels from the canvas origin.
struct Plan {
    /// Each mark, and whether it is drawn now: hidden words and the box
    /// past them still count toward the picture's size, so it does not
    /// change while words appear.
    marks: Vec<(Mark, bool)>,
    /// Whole pixels around every mark: `[x0, y0, x1, y1]`.
    bounds: [f32; 4],
    /// Drawing pixels per canvas pixel.
    zoom: f32,
}

impl Plan {
    /// Where the picture rests on the canvas: centre and size, in canvas
    /// pixels.
    fn rest(&self) -> ([f32; 2], [f32; 2]) {
        let [x0, y0, x1, y1] = self.bounds;
        let z = self.zoom;
        (
            [(x0 + x1) / 2.0 / z, (y0 + y1) / 2.0 / z],
            [(x1 - x0) / z, (y1 - y0) / z],
        )
    }
}

/// `l` drawn for a `canvas`-sized frame at `zoom` × its resting size.
pub fn draw(l: &Letters, canvas: (u32, u32), zoom: f32) -> Option<Lettered> {
    let mut guard = FONTS.lock().ok()?;
    let fonts = &mut *guard;
    let plan = plan(fonts, l, canvas, zoom)?;
    let [ox, oy, x1, y1] = plan.bounds;
    let (pw, ph) = ((x1 - ox) as u32, (y1 - oy) as u32);
    let mut picture = Canvas::new(pw, ph);
    for (mark, shown) in &plan.marks {
        if !shown {
            continue;
        }
        match mark {
            Mark::Rect { rect, color } => picture.rect(
                [rect[0] - ox, rect[1] - oy, rect[2] - ox, rect[3] - oy],
                *color,
            ),
            Mark::Glyph { key, x, y, color } => {
                if let Some(img) = fonts.glyphs.get_image(&mut fonts.system, *key) {
                    let p = img.placement;
                    let at = (x + p.left - ox as i32, y - p.top - oy as i32);
                    picture.glyph(img, at, *color);
                }
            }
        }
    }
    let (center, size) = plan.rest();
    Some(Lettered {
        frame: picture.finish(),
        center,
        size,
    })
}

/// Where `l` rests on a `canvas`-sized frame: the centre and size of its
/// picture, in canvas pixels.
pub fn rest(l: &Letters, canvas: (u32, u32)) -> Option<([f32; 2], [f32; 2])> {
    let mut guard = FONTS.lock().ok()?;
    Some(plan(&mut guard, l, canvas, 1.0)?.rest())
}

fn plan(fonts: &mut Fonts, l: &Letters, canvas: (u32, u32), zoom: f32) -> Option<Plan> {
    let s = &l.style;
    let pose = l.pose;
    if pose.opacity <= 0.0 || l.lines.is_empty() {
        return None;
    }
    let z = zoom.clamp(0.05, MAX_ZOOM);
    let (w, h) = (canvas.0 as f32, canvas.1 as f32);
    // Drawing pixels per 1080p pixel, by the short side so a vertical
    // frame letters as a horizontal one of the same width.
    let k = w.min(h) / 1080.0 * pose.scale * z;
    let lines = shape(fonts, l, (s.size * k).max(1.0), s.spacing * k);
    let (ascent, line_h) = lines
        .first()
        .map_or((0.0, 0.0), |l| (l.ascent, l.ascent + l.descent));
    let block = line_h * lines.len() as f32;
    let (x, top) = match l.place {
        Place::Caption => {
            let p = pin(s, w, h);
            let y = (p.y - h * pose.rise) * z;
            let top = match p.edge {
                CaptionPosition::Bottom => y - block,
                CaptionPosition::Middle => y - block / 2.0,
                CaptionPosition::Top => y,
            };
            (p.x * z, top)
        }
        Place::Center => (w * z / 2.0, h * z / 2.0 - block / 2.0),
        Place::LowerThird => (w * z * 0.06, h * z * 0.88 - block),
    };
    let left = |line: &Line| match l.place {
        Place::LowerThird => x,
        _ => x - line.width / 2.0,
    };
    let look = |j: usize| word_look(s.highlight, l.spoken, j);
    let alpha = pose.opacity;

    let mut marks: Vec<(Mark, bool)> = Vec::new();
    // The horizontal extent of line `i`'s glyphs whose word passes `keep`.
    let extent = |i: usize, keep: &dyn Fn(usize) -> bool| -> Option<(f32, f32)> {
        let line = &lines[i];
        let mut span: Option<(f32, f32)> = None;
        for g in &line.glyphs {
            if line.word_at(g.byte).is_some_and(keep) {
                span = Some(span.map_or((g.x, g.end), |(a, _)| (a, g.end)));
            }
        }
        span
    };
    let pad = s.background.padding.max(0.0) * k;
    let boxed = |i: usize, (x0, x1): (f32, f32)| {
        let line_top = top + line_h * i as f32;
        let origin = left(&lines[i]);
        [
            origin + x0 - pad,
            line_top - pad,
            origin + x1 + pad,
            line_top + line_h + pad,
        ]
    };
    if s.background.enabled {
        let color = rgba(s.background.color, alpha);
        for i in 0..lines.len() {
            let Some(all) = extent(i, &|_| true) else {
                continue;
            };
            // Hidden words come last; the box stops at the last shown one.
            match extent(i, &|j| look(j) != WordLook::Hidden) {
                Some((_, x1)) => {
                    marks.push((
                        Mark::Rect {
                            rect: boxed(i, all),
                            color,
                        },
                        false,
                    ));
                    marks.push((
                        Mark::Rect {
                            rect: boxed(i, (all.0, x1)),
                            color,
                        },
                        true,
                    ));
                }
                None => marks.push((
                    Mark::Rect {
                        rect: boxed(i, all),
                        color,
                    },
                    false,
                )),
            }
        }
    }
    if s.highlight == CaptionHighlight::Box {
        let color = rgba(s.highlight_color, alpha);
        for i in 0..lines.len() {
            // The box can reach any word, so its reach counts.
            if let Some(all) = extent(i, &|_| true) {
                marks.push((
                    Mark::Rect {
                        rect: boxed(i, all),
                        color,
                    },
                    false,
                ));
            }
            if let Some(span) = extent(i, &|j| look(j) == WordLook::Boxed) {
                marks.push((
                    Mark::Rect {
                        rect: boxed(i, span),
                        color,
                    },
                    true,
                ));
            }
        }
    }

    let outline = s.outline * k;
    let ring: Vec<(f32, f32)> = if outline > 0.0 {
        let steps = if outline > 2.0 { 16 } else { 8 };
        let mut ring: Vec<(f32, f32)> = (0..steps)
            .map(|i| {
                let a = i as f32 / steps as f32 * std::f32::consts::TAU;
                (a.cos() * outline, a.sin() * outline)
            })
            .collect();
        // Thick strokes: fill in between the copies.
        if outline > 3.0 {
            ring.extend(ring.clone().into_iter().map(|(x, y)| (x * 0.5, y * 0.5)));
        }
        ring
    } else {
        Vec::new()
    };
    let shadow = s.shadow * k;
    let rad = s.shadow_angle.to_radians();
    let cast = (rad.cos() * shadow, rad.sin() * shadow);
    let (fill, lit, stroke, shade) = (
        rgba(s.color, alpha),
        rgba(s.highlight_color, alpha),
        rgba(s.outline_color, alpha),
        rgba(s.shadow_color, alpha),
    );
    // Shadow, stroke, then fill, each over every line, as the render does.
    for pass in 0..3 {
        if (pass == 0 && shadow <= 0.0) || (pass == 1 && ring.is_empty()) {
            continue;
        }
        for (i, line) in lines.iter().enumerate() {
            let origin = (left(line), top + line_h * i as f32 + ascent);
            for g in &line.glyphs {
                let shown = line.word_at(g.byte).map_or(WordLook::Plain, look) != WordLook::Hidden;
                let at = (origin.0 + g.x - g.layout.x, origin.1);
                let mut mark = |(dx, dy): (f32, f32), color: [f32; 4]| {
                    let p = g.layout.physical((at.0 + dx, at.1 + dy), 1.0);
                    let key = p.cache_key;
                    let mark = Mark::Glyph {
                        key,
                        x: p.x,
                        y: p.y,
                        color,
                    };
                    marks.push((mark, shown));
                };
                let emoji = matches!(
                    fonts.glyphs.get_image(&mut fonts.system, g.layout.physical((0.0, 0.0), 1.0).cache_key),
                    Some(img) if img.content == SwashContent::Color
                );
                match pass {
                    0 if !emoji => {
                        for (dx, dy) in &ring {
                            mark((cast.0 + dx, cast.1 + dy), shade);
                        }
                        mark(cast, shade);
                    }
                    1 if !emoji => {
                        for offset in &ring {
                            mark(*offset, stroke);
                        }
                    }
                    2 if emoji => mark((0.0, 0.0), [1.0, 1.0, 1.0, alpha]),
                    2 => {
                        let lit_word = line.word_at(g.byte).map(look) == Some(WordLook::Lit);
                        mark((0.0, 0.0), if lit_word { lit } else { fill });
                    }
                    _ => {}
                }
            }
        }
    }

    // The picture: everything that could be drawn, in whole pixels.
    let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    let mut grow = |[x0, y0, x1, y1]: [f32; 4]| {
        bounds = [
            bounds[0].min(x0),
            bounds[1].min(y0),
            bounds[2].max(x1),
            bounds[3].max(y1),
        ];
    };
    for (mark, _) in &marks {
        match mark {
            Mark::Rect { rect, .. } => grow(*rect),
            Mark::Glyph { key, x, y, .. } => {
                if let Some(img) = fonts.glyphs.get_image(&mut fonts.system, *key) {
                    let p = img.placement;
                    let (x0, y0) = ((x + p.left) as f32, (y - p.top) as f32);
                    grow([x0, y0, x0 + p.width as f32, y0 + p.height as f32]);
                }
            }
        }
    }
    if bounds[0] >= bounds[2] || bounds[1] >= bounds[3] {
        return None;
    }
    let (x0, y0) = (bounds[0].floor(), bounds[1].floor());
    Some(Plan {
        marks,
        bounds: [
            x0,
            y0,
            bounds[2].ceil().max(x0 + 1.0),
            bounds[3].ceil().max(y0 + 1.0),
        ],
        zoom: z,
    })
}

/// A premultiplied RGBA drawing surface.
struct Canvas {
    width: u32,
    height: u32,
    px: Vec<[f32; 4]>,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            px: vec![[0.0; 4]; width as usize * height as usize],
        }
    }

    /// Straight `color` × `coverage` over pixel (`x`, `y`).
    fn over(&mut self, x: i32, y: i32, color: [f32; 4], coverage: f32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let a = color[3] * coverage;
        if a <= 0.0 {
            return;
        }
        let d = &mut self.px[y as usize * self.width as usize + x as usize];
        for c in 0..3 {
            d[c] = color[c] * a + d[c] * (1.0 - a);
        }
        d[3] = a + d[3] * (1.0 - a);
    }

    /// `rect` (`[x0, y0, x1, y1]`) filled, with partial coverage at its
    /// edges.
    fn rect(&mut self, rect: [f32; 4], color: [f32; 4]) {
        let [x0, y0, x1, y1] = rect;
        for y in y0.floor() as i32..y1.ceil() as i32 {
            let cy = ((y + 1) as f32).min(y1) - (y as f32).max(y0);
            for x in x0.floor() as i32..x1.ceil() as i32 {
                let cx = ((x + 1) as f32).min(x1) - (x as f32).max(x0);
                self.over(x, y, color, (cx * cy).clamp(0.0, 1.0));
            }
        }
    }

    /// A glyph image with its top-left at `at`: a mask in `color`, or a
    /// color glyph faded by `color`'s alpha.
    fn glyph(&mut self, img: &cosmic_text::SwashImage, at: (i32, i32), color: [f32; 4]) {
        let (w, h) = (img.placement.width as i32, img.placement.height as i32);
        match img.content {
            SwashContent::Mask => {
                for y in 0..h {
                    for x in 0..w {
                        let m = img.data[(y * w + x) as usize] as f32 / 255.0;
                        self.over(at.0 + x, at.1 + y, color, m);
                    }
                }
            }
            SwashContent::SubpixelMask => {
                for y in 0..h {
                    for x in 0..w {
                        let i = ((y * w + x) * 4) as usize;
                        let m = img.data[i..i + 3].iter().map(|v| *v as f32).sum::<f32>() / 765.0;
                        self.over(at.0 + x, at.1 + y, color, m);
                    }
                }
            }
            SwashContent::Color => {
                let opacity = color[3];
                for y in 0..h {
                    for x in 0..w {
                        let i = ((y * w + x) * 4) as usize;
                        let p = &img.data[i..i + 4];
                        let c = [
                            p[0] as f32 / 255.0,
                            p[1] as f32 / 255.0,
                            p[2] as f32 / 255.0,
                            p[3] as f32 / 255.0 * opacity,
                        ];
                        self.over(at.0 + x, at.1 + y, c, 1.0);
                    }
                }
            }
        }
    }

    /// Straight RGBA.
    fn finish(self) -> Frame {
        let rgba: Vec<u8> = self
            .px
            .iter()
            .flat_map(|[r, g, b, a]| {
                let un = |c: f32| {
                    if *a > 0.0 {
                        ((c / a).clamp(0.0, 1.0) * 255.0).round() as u8
                    } else {
                        0
                    }
                };
                [
                    un(*r),
                    un(*g),
                    un(*b),
                    (a.clamp(0.0, 1.0) * 255.0).round() as u8,
                ]
            })
            .collect();
        Frame {
            width: self.width,
            height: self.height,
            rgba: Arc::from(rgba),
        }
    }
}
