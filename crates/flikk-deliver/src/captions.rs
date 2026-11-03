//! Caption export (PRD C6): SRT and ASS from caption tracks, plus the ASS
//! document the render graph burns in (captions and titles).

mod motion;

pub use motion::{Pin, Pose, WordLook, pin, pose_at, spoken, word_look, word_starts};

use crate::lettering::{self, Letters};
use anyhow::{Context as _, Result};
use flikk_core::compose;
use flikk_core::memory::{CaptionHighlight, CaptionPosition, CaptionStyle, Color, MotionKind};
use flikk_core::model::{Clip, ClipContent, Project, Property, Track, TrackKind};
use flikk_core::{Time, TimeRange, TrackId};
use std::fmt::Write as _;
use std::path::Path;

/// One caption as shown: wrapped text over a timeline range.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub track: TrackId,
    pub range: TimeRange,
    /// Lines already wrapped and cased per the track's style.
    pub lines: Vec<String>,
}

/// The style a caption track uses: its named preset, else the first one,
/// else the default.
pub fn style_for<'a>(
    track: &Track,
    styles: &'a [CaptionStyle],
) -> std::borrow::Cow<'a, CaptionStyle> {
    track
        .caption_style
        .as_ref()
        .and_then(|name| styles.iter().find(|s| &s.name == name))
        .or_else(|| styles.first())
        .map(std::borrow::Cow::Borrowed)
        .unwrap_or_default()
}

/// Caption cues of one track, or of every unmuted caption track, sorted by
/// start time.
pub fn cues(project: &Project, track: Option<TrackId>, styles: &[CaptionStyle]) -> Vec<Cue> {
    let mut cues: Vec<Cue> = project
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Caption)
        .filter(|t| match track {
            Some(id) => t.id == id,
            None => !t.mix.mute,
        })
        .flat_map(|t| {
            let style = style_for(t, styles).into_owned();
            t.clips.iter().filter_map(move |c| match &c.content {
                ClipContent::Caption { text, .. } if !text.trim().is_empty() => Some(Cue {
                    track: t.id,
                    range: c.range(),
                    lines: wrap(text, &style),
                }),
                _ => None,
            })
        })
        .collect();
    cues.sort_by_key(|c| c.range.start);
    cues
}

