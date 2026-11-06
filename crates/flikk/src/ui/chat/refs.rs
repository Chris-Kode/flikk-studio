//! `@` references to project objects (PRD AG3, TECHNICAL_DESIGN §8). In text
//! they are tokens such as `@clip 3f2a91c0`, `@effect 3f2a91c0/9c1d2e3f`,
//! `@fx cross_dissolve` or `@range 1.000-4.500`, which is what the agent
//! reads; `flikk_core::query::resolve_project_references` parses them.

use crate::ui::kit::{IconName, theme as t};
use flikk_core::audiofx::AudioFxKind;
use flikk_core::command::Edge;
use flikk_core::fx::{self, FxRef};
use flikk_core::model::{MediaKind, Project, TrackKind};
use flikk_core::query::{self, Reference as Resolved};
use flikk_core::{Time, TimeRange};
use gpui_kit::Rgba;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    Clip,
    Media,
    Marker,
    Track,
    Bin,
    Effect,
    Transition,
    Fx,
    AudioFx,
    Range,
    Time,
}

impl RefKind {
    pub const ALL: [RefKind; 11] = [
        Self::Clip,
        Self::Media,
        Self::Marker,
        Self::Track,
        Self::Bin,
        Self::Effect,
        Self::Transition,
        Self::Fx,
        Self::AudioFx,
        Self::Range,
        Self::Time,
    ];

    /// The token's keyword.
    pub fn word(self) -> &'static str {
        match self {
            Self::Clip => "clip",
            Self::Media => "media",
            Self::Marker => "marker",
            Self::Track => "track",
            Self::Bin => "bin",
            Self::Effect => "effect",
            Self::Transition => "transition",
            Self::Fx => "fx",
            Self::AudioFx => "audiofx",
            Self::Range => "range",
            Self::Time => "time",
        }
    }

    /// Group header in the `@` list.
    pub fn title(self) -> &'static str {
        match self {
            Self::Clip => "Clips",
            Self::Media => "Media",
            Self::Marker => "Markers",
            Self::Track => "Tracks",
            Self::Bin => "Folders",
            Self::Effect => "Clip effects",
            Self::Transition => "Transitions",
            Self::Fx => "FX library",
            Self::AudioFx => "Audio effects",
            Self::Range => "Ranges",
            Self::Time => "Times",
        }
    }

    /// How a token of this kind is marked; a clip takes its track's tint.
    pub fn mark(self, project: &Project, token: &str) -> Mark {
        match self {
            Self::Clip => Mark::Dot(clip_tint(project, token).unwrap_or(t::T3)),
            Self::Effect | Self::Transition | Self::Fx => Mark::Dot(t::T2),
            Self::AudioFx => Mark::Dot(t::A),
            Self::Media => Mark::Icon(IconName::Film, t::T2),
            Self::Marker => Mark::Icon(IconName::MapPin, t::T2),
            Self::Track => Mark::Icon(IconName::Rows3, t::T2),
            Self::Bin => Mark::Icon(IconName::Folder, t::T2),
            Self::Range => Mark::Icon(IconName::Brackets, t::T2),
            Self::Time => Mark::Icon(IconName::TextCursor, t::PH),
        }
    }

    /// The kind a token's text names, from its keyword.
    pub fn of_token(text: &str) -> Option<RefKind> {
        let word = text.strip_prefix('@')?.split(' ').next()?;
        Self::ALL.into_iter().find(|k| k.word() == word)
    }

    fn of(reference: &Resolved) -> Option<RefKind> {
        Some(match reference {
            Resolved::Clip { .. } => Self::Clip,
            Resolved::Media { .. } => Self::Media,
            Resolved::Marker { .. } => Self::Marker,
            Resolved::Track { .. } => Self::Track,
            Resolved::Bin { .. } => Self::Bin,
            Resolved::Effect { .. } => Self::Effect,
            Resolved::Transition { .. } => Self::Transition,
            Resolved::Fx { .. } => Self::Fx,
            Resolved::AudioFx { .. } => Self::AudioFx,
            Resolved::Range { .. } => Self::Range,
            Resolved::At { .. } => Self::Time,
            Resolved::Unresolved { .. } => return None,
        })
    }
}

