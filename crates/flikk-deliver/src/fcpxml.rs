//! FCPXML 1.10 export (PRD D2): the bottom video track is the primary
//! storyline; other video tracks are connected clips on positive lanes,
//! audio tracks on negative lanes, captions as `caption` elements.

use crate::otio::file_url;
use anyhow::Result;
use flikk_core::model::{Clip, ClipContent, MediaKind, Project, Track, TrackKind};
use flikk_core::time::FLICKS_PER_SECOND;
use flikk_core::{MediaId, Rate, Time};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

/// Rational seconds on the frame grid of `rate`, e.g. `1001/30000s`.
fn rt(t: Time, rate: Rate) -> String {
    let frames = {
        let num = rate.num as i128;
        let den = rate.den as i128 * FLICKS_PER_SECOND as i128;
        ((t.0 as i128 * num * 2 + den) / (den * 2)) as i64
    };
    if frames == 0 {
        return "0s".into();
    }
    let (n, d) = (frames * rate.den as i64, rate.num as i64);
    let g = gcd(n.unsigned_abs(), d as u64) as i64;
    if d / g == 1 {
        format!("{}s", n / g)
    } else {
        format!("{}/{}s", n / g, d / g)
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a.max(1) } else { gcd(b, a % b) }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

struct Writer<'a> {
    project: &'a Project,
    rate: Rate,
    assets: HashMap<MediaId, String>,
    styles: usize,
}

impl Writer<'_> {
    /// One element for `clip` at `offset` in its parent's time.
    fn element(&mut self, clip: &Clip, offset: Time, lane: Option<i32>, audio: bool) -> String {
        let r = self.rate;
        let lane = lane.map(|l| format!(" lane=\"{l}\"")).unwrap_or_default();
        match &clip.content {
            ClipContent::Media {
                media, source_in, ..
            } => {
                let Some(asset) = self.assets.get(media) else {
                    return String::new();
                };
                let name = self
                    .project
                    .media(*media)
                    .and_then(|m| m.path.file_stem())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut inner = String::new();
                if audio {
                    if clip.audio.gain_db.abs() > 1e-3 {
                        let _ = write!(
                            inner,
                            "<adjust-volume amount=\"{:.2}dB\"/>",
                            clip.audio.gain_db
                        );
                    }
                } else {
                    let tr = &clip.transform;
                    let k = self.project.timeline.settings.width as f32
                        / self.project.timeline.settings.height.max(1) as f32;
                    let [sx, sy] = tr.stretch.map(|s| s * tr.scale);
                    if tr.position != [0.0, 0.0]
                        || (sx - 1.0).abs() > 1e-3
                        || (sy - 1.0).abs() > 1e-3
                        || tr.rotation_deg != 0.0
                    {
                        // FCP positions are percent of frame height, y up.
                        let _ = write!(
                            inner,
                            "<adjust-transform position=\"{:.3} {:.3}\" scale=\"{:.4} {:.4}\" rotation=\"{:.3}\"/>",
                            tr.position[0] * 100.0 * k,
                            -tr.position[1] * 100.0,
                            sx,
                            sy,
                            -tr.rotation_deg
                        );
                    }
                    if (tr.opacity - 1.0).abs() > 1e-3 {
                        let _ = write!(inner, "<adjust-blend amount=\"{:.3}\"/>", tr.opacity);
                    }
                }
                let enable = if audio { "audio" } else { "video" };
                format!(
                    "<asset-clip ref=\"{asset}\"{lane} name=\"{}\" offset=\"{}\" start=\"{}\" duration=\"{}\" srcEnable=\"{enable}\" tcFormat=\"NDF\">{inner}</asset-clip>",
                    esc(&name),
                    rt(offset, r),
                    rt(*source_in, r),
                    rt(clip.duration, r)
                )
            }
            ClipContent::Caption { text, .. } => {
                self.styles += 1;
                let id = format!("ts{}", self.styles);
                format!(
                    "<caption{lane} name=\"{}\" offset=\"{}\" start=\"0s\" duration=\"{}\" role=\"SRT?captionFormat=SRT.en\"><text placement=\"bottom\"><text-style ref=\"{id}\">{}</text-style></text><text-style-def id=\"{id}\"><text-style font=\".AppleSystemUIFont\" fontSize=\"13\" fontFace=\"Regular\" fontColor=\"1 1 1 1\" backgroundColor=\"0 0 0 1\"/></text-style-def></caption>",
                    esc(&text.chars().take(32).collect::<String>()),
                    rt(offset, r),
                    rt(clip.duration, r),
                    esc(text)
                )
            }
            _ => String::new(),
        }
    }
}

