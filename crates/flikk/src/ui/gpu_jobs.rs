//! The title bar's GPU button and the jobs it drops down: what holds or
//! waits for a GPU turn ([`flikk_os::gpu`]), running first, each with its
//! progress when known. The app reads the list again every
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

/// The GPU button, with a dot while a job runs.
pub fn button(
    jobs: &[Listed],
    open: bool,
    on_open_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> Popover {
    let running = jobs.iter().any(|job| job.state == State::Running);
    let jobs = jobs.to_vec();
    Popover::new("gpu-jobs")
        .anchor(Anchor::TopRight)
        .open(open)
        .on_open_change(on_open_change)
        .trigger(
            Button::new("gpu-jobs-button")
                .ghost()
                .icon(IconName::Cpu)
                .tooltip("GPU jobs")
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
                .child("GPU jobs")
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
            el.child(EmptyState::new(IconName::Cpu, "No GPU jobs").compact())
        })
        .when(count > 0, |el| {
            el.child(v_flex().py(px(4.)).children(jobs.into_iter().map(row)))
        })
}

fn row(job: Listed) -> Stateful<Div> {
    let running = job.state == State::Running;
    // One 14 px column for the spinner and the hourglass, so labels align.
    let mark = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(14.))
        .map(|el| {
            if running {
                el.child(Spinner::new().color(t::T2))
            } else {
                el.child(Icon::new(IconName::Hourglass).color(t::T3))
            }
        });
    h_flex()
        .id(("gpu-job", job.id))
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
                .text_color(if running { t::T1 } else { t::T2 })
                .child(job.label),
        )
        .children(job.progress.map(|fraction| {
            h_flex()
                .flex_none()
                .gap(px(6.))
                .child(Progress::new().value(fraction).w(px(48.)).h(px(3.)))
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