/// What leads a reference chip or list row: a square in a track tint, or
/// an icon in its colour.
#[derive(Clone, Copy)]
pub enum Mark {
    Dot(Rgba),
    Icon(IconName, Rgba),
}

/// The tint of the track a `@clip` token's clip sits on.
fn clip_tint(project: &Project, token: &str) -> Option<Rgba> {
    let r = query::resolve_project_references(project, token)
        .into_iter()
        .next()?;
    let Resolved::Clip { clip } = r.reference else {
        return None;
    };
    let (track, _) = project.timeline.clip(clip)?;
    Some(match track.kind {
        TrackKind::Video | TrackKind::Overlay => t::V,
        TrackKind::Audio => t::A,
        TrackKind::Caption => t::C,
        TrackKind::Title => t::TI,
    })
}

#[derive(Clone, Debug)]
pub struct Reference {
    pub kind: RefKind,
    /// What follows the keyword: a short id, `clip/effect`, a key, seconds.
    pub id: String,
    pub label: String,
    /// Track and time, media kind and duration, FX category…
    pub detail: String,
}

impl Reference {
    /// What goes into the message text.
    pub fn token_text(&self) -> String {
        format!("@{} {}", self.kind.word(), self.id)
    }
}

/// Every referenceable object, timeline order first within each kind.
pub fn all(project: &Project) -> Vec<Reference> {
    let tl = &project.timeline;
    let mut clips: Vec<_> = tl
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
        .collect();
    clips.sort_by_key(|(_, c)| c.start);
    let mut out: Vec<Reference> = clips
        .iter()
        .map(|(track, clip)| Reference {
            kind: RefKind::Clip,
            id: clip.id.to_string(),
            label: clip.display_name(project),
            detail: format!("{} · {}", track.name, clock(clip.start)),
        })
        .collect();
    out.extend(project.media.iter().map(|m| Reference {
        kind: RefKind::Media,
        id: m.id.to_string(),
        label: file_stem(&m.path),
        detail: media_detail(m),
    }));
    out.extend(tl.markers.iter().map(|m| Reference {
        kind: RefKind::Marker,
        id: m.id.to_string(),
        label: marker_label(&m.note),
        detail: clock(m.range.start),
    }));
    out.extend(tl.tracks.iter().map(|t| Reference {
        kind: RefKind::Track,
        id: t.id.to_string(),
        label: t.name.clone(),
        detail: track_kind(t.kind).into(),
    }));
    out.extend(project.bins.iter().map(|b| Reference {
        kind: RefKind::Bin,
        id: b.id.to_string(),
        label: b.name.clone(),
        detail: String::new(),
    }));
    for (_, clip) in &clips {
        let name = clip.display_name(project);
        out.extend(clip.effects.iter().map(|e| Reference {
            kind: RefKind::Effect,
            id: format!("{}/{}", clip.id, e.id),
            label: fx_name(project, &e.fx),
            detail: name.clone(),
        }));
        out.extend(clip.audio.effects.iter().map(|e| Reference {
            kind: RefKind::Effect,
            id: format!("{}/{}", clip.id, e.id),
            label: e.kind.label().into(),
            detail: name.clone(),
        }));
        for (edge, t) in [("in", &clip.transition_in), ("out", &clip.transition_out)] {
            if let Some(t) = t {
                out.push(Reference {
                    kind: RefKind::Transition,
                    id: format!("{}/{edge}", clip.id),
                    label: fx_name(project, &t.fx.fx),
                    detail: format!("{name} · {edge}"),
                });
            }
        }
    }
    out.extend(fx::library(project).map(|a| Reference {
        kind: RefKind::Fx,
        id: a.id.to_string(),
        label: a.name.clone(),
        detail: if a.category.is_empty() {
            a.kind.label().into()
        } else {
            a.category.clone()
        },
    }));
    out.extend(AudioFxKind::ALL.into_iter().map(|k| Reference {
        kind: RefKind::AudioFx,
        id: k.name().into(),
        label: k.label().into(),
        detail: String::new(),
    }));
    out
}

/// At most this many entries of one kind in the `@` list.
const PER_KIND: usize = 8;

