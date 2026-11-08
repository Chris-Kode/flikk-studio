//! Silence removal (PRD E4): gaps from `AnalysisService::silences`, mapped
//! from source to timeline time through the clips, kept only where every
//! audible clip is silent, then removed as one undo step. The toolbar's
//! [`SilenceTool`] sets the thresholds and shows what they find.

use super::TimelinePanel;
use crate::ui::kit::{Button, IconName, Slider, Spinner, h_flex, theme as t, v_flex};
use flikk_core::command::Command;
use flikk_core::cuts::FADE;
use flikk_core::model::{Project, TrackKind};
use flikk_core::services::AnalysisService;
use flikk_core::{MediaId, Time, TimeRange};
use gpui_kit::base::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a failed analysis waits before it is asked again.
const RETRY: Duration = Duration::from_secs(30);

/// The toolbar's silence tool: whether its popover is open, its two
/// thresholds and the source silences found so far.
pub struct SilenceTool {
    /// The popover is open: silences are requested, painted on the lanes
    /// and listed in the popover.
    pub open: bool,
    /// Shortest gap that counts, in seconds.
    min: Entity<SliderState>,
    /// Sound kept on each side of a gap, in seconds.
    padding: Entity<SliderState>,
    cache: SilenceCache,
    _sliders: [Subscription; 2],
}

impl SilenceTool {
    /// Closed, with the thresholds at their defaults. The panel renders
    /// again whenever a threshold moves.
    pub fn new(cx: &mut Context<TimelinePanel>) -> Self {
        let min = cx.new(|_| {
            SliderState::new()
                .min(0.2)
                .max(3.0)
                .step(0.1)
                .default_value(0.6)
        });
        let padding = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(0.5)
                .step(0.02)
                .default_value(0.1)
        });
        let _sliders = [
            cx.subscribe(&min, |_, _, _: &SliderEvent, cx| cx.notify()),
            cx.subscribe(&padding, |_, _, _: &SliderEvent, cx| cx.notify()),
        ];
        Self {
            open: false,
            min,
            padding,
            cache: SilenceCache::default(),
            _sliders,
        }
    }

    /// Opens or closes the popover. Opening forgets failed analyses, so
    /// they are asked for again.
    pub fn set_open(&mut self, open: bool) {
        if open && !self.open {
            self.cache.retry_failed();
        }
        self.open = open;
    }

    /// The minimum length and the padding.
    fn params(&self, cx: &App) -> (Time, Time) {
        let value = |s: &Entity<SliderState>| match s.read(cx).value() {
            SliderValue::Single(v) | SliderValue::Range(_, v) => v as f64,
        };
        (
            Time::from_secs_f64(value(&self.min)),
            Time::from_secs_f64(value(&self.padding)),
        )
    }

    /// Timeline ranges the thresholds remove from `project`, from the
    /// silences known so far.
    pub fn found(&self, project: &Project, cx: &App) -> Vec<TimeRange> {
        let (min, pad) = self.params(cx);
        self.cache.timeline_silences(project, min, pad)
    }
}

impl TimelinePanel {
    pub(super) fn apply_silences(&mut self, cx: &mut Context<Self>) {
        let ranges = self.silence.found(self.state.read(cx).bus.project(), cx);
        if ranges.is_empty() {
            return;
        }
        self.silence.open = false;
        let commands = removal_commands(&ranges);
        self.state.update(cx, |s, cx| {
            s.edit_batch(commands, "Remove silences", cx);
        });
        cx.notify();
    }

    /// Ask the analysis engine for source silences we don't have yet.
    pub(super) fn request_silences(&mut self, cx: &mut Context<Self>) {
        let Some(analysis) = self.state.read(cx).services.analysis.clone() else {
            return;
        };
        let (min, _) = self.silence.params(cx);
        let missing = {
            let project = self.state.read(cx).bus.project().clone();
            self.silence.cache.missing(&project, min)
        };
        for item in missing {
            let analysis = analysis.clone();
            cx.spawn(async move |this, cx| {
                let id = item.id;
                let found = cx
                    .background_executor()
                    .spawn(async move { detect(analysis.as_ref(), &item, min) })
                    .await;
                let failed = found.is_none();
                let _ = this.update(cx, |this, cx| {
                    this.silence.cache.insert(min, id, found);
                    cx.notify();
                });
                if failed {
                    // Render again once it may be asked for again.
                    cx.background_executor().timer(RETRY).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.silence.open {
                            cx.notify();
                        }
                    });
                }
            })
            .detach();
        }
    }
}

