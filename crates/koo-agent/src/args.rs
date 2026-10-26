//! Tool arguments at the boundary: times are seconds (`f64`) or `"M:SS.mmm"`
//! strings, ids are the 8-hex short form shown in results (or a full UUID).

use koo_core::model::{Project, Track, TrackKind};
use koo_core::{ClipId, MarkerId, MediaId, SpeakerId, Time, TimeRange, TrackId};
use serde_json::{Map, Value, json};
use std::fmt::Display;
use uuid::Uuid;

/// A tool failure the model can read and act on.
#[derive(Debug, Clone)]
pub struct ToolError(pub String);

impl<E: Display> From<E> for ToolError {
    fn from(e: E) -> Self {
        ToolError(e.to_string())
    }
}

pub type ToolResultOf<T> = std::result::Result<T, ToolError>;

pub fn fail<T>(message: impl Into<String>) -> ToolResultOf<T> {
    Err(ToolError(message.into()))
}

/// Tool arguments with typed accessors.
pub struct Args<'a>(pub &'a Map<String, Value>);

impl<'a> Args<'a> {
    pub fn of(value: &'a Value) -> ToolResultOf<Self> {
        match value.as_object() {
            Some(m) => Ok(Args(m)),
            None => fail("arguments must be an object"),
        }
    }

    fn get(&self, key: &str) -> Option<&'a Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn str(&self, key: &str) -> ToolResultOf<&'a str> {
        self.opt_str(key)?
            .ok_or_else(|| ToolError(format!("`{key}` is required")))
    }

    pub fn opt_str(&self, key: &str) -> ToolResultOf<Option<&'a str>> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.as_str())),
            Some(_) => fail(format!("`{key}` must be a string")),
        }
    }

    pub fn f64(&self, key: &str) -> ToolResultOf<f64> {
        self.opt_f64(key)?
            .ok_or_else(|| ToolError(format!("`{key}` is required")))
    }

    pub fn opt_f64(&self, key: &str) -> ToolResultOf<Option<f64>> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => v
                .as_f64()
                .filter(|f| f.is_finite())
                .map(Some)
                .ok_or_else(|| ToolError(format!("`{key}` must be a number"))),
        }
    }

    pub fn opt_bool(&self, key: &str) -> ToolResultOf<Option<bool>> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => fail(format!("`{key}` must be true or false")),
        }
    }

    pub fn strings(&self, key: &str) -> ToolResultOf<Vec<String>> {
        match self.get(key) {
            None => Ok(Vec::new()),
            Some(Value::String(s)) => Ok(vec![s.clone()]),
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| ToolError(format!("`{key}` must be a list of strings")))
                })
                .collect(),
            Some(_) => fail(format!("`{key}` must be a list of strings")),
        }
    }

    pub fn time(&self, key: &str) -> ToolResultOf<Time> {
        self.opt_time(key)?
            .ok_or_else(|| ToolError(format!("`{key}` is required")))
    }

    pub fn opt_time(&self, key: &str) -> ToolResultOf<Option<Time>> {
        self.get(key)
            .map(|v| parse_time(v).map_err(|e| ToolError(format!("`{key}`: {}", e.0))))
            .transpose()
    }

    /// `{"start": t, "end": t}` under `key`, or `None` when absent.
    pub fn opt_range(&self, key: &str) -> ToolResultOf<Option<TimeRange>> {
        let Some(v) = self.get(key) else {
            return Ok(None);
        };
        let r = Args::of(v).map_err(|_| ToolError(format!("`{key}` must be {{start, end}}")))?;
        let range = TimeRange::new(r.time("start")?, r.time("end")?);
        if range.is_empty() {
            return fail(format!("`{key}` is empty: end must be after start"));
        }
        Ok(Some(range))
    }

    /// The range under `key`, defaulting to the whole timeline.
    pub fn range_or_all(&self, key: &str, project: &Project) -> ToolResultOf<TimeRange> {
        Ok(self
            .opt_range(key)?
            .unwrap_or_else(|| TimeRange::new(Time::ZERO, project.timeline.duration())))
    }

    pub fn range(&self, key: &str) -> ToolResultOf<TimeRange> {
        self.opt_range(key)?
            .ok_or_else(|| ToolError(format!("`{key}` is required")))
    }
}

