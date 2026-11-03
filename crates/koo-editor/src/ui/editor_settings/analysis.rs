//! Settings → Analysis: the steps "Analyze" runs. A step that needs
//! another (speakers need the transcript; beats and sound search, the
//! sound pass) is off while that one is. Media analyzed already keeps its
//! results until it is analyzed again.

use super::page;
use crate::state::EditorState;
use crate::ui::kit::{ScrollableElement as _, Switch, h_flex, theme as t};
use gpui_kit::prelude::FluentBuilder as _;
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
        let rows: Vec<_> = STEPS
            .iter()
            .enumerate()
            .map(|(ix, step)| {
                let possible = step.needs.is_none_or(|needs| needs(&steps));
                let by = online.filter(|_| ix <= 1);
                let set = step.set;
                // Steps that need this one hang under it on a tree line.
                let parent = STEPS.get(ix + 1).is_some_and(|next| next.needs.is_some());
                let child = step.needs.is_some();
                let last = !parent && child;
                h_flex()
                    .relative()
                    .flex_none()
                    .h(px(40.))
                    .gap(px(12.))
                    .border_b_1()
                    .border_color(t::LN)
                    .when(child, |el| el.pl(px(22.)))
                    .when(parent && !child, |el| {
                        el.child(tree_line().top(px(28.)).bottom_0())
                    })
                    .when(child, |el| {
                        el.child(tree_line().top(px(-1.)).h(px(21.)).w(px(10.)).border_b_1())
                            .when(!last, |el| {
                                el.child(tree_line().top(px(20.)).bottom(px(-1.)))
                            })
                    })
                    .child(
                        div()
                            .flex_1()
                            .when(!possible, |el| el.text_color(t::T3))
                            .child(step.label),
                    )
                    .children(by.map(|name| div().text_size(px(11.)).text_color(t::T3).child(name)))
                    .child(
                        Switch::new(("analysis-step", ix))
                            .checked((step.on)(&steps) && possible)
                            .disabled(!possible)
                            .on_click(cx.listener(move |this, on: &bool, _, cx| {
                                let mut steps = this.state.read(cx).analysis_steps;
                                set(&mut steps, *on);
                                this.state
                                    .update(cx, |s, cx| s.set_analysis_steps(steps, cx));
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        page("Analysis", None).children(rows).overflow_y_scrollbar()
    }
}

/// A stretch of the line joining a step to the ones that need it.
fn tree_line() -> Div {
    div()
        .absolute()
        .left(px(6.))
        .border_l_1()
        .border_color(t::LN_CTL)
}
