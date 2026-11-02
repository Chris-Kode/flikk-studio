//! Typing over words: the selected words become an editor, and what is
//! typed there is said in the speaker's voice and cut in where they were
//! (`koo_core::respeak`). The voice is the library voice linked to the
//! speaker; without one, the Voices dialog opens to clone it from what
//! they say in the recording, and the voice kept there is linked.

use super::TranscriptPanel;
use crate::state::voices::{SPEAKER_LIMIT, recording, spans_of};
use crate::ui::voices::{OnKept, Start};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{IconName, Sizable as _, h_flex};
use gpui_kit::*;
use koo_core::MediaId;
use koo_core::cloud::{Capability, Reading};
use koo_core::query::TimelineWord;
use koo_core::respeak;
use std::collections::HashSet;
use std::rc::Rc;

impl TranscriptPanel {
    /// Type over the selected words.
    pub(super) fn retype(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((a, b)) = self.selection() else {
            return;
        };
        let text = self.words[a..=b]
            .iter()
            .map(|w| w.word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        self.retyping = Some((a, b));
        self.retyper.update(cx, |e, cx| {
            e.set_value(text, window, cx);
            e.focus(window, cx);
        });
        self.remeasure(a);
        cx.notify();
    }

    pub(super) fn cancel_retype(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((a, _)) = self.retyping.take() {
            self.remeasure(a);
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    fn remeasure(&mut self, word: usize) {
        if let Some(ix) = self.paragraphs.iter().position(|p| p.words.contains(&word)) {
            self.list.remeasure_items(ix..ix + 1);
        }
    }

    /// Say what was typed in the speaker's voice, in place of the words.
    pub(super) fn say_it(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((a, b)) = self.retyping else {
            return;
        };
        let text = self.retyper.read(cx).value().to_string();
        let fixed = {
            let state = self.state.read(cx);
            match &state.services.analysis {
                Some(analysis) => {
                    respeak::fix(state.bus.project(), &self.words, a, b, &text, |fp| {
                        analysis.analysis(fp)
                    })
                }
                None => Err("Analysis is not available in this build.".into()),
            }
            .and_then(|fix| {
                if crate::ui::voices::available(state) {
                    Ok(fix)
                } else {
                    Err("Pick who reads voiceovers in Settings → Integrations.".into())
                }
            })
        };
        let fix = match fixed {
            Ok(fix) => fix,
            Err(e) => {
                self.state.update(cx, |s, cx| {
                    s.status = Some(e);
                    cx.notify();
                });
                return;
            }
        };
        self.cancel_retype(window, cx);
        let who = fix.speaker();
        let linked = who
            .as_ref()
            .and_then(|w| self.state.read(cx).speaker_voice(w))
            .map(|v| v.reading());
        if let Some(reading) = linked {
            self.state.update(cx, |s, cx| s.say_fix(fix, reading, cx));
            return;
        }

        // No voice of theirs yet: clone it from what they say here.
        let project = self.state.read(cx).bus.project().clone();
        let theirs: Vec<&TimelineWord> = self
            .words
            .iter()
            .filter(|w| w.media == fix.media && w.word.speaker == fix.speaker)
            .collect();
        let Some((source, spans)) = spans_of(&project, &theirs) else {
            return;
        };
        let name = fix
            .speaker
            .and_then(|s| self.speakers.get(&s).cloned())
            .unwrap_or_else(|| "Speaker".into());
        let file_name = name.clone();
        let cut = cx
            .background_executor()
            .spawn(async move { recording(&source, &spans, SPEAKER_LIMIT, &file_name) });
        let state = self.state.clone();
        cx.spawn_in(window, async move |_, cx| {
            let result = cut.await;
            let _ = cx.update(|window, cx| match result {
                Ok(file) => {
                    let kept_state = state.clone();
                    let on_kept: OnKept = Rc::new(move |voice, _, cx| {
                        kept_state.update(cx, |s, cx| {
                            if let Some(who) = &who {
                                s.link_speaker(who, &voice.id, cx);
                            }
                            let reading = s
                                .voice_provider()
                                .and_then(|p| s.library.voice_of(&p, &voice.id))
                                .map(|v| v.reading())
                                .unwrap_or_else(|| Reading {
                                    voice: voice.id.clone(),
                                    voice_name: voice.name.clone(),
                                    stability: Reading::STABILITY,
                                    similarity: Reading::SIMILARITY,
                                });
                            s.say_fix(fix.clone(), reading, cx);
                        });
                    });
                    let start = Start::Clone {
                        files: vec![file],
                        name,
                    };
                    crate::ui::voices::open(state, start, Some(on_kept), window, cx);
                }
                Err(e) => state.update(cx, |s, cx| {
                    s.status = Some(e);
                    cx.notify();
                }),
            });
        })
        .detach();
    }

    /// The editor drawn in place of the words typed over.
    pub(super) fn retype_field(&self, cx: &mut Context<Self>) -> AnyElement {
        let chars = self.retyper.read(cx).value().chars().count().max(8);
        let width = px((chars as f32 * 7.5 + 28.0).min(420.0));
        h_flex()
            .gap_0p5()
            .px(px(2.5))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    this.cancel_retype(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(div().w(width).child(Input::new(&self.retyper).xsmall()))
            .child(
                Button::new("say-it")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Mic)
                    .tooltip("Say it in their voice")
                    .on_click(cx.listener(|this, _, window, cx| this.say_it(window, cx))),
            )
            .into_any_element()
    }

    /// Items whose speech a provider made: their words show as such.
    pub(super) fn generated_media(&self, cx: &App) -> HashSet<MediaId> {
        self.state
            .read(cx)
            .bus
            .project()
            .media
            .iter()
            .filter(|m| {
                m.generated
                    .as_deref()
                    .is_some_and(|g| g.capability == Capability::Voiceover)
            })
            .map(|m| m.id)
            .collect()
    }
}