/// References matching what was typed after `@`: every word must appear in
/// the kind, label, detail or id, so `@mark` lists markers and `@v1 5:`
/// clips on V1 around five minutes. Grouped by kind.
pub fn search(project: &Project, query: &str) -> Vec<Reference> {
    let words = query_words(query);
    let mut out: Vec<Reference> = Vec::new();
    for r in all(project) {
        let hay = format!(
            "{} {} {} {} {}",
            r.kind.word(),
            r.kind.title(),
            r.label,
            r.detail,
            r.id
        );
        let taken = out.iter().filter(|o| o.kind == r.kind).count();
        if taken < PER_KIND && matches(&words, &hay) {
            out.push(r);
        }
    }
    out
}

pub fn query_words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Every word of the query appears in `hay`, ignoring case.
pub fn matches(words: &[String], hay: &str) -> bool {
    let hay = hay.to_lowercase();
    words.iter().all(|w| hay.contains(w.as_str()))
}

/// A piece of message text: plain text or a reference token.
pub enum Segment<'a> {
    Text(&'a str),
    Ref(Mention),
}

/// A reference token found in a message, described for its chip.
#[derive(Clone, Debug)]
pub struct Mention {
    /// The kind its keyword names, also when it no longer resolves.
    pub kind: Option<RefKind>,
    pub token: String,
    pub label: String,
    pub detail: String,
    pub resolved: bool,
}

/// Message text split into plain text and `@` reference tokens.
pub fn segments<'a>(text: &'a str, project: &Project) -> Vec<Segment<'a>> {
    let mut out = Vec::new();
    let mut at = 0;
    for r in query::resolve_project_references(project, text) {
        if !text[r.span.start..].starts_with('@') {
            continue;
        }
        if r.span.start > at {
            out.push(Segment::Text(&text[at..r.span.start]));
        }
        out.push(Segment::Ref(describe(
            project,
            &text[r.span.clone()],
            &r.reference,
        )));
        at = r.span.end;
    }
    if at < text.len() {
        out.push(Segment::Text(&text[at..]));
    }
    out
}

