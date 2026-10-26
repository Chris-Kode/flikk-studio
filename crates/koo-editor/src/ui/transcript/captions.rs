//! Caption generation (C1) and the captions list with inline edits (C3).

use super::TranscriptPanel;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::ClipId;
use koo_core::captions::{CaptionRules, LOW_CONFIDENCE, group_words};
use koo_core::command::Command;
use koo_core::model::{ClipContent, Project, Track, TrackKind};

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
        let words: Vec<_> = self.words.iter().map(|w| w.to_word()).collect();
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
        let mut commands = Vec::with_capacity(phrases.len() + 1);
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

    pub(super) fn captions_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let project = self.state.read(cx).bus.project();
        let playhead = self.state.read(cx).playhead;
        let rows: Vec<_> = caption_track(project)
            .map(|t| {
                t.clips
                    .iter()
                    .filter_map(|c| match &c.content {
                        ClipContent::Caption { text, .. } => Some((c.id, c.range(), text.clone())),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let theme = cx.theme();
        let (muted, active, hover, warn) = (
            theme.muted_foreground,
            theme.list_active,
            theme.list_hover,
            theme.yellow,
        );
        let empty = rows.is_empty();
        let list = rows.into_iter().enumerate().map(|(i, (id, range, text))| {
            let low = self
                .words
                .iter()
                .any(|w| w.confidence < LOW_CONFIDENCE && w.range.overlaps(&range));
            let current = range.contains(playhead);
            let time = h_flex()
                .w(px(60.))
                .flex_none()
                .gap_1()
                .text_color(muted)
                .child(div().size(px(5.)).rounded_full().when(low, |d| d.bg(warn)))
                .child(crate::ui::media_pool::clock(range.start));
            if self.editing == Some(id) {
                return h_flex()
                    .id(("caption", i))
                    .min_h(px(28.))
                    .px_1()
                    .gap_2()
                    .text_xs()
                    .child(time)
                    .child(div().flex_1().child(Input::new(&self.editor).xsmall()))
                    .into_any_element();
            }
            let edit_text = text.clone();
            h_flex()
                .id(("caption", i))
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
                .child(div().flex_1().min_w_0().child(text))
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    if ev.click_count() >= 2 {
                        this.start_edit(id, edit_text.clone(), window, cx);
                    } else {
                        this.state.update(cx, |s, cx| s.seek(range.start, cx));
                    }
                }))
                .into_any_element()
        });
        let list: Vec<AnyElement> = list.collect();
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("captions-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .when(empty, |d| {
                        d.flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .text_color(muted)
                            .child("No captions")
                    })
                    .when(!empty, |d| d.child(v_flex().px_2().pb_2().children(list))),
            )
            .child(self.style_view(window, cx))
            .into_any_element()
    }
}

/// The track captions are generated into: the first caption track.
fn caption_track(project: &Project) -> Option<&Track> {
    project
        .timeline
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Caption)
}
