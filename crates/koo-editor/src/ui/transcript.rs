//! Transcript and captions (PRD E3, C1–C3): the words under the timeline in
//! timeline order, text-based ripple delete, caption generation and inline
//! caption edits. Caption styles have their own panel (`caption_style`).
//! A speaker's words can go to the Voices dialog: as a recording of them, to
//! find shared voices like theirs or to clone it.

mod captions;
mod words;

use crate::state::EditorState;
use crate::ui::RefDrag;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::captions::LOW_CONFIDENCE;
use koo_core::command::Command;
use koo_core::cuts::FADE;
use koo_core::query::TimelineWord;
use koo_core::services::JobState;
use koo_core::{ClipId, Time, TimeRange};
use std::collections::HashMap;
use std::ops::Range;

const POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// A pause this long starts a new paragraph.
const PARAGRAPH_GAP: Time = Time(705_600_000 * 3 / 2);
/// Past this many words a paragraph breaks at the next sentence end, and at
/// twice as many regardless, so each list item stays small.
const PARAGRAPH_WORDS: usize = 80;

/// Project version and finished analysis jobs.
type WordsKey = (u64, usize);

/// A run of words drawn as one list item, with its speaker label.
struct Paragraph {
    words: Range<usize>,
    label: Option<String>,
}

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
    speakers: HashMap<koo_core::SpeakerId, String>,
    words_key: Option<WordsKey>,
    paragraphs: Vec<Paragraph>,
    /// Only the visible paragraphs are laid out.
    list: ListState,
    /// Word clicked last; the selection runs from here to `head`.
    anchor: Option<usize>,
    head: Option<usize>,
    dragging: bool,
    /// A word inside the selection pressed without dragging yet: a click
    /// there selects it alone, a drag takes the selection to the chat.
    pressed_in_selection: Option<usize>,
    /// Caption being edited inline, with its editor.
    editing: Option<ClipId>,
    editor: Entity<InputState>,
    /// Caption rows, rebuilt when this key changes.
    captions: Vec<captions::CaptionRow>,
    captions_key: Option<(u64, Option<WordsKey>, Option<ClipId>)>,
    caption_list: ListState,
    /// Demo only: open on generated captions.
    demo_captions: bool,
    _subscriptions: Vec<Subscription>,
    _poll: Task<()>,
}

