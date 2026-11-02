//! Fixing words by typing them: a misspoken or missing word is said again
//! in the speaker's own voice and cut in where the old words were.
//!
//! 1. [`fix`] reads the words picked in the transcript: the cuts either
//!    side of them (in the pauses, as a word delete places them, see
//!    [`crate::cuts`]), the rest of their sentence, the speech around it,
//!    and quiet stretches of the same recording for its room tone.
//! 2. [`request`] is the voiceover a provider reads: the whole sentence
//!    with the new words in it, the speech before and after as context,
//!    so its delivery fits there.
//! 3. [`make`] times the take's words ([`AnalysisService::time_script`]),
//!    cuts the new words out of it, matches their level and tone to the
//!    speaker's in that sentence, lays the room tone under them and pads
//!    them to the pauses the old words had. The result is a voiceover item
//!    whose script is the new words, so analysis times them like any
//!    voiceover and the transcript shows them.
//! 4. [`Command::ReplaceSpeech`] puts it in place of the old words (see
//!    [`plan`]).
//!
//! [`AnalysisService::time_script`]: crate::services::AnalysisService::time_script
//! [`Command::ReplaceSpeech`]: crate::command::Command::ReplaceSpeech

mod sound;
mod splice;

use crate::analysis::Analysis;
use crate::cloud::{self, GenerateRequest, Reading, SpeechContext};
use crate::cuts::{FADE, Speech};
use crate::ids::*;
use crate::model::{Fingerprint, Project, TrackKind};
use crate::query::TimelineWord;
use crate::time::{FLICKS_PER_SECOND, Time, TimeRange};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub use sound::make;
pub(crate) use splice::plan;

/// At most this many words of the sentence either side are read with the
/// new ones.
const SENTENCE_WORDS: usize = 30;
/// At most this many words before and after the sentence go as context.
const CONTEXT_WORDS: usize = 50;
/// Room tone is looked for this far either side of the words.
const TONE_REACH: Time = Time(FLICKS_PER_SECOND * 20);
/// A pause this long between two words can give room tone…
const TONE_PAUSE: Time = Time(FLICKS_PER_SECOND * 3 / 10);
/// …less this much at either end, kept clear of the words.
const TONE_GUARD: Time = Time(FLICKS_PER_SECOND * 8 / 100);
/// The quietest few pauses are kept.
const TONE_PAUSES: usize = 4;

/// Words to say again, and everything needed to say them and cut them in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fix {
    pub track: TrackId,
    pub clip: ClipId,
    pub media: MediaId,
    /// The clip's audio stream (container index).
    #[serde(default)]
    pub stream: u32,
    /// Timeline range replaced: from the cut before the first word to the
    /// cut before the word after the last.
    pub range: TimeRange,
    /// Source time of the old words, first start to last end.
    pub words: TimeRange,
    /// Source time of `range`.
    pub source: TimeRange,
    /// What was said.
    pub old: String,
    /// What is said instead.
    pub text: String,
    /// The rest of the sentence, read with it.
    pub before: String,
    pub after: String,
    /// Source time of the whole sentence, the old words in it.
    pub sentence: TimeRange,
    /// Speech around the sentence, as context.
    pub previous: String,
    pub next: String,
    pub speaker: Option<SpeakerId>,
    pub fingerprint: Option<Fingerprint>,
    /// Quiet stretches of the source near it, for room tone.
    #[serde(default)]
    pub tone: Vec<TimeRange>,
}

impl Fix {
    /// What the provider reads: the sentence, the new words in it.
    pub fn script(&self) -> String {
        [
            self.before.as_str(),
            self.text.as_str(),
            self.after.as_str(),
        ]
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
    }

    /// Where the new words are among the script's spoken words.
    pub fn new_words(&self) -> std::ops::Range<usize> {
        let first = cloud::spoken_words(&self.before).len();
        first..first + cloud::spoken_words(&self.text).len()
    }

    /// One line for cards and logs.
    pub fn summary(&self) -> String {
        format!("“{}” → “{}”", self.old, self.text)
    }

