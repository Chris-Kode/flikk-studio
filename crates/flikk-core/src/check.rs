//! Mistakes a viewer notices, found before they are: a flash of black
//! between two clips, a clip too short to see, a hard audio join, a caption
//! past the end or over another, a clip on offline media, a cut on a word,
//! a caption out of step with its words, lettering off the safe area and a
//! jump in level at a join. [`check`] reads a project and what is already
//! known about its media (cached analysis, the lettering engine, which files
//! are missing) and never waits on a job; a check whose data is missing for
//! an item skips that item and says so in [`Report::skipped`].
//!
//! Change sets keep what they [`introduced`]; the agent reads the same
//! findings with `check_timeline`; Deliver lists those of its range, with
//! the mix's [`loudness`] measured as a job of its own.

use crate::analysis::{Analysis, Levels, Loudness};
use crate::compose::{self, ACTION_SAFE, TITLE_SAFE};
use crate::cuts;
use crate::ids::{ClipId, MediaId};
use crate::model::{Clip, ClipContent, MediaKind, Project, Track, TrackKind};
use crate::query::{TimelineWord, words_in_range};
use crate::services::Services;
use crate::time::{Time, TimeRange};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

/// A gap on the picture shorter than this between two clips is a flash of
/// black.
const FLASH: Time = Time::from_millis(500);
/// A clip shorter than this many frames is too short to see.
const SHORT_FRAMES: i64 = 3;
/// A caption shown this long before its first word starts too early…
const EARLY: Time = Time::from_millis(500);
/// …and one left this long after its last word ends too late.
const LATE: Time = Time::from_millis(1500);
/// How far from a caption its words are looked for.
const CAPTION_REACH: Time = Time::from_millis(2000);
/// Audio measured either side of a join for its level.
const JOIN_WINDOW: Time = Time::from_millis(400);
/// Audio measured either side of a join to tell whether it is in a pause.
const PAUSE_WINDOW: Time = Time::from_millis(30);
/// At or below this, audio is a pause.
const PAUSE_DB: f32 = -45.0;
/// Frames quieter than this don't count toward a join's level.
const ACTIVE_DB: f32 = -50.0;
/// A change in level across a join larger than this is a jump.
const JUMP_DB: f32 = 10.0;
/// Programme loudness this far from the target is reported.
const LOUDNESS_LU: f32 = 1.0;

/// What a finding is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A gap on the picture between two clips, under half a second.
    Flash,
    /// A clip under three frames.
    ShortClip,
    /// Two audio clips meet without a fade, outside a pause.
    HardJoin,
    /// A caption or title runs past the end of the picture and sound.
    PastEnd,
    /// Two captions on screen at once.
    CaptionOverlap,
    /// Clips whose media file is missing.
    Offline,
    /// A cut that falls on a word ([`crate::cuts`]).
    CutOnWord,
    /// A caption that shows well before its words or stays well after.
    CaptionTiming,
    /// A title outside title-safe, or a caption outside action-safe.
    SafeArea,
    /// A jump in level across a join on one track.
    LevelJump,
    /// Programme loudness away from the target (Deliver only).
    Loudness,
    /// True peak over the ceiling (Deliver only).
    TruePeak,
}

impl Kind {
    /// The name tools use for it.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Flash => "flash",
            Kind::ShortClip => "short_clip",
            Kind::HardJoin => "hard_join",
            Kind::PastEnd => "past_end",
            Kind::CaptionOverlap => "caption_overlap",
            Kind::Offline => "offline",
            Kind::CutOnWord => "cut_on_word",
            Kind::CaptionTiming => "caption_timing",
            Kind::SafeArea => "safe_area",
            Kind::LevelJump => "level_jump",
            Kind::Loudness => "loudness",
            Kind::TruePeak => "true_peak",
        }
    }
}

/// How much a finding matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// A viewer will see or hear it.
    Warning,
    /// Worth a look; it may be meant.
    Notice,
}

/// One mistake, where it is and what it involves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub kind: Kind,
    pub severity: Severity,
    /// Where on the timeline; a cut is a range of no length.
    pub range: TimeRange,
    /// The clips involved, in timeline order; none for the whole mix.
    pub clips: Vec<ClipId>,
    /// One line for the user and the agent, in sentence case.
    pub note: String,
}