pub fn parse_time(v: &Value) -> ToolResultOf<Time> {
    let t = match v {
        Value::Number(n) => n
            .as_f64()
            .filter(|f| f.is_finite())
            .map(Time::from_secs_f64),
        Value::String(s) => Time::parse_clock(s),
        _ => None,
    };
    t.ok_or_else(|| ToolError("times are seconds or \"M:SS.mmm\"".into()))
}

/// Seconds rounded to the millisecond, the form results use.
pub fn secs(t: Time) -> Value {
    json!((t.as_millis() as f64) / 1000.0)
}

pub fn range_json(r: TimeRange) -> Value {
    json!({ "start": secs(r.start), "end": secs(r.end) })
}

pub fn short(id: impl Display) -> String {
    id.to_string()
}

fn matches_id(uuid: &Uuid, text: &str) -> bool {
    let text = text.trim().trim_start_matches('@').to_ascii_lowercase();
    if text.len() < 4 {
        return false;
    }
    let simple = uuid.simple().to_string();
    simple.starts_with(&text) || uuid.hyphenated().to_string() == text
}

fn pick<T: Copy>(kind: &str, text: &str, found: Vec<T>) -> ToolResultOf<T> {
    match found.as_slice() {
        [one] => Ok(*one),
        [] => fail(format!("no {kind} with id `{text}`")),
        _ => fail(format!("`{text}` matches several {kind}s; use more digits")),
    }
}

pub fn clip_id(project: &Project, text: &str) -> ToolResultOf<ClipId> {
    let found = project
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter(|c| matches_id(&c.id.0, text))
        .map(|c| c.id)
        .collect();
    pick("clip", text, found)
}

pub fn clip_ids(project: &Project, texts: &[String]) -> ToolResultOf<Vec<ClipId>> {
    texts.iter().map(|t| clip_id(project, t)).collect()
}

pub fn media_id(project: &Project, text: &str) -> ToolResultOf<MediaId> {
    let found = project
        .media
        .iter()
        .filter(|m| {
            matches_id(&m.id.0, text)
                || m.path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy() == text)
        })
        .map(|m| m.id)
        .collect();
    pick("media item", text, found)
}

/// By id or by name ("V1").
pub fn track_id(project: &Project, text: &str) -> ToolResultOf<TrackId> {
    let tracks = &project.timeline.tracks;
    let by_name: Vec<_> = tracks
        .iter()
        .filter(|t| t.name.eq_ignore_ascii_case(text.trim()))
        .map(|t| t.id)
        .collect();
    if !by_name.is_empty() {
        return pick("track", text, by_name);
    }
    let found = tracks
        .iter()
        .filter(|t| matches_id(&t.id.0, text))
        .map(|t| t.id)
        .collect();
    pick("track", text, found)
}

pub fn marker_id(project: &Project, text: &str) -> ToolResultOf<MarkerId> {
    let found = project
        .timeline
        .markers
        .iter()
        .filter(|m| matches_id(&m.id.0, text))
        .map(|m| m.id)
        .collect();
    pick("marker", text, found)
}

/// Speakers are only known from analysis, so the caller passes candidates.
pub fn speaker_id(candidates: &[SpeakerId], text: &str) -> ToolResultOf<SpeakerId> {
    let found = candidates
        .iter()
        .filter(|s| matches_id(&s.0, text))
        .copied()
        .collect();
    pick("speaker", text, found)
}

/// `tracks` argument → `TrackScope` (`None` = every unlocked track).
pub fn track_scope(project: &Project, args: &Args) -> ToolResultOf<Option<Vec<TrackId>>> {
    let names = args.strings("tracks")?;
    if names.is_empty() {
        return Ok(None);
    }
    names
        .iter()
        .map(|n| track_id(project, n))
        .collect::<ToolResultOf<Vec<_>>>()
        .map(Some)
}

pub fn first_track(project: &Project, kind: TrackKind) -> Option<&Track> {
    project.timeline.tracks.iter().find(|t| t.kind == kind)
}

pub fn kind_name(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
        TrackKind::Title => "title",
        TrackKind::Caption => "caption",
        TrackKind::Overlay => "overlay",
    }
}
