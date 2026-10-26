//! Caption generation (C1) and the captions list with inline edits (C3).

use super::TranscriptPanel;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::captions::{CaptionRules, LOW_CONFIDENCE, group_words};
use koo_core::command::Command;
use koo_core::model::{ClipContent, Project, Track, TrackKind};
use koo_core::{ClipId, TimeRange};

impl TranscriptPanel {
    pub(super) fn generate_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("generate-captions")
            .small()
            .label("Generate captions")
            .disabled(self.words.is_empty())
            .on_click(cx.listener(|this, _, _, cx| this.generate(cx)))
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

    pub(super) fn captions_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.refresh_captions(cx);
        let body = if self.captions.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("No captions")
                .into_any_element()
        } else {
            div()
                .id("captions-list")
                .flex_1()
                .min_h_0()
                .child(
                    list(
                        self.caption_list.clone(),
                        cx.processor(|this, ix: usize, _, cx| this.caption_row(ix, cx)),
                    )
                    .size_full(),
                )
                .vertical_scrollbar(&self.caption_list)
                .into_any_element()
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .child(body)
            .child(self.style_view(window, cx))
            .into_any_element()
    }

    fn caption_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.captions.get(ix) else {
            return div().into_any_element();
        };
        let (id, range) = (row.id, row.range);
        let playhead = self.state.read(cx).playhead;
        let theme = cx.theme();
        let (muted, active, hover, warn) = (
            theme.muted_foreground,
            theme.list_active,
            theme.list_hover,
            theme.yellow,
        );
        let current = range.contains(playhead);
        let time = h_flex()
            .w(px(60.))
            .flex_none()
            .gap_1()
            .text_color(muted)
            .child(
                div()
                    .size(px(5.))
                    .rounded_full()
                    .when(row.low, |d| d.bg(warn)),
            )
            .child(crate::ui::media_pool::clock(range.start));
        let item = if self.editing == Some(id) {
            h_flex()
                .id(("caption", ix))
                .min_h(px(28.))
                .px_1()
                .gap_2()
                .text_xs()
                .child(time)
                .child(div().flex_1().child(Input::new(&self.editor).xsmall()))
        } else {
            let edit_text = row.text.to_string();
            h_flex()
                .id(("caption", ix))
                .min_h(px(28.))
                .px_1()
                .py_1()
                .gap_2()
                .items_start()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .when(current, |d| d.bg(active))
                .when(!current, |d| d.hover(|s| s.bg(hover)))
                .child(time)
                .child(div().flex_1().min_w_0().child(row.text.clone()))
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    if ev.click_count() >= 2 {
                        this.start_edit(id, edit_text.clone(), window, cx);
                    } else {
                        this.state.update(cx, |s, cx| s.seek(range.start, cx));
                    }
                }))
        };
        div().px_2().child(item).into_any_element()
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