/// The project as an FCPXML 1.10 document.
pub fn to_fcpxml(project: &Project) -> String {
    let tl = &project.timeline;
    let s = &tl.settings;
    let rate = s.rate;
    let frame = format!("{}/{}s", rate.den, rate.num);
    let mut resources = format!(
        "<format id=\"r1\" frameDuration=\"{frame}\" width=\"{}\" height=\"{}\"/>\n",
        s.width, s.height
    );
    let mut assets = HashMap::new();
    let used: Vec<MediaId> = tl
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().filter_map(|c| c.media()))
        .collect();
    for item in project.media.iter().filter(|m| used.contains(&m.id)) {
        let id = format!("r{}", assets.len() + 2);
        let info = item.info.as_ref();
        let duration = info.map(|i| i.duration).unwrap_or_else(|| {
            tl.tracks
                .iter()
                .flat_map(|t| &t.clips)
                .filter(|c| c.media() == Some(item.id))
                .filter_map(|c| c.to_source(c.end()))
                .max()
                .unwrap_or(Time::ZERO)
        });
        let has_video = item.kind != MediaKind::Audio;
        let has_audio = info
            .map(|i| !i.audio.is_empty())
            .unwrap_or(item.kind != MediaKind::Image);
        let mut attrs = format!(
            "id=\"{id}\" name=\"{}\" start=\"0s\" duration=\"{}\" hasVideo=\"{}\" hasAudio=\"{}\"",
            esc(&item.path.file_stem().unwrap_or_default().to_string_lossy()),
            rt(duration, rate),
            has_video as u8,
            has_audio as u8
        );
        if has_audio {
            let a = info.and_then(|i| i.audio.first());
            let _ = write!(
                attrs,
                " audioSources=\"1\" audioChannels=\"{}\" audioRate=\"{}\"",
                a.map(|a| a.channels).unwrap_or(2),
                a.map(|a| a.sample_rate).unwrap_or(48_000)
            );
        }
        let _ = writeln!(
            resources,
            "<asset {attrs}><media-rep kind=\"original-media\" src=\"{}\"/></asset>",
            esc(&file_url(&item.path))
        );
        assets.insert(item.id, id);
    }

    let mut w = Writer {
        project,
        rate,
        assets,
        styles: 0,
    };
    let video: Vec<&Track> = tl
        .tracks
        .iter()
        .rev()
        .filter(|t| matches!(t.kind, TrackKind::Video | TrackKind::Overlay))
        .collect();
    let primary = video.first().copied();

    // Primary storyline: gaps and clips covering the whole timeline.
    struct Item {
        start: Time,
        end: Time,
        source_in: Time,
        open: String,
        close: &'static str,
        connected: Vec<String>,
    }
    let mut spine: Vec<Item> = Vec::new();
    let mut cursor = Time::ZERO;
    let total = tl.duration();
    let gap = |from: Time, to: Time, spine: &mut Vec<Item>| {
        spine.push(Item {
            start: from,
            end: to,
            source_in: Time::ZERO,
            open: format!(
                "<gap name=\"Gap\" offset=\"{}\" start=\"0s\" duration=\"{}\">",
                rt(from, rate),
                rt(to - from, rate)
            ),
            close: "</gap>",
            connected: Vec::new(),
        });
    };
    if let Some(track) = primary {
        for clip in track.clips.iter().filter(|c| c.media().is_some()) {
            if clip.start > cursor {
                gap(cursor, clip.start, &mut spine);
            }
            let full = w.element(clip, clip.start, None, false);
            let Some(close) = full.rfind("</asset-clip>") else {
                continue;
            };
            spine.push(Item {
                start: clip.start,
                end: clip.end(),
                source_in: clip.to_source(clip.start).unwrap_or(Time::ZERO),
                open: full[..close].to_string(),
                close: "</asset-clip>",
                connected: Vec::new(),
            });
            cursor = clip.end();
        }
    }
    if total > cursor || spine.is_empty() {
        gap(
            cursor,
            total.max(cursor + rate.frame_duration()),
            &mut spine,
        );
    }

    // Connected clips, attached to the spine item under their start.
    let mut connect = |clip: &Clip, lane: i32, audio: bool, w: &mut Writer| {
        let i = spine
            .iter()
            .position(|s| clip.start < s.end)
            .unwrap_or(spine.len() - 1);
        let parent = &mut spine[i];
        let offset = parent.source_in + (clip.start - parent.start);
        let e = w.element(clip, offset, Some(lane), audio);
        if !e.is_empty() {
            parent.connected.push(e);
        }
    };
    for (n, track) in video.iter().enumerate().skip(1) {
        for clip in &track.clips {
            connect(clip, n as i32, false, &mut w);
        }
    }
    let above = video.len().max(1) as i32;
    for (n, track) in tl
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Caption)
        .enumerate()
    {
        for clip in &track.clips {
            connect(clip, above + n as i32, false, &mut w);
        }
    }
    for (n, track) in tl
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .enumerate()
    {
        for clip in &track.clips {
            connect(clip, -(n as i32) - 1, true, &mut w);
        }
    }

    let mut body = String::new();
    for item in &spine {
        let _ = writeln!(
            body,
            "{}{}{}",
            item.open,
            item.connected.join(""),
            item.close
        );
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE fcpxml>\n<fcpxml version=\"1.10\">\n<resources>\n{resources}</resources>\n<library>\n<event name=\"{name}\">\n<project name=\"{name}\">\n<sequence format=\"r1\" duration=\"{}\" tcStart=\"0s\" tcFormat=\"NDF\" audioLayout=\"stereo\" audioRate=\"{}k\">\n<spine>\n{body}</spine>\n</sequence>\n</project>\n</event>\n</library>\n</fcpxml>\n",
        rt(total, rate),
        s.sample_rate / 1000,
        name = esc(&project.name),
    )
}

/// Write the timeline as `.fcpxml`.
pub fn export_fcpxml(project: &Project, path: &Path) -> Result<()> {
    crate::ffmpeg::write_atomic(path, to_fcpxml(project))
}
