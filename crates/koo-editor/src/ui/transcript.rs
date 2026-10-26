//! Transcript and captions (PRD E3, C1–C3): the words under the timeline in
//! timeline order, text-based ripple delete, caption generation, inline
//! caption edits and caption styles.

mod captions;
mod style;
mod words;

use crate::state::EditorState;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::captions::LOW_CONFIDENCE;
use koo_core::command::Command;
use koo_core::services::JobState;
use koo_core::{ClipId, Time, TimeRange};
use words::TimelineWord;

const POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// A pause this long starts a new paragraph.
const PARAGRAPH_GAP: Time = Time(705_600_000 * 3 / 2);

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Transcript,
    Captions,
}

pub struct TranscriptPanel {
    state: Entity<EditorState>,
    focus: FocusHandle,
    view: View,
    /// Recomputed when the project or analysis changes.
    words: Vec<TimelineWord>,
    words_key: Option<(u64, usize)>,
    /// Word clicked last; the selection runs from here to `head`.
    anchor: Option<usize>,
    head: Option<usize>,
    dragging: bool,
    /// Caption being edited inline, with its editor.
    editing: Option<ClipId>,
    editor: Entity<InputState>,
    font: Entity<InputState>,
    /// Demo only: open on generated captions.
    demo_captions: bool,
    _subscriptions: Vec<Subscription>,
    _poll: Task<()>,
}

