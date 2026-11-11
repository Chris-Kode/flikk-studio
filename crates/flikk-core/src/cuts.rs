//! Where cuts in speech go: in the pause between two words and clear of
//! both (the earlier keeps its tail, the later its attack), at the quietest
//! point near where the cut is wanted, and on a frame boundary when the
//! pause holds one. Word edges come from the analysis, which measures them
//! on the audio. The transcript panel and the agent share this.

use crate::analysis::Analysis;
use crate::ids::{ClipId, MediaId};
use crate::model::{Fingerprint, Project};
use crate::query::{TimelineWord, merge_ranges};
use crate::time::{FLICKS_PER_SECOND, Time, TimeRange};
use std::collections::HashMap;
use std::sync::Arc;

/// Audio fade on both sides of a cut, so the join never clicks.
pub const FADE: Time = Time(FLICKS_PER_SECOND / 100);
/// Kept after a word for its decay.
const TAIL: Time = Time(FLICKS_PER_SECOND * 40 / 1000);
/// Kept before a word for its attack.
const LEAD: Time = Time(FLICKS_PER_SECOND * 30 / 1000);
/// Steps between candidate cuts when no frame boundary fits.
const STEP: Time = Time(FLICKS_PER_SECOND / 100);
/// Moving a cut 20 ms from where it is wanted costs as much as 1 dB of
/// level at the cut…
const DB_PER_MS: f32 = 0.05;
/// …above the quiet of the pause, which reaches this far above its
/// quietest quarter.
const QUIET_DB: f32 = 3.0;

/// A cut placed between two words.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub at: Time,
    /// No pause to cut in: the words run together, so the cut may be heard.
    pub tight: bool,
}

/// Words removed by one ripple delete.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Removal {
    pub range: TimeRange,
    /// An edge had no pause to cut in.
    pub tight: bool,
}

/// A cut that fell on a word, moved off it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    pub cut: Cut,
    /// Index of the word it fell on.
    pub word: usize,
}

/// The index of the word in `words` a cut at `t` falls on: inside it, or
/// closer to it than a cut keeps clear of (its attack before, its decay
/// after). `None` when `t` is clear of every word.
pub fn on_word(words: &[TimelineWord], t: Time) -> Option<usize> {
    words
        .iter()
        .position(|w| w.timeline.start - LEAD < t && t < w.timeline.end + TAIL)
}

/// The project's speech: words (from [`crate::query::words_in_range`])
/// and the levels of their sources.
pub struct Speech<'p> {
    project: &'p Project,
    analyses: HashMap<MediaId, Arc<Analysis>>,
}

impl<'p> Speech<'p> {
    pub fn new(
        project: &'p Project,
        analysis: impl Fn(&Fingerprint) -> Option<Arc<Analysis>>,
    ) -> Self {
        let analyses = project
            .media
            .iter()
            .filter_map(|m| Some((m.id, analysis(m.fingerprint.as_ref()?)?)))
            .collect();
        Self { project, analyses }
    }

    /// Ripple-delete ranges that remove `words[first..=last]`, latest first
    /// so earlier ones stay valid. The pause before the first word stays;
    /// the one after the last goes with it.
    pub fn removals(&self, words: &[TimelineWord], first: usize, last: usize) -> Vec<Removal> {
        let last = last.min(words.len().saturating_sub(1));
        let mut out: Vec<Removal> = Vec::new();
        let mut i = first;
        while i <= last && i < words.len() {
            // A run of words in one clip.
            let clip = words[i].clip;
            let mut j = i;
            while j < last && words[j + 1].clip == clip {
                j += 1;
            }
            let prev = words[..i].iter().rev().find(|w| w.clip == clip);
            let next = words[j + 1..].iter().find(|w| w.clip == clip);
            if let (Some(start), Some(end)) = (
                self.between(prev, Some(&words[i])),
                self.between(Some(&words[j]), next),
            ) && start.at < end.at
            {
                out.push(Removal {
                    range: TimeRange::new(start.at, end.at),
                    tight: start.tight || end.tight,
                });
            }
            i = j + 1;
        }
        let merged = merge_ranges(out.iter().map(|r| r.range).collect());
        let mut merged: Vec<Removal> = merged
            .into_iter()
            .map(|range| Removal {
                range,
                tight: out.iter().any(|r| r.tight && r.range.overlaps(&range)),
            })
            .collect();
        merged.reverse();
        merged
    }