/// Greedy word wrap to the style's line length. Never drops words: a cue
/// longer than `max_lines` keeps its extra lines.
pub fn wrap(text: &str, style: &CaptionStyle) -> Vec<String> {
    let text = if style.uppercase {
        text.to_uppercase()
    } else {
        text.to_string()
    };
    let max = style.max_chars_per_line.max(1) as usize;
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > max {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}

fn srt_time(t: Time) -> String {
    let ms = t.as_millis().max(0);
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

fn ass_time(t: Time) -> String {
    let cs = (t.as_millis().max(0) + 5) / 10;
    format!(
        "{}:{:02}:{:02}.{:02}",
        cs / 360_000,
        cs / 6000 % 60,
        cs / 100 % 60,
        cs % 100
    )
}

pub fn to_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (i, cue) in cues.iter().enumerate() {
        let _ = write!(
            out,
            "{}\r\n{} --> {}\r\n{}\r\n\r\n",
            i + 1,
            srt_time(cue.range.start),
            srt_time(cue.range.end),
            cue.lines.join("\r\n")
        );
    }
    out
}

/// ASS colour `&HAABBGGRR`.
const WHITE: &str = "&H00FFFFFF";
const BLACK: &str = "&H00000000";
const SHADOW: &str = "&H80000000";
const CLEAR: &str = "&HFF000000";

/// `&HAABBGGRR`, for style lines; ASS alpha counts up to transparent.
fn ass_color(c: Color) -> String {
    format!("&H{:02X}{:02X}{:02X}{:02X}", 255 - c.a, c.b, c.g, c.r)
}

/// `\Nc&HBBGGRR&\Na&HAA&`, for override tags on component `n`.
fn ass_paint(n: u8, c: Color) -> String {
    format!(
        "\\{n}c&H{:02X}{:02X}{:02X}&\\{n}a&H{:02X}&",
        c.b,
        c.g,
        c.r,
        255 - c.a
    )
}

fn ass_escape(word: &str) -> String {
    word.replace('\\', "\u{29F5}")
        .replace('{', "(")
        .replace('}', ")")
}

fn ass_text(lines: &[String]) -> String {
    lines
        .iter()
        .map(|l| ass_escape(l))
        .collect::<Vec<_>>()
        .join("\\N")
}

/// The words of `lines` in order, each wrapped in the override tags
/// `dress` gives its index: before the separator that precedes it, before
/// the word, and after it.
fn dressed(lines: &[String], mut dress: impl FnMut(usize) -> [String; 3]) -> String {
    let mut out = String::new();
    let mut j = 0;
    for (l, line) in lines.iter().enumerate() {
        for (k, word) in line.split_whitespace().enumerate() {
            let [lead, open, close] = dress(j);
            out.push_str(&lead);
            if k > 0 {
                out.push(' ');
            } else if l > 0 {
                out.push_str("\\N");
            }
            out.push_str(&open);
            out.push_str(&ass_escape(word));
            out.push_str(&close);
            j += 1;
        }
    }
    out
}

fn tags(t: &str) -> String {
    if t.is_empty() {
        String::new()
    } else {
        format!("{{{t}}}")
    }
}

/// How far apart moments of a moving transform are sampled, in seconds;
/// FFmpeg moves lettering in straight lines between them.
const STEP: f64 = 1.0 / 30.0;

/// Where a clip's transform puts lettering at one moment, as ASS override
/// values about the event's alignment point. The transform is written as
/// libass applies it: scale, then a horizontal shear, then rotation (any
/// rotation, skew and stretch can be), so no `\\fay` is needed.
#[derive(Clone, Copy, PartialEq)]
struct Moved {
    pos: [f32; 2],
    /// `\\fscx`, `\\fscy`: percent, the pose's scale included.
    scale: [f32; 2],
    /// `\\frz`: degrees counter-clockwise.
    turn: f32,
    /// `\\fax`.
    shear: f32,
    opacity: f32,
}

impl Moved {
    /// Override tags for a span from `self` to `to` over `ms`.
    fn tags(&self, to: &Moved, ms: i64) -> String {
        let mut tags = String::new();
        let ([x0, y0], [x1, y1]) = (self.pos, to.pos);
        if (x0 - x1).abs() < 0.05 && (y0 - y1).abs() < 0.05 {
            let _ = write!(tags, "\\pos({x0:.1},{y0:.1})");
        } else {
            let _ = write!(tags, "\\move({x0:.1},{y0:.1},{x1:.1},{y1:.1},0,{ms})");
        }
        // No \\org: the default origin is the alignment point, which
        // follows \\move.
        let look = |m: &Moved| {
            format!(
                "\\fscx{:.2}\\fscy{:.2}\\frz{:.3}\\fax{:.4}",
                m.scale[0], m.scale[1], m.turn, m.shear
            )
        };
        let (a, b) = (look(self), look(to));
        tags.push_str(&a);
        if a != b {
            let _ = write!(tags, "\\t(0,{ms},{b})");
        }
        if self.opacity < 0.999 || to.opacity < 0.999 {
            let alpha = |o: f32| (255.0 * (1.0 - o.clamp(0.0, 1.0))).round() as u8;
            let (a0, a1) = (alpha(self.opacity), alpha(to.opacity));
            let _ = write!(tags, "\\fade({a0},{a1},{a1},0,{ms},{ms},{ms})");
        }
        tags
    }
}

/// Whether `clip`'s transform moves its lettering at all, and from when to
/// when it changes (clip seconds, between its first and last keyframe).
fn transform_use(clip: &Clip) -> (bool, Option<(f64, f64)>) {
    let keyed = clip
        .keyframes
        .iter()
        .filter(|k| Property::GEOMETRY.contains(&k.property) || k.property == Property::Opacity)
        .map(|k| k.at.as_secs_f64());
    let changes = keyed.fold(None, |span: Option<(f64, f64)>, t| {
        Some(span.map_or((t, t), |(a, b)| (a.min(t), b.max(t))))
    });
    let tf = &clip.transform;
    let still = tf.position != [0.0, 0.0]
        || (tf.scale - 1.0).abs() > 1e-6
        || tf.rotation_deg.abs() > 1e-6
        || (tf.opacity - 1.0).abs() > 1e-6
        || compose::is_free_form(tf);
    (changes.is_some() || still, changes)
}

/// Where `clip`'s transform at clip time `t` puts `letters`, whose
/// alignment point rests at `at` on a `width` × `height` frame.
fn moved(
    letters: &Letters,
    clip: &Clip,
    t: f64,
    at: [f32; 2],
    drop: f32,
    (width, height): (f32, f32),
) -> Option<Moved> {
    let canvas = (width as u32, height as u32);
    // Measured fully shown: a fade's ends still have a place.
    let shown = Letters {
        pose: Pose {
            opacity: 1.0,
            ..letters.pose
        },
        ..letters.clone()
    };
    let (center, size) = lettering::rest(&shown, canvas)?;
    let tf = compose::lettering_transform(clip.transform_at(Time::from_secs_f64(t)));
    let place = compose::placement_from(&tf, center, size, canvas);
    let uv = [
        (at[0] - center[0]) / size[0].max(1e-3) + 0.5,
        (at[1] - center[1]) / size[1].max(1e-3) + 0.5,
    ];
    // The whole map from the style's unscaled lettering, split as
    // rotation · (scale, then shear): its QR decomposition.
    let [a, b, c, d] = place.linear();
    let k = letters.pose.scale * tf.scale.max(0.0);
    let (kx, ky) = (k * tf.stretch[0].max(0.0), k * tf.stretch[1].max(0.0));
    let [m00, m01, m10, m11] = [a * kx, b * ky, c * kx, d * ky];
    let turn = m10.atan2(m00);
    let (sin, cos) = turn.sin_cos();
    let sx = m00.hypot(m10);
    let skewed = cos * m01 + sin * m11;
    let sy = cos * m11 - sin * m01;
    // libass shears each line about its top, `drop` above the alignment
    // point (on average); take that back out.
    let shift = [cos * skewed * drop, sin * skewed * drop];
    let [x, y] = place.to_canvas(uv);
    Some(Moved {
        pos: [x - shift[0], y - shift[1]],
        scale: [sx * 100.0, sy * 100.0],
        turn: -turn.to_degrees(),
        shear: if sx > 1e-6 { skewed / sx } else { 0.0 },
        opacity: letters.pose.opacity * tf.opacity.clamp(0.0, 1.0),
    })
}

/// How far below its lines' tops an event's alignment point sits, on
/// average, for `lines` lines of an `edge`-aligned block whose lines are
/// `line` pixels apart (libass spaces them by the font size).
fn line_drop(edge: CaptionPosition, lines: usize, line: f32) -> f32 {
    let n = lines.max(1) as f32;
    match edge {
        CaptionPosition::Bottom => line * (n + 1.0) / 2.0,
        CaptionPosition::Middle => line / 2.0,
        CaptionPosition::Top => -line * (n - 1.0) / 2.0,
    }
}

/// `cuts` with a cut every [`STEP`] seconds where they overlap `within`.
fn fine_cuts(cuts: Vec<f64>, (w0, w1): (f64, f64)) -> Vec<f64> {
    let mut out = Vec::with_capacity(cuts.len());
    for span in cuts.windows(2) {
        let (a, b) = (span[0], span[1]);
        out.push(a);
        let (lo, hi) = (a.max(w0), b.min(w1));
        if lo >= hi {
            continue;
        }
        out.push(lo);
        let n = ((hi - lo) / STEP).ceil().max(1.0) as usize;
        out.extend((1..n).map(|i| lo + (hi - lo) * i as f64 / n as f64));
        out.push(hi);
    }
    out.extend(cuts.last());
    out.dedup_by(|x, y| (*x - *y).abs() < 1e-6);
    out
}

/// Dialogue lines for one caption over `window`: one event per span where
/// only its pose changes (and its transform, sampled every [`STEP`] when it
/// moves), on up to three layers (the background, the spoken word's box,
/// the text) so the boxes sit behind every glyph.
#[allow(clippy::too_many_arguments)]
fn write_caption(
    out: &mut String,
    n: usize,
    style: &CaptionStyle,
    styles: &[CaptionStyle],
    track: &Track,
    caption: &Clip,
    window: TimeRange,
    (width, height): (f32, f32),
) {
    let ClipContent::Caption {
        text, words: times, ..
    } = &caption.content
    else {
        return;
    };
    let clip = caption.range();
    let Some(shown) = clip.intersect(&window) else {
        return;
    };
    let lines = wrap(text, style);
    if lines.is_empty() {
        return;
    }
    let k = width.min(height) / 1080.0;
    let duration = clip.duration().as_secs_f64();
    let starts = word_starts(text, times, duration);
    let from = (shown.start - clip.start).as_secs_f64();
    let to = (shown.end - clip.start).as_secs_f64();
    let mut cuts = vec![from];
    cuts.extend(
        motion::spans(style, &starts, duration)
            .into_iter()
            .filter(|t| *t > from + 0.005 && *t < to - 0.005),
    );
    cuts.push(to);
    let (transformed, changes) = transform_use(caption);
    let moving = style.intro.kind != MotionKind::None || style.outro.kind != MotionKind::None;
    if transformed && moving {
        cuts = fine_cuts(cuts, (0.0, duration));
    } else if let Some(span) = changes {
        cuts = fine_cuts(cuts, span);
    }
    let pin = pin(style, width, height);
    let align = match pin.edge {
        CaptionPosition::Bottom => 2,
        CaptionPosition::Middle => 5,
        CaptionPosition::Top => 8,
    };
    let rad = style.shadow_angle.to_radians();
    let shadow = format!(
        "\\xshad{:.1}\\yshad{:.1}",
        style.shadow * k * rad.cos(),
        style.shadow * k * rad.sin()
    );
    let at = |t: f64| ass_time(clip.start - window.start + Time::from_secs_f64(t));
    for span in cuts.windows(2) {
        let (t0, t1) = (span[0], span[1]);
        let said = spoken(&starts, (t0 + t1) / 2.0);
        let look = |j: usize| word_look(style.highlight, said, j);
        let (p0, p1) = (pose_at(style, duration, t0), pose_at(style, duration, t1));
        let ms = ((t1 - t0) * 1000.0).round() as i64;
        let mut motion = format!("\\an{align}");
        let (y0, y1) = (pin.y - p0.rise * height, pin.y - p1.rise * height);
        let size = (width, height);
        let letters = |t: f64| Letters::of(track, caption, Time::from_secs_f64(t), styles);
        let moves = transformed
            .then(|| {
                let drop = line_drop(pin.edge, lines.len(), style.size * k);
                let m0 = moved(&letters(t0)?, caption, t0, [pin.x, y0], drop, size)?;
                let m1 = moved(&letters(t1)?, caption, t1, [pin.x, y1], drop, size)?;
                Some((m0, m1))
            })
            .flatten();
        if let Some((m0, m1)) = moves {
            motion.push_str(&m0.tags(&m1, ms));
        } else if (y0 - y1).abs() < 0.05 {
            let _ = write!(motion, "\\pos({:.1},{y0:.1})", pin.x);
        } else {
            let x = pin.x;
            let _ = write!(motion, "\\move({x:.1},{y0:.1},{x:.1},{y1:.1},0,{ms})");
        }
        if moves.is_none() && (p0.scale != 1.0 || p1.scale != 1.0) {
            let (s0, s1) = (p0.scale * 100.0, p1.scale * 100.0);
            let _ = write!(
                motion,
                "\\fscx{s0:.1}\\fscy{s0:.1}\\t(0,{ms},\\fscx{s1:.1}\\fscy{s1:.1})"
            );
        }
        if moves.is_none() && (p0.opacity != 1.0 || p1.opacity != 1.0) {
            let alpha = |p: Pose| (255.0 * (1.0 - p.opacity)).round() as u8;
            let (a0, a1) = (alpha(p0), alpha(p1));
            let _ = write!(motion, "\\fade({a0},{a1},{a1},0,{ms},{ms},{ms})");
        }
        let mut event = |layer: u32, style_name: &str, extra: &str, body: String| {
            let _ = writeln!(
                out,
                "Dialogue: {layer},{},{},{style_name},,0,0,0,,{{{motion}{extra}}}{body}",
                at(t0),
                at(t1)
            );
        };
        // Hidden words come last: hide from the separator before the first.
        let hide = |j: usize, hidden: &mut bool| {
            if look(j) == WordLook::Hidden && !*hidden {
                *hidden = true;
                return "{\\alpha&HFF&}".to_owned();
            }
            String::new()
        };
        if style.background.enabled {
            let mut hidden = false;
            let body = dressed(&lines, |j| {
                [hide(j, &mut hidden), String::new(), String::new()]
            });
            event(0, &format!("B{n}"), "", body);
        }
        if style.highlight == CaptionHighlight::Box && said.is_some() {
            let lit = tags(&ass_paint(3, style.highlight_color));
            let body = dressed(&lines, |j| match look(j) {
                WordLook::Boxed => [String::new(), lit.clone(), "{\\3a&HFF&}".to_owned()],
                _ => Default::default(),
            });
            event(1, &format!("B{n}"), "\\3a&HFF&", body);
        }
        let (lit, plain) = (
            tags(&ass_paint(1, style.highlight_color)),
            tags(&ass_paint(1, style.color)),
        );
        let mut hidden = false;
        let body = dressed(&lines, |j| match look(j) {
            WordLook::Lit => [String::new(), lit.clone(), plain.clone()],
            _ => [hide(j, &mut hidden), String::new(), String::new()],
        });
        event(2, &format!("C{n}"), &shadow, body);
    }
}

/// An ASS document for `project` at `width`×`height`: one style per
/// caption track, plus title styles. Times are relative to `range.start`
/// and cues outside `range` are left out.
pub fn to_ass(
    project: &Project,
    styles: &[CaptionStyle],
    width: u32,
    height: u32,
    range: TimeRange,
    titles: bool,
) -> String {
    let k = width.min(height) as f32 / 1080.0;
    let mut out = String::new();
    let _ = write!(
        out,
        "[Script Info]\nScriptType: v4.00+\nPlayResX: {width}\nPlayResY: {height}\n\
         WrapStyle: 2\nScaledBorderAndShadow: yes\n\n[V4+ Styles]\n\
         Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, \
         BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, \
         BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n"
    );
    let caption_tracks: Vec<&Track> = project
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Caption && !t.mix.mute)
        .collect();
    let margin = (height as f32 * motion::MARGIN).round() as u32;
    for (i, track) in caption_tracks.iter().enumerate() {
        let style = style_for(track, styles);
        let flag = |on: bool| if on { -1 } else { 0 };
        // Text, then the boxes: per-glyph opaque boxes (BorderStyle 3) in
        // the outline colour, the text itself left clear.
        let face = format!("{},{:.1}", style.font.replace(',', " "), style.size * k);
        let weight = format!(
            "{},{},0,0,100,100,{:.1},0",
            flag(style.bold),
            flag(style.italic),
            style.spacing * k
        );
        let _ = writeln!(
            out,
            "Style: C{i},{face},{fill},{fill},{stroke},{shade},{weight},1,{:.1},{:.1},2,{margin},{margin},{margin},1",
            style.outline * k,
            style.shadow * k,
            fill = ass_color(style.color),
            stroke = ass_color(style.outline_color),
            shade = ass_color(style.shadow_color),
        );
        let _ = writeln!(
            out,
            "Style: B{i},{face},{CLEAR},{CLEAR},{},{CLEAR},{weight},3,{:.1},0,2,{margin},{margin},{margin},1",
            ass_color(style.background.color),
            style.background.padding.max(0.0) * k,
        );
    }
    let _ = writeln!(
        out,
        "Style: Title,Inter,{:.1},{WHITE},{WHITE},{BLACK},{SHADOW},-1,0,0,0,100,100,0,0,1,{:.1},0,5,{margin},{margin},{margin},1",
        96.0 * k,
        3.0 * k
    );
    let _ = writeln!(
        out,
        "Style: LowerThird,Inter,{:.1},{WHITE},{WHITE},{BLACK},{SHADOW},-1,0,0,0,100,100,0,0,1,{:.1},0,1,{},{margin},{},1",
        54.0 * k,
        2.0 * k,
        (width as f32 * 0.06).round(),
        (height as f32 * 0.12).round()
    );
    out.push_str(
        "\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );

    for (i, track) in caption_tracks.iter().enumerate() {
        let style = style_for(track, styles);
        for clip in track.clips.iter().filter(|c| c.enabled) {
            write_caption(
                &mut out,
                i,
                &style,
                styles,
                track,
                clip,
                range,
                (width as f32, height as f32),
            );
        }
    }
    let mut event = |layer: u32, r: TimeRange, style: &str, text: &str| {
        if let Some(r) = r.intersect(&range) {
            let _ = writeln!(
                out,
                "Dialogue: {layer},{},{},{style},,0,0,0,,{text}",
                ass_time(r.start - range.start),
                ass_time(r.end - range.start)
            );
        }
    };
    if titles {
        for track in project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Title && !t.mix.mute)
        {
            for clip in track.clips.iter().filter(|c| c.enabled) {
                if let ClipContent::Title { template, text } = &clip.content {
                    let lower = template.to_lowercase().contains("lower");
                    let style = if lower { "LowerThird" } else { "Title" };
                    let lines: Vec<String> = text.lines().map(str::to_string).collect();
                    let body = ass_text(&lines);
                    let (transformed, changes) = transform_use(clip);
                    if !transformed {
                        event(3, clip.range(), style, &body);
                        continue;
                    }
                    // The style's alignment point: centre, or bottom-left.
                    let (w, h) = (width as f32, height as f32);
                    let at = if lower {
                        [(w * 0.06).round(), h - (h * 0.12).round()]
                    } else {
                        [w / 2.0, h / 2.0]
                    };
                    let letters = Letters::title(text, lower);
                    // Bottom-left or centred, as the title styles align.
                    let (edge, size) = if lower {
                        (CaptionPosition::Bottom, 54.0 * k)
                    } else {
                        (CaptionPosition::Middle, 96.0 * k)
                    };
                    let drop = line_drop(edge, lines.len(), size);
                    let span = |t: f64| moved(&letters, clip, t, at, drop, (w, h));
                    let length = clip.duration.as_secs_f64();
                    let mut cuts = vec![0.0, length];
                    if let Some(span) = changes {
                        cuts = fine_cuts(cuts, span);
                    }
                    for pair in cuts.windows(2) {
                        let (t0, t1) = (pair[0], pair[1]);
                        let (Some(m0), Some(m1)) = (span(t0), span(t1)) else {
                            continue;
                        };
                        let ms = ((t1 - t0) * 1000.0).round() as i64;
                        let r = TimeRange::new(
                            clip.start + Time::from_secs_f64(t0),
                            clip.start + Time::from_secs_f64(t1),
                        );
                        event(3, r, style, &format!("{{{}}}{body}", m0.tags(&m1, ms)));
                    }
                }
            }
        }
    }
    out
}

/// Write the captions of `track` (or all caption tracks) as SubRip.
pub fn export_srt(
    project: &Project,
    track: Option<TrackId>,
    styles: &[CaptionStyle],
    path: &Path,
) -> Result<()> {
    let text = to_srt(&cues(project, track, styles));
    std::fs::write(path, text).with_context(|| format!("could not write {}", path.display()))
}

/// Write every caption track as one styled ASS file at sequence size.
pub fn export_ass(project: &Project, styles: &[CaptionStyle], path: &Path) -> Result<()> {
    let s = &project.timeline.settings;
    let all = TimeRange::new(Time::ZERO, project.timeline.duration());
    let text = to_ass(project, styles, s.width, s.height, all, false);
    std::fs::write(path, text).with_context(|| format!("could not write {}", path.display()))
}
