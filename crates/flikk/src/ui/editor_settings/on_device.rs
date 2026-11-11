//! Settings → Integrations, "On this Mac": the models the editor runs on
//! this computer, named by what they do. Each row shows whether its model
//! is downloaded, its download, or the provider doing the work instead.
//! Models are only ever downloaded (or deleted) from here, when the user
//! asks. Transcription is not listed: its row in "Use for" picks where it
//! runs and, here, its size.

use super::{group_label, meta, size_label};
use crate::state::EditorState;
use crate::ui::kit::{Button, Icon, IconName, Progress, Tooltip, h_flex, theme as t, v_flex};
use flikk_models::{Model, Status, Task as ModelTask};
use gpui_kit::*;

/// The section's title.
const HERE: &str = if cfg!(target_os = "macos") {
    "On this Mac"
} else {
    "On this computer"
};

/// What "Use for" calls running a capability here.
pub(super) const THIS_MACHINE: &str = if cfg!(target_os = "macos") {
    "This Mac"
} else {
    "This computer"
};

/// Transcription's sizes as "Use for" offers them, smallest first.
pub(crate) fn transcription_sizes() -> impl Iterator<Item = (&'static str, &'static Model)> {
    ["Small", "Large"]
        .into_iter()
        .zip(ModelTask::Transcription.models().iter().copied())
}

/// The section: its title with Download all, then two columns of tasks.
pub(super) fn section(state: &Entity<EditorState>, cx: &App) -> AnyElement {
    let s = state.read(cx);
    // The tasks the provider transcribing does instead, and its name.
    let online = s
        .online_transcription()
        .map(|p| (flikk_analysis::LocalAnalysis::online_tasks(), p.name));
    let by = |task: ModelTask| {
        online
            .as_ref()
            .filter(|(tasks, _)| tasks.contains(&task))
            .map(|(_, name)| *name)
    };
    let missing: Vec<&'static Model> = s
        .model_tasks
        .iter()
        .filter(|&&task| {
            by(task).is_none()
                && matches!(
                    s.models.status(task.model()),
                    Status::Missing | Status::Failed(_)
                )
        })
        .map(|task| task.model())
        .collect();
    let download_all = (missing.len() > 1).then(|| {
        let bytes = missing.iter().map(|m| m.missing_bytes()).sum();
        let state = state.clone();
        Button::new("model-download-all")
            .small()
            .icon(IconName::Download)
            .label("Download all")
            .child(sized(bytes))
            .on_click(move |_, _, cx| {
                state.update(cx, |s, cx| {
                    for &model in &missing {
                        s.download_model(model, cx);
                    }
                })
            })
    });
    let rows: Vec<AnyElement> = s
        .model_tasks
        .iter()
        .filter(|&&task| task != ModelTask::Transcription)
        .map(|&task| {
            let model = task.model();
            let status = s.models.status(model);
            let cell = match (by(task), &status) {
                // Its model is not needed: only a copy on disk can be
                // deleted.
                (Some(_), Status::Downloaded) | (None, _) => {
                    model_status(state, task, model, status, cx)
                }
                (Some(name), _) => div()
                    .text_size(px(11.))
                    .text_color(t::T3)
                    .child(name)
                    .into_any_element(),
            };
            h_flex()
                .h(px(32.))
                .gap(px(8.))
                .border_b_1()
                .border_color(t::LN)
                .child(div().flex_1().min_w_0().truncate().child(task.label()))
                .child(cell)
                .into_any_element()
        })
        .collect();
    // Filled row by row: left, right, left…
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for (ix, row) in rows.into_iter().enumerate() {
        if ix % 2 == 0 {
            left.push(row)
        } else {
            right.push(row)
        }
    }
    v_flex()
        .flex_none()
        .mt(px(20.))
        .child(group_label(HERE).children(download_all))
        .child(
            h_flex()
                .items_start()
                .gap(px(32.))
                .child(v_flex().flex_1().min_w_0().children(left))
                .child(v_flex().flex_1().min_w_0().children(right)),
        )
        .into_any_element()
}

/// Where `task`'s `model` stands: downloaded (with Delete), its Download,
/// its progress (with Cancel), or why it failed (with Retry).
pub(super) fn model_status(
    state: &Entity<EditorState>,
    task: ModelTask,
    model: &'static Model,
    status: Status,
    _: &App,
) -> AnyElement {
    let ix = ModelTask::ALL.iter().position(|&t| t == task).unwrap_or(0);
    let state = state.clone();
    match status {
        Status::Downloaded => h_flex()
            .gap(px(4.))
            .child(Icon::new(IconName::CircleCheck).size(px(14.)).color(t::OK))
            .child(
                Button::new(("model-delete", ix))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Trash)
                    .tooltip("Delete")
                    .on_click(move |_, _, cx| state.update(cx, |s, cx| s.delete_model(model, cx))),
            )
            .into_any_element(),
        Status::Missing => Button::new(("model-download", ix))
            .small()
            .label("Download")
            .child(sized(model.missing_bytes()))
            .on_click(move |_, _, cx| state.update(cx, |s, cx| s.download_model(model, cx)))
            .into_any_element(),
        Status::Downloading(fraction) => h_flex()
            .gap(px(8.))
            .child(Progress::new().value(fraction).w(px(64.)))
            .child(
                meta(format!("{:.0} %", fraction * 100.))
                    .w(px(32.))
                    .flex()
                    .justify_end(),
            )
            .child(
                Button::new(("model-cancel", ix))
                    .ghost()
                    .xsmall()
                    .icon(IconName::X)
                    .tooltip("Cancel")
                    .on_click(move |_, _, cx| {
                        state.update(cx, |s, cx| s.cancel_download(model, cx))
                    }),
            )
            .into_any_element(),
        Status::Failed(message) => h_flex()
            .gap(px(8.))
            .child(
                h_flex()
                    .id(("model-failed", ix))
                    .min_w_0()
                    .max_w(px(180.))
                    .gap(px(6.))
                    .text_size(px(11.))
                    .text_color(t::T2)
                    .child(Icon::new(IconName::CircleAlert).size(px(14.)).color(t::ERR))
                    .child(div().min_w_0().truncate().child(message.clone()))
                    .tooltip(Tooltip::text(message)),
            )
            .child(
                Button::new(("model-retry", ix))
                    .small()
                    .label("Retry")
                    .on_click(move |_, _, cx| {
                        state.update(cx, |s, cx| s.download_model(model, cx))
                    }),
            )
            .into_any_element(),
    }
}

/// A download's size, in a button after its label.
fn sized(bytes: u64) -> Div {
    div()
        .font_family(t::mono())
        .text_color(t::T3)
        .child(size_label(bytes))
}
