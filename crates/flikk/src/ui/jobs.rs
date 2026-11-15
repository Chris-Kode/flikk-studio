//! The title bar's Jobs button and the jobs it drops down: what holds or
//! waits for a GPU turn ([`flikk_os::gpu`]), running first, each with its
//! progress when known. A finished job turns its bar green for a moment,
//! then leaves the list. The app reads the list again every
//! [`POLL`] and passes it in.

use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, EmptyState, Icon, IconName, Indicator, Popover, Progress, Spinner, Tone, h_flex, v_flex,
};
use flikk_os::gpu::{Listed, State};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::time::Duration;

/// How often the app reads the list again, as the render cache polls: a
/// row is never more than a beat behind its job.
pub const POLL: Duration = Duration::from_millis(250);

/// The Jobs button, with a dot while a job runs.
pub fn button(
    jobs: &[Listed],
    open: bool,
    on_open_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> Popover {
    let running = jobs.iter().any(|job| job.state == State::Running);
    let jobs = jobs.to_vec();
    Popover::new("jobs")
        .anchor(Anchor::TopRight)
        .open(open)
        .on_open_change(on_open_change)
        .trigger(
            Button::new("jobs-button")
                .ghost()
                .icon(IconName::Activity)
                .tooltip("Jobs")
                .relative()
                .when(running, |b| b.child(Indicator::Dot(Tone::Neutral))),
        )
        .content(move |_, _, _| list(jobs))
}

fn list(jobs: Vec<Listed>) -> Div {
    let count = jobs.len();
    v_flex()
        .w(px(300.))
        .child(
            h_flex()
                .gap(px(6.))
                .h(t::PANEL_HEADER)
                .px(px(12.))
                .border_b_1()
                .border_color(t::LN)
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .child("Jobs")
                .when(count > 0, |el| {
                    el.child(
                        div()
                            .font_family(t::mono())
                            .text_color(t::T3)
                            .child(count.to_string()),
                    )
                }),
        )
        .when(count == 0, |el| {
            el.child(EmptyState::new(IconName::Activity, "No jobs").compact())
        })
        .when(count > 0, |el| {
            el.child(v_flex().py(px(4.)).children(jobs.into_iter().map(row)))
        })
}

fn row(job: Listed) -> Stateful<Div> {
    let waiting = job.state == State::Waiting;
    // One 14 px column for the spinner, the hourglass and the check, so
    // labels align.
    let mark = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(14.))
        .child(match job.state {
            State::Running => Spinner::new().color(t::T2).into_any_element(),
            State::Waiting => Icon::new(IconName::Hourglass)
                .color(t::T3)
                .into_any_element(),
            State::Done => Icon::new(IconName::Check).color(t::OK).into_any_element(),
        });
    let bar = match job.state {
        State::Done => Some(Progress::new().done()),
        State::Running | State::Waiting => job.progress.map(|f| Progress::new().value(f)),
    };
    h_flex()
        .id(("job", job.id))
        .gap(px(8.))
        .h(px(26.))
        .px(px(12.))
        .text_size(px(11.))
        .child(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(if waiting { t::T2 } else { t::T1 })
                .child(job.label),
        )
        .children(bar.map(|bar| {
            let fraction = job.progress.unwrap_or(1.);
            h_flex()
                .flex_none()
                .gap(px(6.))
                .child(bar.w(px(48.)).h(px(3.)))
                .child(
                    div()
                        .w(px(32.))
                        .text_right()
                        .font_family(t::mono())
                        .text_color(t::T3)
                        .child(format!("{:.0} %", fraction * 100.)),
                )
        }))
}