impl TranscriptPanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| InputState::new(window, cx));
        let font = cx.new(|cx| InputState::new(window, cx).placeholder("Font"));
        let _subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.subscribe_in(&editor, window, |this, input, ev, window, cx| match ev {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    let text = input.read(cx).value().trim().to_owned();
                    this.finish_edit(Some(text), window, cx);
                }
                _ => {}
            }),
            cx.subscribe_in(&font, window, |this, input, ev, _, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let font = input.read(cx).value().trim().to_owned();
                    if !font.is_empty() {
                        this.edit_style(|s| s.font = font, cx);
                    }
                }
            }),
        ];
        let _poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let alive = this.update(cx, |this, cx| {
                    if this.words_key != this.current_key(cx) {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        Self {
            state,
            focus: cx.focus_handle(),
            view: View::Transcript,
            words: Vec::new(),
            words_key: None,
            anchor: None,
            head: None,
            dragging: false,
            editing: None,
            editor,
            font,
            demo_captions: crate::ui::media_pool::demo::enabled()
                && std::env::var("KOOEDITOR_DEMO_VIEW").is_ok_and(|v| v == "captions"),
            _subscriptions,
            _poll,
        }
    }

    /// Changes with the project and whenever an analysis job finishes, since
    /// new transcripts arrive without a project change.
    fn current_key(&self, cx: &App) -> Option<(u64, usize)> {
        let state = self.state.read(cx);
        let analysis = state.services.analysis.as_ref()?;
        let done = analysis
            .jobs()
            .iter()
            .filter(|j| j.state == JobState::Done)
            .count();
        Some((state.bus.version(), done))
    }

    fn refresh_words(&mut self, cx: &App) {
        let key = self.current_key(cx);
        if key == self.words_key {
            return;
        }
        self.words_key = key;
        let state = self.state.read(cx);
        self.words = match &state.services.analysis {
            Some(analysis) => words::timeline_words(state.bus.project(), analysis.as_ref()),
            None => Vec::new(),
        };
        let n = self.words.len();
        self.anchor = self.anchor.filter(|&i| i < n);
        self.head = self.head.filter(|&i| i < n);
    }

    fn selection(&self) -> Option<(usize, usize)> {
        let (a, h) = (self.anchor?, self.head?);
        Some((a.min(h), a.max(h)))
    }

    fn publish_selection(&self, cx: &mut Context<Self>) {
        let words = self.selection().and_then(|(a, b)| {
            let (first, last) = (&self.words[a], &self.words[b]);
            (first.media == last.media).then(|| {
                (
                    first.media,
                    TimeRange::new(first.source.start, last.source.end),
                )
            })
        });
        self.state.update(cx, |s, cx| s.select_words(words, cx));
    }

    fn word_down(
        &mut self,
        i: usize,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        if ev.modifiers.shift && self.anchor.is_some() {
            self.head = Some(i);
        } else if ev.click_count >= 2 {
            self.anchor = Some(i);
            self.head = Some(i);
        } else {
            self.anchor = Some(i);
            self.head = None;
            self.dragging = true;
            let t = self.words[i].range.start;
            self.state.update(cx, |s, cx| s.seek(t, cx));
        }
        self.publish_selection(cx);
        cx.notify();
    }

    fn word_drag(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.dragging && self.anchor.is_some() && self.head != Some(i) {
            if self.anchor == Some(i) && self.head.is_none() {
                return;
            }
            self.head = Some(i);
            cx.notify();
        }
    }

    fn drag_end(&mut self, cx: &mut Context<Self>) {
        if self.dragging {
            self.dragging = false;
            self.publish_selection(cx);
        }
    }

    /// Ripple-delete the selected words' timeline ranges as one undo step.
    fn delete_selection(&mut self, cx: &mut Context<Self>) {
        let Some((a, b)) = self.selection() else {
            return;
        };
        let commands: Vec<Command> = words::delete_ranges(&self.words, a, b)
            .into_iter()
            .map(|range| Command::RippleDelete {
                range,
                tracks: None,
            })
            .collect();
        if commands.is_empty() {
            return;
        }
        let at = self.words[a].range.start;
        self.anchor = None;
        self.head = None;
        self.state.update(cx, |s, cx| {
            if s.execute_batch(commands, "delete_words", cx).is_some() {
                s.seek(at, cx);
            }
            s.select_words(None, cx);
        });
    }

    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        match ev.keystroke.key.as_str() {
            "delete" | "backspace" => self.delete_selection(cx),
            "escape" => {
                self.anchor = None;
                self.head = None;
                self.publish_selection(cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    // ------------------------------------------------------------- render

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = match self.view {
            View::Transcript => 0,
            View::Captions => 1,
        };
        h_flex()
            .p_2()
            .gap_2()
            .child(
                TabBar::new("transcript-view")
                    .segmented()
                    .small()
                    .selected_index(selected)
                    .child(Tab::new().label("Transcript"))
                    .child(Tab::new().label("Captions"))
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.view = if *ix == 0 {
                            View::Transcript
                        } else {
                            View::Captions
                        };
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .child(self.generate_button(cx))
    }

    fn transcript_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, selection, accent, warn) = (
            theme.muted_foreground,
            theme.selection,
            theme.accent,
            theme.yellow,
        );
        if self.words.is_empty() {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(muted)
                .child("No transcript")
                .into_any_element();
        }
        let playhead = self.state.read(cx).playhead;
        let sel = self.selection();
        let mut paragraphs: Vec<Div> = Vec::new();
        let mut line: Vec<AnyElement> = Vec::new();
        let mut label: Option<String> = None;
        let flush =
            |paragraphs: &mut Vec<Div>, line: &mut Vec<AnyElement>, label: Option<String>| {
                if line.is_empty() {
                    return;
                }
                paragraphs.push(
                    v_flex()
                        .gap_0p5()
                        .children(label.map(|l| {
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(muted)
                                .child(l)
                        }))
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_x(px(4.))
                                .children(std::mem::take(line)),
                        ),
                );
            };
        let mut prev: Option<&TimelineWord> = None;
        let mut prev_speaker = None;
        for (i, w) in self.words.iter().enumerate() {
            let speaker_change = prev.is_some_and(|_| w.speaker != prev_speaker);
            let pause = prev.is_some_and(|p| w.range.start - p.range.end > PARAGRAPH_GAP);
            if prev.is_none() || speaker_change || pause {
                flush(&mut paragraphs, &mut line, label.take());
                label = if prev.is_none() || speaker_change {
                    w.speaker_label.clone()
                } else {
                    None
                };
            }
            prev = Some(w);
            prev_speaker = w.speaker;
            let selected = sel.is_some_and(|(a, b)| a <= i && i <= b);
            let current = w.range.contains(playhead);
            let low = w.confidence < LOW_CONFIDENCE;
            line.push(
                div()
                    .id(("word", i))
                    .px(px(2.5))
                    .cursor_text()
                    .when(selected, |d| d.bg(selection))
                    .when(current && !selected, |d| d.bg(accent))
                    .when(low, |d| {
                        d.underline()
                            .text_decoration_wavy()
                            .text_decoration_color(warn)
                    })
                    .child(w.text.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                            this.word_down(i, ev, window, cx)
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                        if ev.pressed_button == Some(MouseButton::Left) {
                            this.word_drag(i, cx);
                        }
                    }))
                    .into_any_element(),
            );
        }
        flush(&mut paragraphs, &mut line, label);
        div()
            .id("transcript-words")
            .flex_1()
            .min_h_0()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.drag_end(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.drag_end(cx)),
            )
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .px_3()
                    .pb_3()
                    .gap_3()
                    .text_sm()
                    .line_height(rems(1.6))
                    .children(paragraphs),
            )
            .into_any_element()
    }
}

impl Render for TranscriptPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh_words(cx);
        if self.demo_captions && !self.words.is_empty() {
            self.demo_captions = false;
            self.generate(cx);
        }
        let body = match self.view {
            View::Transcript => self.transcript_view(cx),
            View::Captions => self.captions_view(window, cx),
        };
        v_flex()
            .size_full()
            .key_context("Transcript")
            .child(self.header(cx))
            .child(body)
    }
}