/// The silence tool's popover: thresholds, what they find, and Remove.
pub(super) fn silence_content(panel: &Entity<TimelinePanel>, cx: &mut App) -> AnyElement {
    let this = panel.read(cx);
    let close = {
        let panel = panel.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            panel.update(cx, |this, cx| {
                this.silence.open = false;
                cx.notify();
            })
        }
    };
    let card = v_flex()
        .w(px(236.))
        .px(px(12.))
        .py(px(10.))
        .gap(px(9.))
        .child(
            h_flex()
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::MEDIUM)
                        .child("Remove silences"),
                )
                .child(
                    Button::new("silence-close")
                        .ghost()
                        .xsmall()
                        .icon(IconName::X)
                        .on_click(close),
                ),
        );
    if this.state.read(cx).services.analysis.is_none() {
        return card
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(t::T2)
                    .child("Analysis is not available"),
            )
            .into_any_element();
    }
    let (min, pad) = this.silence.params(cx);
    let found = this.silence.found(this.state.read(cx).bus.project(), cx);
    let total: f64 = found.iter().map(|r| r.duration().as_secs_f64()).sum();
    let loading = this.silence.cache.is_loading();
    let failed = this.silence.cache.has_failed();
    let row = |label: &'static str, slider: &Entity<SliderState>, value: String| {
        h_flex()
            .gap(px(8.))
            .text_size(px(11.))
            .child(div().w(px(62.)).flex_none().text_color(t::T2).child(label))
            .child(Slider::new(slider).flex_1())
            .child(
                div()
                    .w(px(44.))
                    .flex_none()
                    .text_right()
                    .font_family(t::mono())
                    .child(value),
            )
    };
    let summary = if loading {
        h_flex()
            .gap(px(6.))
            .text_color(t::T2)
            .child(Spinner::new().size(px(11.)))
            .child("Analyzing…")
    } else if found.is_empty() && failed {
        h_flex().text_color(t::T3).child("Analysis failed")
    } else if found.is_empty() {
        h_flex().text_color(t::T3).child("Nothing found")
    } else {
        h_flex()
            .font_family(t::mono())
            .text_color(t::T2)
            .child(format!("{} regions · ", found.len()))
            .child(div().text_color(t::ERR_TX).child(format!("−{total:.1} s")))
    };
    let panel = panel.clone();
    card.child(row(
        "Min length",
        &this.silence.min,
        format!("{:.1} s", min.as_secs_f64()),
    ))
    .child(row(
        "Padding",
        &this.silence.padding,
        format!("{:.2} s", pad.as_secs_f64()),
    ))
    .child(
        h_flex()
            .gap(px(8.))
            .pt(px(9.))
            .border_t_1()
            .border_color(t::LN)
            .text_size(px(11.))
            .child(summary.flex_1())
            .child(
                Button::new("apply-silences")
                    .primary()
                    .small()
                    .label("Remove")
                    .disabled(found.is_empty())
                    .on_click(move |_, _, cx| panel.update(cx, |this, cx| this.apply_silences(cx))),
            ),
    )
    .into_any_element()
}

/// One media item's silences.
enum Silences {
    InFlight,
    /// The analysis failed then; asked again after [`RETRY`].
    Failed(Instant),
    Ready(Vec<TimeRange>),
}

/// Source silences per media item, for one minimum length.
#[derive(Default)]
struct SilenceCache {
    min: Time,
    by_media: HashMap<MediaId, Silences>,
}

impl SilenceCache {
    /// Media whose silences for `min` are not known or requested yet, or
    /// failed over [`RETRY`] ago. Marks them as requested.
    fn missing(&mut self, project: &Project, min: Time) -> Vec<flikk_core::model::MediaItem> {
        if self.min != min {
            self.min = min;
            self.by_media.clear();
        }
        let mut out = Vec::new();
        for clip in sound_clips(project) {
            let Some(id) = clip.1 else { continue };
            match self.by_media.get(&id) {
                Some(Silences::Failed(at)) if at.elapsed() >= RETRY => {}
                Some(_) => continue,
                None => {}
            }
            self.by_media.insert(id, Silences::InFlight);
            if let Some(item) = project.media(id) {
                out.push(item.clone());
            }
        }
        out
    }

