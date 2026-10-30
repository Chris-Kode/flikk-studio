//! Settings → Analysis: the steps "Analyze" runs. A step that needs
//! another (speakers need the transcript; beats and sound search, the
//! sound pass) is off while that one is. Media analyzed already keeps its
//! results until it is analyzed again.

use super::row;
use crate::state::EditorState;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use koo_core::analysis::AnalysisSteps;

/// A step's switch: its label, whether it is on, which step it needs (on
/// for it to run), and how it is set.
struct Step {
    label: &'static str,
    on: fn(&AnalysisSteps) -> bool,
    needs: Option<fn(&AnalysisSteps) -> bool>,
    set: fn(&mut AnalysisSteps, bool),
}

const STEPS: [Step; 6] = [
    Step {
        label: "Transcript",
        on: |s| s.transcript,
        needs: None,
        set: |s, on| s.transcript = on,
    },
    Step {
        label: "Speakers",
        on: |s| s.speakers,
        needs: Some(|s| s.transcript),
        set: |s, on| s.speakers = on,
    },
    Step {
        label: "Sounds and music",
        on: |s| s.sounds,
        needs: None,
        set: |s, on| s.sounds = on,
    },
    Step {
        label: "Beats",
        on: |s| s.beats,
        needs: Some(|s| s.sounds),
        set: |s, on| s.beats = on,
    },
    Step {
        label: "Sound search",
        on: |s| s.sound_search,
        needs: Some(|s| s.sounds),
        set: |s, on| s.sound_search = on,
    },
    Step {
        label: "Shot changes",
        on: |s| s.shots,
        needs: None,
        set: |s, on| s.shots = on,
    },
];

pub struct AnalysisTab {
    state: Entity<EditorState>,
    _observe: Subscription,
}

impl AnalysisTab {
    pub fn new(state: Entity<EditorState>, cx: &mut Context<Self>) -> Self {
        Self {
            _observe: cx.observe(&state, |_, _, cx| cx.notify()),
            state,
        }
    }
}

impl Render for AnalysisTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.state.read(cx);
        let steps = s.analysis_steps;
        // Who transcribes, beside the steps it does.
        let online = s.online_transcription().map(|p| p.name);
        let muted = cx.theme().muted_foreground;
        let rows: Vec<_> = STEPS
            .iter()
            .enumerate()
            .map(|(ix, step)| {
                let possible = step.needs.is_none_or(|needs| needs(&steps));
                let by = online.filter(|_| ix <= 1);
                let set = step.set;
                row(step.label, cx)
                    .child(
                        h_flex()
                            .flex_1()
                            .justify_end()
                            .gap_3()
                            .children(by.map(|name| div().text_xs().text_color(muted).child(name)))
                            .child(
                                Switch::new(("analysis-step", ix))
                                    .small()
                                    .checked((step.on)(&steps) && possible)
                                    .disabled(!possible)
                                    .on_click(cx.listener(move |this, on: &bool, _, cx| {
                                        let mut steps = this.state.read(cx).analysis_steps;
                                        set(&mut steps, *on);
                                        this.state
                                            .update(cx, |s, cx| s.set_analysis_steps(steps, cx));
                                    })),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        v_flex().size_full().children(rows)
    }
}