impl TranscriptPanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| InputState::new(window, cx));
        let _subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.subscribe_in(&editor, window, |this, input, ev, window, cx| match ev {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    let text = input.read(cx).value().trim().to_owned();
                    this.finish_edit(Some(text), window, cx);
                }
                _ => {}
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
            speakers: HashMap::new(),
            words_key: None,
            paragraphs: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            anchor: None,
            head: None,
            dragging: false,
            pressed_in_selection: None,
            editing: None,
            editor,
            captions: Vec::new(),
            captions_key: None,
            caption_list: ListState::new(0, ListAlignment::Top, px(400.)),
            demo_captions: crate::demo::enabled()
                && std::env::var("KOOEDITOR_DEMO_VIEW").is_ok_and(|v| v == "captions"),
            _subscriptions,
            _poll,
        }
    }

    /// Changes with the project and whenever an analysis job finishes, since
    /// new transcripts arrive without a project change.
    fn current_key(&self, cx: &App) -> Option<WordsKey> {
        let state = self.state.read(cx);
        let analysis = state.services.analysis.as_ref()?;
        let done = analysis
            .jobs()
            .iter()
            .filter(|j| j.state == JobState::Done)
            .count();
        Some((state.bus.version(), done))
    }

    fn refresh_words(&mut self, cx: &mut App) {
        let key = self.current_key(cx);
        if key == self.words_key {
            return;
        }
        self.words_key = key;
        self.state.update(cx, |s, _| s.sync_caption_style());
        let state = self.state.read(cx);
        (self.words, self.speakers) = match &state.services.analysis {
            Some(analysis) => words::timeline_words(state.bus.project(), analysis.as_ref()),
            None => Default::default(),
        };
        self.paragraphs = paragraphs(&self.words, &self.speakers);
        if self.list.item_count() == self.paragraphs.len() {
            self.list.remeasure_items(0..self.paragraphs.len());
        } else {
            self.list.reset(self.paragraphs.len());
        }
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
        let inside = self.selection().is_some_and(|(a, b)| a <= i && i <= b);
        if inside && !ev.modifiers.shift && ev.click_count == 1 {
            self.pressed_in_selection = Some(i);
            return;
        }
        if ev.modifiers.shift && self.anchor.is_some() {
            self.head = Some(i);
        } else if ev.click_count >= 2 {
            self.anchor = Some(i);
            self.head = Some(i);
        } else {
            self.anchor = Some(i);
            self.head = None;
            self.dragging = true;
            let t = self.words[i].timeline.start;
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
        if let Some(i) = self.pressed_in_selection.take()
            && let Some(t) = self.words.get(i).map(|w| w.timeline.start)
        {
            self.anchor = Some(i);
            self.head = None;
            self.state.update(cx, |s, cx| s.seek(t, cx));
            self.publish_selection(cx);
            cx.notify();
        }
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
        let state = self.state.read(cx);
        let Some(analysis) = state.services.analysis.clone() else {
            return;
        };
        let ranges = words::delete_ranges(state.bus.project(), &*analysis, &self.words, a, b);
        let Some(at) = ranges.iter().map(|r| r.start).min() else {
            return;
        };
        let commands: Vec<Command> = ranges
            .into_iter()
            .map(|range| Command::RippleDelete {
                range,
                tracks: None,
                fade: FADE,
            })
            .collect();
        self.anchor = None;
        self.head = None;
        self.state.update(cx, |s, cx| {
            if s.execute_batch(commands, "delete_words", cx).is_some() {
                s.seek(at, cx);
            }
            s.select_words(None, cx);
        });
    }

    /// The selected words as a chat reference: their timeline range,
    /// labelled with the quoted words.
    fn selection_ref(&self) -> Option<(String, SharedString)> {
        let (a, b) = self.selection()?;
        let range = TimeRange::new(self.words[a].timeline.start, self.words[b].timeline.end);
        let text = self.words[a..=b]
            .iter()
            .map(|w| w.word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let mut label: String = text.chars().take(32).collect();
        if text.chars().count() > 32 {
            label.push('…');
        }
        Some((
            format!(
                "@range {:.3}-{:.3}",
                range.start.as_secs_f64(),
                range.end.as_secs_f64()
            ),
            format!("“{label}”").into(),
        ))
    }

    /// The name of who says the selection's first word, when voices can
    /// be found and made.
    fn selected_speaker(&self, cx: &App) -> Option<String> {
        if !crate::ui::voices::available(self.state.read(cx)) {
            return None;
        }
        let (a, _) = self.selection()?;
        let speaker = self.words.get(a)?.word.speaker?;
        Some(
            self.speakers
                .get(&speaker)
                .cloned()
                .unwrap_or_else(|| "the Speaker".into()),
        )
    }

    /// Cut the selection's speaker into one recording, then open the
    /// Voices dialog on it: to clone their voice, or to find ones like it.
    fn speaker_voice(&mut self, clone: bool, window: &mut Window, cx: &mut Context<Self>) {
        use crate::state::voices::{SPEAKER_LIMIT, recording, speaker_spans};
        let Some(name) = self.selected_speaker(cx) else {
            return;
        };
        let Some(speaker) = self
            .selection()
            .and_then(|(a, _)| self.words.get(a)?.word.speaker)
        else {
            return;
        };
        let project = self.state.read(cx).bus.project().clone();
        let Some((source, spans)) = speaker_spans(&project, &self.words, speaker) else {
            return;
        };
        let file_name = name.clone();
        let cut = cx
            .background_executor()
            .spawn(async move { recording(&source, &spans, SPEAKER_LIMIT, &file_name) });
        let state = self.state.clone();
        cx.spawn_in(window, async move |_, cx| {
            let result = cut.await;
            let _ = cx.update(|window, cx| match result {
                Ok(file) => {
                    let start = if clone {
                        crate::ui::voices::Start::Clone {
                            files: vec![file],
                            name,
                        }
                    } else {
                        crate::ui::voices::Start::Similar {
                            recording: file,
                            label: name,
                        }
                    };
                    crate::ui::voices::open(state, start, None, window, cx);
                }
                Err(e) => state.update(cx, |s, cx| {
                    s.status = Some(e);
                    cx.notify();
                }),
            });
        })
        .detach();
    }

    fn add_to_chat(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = self.selection_ref() {
            self.state.update(cx, |s, cx| s.mention(vec![r], cx));
        }
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
        if self.words.is_empty() {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("No transcript")
                .into_any_element();
        }
        let panel = cx.entity();
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
            .child(
                list(
                    self.list.clone(),
                    cx.processor(|this, ix: usize, _, cx| this.paragraph(ix, cx)),
                )
                .size_full()
                .text_sm()
                .line_height(rems(1.6)),
            )
            .vertical_scrollbar(&self.list)
            .context_menu(move |menu, _, cx| {
                if panel.read(cx).selection().is_none() {
                    return menu;
                }
                let speaker = panel.read(cx).selected_speaker(cx);
                let p = panel.clone();
                let menu = menu.item(
                    PopupMenuItem::new("Add to chat")
                        .on_click(move |_, _, cx| p.update(cx, |this, cx| this.add_to_chat(cx))),
                );
                let Some(name) = speaker else {
                    return menu;
                };
                let (p1, p2) = (panel.clone(), panel.clone());
                menu.item(PopupMenuItem::separator())
                    .item(
                        PopupMenuItem::new(format!("Find Voices Like {name}…")).on_click(
                            move |_, window, cx| {
                                p1.update(cx, |this, cx| this.speaker_voice(false, window, cx))
                            },
                        ),
                    )
                    .item(
                        PopupMenuItem::new(format!("Clone {name}’s Voice…")).on_click(
                            move |_, window, cx| {
                                p2.update(cx, |this, cx| this.speaker_voice(true, window, cx))
                            },
                        ),
                    )
            })
            .into_any_element()
    }

    fn paragraph(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(p) = self.paragraphs.get(ix) else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let (muted, selection, accent, warn) = (
            theme.muted_foreground,
            theme.selection,
            theme.accent,
            theme.yellow,
        );
        let playhead = self.state.read(cx).playhead;
        let sel = self.selection();
        let drag = self.selection_ref().map(|r| RefDrag { refs: vec![r] });
        let panel = cx.entity().downgrade();
        let words = p.words.clone().map(|i| {
            let w = &self.words[i];
            let selected = sel.is_some_and(|(a, b)| a <= i && i <= b);
            let current = w.timeline.contains(playhead);
            let low = w.word.confidence < LOW_CONFIDENCE;
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
                .child(w.word.text.clone())
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
                .when_some(drag.clone().filter(|_| selected), |d, drag| {
                    let panel = panel.clone();
                    d.on_drag(drag, move |drag, _, _, cx| {
                        // Dragged away: the press was not a click.
                        let _ = panel.update(cx, |this, _| this.pressed_in_selection = None);
                        cx.new(|_| drag.clone())
                    })
                })
        });
        v_flex()
            .px_3()
            .pb_3()
            .gap_0p5()
            .children(p.label.clone().map(|l| {
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(muted)
                    .child(l)
            }))
            .child(h_flex().flex_wrap().children(words))
            .into_any_element()
    }
}

/// Splits the words at speaker changes and long pauses, and long monologues
/// at sentence ends. Only a speaker change (or the start) carries a label.
fn paragraphs(
    words: &[TimelineWord],
    speakers: &HashMap<koo_core::SpeakerId, String>,
) -> Vec<Paragraph> {
    let mut out: Vec<Paragraph> = Vec::new();
    let mut start = 0;
    let mut label = None;
    for (i, w) in words.iter().enumerate() {
        let speaker = w.word.speaker;
        let new_speaker = i == 0 || words[i - 1].word.speaker != speaker;
        let split = i > 0 && {
            let prev = &words[i - 1];
            let len = i - start;
            new_speaker
                || w.timeline.start - prev.timeline.end > PARAGRAPH_GAP
                || len >= PARAGRAPH_WORDS * 2
                || (len >= PARAGRAPH_WORDS && prev.word.text.ends_with(['.', '?', '!']))
        };
        if split {
            out.push(Paragraph {
                words: start..i,
                label: label.take(),
            });
            start = i;
        }
        if new_speaker {
            label = speaker.and_then(|s| speakers.get(&s).cloned());
        }
    }
    if start < words.len() {
        out.push(Paragraph {
            words: start..words.len(),
            label,
        });
    }
    out
}

impl Render for TranscriptPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh_words(cx);
        if self.demo_captions && !self.words.is_empty() {
            self.demo_captions = false;
            self.generate(cx);
        }
        let body = match self.view {
            View::Transcript => self.transcript_view(cx),
            View::Captions => self.captions_view(cx),
        };
        v_flex()
            .size_full()
            .key_context("Transcript")
            .child(self.header(cx))
            .child(body)
    }
}