    /// The result for `media`; `None` when the analysis failed.
    fn insert(&mut self, min: Time, media: MediaId, ranges: Option<Vec<TimeRange>>) {
        if min == self.min {
            let silences = match ranges {
                Some(r) => Silences::Ready(r),
                None => Silences::Failed(Instant::now()),
            };
            self.by_media.insert(media, silences);
        }
    }

    /// Forget failures, so the next [`Self::missing`] asks for them again.
    fn retry_failed(&mut self) {
        self.by_media
            .retain(|_, s| !matches!(s, Silences::Failed(_)));
    }

    fn is_loading(&self) -> bool {
        self.by_media
            .values()
            .any(|s| matches!(s, Silences::InFlight))
    }

    fn has_failed(&self) -> bool {
        self.by_media
            .values()
            .any(|s| matches!(s, Silences::Failed(_)))
    }

    /// Timeline ranges to remove: inside clip coverage, silent on every
    /// audible clip, at least `min` long after shrinking by `padding` per side.
    fn timeline_silences(&self, project: &Project, min: Time, padding: Time) -> Vec<TimeRange> {
        let mut covered = Vec::new();
        let mut sound = Vec::new();
        for (clip, media) in sound_clips(project) {
            covered.push(clip.range());
            let silences = media.and_then(|m| match self.by_media.get(&m) {
                Some(Silences::Ready(s)) => Some(s),
                _ => None,
            });
            let Some(silences) = silences else {
                // Unknown or failed: treat the whole clip as sound.
                sound.push(clip.range());
                continue;
            };
            if clip.media().is_none() {
                continue;
            }
            let mut quiet: Vec<TimeRange> = silences
                .iter()
                .filter_map(|s| clip.source_to_timeline_range(*s).intersect(&clip.range()))
                .collect();
            quiet.sort_by_key(|r| r.start);
            sound.extend(subtract(clip.range(), &quiet));
        }
        let covered = union(covered);
        let sound = union(sound);
        let mut out = Vec::new();
        for c in covered {
            for gap in subtract(c, &sound) {
                let r = TimeRange::new(gap.start + padding, gap.end - padding);
                if gap.duration() >= min && !r.is_empty() {
                    out.push(r);
                }
            }
        }
        out
    }
}

/// `RippleDelete`s for `ranges`, latest first so earlier ranges stay valid.
fn removal_commands(ranges: &[TimeRange]) -> Vec<Command> {
    let mut ranges = ranges.to_vec();
    ranges.sort_by_key(|r| std::cmp::Reverse(r.start));
    ranges
        .into_iter()
        .map(|range| Command::RippleDelete {
            range,
            tracks: None,
            fade: FADE,
        })
        .collect()
}

/// Run on a background thread.
fn detect(
    analysis: &dyn AnalysisService,
    item: &flikk_core::model::MediaItem,
    min: Time,
) -> Option<Vec<TimeRange>> {
    analysis.silences(item, min).ok()
}

/// Media clips whose sound decides silence: audio tracks, or video tracks
/// when the timeline has no audio clips.
fn sound_clips(project: &Project) -> Vec<(&flikk_core::model::Clip, Option<MediaId>)> {
    let pick = |kind: TrackKind| -> Vec<_> {
        project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == kind && !t.mix.mute)
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.media().is_some())
            .map(|c| (c, c.media()))
            .collect()
    };
    let audio = pick(TrackKind::Audio);
    if audio.is_empty() {
        pick(TrackKind::Video)
    } else {
        audio
    }
}

fn union(mut ranges: Vec<TimeRange>) -> Vec<TimeRange> {
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<TimeRange> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// `range` minus sorted, non-overlapping `holes`.
fn subtract(range: TimeRange, holes: &[TimeRange]) -> Vec<TimeRange> {
    let mut out = Vec::new();
    let mut cursor = range.start;
    for h in holes {
        if h.end <= cursor || h.start >= range.end {
            continue;
        }
        if h.start > cursor {
            out.push(TimeRange::new(cursor, h.start));
        }
        cursor = cursor.max(h.end);
    }
    if cursor < range.end {
        out.push(TimeRange::new(cursor, range.end));
    }
    out
}