    /// The cuts either side of `words[first..=last]`, all in one clip: in
    /// the pause before the first (keeping it) and before the word after
    /// the last (taking the pause after them), as [`Speech::removals`]
    /// places them.
    pub fn around(&self, words: &[TimelineWord], first: usize, last: usize) -> Option<(Cut, Cut)> {
        let (a, b) = (words.get(first)?, words.get(last)?);
        let clip = a.clip;
        if b.clip != clip || first > last {
            return None;
        }
        let prev = words[..first].iter().rev().find(|w| w.clip == clip);
        let next = words[last + 1..].iter().find(|w| w.clip == clip);
        let start = self.between(prev, Some(a))?;
        let end = self.between(Some(b), next)?;
        (start.at < end.at).then_some((start, end))
    }

    /// Where a cut wanted at `t` should go when `t` falls on a word or too
    /// close to one: the nearer of the cuts before and after that word.
    /// `None` when `t` is clear of every word.
    pub fn snap(&self, words: &[TimelineWord], t: Time) -> Option<Snap> {
        let k = on_word(words, t)?;
        let w = &words[k];
        let prev = words[..k].iter().rev().find(|p| p.clip == w.clip);
        let next = words[k + 1..].iter().find(|n| n.clip == w.clip);
        let before = self.between(prev, Some(w))?;
        let after = self.between(Some(w), next)?;
        let cut = if (t - before.at).0.abs() <= (after.at - t).0.abs() {
            before
        } else {
            after
        };
        Some(Snap { cut, word: k })
    }

    /// The cut between two words of one clip; a missing one means the
    /// clip's edge.
    fn between(&self, prev: Option<&TimelineWord>, next: Option<&TimelineWord>) -> Option<Cut> {
        let clip = prev.or(next)?.clip;
        let bounds = self.project.timeline.clip(clip)?.1.range();
        let lo = prev
            .map_or(bounds.start, |w| w.timeline.end + TAIL)
            .max(bounds.start);
        let hi = next
            .map_or(bounds.end, |w| w.timeline.start - LEAD)
            .min(bounds.end);
        if lo <= hi {
            // Keep the pause before the next word; without one, cut just
            // after the last.
            let want = if next.is_some() { hi } else { lo };
            let at = self.best(clip, TimeRange::new(lo, hi), want, true);
            return Some(Cut { at, tight: false });
        }
        // No room clear of both: the quietest point between them.
        let a = prev.map_or(bounds.start, |w| w.timeline.end);
        let b = next.map_or(bounds.end, |w| w.timeline.start);
        let span = TimeRange::new(a.min(b), a.max(b));
        let want = Time(span.start.0 + span.duration().0 / 2);
        Some(Cut {
            at: self.best(clip, span, want, false),
            tight: true,
        })
    }

    /// The point of `span` (inclusive) for a cut wanted at `want`: quiet and
    /// close to `want`, from the frame boundaries in it when `frames` and
    /// there are any.
    fn best(&self, clip: ClipId, span: TimeRange, want: Time, frames: bool) -> Time {
        let rate = self.project.timeline.settings.rate;
        let mut candidates = Vec::new();
        if frames {
            let mut k = span.start.to_frame(rate);
            if Time::from_frame(k, rate) < span.start {
                k += 1;
            }
            while Time::from_frame(k, rate) <= span.end {
                candidates.push(Time::from_frame(k, rate));
                k += 1;
            }
        }
        if candidates.is_empty() {
            let mut t = span.start;
            while t < span.end {
                candidates.push(t);
                t = t + STEP;
            }
            candidates.push(span.end);
        }
        // The louder of the frames either side of each cut.
        let levels: Vec<f32> = candidates
            .iter()
            .map(|&t| {
                self.level(clip, t - STEP)
                    .into_iter()
                    .chain(self.level(clip, t))
                    .fold(-100.0, f32::max)
            })
            .collect();
        // Anything about as quiet as the quietest quarter is quiet enough:
        // room tone and digital silence are equally good places to cut,
        // a breath or a click is not.
        let mut sorted = levels.clone();
        sorted.sort_by(f32::total_cmp);
        let quiet = sorted.get(sorted.len() / 4).copied().unwrap_or(-100.0) + QUIET_DB;
        candidates
            .into_iter()
            .zip(levels)
            .map(|(t, level)| {
                let ms = (t - want).0.abs() as f32 * 1000.0 / FLICKS_PER_SECOND as f32;
                (t, (level - quiet).max(0.0) + DB_PER_MS * ms)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(want, |(t, _)| t)
    }

    /// dBFS at timeline time `t` in `clip`'s source.
    fn level(&self, clip: ClipId, t: Time) -> Option<f32> {
        let (_, c) = self.project.timeline.clip(clip)?;
        let levels = self.analyses.get(&c.media()?)?.levels.as_ref()?;
        levels.at(c.to_source(t)?)
    }
}
