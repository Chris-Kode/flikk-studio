//! Caption export (PRD C6): SRT and ASS from caption tracks, plus the ASS
//! document the render graph burns in (captions and titles).

mod motion;

pub use motion::{Pin, Pose, WordLook, pin, pose_at, spoken, word_look, word_starts};

use anyhow::{Context as _, Result};
use koo_core::memory::{CaptionHighlight, CaptionPosition, CaptionStyle, Color};
use koo_core::model::{ClipContent, Project, Track, TrackKind};
use koo_core::{Time, TimeRange, TrackId};
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

/// Dialogue lines for one caption over `window`: one event per span where
/// only its pose changes, on up to three layers (the background, the
/// spoken word's box, the text) so the boxes sit behind every glyph.
#[allow(clippy::too_many_arguments)]
fn write_caption(
    out: &mut String,
    n: usize,
    style: &CaptionStyle,
    clip: TimeRange,
    text: &str,
    times: &[Time],
    window: TimeRange,
    (width, height): (f32, f32),
) {
    let Some(shown) = clip.intersect(&window) else {
        return;
    };
    let lines = wrap(text, style);
    if lines.is_empty() {
        return;
    }
    let k = height / 1080.0;
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
        if (y0 - y1).abs() < 0.05 {
            let _ = write!(motion, "\\pos({:.1},{y0:.1})", pin.x);
        } else {
            let x = pin.x;
            let _ = write!(motion, "\\move({x:.1},{y0:.1},{x:.1},{y1:.1},0,{ms})");
        }
        if p0.scale != 1.0 || p1.scale != 1.0 {
            let (s0, s1) = (p0.scale * 100.0, p1.scale * 100.0);
            let _ = write!(
                motion,
                "\\fscx{s0:.1}\\fscy{s0:.1}\\t(0,{ms},\\fscx{s1:.1}\\fscy{s1:.1})"
            );
        }
        if p0.opacity != 1.0 || p1.opacity != 1.0 {
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
    let k = height as f32 / 1080.0;
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
        for clip in &track.clips {
            if let ClipContent::Caption { text, words, .. } = &clip.content {
                write_caption(
                    &mut out,
                    i,
                    &style,
                    clip.range(),
                    text,
                    words,
                    range,
                    (width as f32, height as f32),
                );
            }
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
            for clip in &track.clips {
                if let ClipContent::Title { template, text } = &clip.content {
                    let style = if template.to_lowercase().contains("lower") {
                        "LowerThird"
                    } else {
                        "Title"
                    };
                    let lines: Vec<String> = text.lines().map(str::to_string).collect();
                    event(3, clip.range(), style, &ass_text(&lines));
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