impl Finding {
    /// What makes it the same finding after an edit moved it: its kind and
    /// the clips involved, not where they are.
    fn identity(&self) -> (Kind, Vec<ClipId>) {
        let mut clips = self.clips.clone();
        clips.sort();
        (self.kind, clips)
    }
}

/// What [`check`] found.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// In timeline order.
    pub findings: Vec<Finding>,
    /// Checks that left out some items for want of data (not analyzed, no
    /// lettering engine): not failures, just not looked at.
    pub skipped: BTreeSet<Kind>,
}

/// What a check reads besides the project.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    /// Cached analysis results (never started or waited on) and the
    /// lettering engine.
    pub services: Services,
    /// Media whose file is missing.
    pub offline: HashSet<MediaId>,
}

impl Sources {
    /// `services`, with the media of `project` whose file is not on disk
    /// as offline.
    pub fn on_disk(project: &Project, services: &Services) -> Self {
        let offline = project
            .media
            .iter()
            .filter(|m| !m.path.exists())
            .map(|m| m.id)
            .collect();
        Self {
            services: services.clone(),
            offline,
        }
    }
}

/// Every finding in `project`, or in `range` of it, with what `sources`
/// already know. Pure and quick enough to run on every change set, but it
/// reads analysis and letters captions: run it off the UI thread, on a
/// snapshot.
pub fn check(project: &Project, range: Option<TimeRange>, sources: &Sources) -> Report {
    let range = range.unwrap_or_else(|| TimeRange::new(Time::ZERO, Time(i64::MAX)));
    let mut c = Checker {
        project,
        sources,
        range,
        frame: project.timeline.settings.rate.frame_duration().max(Time(1)),
        analyses: HashMap::new(),
        report: Report::default(),
    };
    c.flashes();
    c.short_clips();
    c.joins();
    c.lettering_in_time();
    c.offline();
    c.cuts_on_words();
    c.caption_timing();
    c.safe_area();
    let mut report = c.report;
    report.findings.retain(|f| touches(range, f.range));
    report
        .findings
        .sort_by_key(|f| (f.range.start, f.range.end, f.kind));
    report
}

/// The findings of `after` that `before` doesn't have: those an edit
/// introduced. A finding the edit only moved (the same kind on the same
/// clips) is not new.
pub fn introduced(before: &[Finding], after: &[Finding]) -> Vec<Finding> {
    let known: HashSet<_> = before.iter().map(Finding::identity).collect();
    after
        .iter()
        .filter(|f| !known.contains(&f.identity()))
        .cloned()
        .collect()
}

/// The findings of a mix of `range` measured as `measured`, against a
/// target of `target_lufs` integrated and a ceiling of `ceiling_db` true
/// peak.
pub fn loudness(
    measured: Loudness,
    target_lufs: f32,
    ceiling_db: f32,
    range: TimeRange,
) -> Vec<Finding> {
    let mut out = Vec::new();
    let off = measured.integrated_lufs - target_lufs;
    if off.abs() > LOUDNESS_LU {
        out.push(Finding {
            kind: Kind::Loudness,
            severity: Severity::Warning,
            range,
            clips: Vec::new(),
            note: format!(
                "Mix at {} LUFS, target {}",
                db(measured.integrated_lufs),
                db(target_lufs)
            ),
        });
    }
    if measured.true_peak_db > ceiling_db {
        out.push(Finding {
            kind: Kind::TruePeak,
            severity: Severity::Warning,
            range,
            clips: Vec::new(),
            note: format!(
                "True peak {} dBTP, over {}",
                db(measured.true_peak_db),
                db(ceiling_db)
            ),
        });
    }
    out
}

/// One decimal, with a typographic minus.
fn db(v: f32) -> String {
    format!("{v:.1}").replace('-', "\u{2212}")
}

/// Whether a finding at `r` belongs to `range`.
fn touches(range: TimeRange, r: TimeRange) -> bool {
    r.overlaps(&range) || range.contains(r.start)
}

struct Checker<'a> {
    project: &'a Project,
    sources: &'a Sources,
    range: TimeRange,
    frame: Time,
    analyses: HashMap<MediaId, Option<Arc<Analysis>>>,
    report: Report,
}

