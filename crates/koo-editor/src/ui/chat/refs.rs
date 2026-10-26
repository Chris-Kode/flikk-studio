//! `@` references to project objects (PRD AG3). In text they are written as
//! `@<kind> <8-hex id>`, e.g. `@clip 3f2a91c0`, which is what the agent reads.

use koo_core::Time;
use koo_core::model::{ClipContent, Project, TrackKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    Clip,
    Marker,
    Track,
    Bin,
}

impl RefKind {
    const ALL: [RefKind; 4] = [Self::Clip, Self::Marker, Self::Track, Self::Bin];

    pub fn word(self) -> &'static str {
        match self {
            Self::Clip => "clip",
            Self::Marker => "marker",
            Self::Track => "track",
            Self::Bin => "bin",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Reference {
    pub kind: RefKind,
    /// Short id as printed by the id's `Display`.
    pub id: String,
    pub label: String,
    /// Timeline time, or the track kind for tracks.
    pub detail: String,
}

impl Reference {
    /// What goes into the message text.
    pub fn token_text(&self) -> String {
        format!("@{} {}", self.kind.word(), self.id)
    }
}

/// Every referenceable object, timeline order first.
pub fn all(project: &Project) -> Vec<Reference> {
    let tl = &project.timeline;
    let mut clips: Vec<_> = tl
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
        .collect();
    clips.sort_by_key(|(_, c)| c.start);
    let mut out: Vec<Reference> = clips
        .into_iter()
        .map(|(track, clip)| Reference {
            kind: RefKind::Clip,
            id: clip.id.to_string(),
            label: clip_label(project, &clip.content),
            detail: format!("{} · {}", track.name, clock(clip.start)),
        })
        .collect();
    out.extend(tl.markers.iter().map(|m| Reference {
        kind: RefKind::Marker,
        id: m.id.to_string(),
        label: if m.note.is_empty() {
            "Marker".into()
        } else {
            m.note.clone()
        },
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
    out
}

/// References matching what was typed after `@`: every word must appear in
/// the kind, label, detail or id, so `@mark` lists markers and `@v1 5:`
/// clips on V1 around five minutes.
pub fn search(project: &Project, query: &str) -> Vec<Reference> {
    let query = query.to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    all(project)
        .into_iter()
        .filter(|r| {
            let hay = format!("{} {} {} {}", r.kind.word(), r.label, r.detail, r.id).to_lowercase();
            words.iter().all(|w| hay.contains(w))
        })
        .take(30)
        .collect()
}

/// Message text for display: references become inline-code labels.
pub fn display(text: &str, project: &Project) -> String {
    let refs = all(project);
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 1..];
        let found = RefKind::ALL.iter().find_map(|k| {
            let id = tail.strip_prefix(k.word())?.strip_prefix(' ')?.get(..8)?;
            id.chars()
                .all(|c| c.is_ascii_hexdigit())
                .then_some((*k, id))
        });
        match found {
            Some((kind, id)) => {
                let label = refs
                    .iter()
                    .find(|r| r.kind == kind && r.id == id)
                    .map(|r| r.label.clone())
                    .unwrap_or_else(|| format!("{} {id}", kind.word()));
                out.push_str(&format!("`@{label}`"));
                rest = &tail[kind.word().len() + 1 + 8..];
            }
            None => {
                out.push('@');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

fn clip_label(project: &Project, content: &ClipContent) -> String {
    match content {
        ClipContent::Media { media, .. } => project
            .media(*media)
            .and_then(|m| m.path.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Clip".into()),
        ClipContent::Title { text, .. } | ClipContent::Caption { text, .. } => {
            let mut s: String = text.chars().take(28).collect();
            if text.chars().count() > 28 {
                s.push('…');
            }
            s
        }
        ClipContent::Overlay { asset, .. } => asset
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Overlay".into()),
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
