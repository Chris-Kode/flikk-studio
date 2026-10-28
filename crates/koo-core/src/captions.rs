//! Caption phrasing (PRD C1): group recognized words into captions a viewer
//! can read, timed to speech onset and offset.
//!
//! Pure functions over [`Word`]s, so the UI, the agent and recipes produce the
//! same captions. Words are taken as they are; low-confidence ones are flagged
//! for review, never rewritten.

use crate::analysis::Word;
use crate::ids::SpeakerId;
use crate::memory::CaptionStyle;
use crate::time::{Time, TimeRange};
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// Words below this recognizer confidence are flagged in captions and the
/// transcript.
pub const LOW_CONFIDENCE: f32 = 0.5;

/// Where a phrase may end and how long it may stay on screen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptionRules {
    pub max_chars_per_line: usize,
    pub max_lines: usize,
    /// A silence longer than this always starts a new caption.
    pub pause: Time,
    /// Short captions are held on screen this long, if the next one allows.
    pub min_duration: Time,
    pub max_duration: Time,
}

impl Default for CaptionRules {
    fn default() -> Self {
        Self::from_style(&CaptionStyle::default())
    }
}

impl CaptionRules {
    pub fn from_style(style: &CaptionStyle) -> Self {
        Self {
            max_chars_per_line: style.max_chars_per_line.max(8) as usize,
            max_lines: style.max_lines.max(1) as usize,
            pause: Time::from_millis(400),
            min_duration: Time::from_millis(800),
            max_duration: Time::from_millis(6000),
        }
    }

    fn max_chars(&self) -> usize {
        self.max_chars_per_line * self.max_lines
    }
}

/// One caption: a run of consecutive input words.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptionPhrase {
    /// From the first word's onset to the last word's offset, extended to
    /// `min_duration` where the gap to the next phrase allows.
    pub range: TimeRange,
    /// Index range into the input words.
    pub words: Range<usize>,
    pub text: String,
    /// `text` wrapped to the style's line length.
    pub lines: Vec<String>,
    pub speaker: Option<SpeakerId>,
    /// Indices (into the input words) of words below [`LOW_CONFIDENCE`].
    pub low_confidence: Vec<usize>,
}

impl CaptionPhrase {
    pub fn needs_review(&self) -> bool {
        !self.low_confidence.is_empty()
    }

    /// When each word of `text` is spoken, from the phrase's start (see
    /// `ClipContent::Caption`).
    pub fn word_times(&self, words: &[Word]) -> Vec<Time> {
        words[self.words.clone()]
            .iter()
            .filter(|w| !clean(&w.text).is_empty())
            .flat_map(|w| {
                // A recognized token may hold several words.
                let n = clean(&w.text).split_whitespace().count();
                std::iter::repeat_n(w.range.start - self.range.start, n)
            })
            .collect()
    }
}

/// Group `words` (sorted by start, in one time base) into caption phrases.
///
/// A phrase ends at sentence punctuation, before a pause longer than
/// `rules.pause`, on a speaker change, when the next word would overflow
/// `max_chars_per_line × max_lines`, or when it would run past
/// `max_duration`. Soft punctuation (`,;:`) ends a phrase once it fills a line.
pub fn group_words(words: &[Word], rules: &CaptionRules) -> Vec<CaptionPhrase> {
    let mut phrases = Vec::new();
    let mut start = 0;
    while start < words.len() {
        // Skip words with nothing to show.
        if clean(&words[start].text).is_empty() {
            start += 1;
            continue;
        }
        let mut end = start + 1;
        let mut text = clean(&words[start].text).to_owned();
        while end < words.len() {
            let prev = &words[end - 1];
            if ends_sentence(&prev.text) {
                break;
            }
            if ends_clause(&prev.text) && text.chars().count() >= rules.max_chars_per_line {
                break;
            }
            let next = &words[end];
            let word = clean(&next.text);
            if word.is_empty() {
                end += 1;
                continue;
            }
            if next.range.start - prev.range.end > rules.pause
                || next.speaker != words[start].speaker
                || next.range.end - words[start].range.start > rules.max_duration
            {
                break;
            }
            let candidate = format!("{text} {word}");
            if !fits(&candidate, rules) {
                end = balance(words, start, end, rules);
                text = join(&words[start..end]);
                break;
            }
            text = candidate;
            end += 1;
        }
        let last = words[start..end]
            .iter()
            .rev()
            .find(|w| !clean(&w.text).is_empty())
            .unwrap_or(&words[start]);
        phrases.push(CaptionPhrase {
            range: TimeRange::new(words[start].range.start, last.range.end),
            words: start..end,
            lines: wrap_lines(&text, rules.max_chars_per_line),
            text,
            speaker: words[start].speaker,
            low_confidence: (start..end)
                .filter(|&i| words[i].confidence < LOW_CONFIDENCE)
                .collect(),
        });
        start = end;
    }
    hold_short(&mut phrases, rules.min_duration);
    separate(&mut phrases);
    phrases
}

