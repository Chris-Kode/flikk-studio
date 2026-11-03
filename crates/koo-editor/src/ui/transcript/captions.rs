//! Caption generation (C1) and the captions list with inline edits (C3).

use super::{TranscriptPanel, footer};
use crate::ui::kit::theme as t;
use crate::ui::kit::{Button, EmptyState, IconName, Input, ScrollableElement as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::captions::{CaptionRules, LOW_CONFIDENCE, group_words};
use koo_core::command::Command;
use koo_core::model::{ClipContent, Project, Track, TrackKind};
use koo_core::{ClipId, TimeRange};

impl TranscriptPanel {
    /// How many captions there are, and generating them anew.
    pub(super) fn captions_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let track = caption_track(self.state.read(cx).bus.project()).map(|t| t.name.clone());
        let count = (!self.captions.is_empty()).then(|| {
            h_flex()
                .gap(px(4.))
                .whitespace_nowrap()
                .child(
                    div()
                        .font_family(t::mono())
                        .text_color(t::T1)
                        .child(self.captions.len().to_string()),
                )
                .child(match track {
                    Some(name) => format!("captions · {name}"),
                    None => "captions".into(),
                })
        });
        footer()
            .children(count)
            .child(div().flex_1())
            .child(
                Button::new("generate-captions")
                    .small()
                    .label("Generate captions")
                    .disabled(self.words.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.generate(cx))),
            )
            .into_any_element()
    }

    /// Replace the caption track's clips with phrases grouped from the
    /// transcript, in one batch. A caption track is added first if missing.
    pub(super) fn generate(&mut self, cx: &mut Context<Self>) {
        if self.words.is_empty() {
            return;
        }
        let style = self.state.read(cx).active_caption_style();
        let words: Vec<_> = self.words.iter().map(super::words::to_word).collect();
        let phrases = group_words(&words, &CaptionRules::from_style(&style));
        if caption_track(self.state.read(cx).bus.project()).is_none() {
            let added = self.state.update(cx, |s, cx| {
                s.execute(
                    Command::AddTrack {
                        kind: TrackKind::Caption,
                        index: 0,
                        name: "C1".into(),
                    },
                    cx,
                )
            });
            if added.is_none() {
                return;
            }
        }
        let Some(track) = caption_track(self.state.read(cx).bus.project()) else {
            return;
        };
        let existing: Vec<ClipId> = track.clips.iter().map(|c| c.id).collect();
        let mut commands = Vec::with_capacity(phrases.len() + 2);
        if track.caption_style.as_deref() != Some(style.name.as_str()) {
            commands.push(Command::SetCaptionStyle {
                track: track.id,
                style: Some(style.name.clone()),
            });
        }
        if !existing.is_empty() {
            commands.push(Command::DeleteClips {
                clips: existing,
                ripple: false,
            });
        }
        commands.extend(phrases.into_iter().map(|p| Command::AddCaption {
            track: track.id,
            range: p.range,
            words: p.word_times(&words),
            text: p.text,
            speaker: p.speaker,
        }));
        self.state.update(cx, |s, cx| {
            s.execute_batch(commands, "generate_captions", cx);
        });
        self.view = super::View::Captions;
        cx.notify();
    }

    fn start_edit(
        &mut self,
        clip: ClipId,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editing = Some(clip);
        self.editor.update(cx, |e, cx| {
            e.set_value(text, window, cx);
            e.focus(window, cx);
        });
        cx.notify();
    }

    /// Commit (`Some`) or drop the inline edit.
    pub(super) fn finish_edit(
        &mut self,
        text: Option<String>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(clip) = self.editing.take() else {
            return;
        };
        let current = self
            .state
            .read(cx)
            .bus
            .project()
            .timeline
            .clip(clip)
            .and_then(|(_, c)| match &c.content {
                ClipContent::Caption { text, .. } => Some(text.clone()),
                _ => None,
            });
        if let (Some(text), Some(current)) = (text, current)
            && !text.is_empty()
            && text != current
        {
            self.state.update(cx, |s, cx| {
                s.execute(
                    Command::EditCaption {
                        clip,
                        text: Some(text),
                        range: None,
                    },
                    cx,
                );
            });
        }
        cx.notify();
    }

    /// Rebuilds the cached caption rows when the project or transcript
    /// changed; keeps the scroll position when the row count holds.
    pub(super) fn refresh_captions(&mut self, cx: &App) {
        let key = (
            self.state.read(cx).bus.version(),
            self.words_key,
            self.editing,
        );
        if self.captions_key == Some(key) {
            return;
        }
        self.captions_key = Some(key);
        let project = self.state.read(cx).bus.project();
        let rows: Vec<CaptionRow> = caption_track(project)
            .map(|t| {
                t.clips
                    .iter()
                    .filter_map(|c| match &c.content {
                        ClipContent::Caption { text, .. } => Some(CaptionRow {
                            id: c.id,
                            range: c.range(),
                            text: text.clone().into(),
                            low: self.low_confidence(c.range()),
                        }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        if rows.len() == self.captions.len() {
            self.caption_list.remeasure_items(0..rows.len());
        } else {
            self.caption_list.reset(rows.len());
        }
        self.captions = rows;
    }

    /// Whether any word under `range` was recognized with low confidence.
    fn low_confidence(&self, range: TimeRange) -> bool {
        // Words are in timeline order: scan only those near the caption.
        let from = self
            .words
            .partition_point(|w| w.timeline.start < range.start)
            .saturating_sub(1);
        self.words[from..]
            .iter()
            .take_while(|w| w.timeline.start < range.end)
            .any(|w| w.word.confidence < LOW_CONFIDENCE && w.timeline.overlaps(&range))
    }

    pub(super) fn captions_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.refresh_captions(cx);
        if self.captions.is_empty() {
            return EmptyState::new(IconName::Captions, "No captions").into_any_element();
        }
        div()
            .id("captions-list")
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                list(
                    self.caption_list.clone(),
                    cx.processor(|this, ix: usize, _, cx| this.caption_row(ix, cx)),
                )
                .size_full()
                .py(px(6.)),
            )
            .vertical_scrollbar(&self.caption_list)
            .into_any_element()
    }

    fn caption_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.captions.get(ix) else {
            return div().into_any_element();
        };
        let (id, range) = (row.id, row.range);
        let current = range.contains(self.state.read(cx).playhead);
        let time = h_flex()
            .w(px(60.))
            .h(px(16.))
            .flex_none()
            .gap(px(5.))
            .font_family(t::mono())
            .text_size(px(11.))
            .text_color(if current { t::T1 } else { t::T3 })
            .child(
                div()
                    .size(px(5.))
                    .flex_none()
                    .rounded_full()
                    .when(row.low, |d| d.bg(t::WARN)),
            )
            .child(crate::ui::media_pool::clock(range.start));
        let item = h_flex()
            .id(("caption", ix))
            .min_h(px(28.))
            .px(px(6.))
            .gap(px(8.))
            .rounded(t::RADIUS)
            .text_size(px(12.))
            .line_height(px(16.));
        let item = if self.editing == Some(id) {
            item.child(time)
                .child(div().flex_1().child(Input::new(&self.editor).small()))
        } else {
            let edit_text = row.text.to_string();
            item.py(px(6.))
                .items_start()
                .cursor_pointer()
                .when(current, |d| d.bg(t::HV))
                .when(!current, |d| d.hover(|s| s.bg(t::R1)))
                .child(time)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(if current {
                            t::ON_AC
                        } else {
                            t::alpha(t::T1, 0.8)
                        })
                        .child(row.text.clone()),
                )
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    if ev.click_count() >= 2 {
                        this.start_edit(id, edit_text.clone(), window, cx);
                    } else {
                        this.state.update(cx, |s, cx| s.seek(range.start, cx));
                    }
                }))
        };
        div().px(px(8.)).pb(px(1.)).child(item).into_any_element()
    }
}

/// A caption clip as listed, cached between renders.
pub(super) struct CaptionRow {
    id: ClipId,
    range: TimeRange,
    text: SharedString,
    low: bool,
}

/// The track captions are generated into: the first caption track.
fn caption_track(project: &Project) -> Option<&Track> {
    project
        .timeline
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Caption)
}
