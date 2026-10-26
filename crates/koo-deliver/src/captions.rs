//! Caption export (PRD C6): SRT and ASS from caption tracks, plus the ASS
//! document the render graph burns in (captions and titles).

use anyhow::{Context as _, Result};
use koo_core::memory::{CaptionPosition, CaptionStyle};
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

fn ass_text(lines: &[String]) -> String {
    lines
        .iter()
        .map(|l| {
            l.replace('\\', "\u{29F5}")
                .replace('{', "(")
                .replace('}', ")")
        })
        .collect::<Vec<_>>()
        .join("\\N")
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
    let margin = (height as f32 * 0.07).round() as u32;
    for (i, track) in caption_tracks.iter().enumerate() {
        let style = style_for(track, styles);
        let align = match style.position {
            CaptionPosition::Bottom => 2,
            CaptionPosition::Middle => 5,
            CaptionPosition::Top => 8,
        };
        let _ = writeln!(
            out,
            "Style: C{i},{},{:.1},{WHITE},{WHITE},{BLACK},{SHADOW},-1,0,0,0,100,100,0,0,1,{:.1},{:.1},{align},{margin},{margin},{margin},1",
            style.font.replace(',', " "),
            style.size * k,
            style.outline * k,
            style.shadow * k,
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
    for (i, track) in caption_tracks.iter().enumerate() {
        let style = style_for(track, styles);
        for clip in &track.clips {
            if let ClipContent::Caption { text, .. } = &clip.content {
                event(
                    1,
                    clip.range(),
                    &format!("C{i}"),
                    &ass_text(&wrap(text, &style)),
                );
            }
        }
    }
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
                    event(2, clip.range(), style, &ass_text(&lines));
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