/// Make every phrase non-empty and keep it clear of the next, so they can
/// share one caption track. Aligned words may overrun the next word, and the
/// same speech may be heard on two tracks.
fn separate(phrases: &mut [CaptionPhrase]) {
    let shortest = Time::from_millis(100);
    for i in 0..phrases.len() {
        if i > 0 {
            let prev = phrases[i - 1].range;
            let start = phrases[i].range.start;
            if start < prev.end {
                if start > prev.start {
                    phrases[i - 1].range.end = start;
                } else {
                    phrases[i].range.start = prev.end;
                }
            }
        }
        let r = &mut phrases[i].range;
        if r.end < r.start + shortest {
            r.end = r.start + shortest;
        }
    }
}

/// Where a phrase that overflowed after `words[start..end]` should end
/// instead. A sentence that fits in two captions is split near its middle;
/// a longer one at the last natural break past half a caption. Clause
/// punctuation and conjunctions are preferred break points.
fn balance(words: &[Word], start: usize, end: usize, rules: &CaptionRules) -> usize {
    let mut stop = end;
    while stop < words.len() && !hard_break(words, start, stop, rules) {
        stop += 1;
    }
    let len = |a: usize, b: usize| join(&words[a..b]).chars().count();
    let cap = rules.max_chars();
    let candidates = start + 1..=end;
    if len(start, stop) <= 2 * cap {
        let best = candidates
            .clone()
            .filter(|&k| fits(&join(&words[k..stop]), rules))
            .min_by_key(|&k| {
                let diff = len(start, k).abs_diff(len(k, stop));
                diff.saturating_sub(if natural_break(words, k) { cap / 3 } else { 0 })
            });
        if let Some(k) = best {
            return k;
        }
    }
    candidates
        .rev()
        .find(|&k| natural_break(words, k) && len(start, k) * 2 >= cap)
        .unwrap_or(end)
}

/// Whether a phrase starting at `start` must end before `words[i]`.
fn hard_break(words: &[Word], start: usize, i: usize, rules: &CaptionRules) -> bool {
    let prev = &words[i - 1];
    ends_sentence(&prev.text)
        || words[i].range.start - prev.range.end > rules.pause
        || words[i].speaker != words[start].speaker
}

/// A break before `words[k]` reads naturally.
fn natural_break(words: &[Word], k: usize) -> bool {
    const JOINERS: &[&str] = &[
        "and", "but", "or", "so", "because", "that", "which", "who", "when", "where", "while",
        "if", "then", "to", "with", "for",
    ];
    k > 0
        && k < words.len()
        && (ends_clause(&words[k - 1].text) || {
            let w = clean(&words[k].text).to_lowercase();
            JOINERS.contains(&w.as_str())
        })
}

fn fits(text: &str, rules: &CaptionRules) -> bool {
    text.chars().count() <= rules.max_chars()
        && wrap_lines(text, rules.max_chars_per_line).len() <= rules.max_lines
}

fn join(words: &[Word]) -> String {
    words
        .iter()
        .map(|w| clean(&w.text))
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Extend phrases shorter than `min` without running into the next one.
fn hold_short(phrases: &mut [CaptionPhrase], min: Time) {
    for i in 0..phrases.len() {
        let r = phrases[i].range;
        if r.duration() >= min {
            continue;
        }
        let limit = phrases.get(i + 1).map(|p| p.range.start);
        let end = match limit {
            Some(next) => (r.start + min).min(next),
            None => r.start + min,
        };
        phrases[i].range.end = end.max(r.end);
    }
}

/// Greedy word wrap; a single word longer than `max` gets its own line.
pub fn wrap_lines(text: &str, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= max => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

/// Recognizers often emit leading spaces or empty tokens.
fn clean(text: &str) -> &str {
    text.trim()
}

fn ends_sentence(text: &str) -> bool {
    let t = text.trim_end_matches(['"', '\'', ')', '”', '’']);
    t.ends_with(['.', '!', '?', '…'])
}

fn ends_clause(text: &str) -> bool {
    text.trim_end().ends_with([',', ';', ':', '—'])
}