impl Checker<'_> {
    fn push(
        &mut self,
        kind: Kind,
        severity: Severity,
        range: TimeRange,
        clips: Vec<ClipId>,
        note: String,
    ) {
        self.report.findings.push(Finding {
            kind,
            severity,
            range,
            clips,
            note,
        });
    }

    fn skip(&mut self, kind: Kind) {
        self.report.skipped.insert(kind);
    }

    /// Clips of `track` that are seen or heard and reach into the range.
    fn live<'t>(&self, track: &'t Track) -> impl Iterator<Item = &'t Clip> + use<'t> {
        let range = self.range;
        track
            .clips
            .iter()
            .filter(move |c| c.enabled && touches(range, c.range()))
    }

    /// The cached analysis of `media`, if it has one.
    fn analysis(&mut self, media: MediaId) -> Option<Arc<Analysis>> {
        let (project, sources) = (self.project, self.sources);
        self.analyses
            .entry(media)
            .or_insert_with(|| {
                let fingerprint = project.media(media)?.fingerprint.as_ref()?;
                sources.services.analysis.as_ref()?.analysis(fingerprint)
            })
            .clone()
    }

    fn frames(&self, t: Time) -> i64 {
        (t.0 + self.frame.0 / 2) / self.frame.0
    }

    // ---------------------------------------------------------- project

    /// Gaps on the picture, under [`FLASH`], between two clips.
    fn flashes(&mut self) {
        let tl = &self.project.timeline;
        let mut clips: Vec<&Clip> = tl
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Video && !t.mix.mute)
            .flat_map(|t| t.clips.iter().filter(|c| c.enabled))
            .collect();
        clips.sort_by_key(|c| c.start);
        // The clip reaching furthest so far, and how far.
        let mut reach: Option<&Clip> = None;
        for clip in clips {
            if let Some(before) = reach {
                let gap = TimeRange::new(before.end(), clip.start);
                if !gap.is_empty() && gap.duration() < FLASH {
                    let frames = self.frames(gap.duration()).max(1);
                    let note = match frames {
                        1 => "1 frame of black between two clips".to_owned(),
                        n => format!("{n} frames of black between two clips"),
                    };
                    self.push(
                        Kind::Flash,
                        Severity::Warning,
                        gap,
                        vec![before.id, clip.id],
                        note,
                    );
                }
            }
            if reach.is_none_or(|r| clip.end() > r.end()) {
                reach = Some(clip);
            }
        }
    }

    /// Clips under [`SHORT_FRAMES`] frames, on any track.
    fn short_clips(&mut self) {
        let shortest = Time(self.frame.0 * SHORT_FRAMES);
        let short: Vec<(ClipId, TimeRange, i64)> = self
            .project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| self.live(t))
            .filter(|c| c.duration < shortest)
            .map(|c| (c.id, c.range(), self.frames(c.duration).max(1)))
            .collect();
        for (id, range, frames) in short {
            let note = match frames {
                1 => "Clip lasts 1 frame".to_owned(),
                n => format!("Clip lasts {n} frames"),
            };
            self.push(Kind::ShortClip, Severity::Warning, range, vec![id], note);
        }
    }

    /// Joins of two clips on an audio track: hard ones outside a pause,
    /// and jumps in level.
    fn joins(&mut self) {
        let tl = &self.project.timeline;
        let mut joins = Vec::new();
        for track in tl.tracks.iter().filter(|t| tl.audible(t)) {
            let clips: Vec<&Clip> = self.live(track).collect();
            for pair in clips.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let gap = b.start - a.end();
                if gap >= Time::ZERO && gap < self.frame && !continues(a, b) {
                    joins.push((a, b));
                }
            }
        }
        for (a, b) in joins {
            let at = TimeRange::new(b.start, b.start);
            let crossfade = a.transition_out.is_some() || b.transition_in.is_some();
            let (out, into) = (
                self.peak(a, TimeRange::new(a.end() - PAUSE_WINDOW, a.end())),
                self.peak(b, TimeRange::new(b.start, b.start + PAUSE_WINDOW)),
            );
            let paused =
                matches!((out, into), (Some(x), Some(y)) if x <= PAUSE_DB && y <= PAUSE_DB);
            if !crossfade
                && a.audio.fade_out <= Time::ZERO
                && b.audio.fade_in <= Time::ZERO
                && !paused
            {
                self.push(
                    Kind::HardJoin,
                    Severity::Notice,
                    at,
                    vec![a.id, b.id],
                    "Hard audio join without a fade".into(),
                );
            }
            if crossfade {
                continue;
            }
            let before = self.active_level(a, TimeRange::new(a.end() - JOIN_WINDOW, a.end()));
            let after = self.active_level(b, TimeRange::new(b.start, b.start + JOIN_WINDOW));
            match (before, after) {
                (Some(Some(x)), Some(Some(y))) if (x - y).abs() > JUMP_DB => {
                    let jump = (y - x).round() as i32;
                    let note = if jump > 0 {
                        format!("Level jumps up {jump} dB at the join")
                    } else {
                        format!("Level drops {} dB at the join", -jump)
                    };
                    self.push(
                        Kind::LevelJump,
                        Severity::Warning,
                        at,
                        vec![a.id, b.id],
                        note,
                    );
                }
                (None, _) | (_, None) => self.skip(Kind::LevelJump),
                _ => {}
            }
        }
    }

    /// Captions and titles past the end of the picture and sound, and two
    /// captions on screen at once.
    fn lettering_in_time(&mut self) {
        let tl = &self.project.timeline;
        let end = tl
            .tracks
            .iter()
            .filter(|t| {
                matches!(
                    t.kind,
                    TrackKind::Video | TrackKind::Audio | TrackKind::Overlay
                )
            })
            .flat_map(|t| t.clips.iter().filter(|c| c.enabled))
            .map(Clip::end)
            .max();
        let mut found = Vec::new();
        let lettering = tl
            .tracks
            .iter()
            .filter(|t| matches!(t.kind, TrackKind::Caption | TrackKind::Title) && !t.mix.mute);
        if let Some(end) = end {
            for track in lettering.clone() {
                for c in self.live(track).filter(|c| c.end() > end + self.frame) {
                    let what = if track.kind == TrackKind::Title {
                        "Title"
                    } else {
                        "Caption"
                    };
                    let over = (c.end() - end).as_secs_f64();
                    let note = format!("{what} runs {over:.1} s past the end");
                    found.push((
                        Kind::PastEnd,
                        TimeRange::new(end.max(c.start), c.end()),
                        vec![c.id],
                        note,
                    ));
                }
            }
        }
        let mut captions: Vec<&Clip> = lettering
            .filter(|t| t.kind == TrackKind::Caption)
            .flat_map(|t| self.live(t))
            .filter(|c| matches!(c.content, ClipContent::Caption { .. }))
            .collect();
        captions.sort_by_key(|c| c.start);
        for (i, a) in captions.iter().enumerate() {
            for b in captions[i + 1..].iter().take_while(|b| b.start < a.end()) {
                if let Some(both) = a.range().intersect(&b.range()) {
                    let note = "Two captions on screen at once".to_owned();
                    found.push((Kind::CaptionOverlap, both, vec![a.id, b.id], note));
                }
            }
        }
        for (kind, range, clips, note) in found {
            self.push(kind, Severity::Warning, range, clips, note);
        }
    }

    /// Clips on media whose file is missing, one finding per file.
    fn offline(&mut self) {
        let mut by_media: Vec<(MediaId, Vec<&Clip>)> = Vec::new();
        for track in &self.project.timeline.tracks {
            for c in self.live(track) {
                let Some(media) = c.media().filter(|m| self.sources.offline.contains(m)) else {
                    continue;
                };
                match by_media.iter_mut().find(|(m, _)| *m == media) {
                    Some((_, clips)) => clips.push(c),
                    None => by_media.push((media, vec![c])),
                }
            }
        }
        for (media, mut clips) in by_media {
            clips.sort_by_key(|c| c.start);
            let name = self
                .project
                .media(media)
                .map(|m| match &m.name {
                    Some(name) => name.clone(),
                    None => m
                        .path
                        .file_name()
                        .map_or_else(|| "Media".into(), |n| n.to_string_lossy().into_owned()),
                })
                .unwrap_or_else(|| "Media".into());
            let note = match clips.len() {
                1 => format!("{name} is offline"),
                n => format!("{name} is offline ({n} clips)"),
            };
            let range = clips[0].range();
            let ids = clips.iter().map(|c| c.id).collect();
            self.push(Kind::Offline, Severity::Warning, range, ids, note);
        }
    }

    // --------------------------------------------------------- analysis

    /// Words of the clips under `range`, heard as the transcript panel
    /// hears them; marks `kind` skipped when footage or sound under it is
    /// not analyzed.
    fn words(&mut self, range: TimeRange, kind: Kind) -> Vec<TimelineWord> {
        let project = self.project;
        let media: Vec<MediaId> = project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().filter(|c| c.range().overlaps(&range)))
            .filter_map(Clip::media)
            .filter(|m| {
                project
                    .media(*m)
                    .is_some_and(|m| m.kind != MediaKind::Image)
            })
            .collect();
        let mut unanalyzed = false;
        for m in media {
            unanalyzed |= self.analysis(m).is_none();
        }
        if unanalyzed {
            self.skip(kind);
        }
        let analyses = &self.analyses;
        words_in_range(project, range, |fp| {
            let media = project
                .media
                .iter()
                .find(|m| m.fingerprint.as_ref() == Some(fp))?;
            analyses.get(&media.id).cloned().flatten()
        })
    }

    /// Edges of audio clips that cut into a word: the clip starts or ends
    /// where its source still speaks.
    fn cuts_on_words(&mut self) {
        let tl = &self.project.timeline;
        let mut edges: Vec<(&Clip, Time)> = Vec::new();
        for track in tl.tracks.iter().filter(|t| tl.audible(t)) {
            let clips: Vec<&Clip> = self.live(track).filter(|c| c.media().is_some()).collect();
            for (i, &c) in clips.iter().enumerate() {
                // A split the source plays on through is not a cut.
                let joined = |a: &Clip, b: &Clip| a.end() == b.start && continues(a, b);
                let after_split = i > 0 && joined(clips[i - 1], c);
                let before_split = clips.get(i + 1).is_some_and(|&next| joined(c, next));
                if !after_split && c.source_range().is_some_and(|s| s.start > Time::ZERO) {
                    edges.push((c, c.start));
                }
                if !before_split && self.source_continues(c) {
                    edges.push((c, c.end()));
                }
            }
        }
        let mut found = Vec::new();
        for (clip, at) in edges {
            let words: Vec<TimelineWord> = self
                .words(clip.range(), Kind::CutOnWord)
                .into_iter()
                .filter(|w| w.clip == clip.id)
                .collect();
            if let Some(k) = cuts::on_word(&words, at) {
                let note = format!("Cut clips \u{201c}{}\u{201d}", words[k].word.text.trim());
                found.push((TimeRange::new(at, at), clip.id, note));
            }
        }
        for (range, clip, note) in found {
            self.push(Kind::CutOnWord, Severity::Warning, range, vec![clip], note);
        }
    }

    /// Whether `clip`'s source goes on after its end.
    fn source_continues(&self, clip: &Clip) -> bool {
        let Some(source) = clip.source_range() else {
            return false;
        };
        clip.media()
            .and_then(|m| self.project.media(m))
            .and_then(|m| m.info.as_ref())
            .is_none_or(|info| source.end + self.frame < info.duration)
    }

    /// Captions shown well before their first word or left well after
    /// their last, against the words heard under them.
    fn caption_timing(&mut self) {
        let tl = &self.project.timeline;
        let captions: Vec<&Clip> = tl
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Caption && !t.mix.mute)
            .flat_map(|t| self.live(t))
            .filter(|c| matches!(c.content, ClipContent::Caption { .. }))
            .collect();
        let mut found = Vec::new();
        for caption in captions {
            let ClipContent::Caption { text, .. } = &caption.content else {
                continue;
            };
            let tokens: Vec<String> = text
                .split_whitespace()
                .map(token)
                .filter(|t| !t.is_empty())
                .collect();
            let (Some(first), Some(last)) = (tokens.first(), tokens.last()) else {
                continue;
            };
            let near = TimeRange::new(
                (caption.start - CAPTION_REACH).max(Time::ZERO),
                caption.end() + CAPTION_REACH,
            );
            let heard = self.words(near, Kind::CaptionTiming);
            let nearest = |word: &str, to: Time| {
                heard
                    .iter()
                    .filter(|w| token(&w.word.text) == word)
                    .min_by_key(|w| (w.timeline.start - to).0.abs())
                    .map(|w| w.timeline)
            };
            let (Some(opens), Some(closes)) =
                (nearest(first, caption.start), nearest(last, caption.end()))
            else {
                continue;
            };
            if opens.start - caption.start > EARLY {
                let by = (opens.start - caption.start).as_secs_f64();
                found.push((
                    TimeRange::new(caption.start, opens.start),
                    caption.id,
                    format!("Caption shows {by:.1} s before its words"),
                ));
            }
            if caption.end() - closes.end > LATE {
                let by = (caption.end() - closes.end).as_secs_f64();
                found.push((
                    TimeRange::new(closes.end, caption.end()),
                    caption.id,
                    format!("Caption stays {by:.1} s after its words"),
                ));
            }
        }
        for (range, clip, note) in found {
            self.push(
                Kind::CaptionTiming,
                Severity::Notice,
                range,
                vec![clip],
                note,
            );
        }
    }

    /// Titles whose box at their middle is outside title-safe, and
    /// captions outside action-safe: caption styles sit lower by design
    /// (7 % from the edge), and every caption would be reported.
    fn safe_area(&mut self) {
        let project = self.project;
        let s = &project.timeline.settings;
        let canvas = (s.width.max(1), s.height.max(1));
        let services = &self.sources.services;
        let mut found = Vec::new();
        let mut unlettered = false;
        for track in project
            .timeline
            .tracks
            .iter()
            .filter(|t| matches!(t.kind, TrackKind::Caption | TrackKind::Title) && !t.mix.mute)
        {
            let (area, note) = match track.kind {
                TrackKind::Title => (TITLE_SAFE, "Title outside title-safe"),
                _ => (ACTION_SAFE, "Caption outside action-safe"),
            };
            let area = compose::safe_area(canvas, area);
            for clip in self.live(track).filter(|c| compose::is_lettering(track, c)) {
                if services.lettering.is_none() {
                    unlettered = true;
                    break;
                }
                let middle = (clip.start + Time(clip.duration.0 / 2)).floor_to_frame(s.rate);
                let Some(shape) = compose::lettering_shape(track, clip, middle, canvas, services)
                else {
                    continue;
                };
                if !compose::inside(shape.bounds(), area) {
                    found.push((clip.range(), clip.id, note.to_owned()));
                }
            }
        }
        if unlettered {
            self.skip(Kind::SafeArea);
        }
        for (range, clip, note) in found {
            self.push(Kind::SafeArea, Severity::Warning, range, vec![clip], note);
        }
    }

    /// The loudest dBFS level `clip` plays over `range` (timeline), with
    /// its gain; `None` when its media has no levels.
    fn peak(&mut self, clip: &Clip, range: TimeRange) -> Option<f32> {
        let db = self.samples(clip, range)?;
        db.into_iter()
            .reduce(f32::max)
            .map(|v| v + clip.audio.gain_db)
    }

    /// The mean level of what is heard of `clip` over `range` (timeline),
    /// with its gain: `None` when its media has no levels, `Some(None)`
    /// when it is mostly quiet there.
    fn active_level(&mut self, clip: &Clip, range: TimeRange) -> Option<Option<f32>> {
        let db = self.samples(clip, range)?;
        let active: Vec<f32> = db.iter().copied().filter(|d| *d > ACTIVE_DB).collect();
        if active.is_empty() || active.len() * 4 < db.len() {
            return Some(None);
        }
        let mean = active.iter().sum::<f32>() / active.len() as f32;
        Some(Some(mean + clip.audio.gain_db))
    }

    /// The dBFS levels `clip` plays over timeline `range`, in 10 ms
    /// steps; `None` when its media has no levels.
    fn samples(&mut self, clip: &Clip, range: TimeRange) -> Option<Vec<f32>> {
        let analysis = self.analysis(clip.media()?)?;
        let levels = analysis.levels.as_ref()?;
        let range = range.intersect(&clip.range()).unwrap_or(range);
        let mut out = Vec::new();
        let mut t = range.start;
        while t < range.end {
            out.extend(clip.to_source(t).and_then(|s| levels.at(s)));
            t = t + Levels::FRAME;
        }
        Some(out)
    }
}

/// Whether `b` plays on from where `a` stops in the same source at the
/// same speed: a split, which is no join at all.
fn continues(a: &Clip, b: &Clip) -> bool {
    let (Some(sa), Some(sb)) = (a.source_range(), b.source_range()) else {
        return false;
    };
    a.media() == b.media()
        && a.speed == b.speed
        && a.retime.keys.is_empty()
        && b.retime.keys.is_empty()
        && (sb.start - sa.end).0.abs() <= Time::from_millis(1).0
}

/// A word as compared: lower case, letters and digits only.
fn token(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
