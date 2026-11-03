//! Settings → Models: each task analysis runs on a model, the model it
//! runs on (picked here when it has more than one) and whether that is
//! downloaded. Models are only ever downloaded (or deleted) from here, when
//! the user asks. While a provider transcribes, the speech tasks show its
//! name: their models are not needed.

use super::{meta, page, row, size_label};
use crate::state::EditorState;
use crate::ui::kit::{
    Button, Icon, IconName, Progress, ScrollableElement as _, Select, SelectEvent, SelectState,
    Tooltip, h_flex, theme as t,
};
use flikk_models::{Model, Status, Task as ModelTask};
use gpui_kit::*;
use std::time::Duration;

/// How often a running download's progress is redrawn.
const PROGRESS_POLL: Duration = Duration::from_millis(200);

pub struct ModelsTab {
    state: Entity<EditorState>,
    /// Each task with the list of models that can do it.
    pickers: Vec<(ModelTask, Entity<SelectState<SharedString>>)>,
    /// A download was running at the last poll.
    busy: bool,
    _observe: Subscription,
    _picks: Vec<Subscription>,
    _poll: Task<()>,
}

impl ModelsTab {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tasks = state.read(cx).model_tasks.clone();
        let pickers: Vec<_> = tasks
            .into_iter()
            .map(|task| {
                let models = task.models();
                let names: Vec<SharedString> = models.iter().map(|m| m.name.into()).collect();
                let selected = models
                    .iter()
                    .position(|m| m.id == task.model().id)
                    .unwrap_or(0);
                let picker = cx.new(|cx| SelectState::new(names, Some(selected), window, cx));
                (task, picker)
            })
            .collect();
        let _picks = pickers
            .iter()
            .map(|&(task, ref picker)| {
                cx.subscribe(
                    picker,
                    move |this, _, SelectEvent::Confirm(name): &SelectEvent<SharedString>, cx| {
                        this.pick(task, name, cx)
                    },
                )
            })
            .collect();
        let _poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PROGRESS_POLL).await;
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            _observe: cx.observe(&state, |_, _, cx| cx.notify()),
            state,
            pickers,
            busy: false,
            _picks,
            _poll,
        }
    }

    /// Redraw while a download runs, and once more after it ends.
    fn poll(&mut self, cx: &mut Context<Self>) {
        let busy = self.state.read(cx).models.busy();
        if busy || self.busy {
            cx.notify();
        }
        self.busy = busy;
    }

    /// Run `task` on the model named `name` in its picker.
    fn pick(&mut self, task: ModelTask, name: &str, cx: &mut Context<Self>) {
        if let Some(&model) = task.models().iter().find(|m| m.name == name) {
            self.state.update(cx, |s, cx| s.pick_model(task, model, cx));
        }
    }

    fn download(&mut self, model: &'static Model, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.download_model(model, cx));
    }

    fn render_status(
        &self,
        ix: usize,
        model: &'static Model,
        status: Status,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match status {
            Status::Downloaded => h_flex()
                .gap(px(8.))
                .child(
                    meta(size_label(model.files.iter().map(|f| f.size).sum())).text_size(px(11.)),
                )
                .child(Icon::new(IconName::Check).size(px(14.)).color(t::OK))
                .child(
                    Button::new(("model-delete", ix))
                        .ghost()
                        .small()
                        .icon(IconName::Trash)
                        .tooltip("Delete")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |s, cx| s.delete_model(model, cx))
                        })),
                )
                .into_any_element(),
            Status::Missing => Button::new(("model-download", ix))
                .small()
                .label("Download")
                .child(sized(model.missing_bytes()))
                .on_click(cx.listener(move |this, _, _, cx| this.download(model, cx)))
                .into_any_element(),
            Status::Downloading(fraction) => h_flex()
                .gap(px(8.))
                .child(Progress::new().value(fraction).w(px(120.)))
                .child(
                    meta(format!("{:.0}%", fraction * 100.))
                        .w(px(32.))
                        .text_size(px(11.))
                        .text_color(t::T2),
                )
                .child(
                    Button::new(("model-cancel", ix))
                        .ghost()
                        .small()
                        .icon(IconName::X)
                        .tooltip("Cancel")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |s, cx| s.cancel_download(model, cx))
                        })),
                )
                .into_any_element(),
            Status::Failed(message) => h_flex()
                .gap(px(8.))
                .child(
                    div()
                        .id(("model-failed", ix))
                        .child(Icon::new(IconName::CircleAlert).size(px(14.)).color(t::ERR))
                        .tooltip(Tooltip::text(message)),
                )
                .child(
                    Button::new(("model-retry", ix))
                        .small()
                        .label("Retry")
                        .on_click(cx.listener(move |this, _, _, cx| this.download(model, cx))),
                )
                .into_any_element(),
        }
    }
}

impl Render for ModelsTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let downloads = self.state.read(cx).models.clone();
        // The tasks a provider does instead, and its name.
        let online = self
            .state
            .read(cx)
            .online_transcription()
            .map(|p| (flikk_analysis::LocalAnalysis::online_tasks(), p.name));
        let by = |task: &ModelTask| {
            online
                .as_ref()
                .filter(|(tasks, _)| tasks.contains(task))
                .map(|(_, name)| *name)
        };
        let rows: Vec<_> = self
            .pickers
            .iter()
            .map(|(task, picker)| (*task, picker.clone(), downloads.status(task.model())))
            .collect();
        let missing: Vec<&'static Model> = rows
            .iter()
            .filter(|(task, _, status)| {
                by(task).is_none() && matches!(status, Status::Missing | Status::Failed(_))
            })
            .map(|(task, _, _)| task.model())
            .collect();
        let download_all = (missing.len() > 1).then(|| {
            let bytes = missing.iter().map(|m| m.missing_bytes()).sum();
            Button::new("model-download-all")
                .small()
                .icon(IconName::Download)
                .label("Download all")
                .child(sized(bytes))
                .on_click(cx.listener(move |this, _, _, cx| {
                    for &model in &missing {
                        this.download(model, cx);
                    }
                }))
                .into_any_element()
        });
        let rows: Vec<_> = rows
            .into_iter()
            .enumerate()
            .map(|(ix, (task, picker, status))| {
                let provider = by(&task);
                // Its model is not needed: only a copy on disk can be
                // deleted.
                let status_cell = match (provider, &status) {
                    (Some(_), Status::Downloaded) | (None, _) => {
                        self.render_status(ix, task.model(), status, cx)
                    }
                    (Some(name), _) => div()
                        .text_size(px(11.))
                        .text_color(t::T3)
                        .child(name)
                        .into_any_element(),
                };
                row(task.label(), 130.)
                    .child(
                        div().w(px(240.)).flex_none().child(
                            Select::new(&picker)
                                .small()
                                .disabled(task.models().len() < 2 || provider.is_some()),
                        ),
                    )
                    .child(h_flex().flex_1().min_w_0().justify_end().child(status_cell))
            })
            .collect();
        page("Models", download_all)
            .child(
                div()
                    .flex_none()
                    .border_t_1()
                    .border_color(t::LN)
                    .children(rows),
            )
            .overflow_y_scrollbar()
    }
}

/// A download's size, in a button after its label.
fn sized(bytes: u64) -> Div {
    div().font_family(t::mono()).child(size_label(bytes))
}