    /// Who says the words, to find the voice to say them in.
    pub fn speaker(&self) -> Option<crate::library::VoiceSpeaker> {
        Some(crate::library::VoiceSpeaker {
            fingerprint: self.fingerprint.clone()?,
            speaker: self.speaker,
        })
    }

    /// Where the words play in `project` now: the same source of the same
    /// clip, which edits elsewhere may have moved. `None` once the clip no
    /// longer plays all of it.
    pub fn placed(&self, project: &Project) -> Option<TimeRange> {
        let (track, clip) = project.timeline.clip(self.clip)?;
        let crate::model::ClipContent::Media { source_in, .. } = &clip.content else {
            return None;
        };
        if track.id != self.track || (clip.speed - 1.0).abs() > 1e-9 {
            return None;
        }
        let start = clip.start + (self.source.start - *source_in);
        let range = TimeRange::new(start, start + self.source.duration());
        (clip.start <= range.start && range.end <= clip.end()).then_some(range)
    }

    /// The command that cuts `media`, the new words made for this fix, in
    /// where the old ones play in `project` now.
    pub fn command(
        &self,
        project: &Project,
        media: MediaId,
    ) -> Result<crate::command::Command, String> {
        let range = self
            .placed(project)
            .ok_or("The words were edited since; the new ones are in the pool.")?;
        Ok(crate::command::Command::ReplaceSpeech {
            range,
            track: self.track,
            media,
            fade: FADE,
        })
    }
}

/// The voiceover that says `fix`'s words in `reading`'s voice.
pub fn request(fix: &Fix, reading: &Reading) -> GenerateRequest {
    GenerateRequest::Voiceover {
        text: fix.script(),
        reading: reading.clone(),
        context: Some(SpeechContext {
            previous: fix.previous.clone(),
            next: fix.next.clone(),
        }),
    }
}