/// Label and details of a resolved token.
pub fn describe(project: &Project, token: &str, reference: &Resolved) -> Mention {
    let tl = &project.timeline;
    let found = |label: String, detail: String| Some((label, detail));
    let described = match reference {
        Resolved::Clip { clip } => tl.clip(*clip).and_then(|(track, c)| {
            found(
                c.display_name(project),
                format!("{} · {}", track.name, span(c.range())),
            )
        }),
        Resolved::Media { media } => project
            .media(*media)
            .and_then(|m| found(file_stem(&m.path), media_detail(m))),
        Resolved::Marker { marker } => tl
            .marker(*marker)
            .and_then(|m| found(marker_label(&m.note), span(m.range))),
        Resolved::Track { track } => tl
            .track(*track)
            .and_then(|t| found(t.name.clone(), track_kind(t.kind).into())),
        Resolved::Bin { bin } => project
            .bins
            .iter()
            .find(|b| b.id == *bin)
            .and_then(|b| found(b.name.clone(), String::new())),
        Resolved::Effect { clip, effect } => tl.clip(*clip).and_then(|(_, c)| {
            let label = c
                .effects
                .iter()
                .find(|e| e.id == *effect)
                .map(|e| fx_name(project, &e.fx))
                .or_else(|| {
                    c.audio
                        .effects
                        .iter()
                        .find(|e| e.id == *effect)
                        .map(|e| e.kind.label().to_owned())
                })?;
            found(label, c.display_name(project))
        }),
        Resolved::Transition { clip, edge } => tl.clip(*clip).and_then(|(_, c)| {
            let t = match edge {
                Edge::Start => c.transition_in.as_ref(),
                Edge::End => c.transition_out.as_ref(),
            };
            let side = match edge {
                Edge::Start => "in",
                Edge::End => "out",
            };
            let label = t.map_or_else(|| "Transition".into(), |t| fx_name(project, &t.fx.fx));
            found(label, format!("{} · {side}", c.display_name(project)))
        }),
        Resolved::Fx { fx } => fx::asset(project, fx).and_then(|a| {
            let detail = [a.kind.label(), a.category.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            found(a.name.clone(), detail)
        }),
        Resolved::AudioFx { kind } => found(kind.label().into(), "Audio effect".into()),
        Resolved::Range { range } => found(span(*range), secs_label(range.duration())),
        Resolved::At { time } => found(clock_tenths(*time), String::new()),
        Resolved::Unresolved { .. } => None,
    };
    let kind = RefKind::of(reference).or_else(|| RefKind::of_token(token));
    match described {
        Some((label, detail)) => Mention {
            kind,
            token: token.to_owned(),
            label,
            detail,
            resolved: true,
        },
        None => Mention {
            kind,
            token: token.to_owned(),
            label: token.trim_start_matches('@').to_owned(),
            detail: "Not found".into(),
            resolved: false,
        },
    }
}

/// Message text for markdown display (agent replies): references become
/// `flikk:` links the chat opens in the editor.
pub fn display(text: &str, project: &Project) -> String {
    let mut out = String::with_capacity(text.len());
    for segment in segments(text, project) {
        match segment {
            Segment::Text(t) => out.push_str(t),
            Segment::Ref(m) => {
                let label: String = m
                    .label
                    .chars()
                    .filter(|c| !matches!(c, '[' | ']' | '`' | '*' | '_'))
                    .collect();
                out.push_str(&format!("[@{label}]({})", link(&m.token)));
            }
        }
    }
    out
}

/// The link target for a token: `flikk:<keyword>:<id>`.
fn link(token: &str) -> String {
    format!(
        "flikk:{}",
        token.trim_start_matches('@').replacen(' ', ":", 1)
    )
}

/// The token a [`display`] link points at.
pub fn token_of_link(url: &str) -> Option<String> {
    let (word, id) = url.strip_prefix("flikk:")?.split_once(':')?;
    Some(format!("@{word} {id}"))
}

// ----------------------------------------------------------- token helpers

/// `@time` token and chip label for timeline time `t`.
pub fn time_token(t: Time) -> (String, String) {
    (format!("@time {:.3}", t.as_secs_f64()), clock_tenths(t))
}

/// `@range` token and chip label for a timeline range.
pub fn range_token(r: TimeRange) -> (String, String) {
    (
        format!(
            "@range {:.3}-{:.3}",
            r.start.as_secs_f64(),
            r.end.as_secs_f64()
        ),
        span(r),
    )
}

fn fx_name(project: &Project, r: &FxRef) -> String {
    fx::asset(project, r).map_or_else(|| r.to_string(), |a| a.name.clone())
}

fn file_stem(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Media".into())
}

fn marker_label(note: &str) -> String {
    if note.trim().is_empty() {
        "Marker".into()
    } else {
        note.trim().to_owned()
    }
}

fn media_detail(m: &flikk_core::model::MediaItem) -> String {
    let kind = match m.kind {
        MediaKind::Video => "Video",
        MediaKind::Audio => "Audio",
        MediaKind::Image => "Image",
    };
    match &m.info {
        Some(info) if m.kind != MediaKind::Image => format!("{kind} · {}", clock(info.duration)),
        _ => kind.into(),
    }
}

fn track_kind(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "Video",
        TrackKind::Audio => "Audio",
        TrackKind::Title => "Titles",
        TrackKind::Caption => "Captions",
        TrackKind::Overlay => "Overlays",
    }
}

/// `M:SS`, or `H:MM:SS` past an hour.
pub fn clock(t: Time) -> String {
    let s = t.as_secs_f64().max(0.0) as i64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// [`clock`] with tenths of a second.
pub fn clock_tenths(t: Time) -> String {
    let tenths = (t.as_secs_f64().max(0.0) * 10.0).round() as i64;
    format!(
        "{}.{}",
        clock(Time::from_secs_f64((tenths / 10) as f64)),
        tenths % 10
    )
}

/// `0:01.5–0:04.0`.
pub fn span(r: TimeRange) -> String {
    if r.is_empty() {
        clock_tenths(r.start)
    } else {
        format!("{}–{}", clock_tenths(r.start), clock_tenths(r.end))
    }
}

fn secs_label(t: Time) -> String {
    format!("{:.1} s", t.as_secs_f64())
}