/// `words[first..=last]` (timeline words, as [`crate::query::words_in_range`]
/// lists them) said as `text` instead. Why not, in a line for the user.
pub fn fix(
    project: &Project,
    words: &[TimelineWord],
    first: usize,
    last: usize,
    text: &str,
    analysis: impl Fn(&Fingerprint) -> Option<Arc<Analysis>>,
) -> Result<Fix, String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if first > last || last >= words.len() {
        return Err("Pick the words to fix.".into());
    }
    let picked = &words[first..=last];
    let (a, b) = (&words[first], &words[last]);
    if picked.iter().any(|w| w.clip != a.clip) {
        return Err("Pick words said in one clip.".into());
    }
    let old = picked
        .iter()
        .map(|w| w.word.text.trim())
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        return Err("Type what to say; delete words to take them out.".into());
    }
    if text == old {
        return Err("Type something else to say.".into());
    }
    cloud::check_script(&text).map_err(|e| e.to_string())?;
    let (track, clip) = project
        .timeline
        .clip(a.clip)
        .ok_or("The clip is gone.".to_owned())?;
    if track.kind != TrackKind::Audio {
        return Err("Those words aren't on an audio track.".into());
    }
    if track.locked {
        return Err(format!("{} is locked.", track.name));
    }
    if (clip.speed - 1.0).abs() > 1e-9 {
        return Err("Those words are in a retimed clip.".into());
    }
    let crate::model::ClipContent::Media { audio_stream, .. } = &clip.content else {
        return Err("Those words aren't in a recording.".into());
    };
    let item = project.media(a.media).ok_or("The recording is gone.")?;
    let stream = audio_stream
        .or_else(|| item.info.as_ref()?.audio.first().map(|s| s.index))
        .unwrap_or(0);
    let found = item.fingerprint.as_ref().and_then(&analysis);
    let speech = Speech::new(project, &analysis);
    let (start, end) = speech
        .around(words, first, last)
        .ok_or("There is no room to cut those words in.")?;
    let range = TimeRange::new(start.at, end.at);
    let source = TimeRange::new(
        clip.to_source(range.start).unwrap_or(a.source.start),
        clip.to_source(range.end).unwrap_or(b.source.end),
    );

    // The sentence: back to the end of the one before, on to its own end,
    // in this voice (or in words fixed in it before, which a provider said).
    let respoken = |m: MediaId| {
        project
            .media(m)
            .and_then(|m| m.generated.as_deref())
            .is_some_and(|g| g.capability == cloud::Capability::Voiceover)
    };
    let same = |w: &TimelineWord| {
        (w.media == a.media && w.word.speaker == a.word.speaker) || respoken(w.media)
    };
    let mut i = first;
    while i > 0 && first - i < SENTENCE_WORDS {
        let p = &words[i - 1];
        if !same(p) || ends_sentence(&p.word.text) {
            break;
        }
        i -= 1;
    }
    let mut j = last;
    while j + 1 < words.len() && j - last < SENTENCE_WORDS && !ends_sentence(&words[j].word.text) {
        if !same(&words[j + 1]) {
            break;
        }
        j += 1;
    }
    let join = |ws: &[TimelineWord]| {
        ws.iter()
            .map(|w| w.word.text.trim())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let previous = join(&words[i.saturating_sub(CONTEXT_WORDS)..i]);
    let next = join(&words[(j + 1).min(words.len())..(j + 1 + CONTEXT_WORDS).min(words.len())]);
    // The speaker's own recording of the sentence, to match.
    let theirs = || words[i..=j].iter().filter(|w| w.media == a.media);
    let sentence = TimeRange::new(
        theirs()
            .map(|w| w.source.start)
            .min()
            .unwrap_or(a.source.start),
        theirs().map(|w| w.source.end).max().unwrap_or(b.source.end),
    );

    Ok(Fix {
        track: track.id,
        clip: clip.id,
        media: a.media,
        stream,
        range,
        words: TimeRange::new(a.source.start, b.source.end),
        source,
        old,
        text,
        before: join(&words[i..first]),
        after: join(&words[last + 1..=j]),
        sentence,
        previous,
        next,
        speaker: a.word.speaker,
        fingerprint: item.fingerprint.clone(),
        tone: found
            .map(|f| room_tone(&f, TimeRange::new(a.source.start, b.source.end)))
            .unwrap_or_default(),
    })
}

/// Whether a word ends its sentence.
fn ends_sentence(text: &str) -> bool {
    text.trim_end_matches(['"', '”', '’', '\'', ')'])
        .ends_with(['.', '?', '!', '…', '。', '？', '！'])
}

/// The quietest pauses between words near `around`, in source time.
fn room_tone(analysis: &Analysis, around: TimeRange) -> Vec<TimeRange> {
    let Some(transcript) = &analysis.transcript else {
        return Vec::new();
    };
    let reach = TimeRange::new(around.start - TONE_REACH, around.end + TONE_REACH);
    let mut words: Vec<TimeRange> = transcript.words.iter().map(|w| w.range).collect();
    words.sort_by_key(|r| r.start);
    let level = |r: TimeRange| -> f32 {
        let Some(levels) = &analysis.levels else {
            return 0.0;
        };
        let mut t = r.start;
        let mut loudest = -100.0f32;
        while t < r.end {
            if let Some(db) = levels.at(t) {
                loudest = loudest.max(db);
            }
            t = t + crate::analysis::Levels::FRAME;
        }
        loudest
    };
    let mut pauses: Vec<(f32, TimeRange)> = words
        .windows(2)
        .map(|w| TimeRange::new(w[0].end + TONE_GUARD, w[1].start - TONE_GUARD))
        .filter(|r| r.duration() >= TONE_PAUSE - TONE_GUARD - TONE_GUARD)
        .filter(|r| r.overlaps(&reach) && r.start >= Time::ZERO)
        .map(|r| (level(r), r))
        .collect();
    pauses.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<TimeRange> = pauses
        .into_iter()
        .take(TONE_PAUSES)
        .map(|(_, r)| r)
        .collect();
    out.sort_by_key(|r| r.start);
    out
}
