//! Change-set card (PRD AG4–AG6): summary, Preview / Accept all / Review /
//! Reject, the review list with per-item selection, and the outcome.

use super::ChatPanel;
use super::refs::clock;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::changeset::{ChangeItem, ChangeSet, ChangeSetState, ItemState};
use koo_core::command::{Command, ToolCategory, Warning};
use koo_core::model::Project;
use koo_core::ops::{DiffKind, Op};
use koo_core::{ChangeItemId, ChangeSetId, Time};
use std::collections::{HashMap, HashSet};

/// Per-card UI state, keyed by change set.
#[derive(Default)]
pub struct Review {
    pub open: bool,
    /// Pending items the user unticked.
    pub excluded: HashSet<ChangeItemId>,
}

pub type Reviews = HashMap<ChangeSetId, Review>;

impl ChatPanel {
    pub(super) fn render_change_set(
        &mut self,
        id: ChangeSetId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        let Some(set) = state.bus.change_set(id).cloned() else {
            return div().into_any_element();
        };
        let previewing = state.preview == Some(id);
        let project = state.bus.project().clone();
        let review = self.reviews.entry(id).or_default();
        let open = review.open;
        let excluded = review.excluded.clone();
        let theme = cx.theme();

        let total = set.items.len();
        let warnings = set.items.iter().filter(|i| !i.warnings.is_empty()).count();
        let listen = set.items.iter().filter(|i| needs_listen(i)).count();
        let resolved = set.state == ChangeSetState::Resolved;

        let summary = h_flex()
            .gap_3()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(format!(
                "{total} {}",
                if total == 1 { "change" } else { "changes" }
            ))
            .when(warnings > 0, |el| {
                el.child(
                    h_flex()
                        .gap_1()
                        .text_color(theme.yellow)
                        .child(Icon::new(IconName::TriangleAlert).size_3())
                        .child(warnings.to_string()),
                )
            })
            .when(listen > 0 && !resolved, |el| {
                el.child(
                    h_flex()
                        .id(("listen", id_hash(id)))
                        .gap_1()
                        .child(Icon::new(IconName::Mic).size_3())
                        .child(listen.to_string())
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Needs a listen")
                                .build(window, cx)
                        }),
                )
            });

        let review_button = Button::new(("review", id_hash(id)))
            .label("Review")
            .xsmall()
            .ghost()
            .selected(open)
            .on_click(cx.listener(move |this, _, _, cx| {
                let r = this.reviews.entry(id).or_default();
                r.open = !r.open;
                this.remeasure(cx);
            }));
        let actions = if resolved {
            h_flex()
                .child(outcome(&set, cx))
                .child(div().flex_1())
                .child(review_button)
                .into_any_element()
        } else {
            h_flex()
                .gap_1()
                .child(
                    Button::new(("preview", id_hash(id)))
                        .icon(if previewing {
                            IconName::EyeOff
                        } else {
                            IconName::Eye
                        })
                        .label("Preview")
                        .xsmall()
                        .ghost()
                        .selected(previewing)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |s, cx| {
                                let next = (s.preview != Some(id)).then_some(id);
                                s.set_preview(next, cx)
                            })
                        })),
                )
                .when(open, |el| {
                    let all = excluded.is_empty();
                    el.child(
                        Button::new(("toggle-all", id_hash(id)))
                            .label(if all { "None" } else { "All" })
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let items: Vec<_> = this
                                    .state
                                    .read(cx)
                                    .bus
                                    .change_set(id)
                                    .map(|s| s.pending_items().map(|i| i.id).collect())
                                    .unwrap_or_default();
                                let r = this.reviews.entry(id).or_default();
                                if r.excluded.is_empty() {
                                    r.excluded.extend(items);
                                } else {
                                    r.excluded.clear();
                                }
                                cx.notify();
                            })),
                    )
                })
                .child(div().flex_1())
                .child(review_button)
                .child(
                    Button::new(("reject", id_hash(id)))
                        .label("Reject")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |s, cx| {
                                if s.preview == Some(id) {
                                    s.set_preview(None, cx);
                                }
                                s.reject(id, None, cx)
                            })
                        })),
                )
                .child({
                    // While reviewing, the primary action takes the ticked items.
                    let chosen = set
                        .pending_items()
                        .filter(|i| !open || !excluded.contains(&i.id))
                        .count();
                    Button::new(("accept", id_hash(id)))
                        .label(if chosen == total {
                            "Accept all".to_string()
                        } else {
                            format!("Accept {chosen}")
                        })
                        .xsmall()
                        .primary()
                        .disabled(chosen == 0)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if open {
                                this.accept_selected(id, cx);
                            } else {
                                this.state.update(cx, |s, cx| {
                                    if s.preview == Some(id) {
                                        s.set_preview(None, cx);
                                    }
                                    s.accept(id, None, cx);
                                })
                            }
                        }))
                })
                .into_any_element()
        };

        let list = open.then(|| self.render_items(&set, &project, &excluded, cx));

        let theme = cx.theme();
        v_flex()
            .w_full()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(if previewing { theme.ring } else { theme.border })
            .bg(theme.secondary.opacity(0.4))
            .child(
                v_flex()
                    .gap_1()
                    .px_3()
                    .pt_2p5()
                    .pb_2()
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .line_clamp(2)
                            .child(set.title.clone()),
                    )
                    .child(summary),
            )
            .children(list)
            .child(
                div()
                    .px_2()
                    .py_1p5()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(actions),
            )
            .into_any_element()
    }

    fn render_items(
        &self,
        set: &ChangeSet,
        project: &Project,
        excluded: &HashSet<ChangeItemId>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = set.id;
        let theme = cx.theme().clone();
        let mut items: Vec<_> = set
            .items
            .iter()
            .enumerate()
            .map(|(ix, item)| (ix, item, item_time(item, project)))
            .collect();
        items.sort_by_key(|(ix, _, at)| (at.is_none(), *at, *ix));
        let rows = items.into_iter().map(|(ix, item, at)| {
            let item_id = item.id;
            let tint = match diff_kind(item) {
                DiffKind::Added => theme.green,
                DiffKind::Removed => theme.red,
                DiffKind::Modified => theme.yellow,
            };
            let reason = if ix == 0 && item.reason == set.title {
                describe(&item.command)
            } else {
                item.reason.clone()
            };
            let pending = item.state == ItemState::Pending;
            let warning = item.warnings.first().map(warning_text);
            h_flex()
                .id(("item", ix))
                .h(px(28.))
                .px_3()
                .gap_2()
                .text_xs()
                .cursor_pointer()
                .hover(|el| el.bg(theme.accent.opacity(0.5)))
                .when(!pending, |el| el.opacity(0.6))
                .child(if pending {
                    Checkbox::new(("item-check", ix))
                        .checked(!excluded.contains(&item_id))
                        .xsmall()
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            let r = this.reviews.entry(id).or_default();
                            if *checked {
                                r.excluded.remove(&item_id);
                            } else {
                                r.excluded.insert(item_id);
                            }
                            cx.notify();
                        }))
                        .into_any_element()
                } else {
                    state_icon(item.state, &theme).into_any_element()
                })
                .child(div().size_1p5().flex_none().rounded_full().bg(tint))
                .child(div().flex_1().min_w_0().truncate().child(reason))
                .when(needs_listen(item), |el| {
                    el.child(
                        Icon::new(IconName::Mic)
                            .size_3()
                            .text_color(theme.muted_foreground),
                    )
                })
                .when_some(warning, |el, text| {
                    el.child(
                        div()
                            .id(("item-warning", ix))
                            .child(
                                Icon::new(IconName::TriangleAlert)
                                    .size_3()
                                    .text_color(theme.yellow),
                            )
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(text.clone())
                                    .build(window, cx)
                            }),
                    )
                })
                .when(item.state == ItemState::Conflicted, |el| {
                    el.child(div().text_color(theme.red).child("Conflict"))
                })
                .when_some(at, |el, t| {
                    el.child(
                        div()
                            .flex_none()
                            .text_color(theme.muted_foreground)
                            .child(clock(t)),
                    )
                })
                .when_some(at, |el, t| {
                    el.on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.seek(t, cx))
                    }))
                })
        });
        v_flex()
            .border_t_1()
            .border_color(theme.border)
            .child(
                v_flex()
                    .id(("items", id_hash(id)))
                    .py_1()
                    .max_h(px(280.))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }

    /// Accept the ticked items as one undo step and reject the rest.
    fn accept_selected(&mut self, id: ChangeSetId, cx: &mut Context<Self>) {
        let excluded = self
            .reviews
            .get(&id)
            .map(|r| r.excluded.clone())
            .unwrap_or_default();
        self.state.update(cx, |s, cx| {
            let Some(set) = s.bus.change_set(id) else {
                return;
            };
            let (take, skip): (Vec<_>, Vec<_>) = set
                .pending_items()
                .map(|i| i.id)
                .partition(|i| !excluded.contains(i));
            if s.preview == Some(id) {
                s.set_preview(None, cx);
            }
            if !take.is_empty() {
                s.accept(id, Some(&take), cx);
            }
            s.reject(id, Some(&skip), cx);
        });
    }
}

fn outcome(set: &ChangeSet, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let count = |state| set.items.iter().filter(|i| i.state == state).count();
    let (accepted, conflicted, total) = (
        count(ItemState::Accepted),
        count(ItemState::Conflicted),
        set.items.len(),
    );
    let (icon, color, text) = if accepted == total {
        (IconName::CircleCheck, theme.green, "Accepted".to_string())
    } else if accepted == 0 && conflicted == 0 {
        (IconName::CircleX, theme.muted_foreground, "Rejected".into())
    } else {
        let mut text = format!("Accepted {accepted} of {total}");
        if conflicted > 0 {
            text.push_str(&format!(
                " · {conflicted} {}",
                if conflicted == 1 {
                    "conflict"
                } else {
                    "conflicts"
                }
            ));
        }
        let color = if conflicted > 0 {
            theme.red
        } else {
            theme.yellow
        };
        (IconName::CircleCheck, color, text)
    };
    h_flex()
        .h(px(24.))
        .px_1()
        .gap_1p5()
        .text_xs()
        .text_color(color)
        .child(Icon::new(icon).size_3p5())
        .child(text)
}

fn state_icon(state: ItemState, theme: &gpui_kit::component::Theme) -> impl IntoElement {
    let (icon, color) = match state {
        ItemState::Accepted => (IconName::Check, theme.green),
        ItemState::Rejected => (IconName::Close, theme.muted_foreground),
        ItemState::Conflicted => (IconName::CircleAlert, theme.red),
        ItemState::Pending => (IconName::Dash, theme.muted_foreground),
    };
    div()
        .size_4()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .child(Icon::new(icon).size_3().text_color(color))
}

fn id_hash(id: ChangeSetId) -> usize {
    id.0.as_u128() as usize
}

fn diff_kind(item: &ChangeItem) -> DiffKind {
    use Command::*;
    match &item.command {
        RippleDelete { .. } | DeleteClips { .. } | RemoveMarker { .. } | RemoveTrack { .. } => {
            DiffKind::Removed
        }
        AddMarker { .. }
        | AddCaption { .. }
        | AddTrack { .. }
        | AddOverlay { .. }
        | ApplyTitle { .. }
        | InsertClip { .. }
        | AddZoom { .. } => DiffKind::Added,
        _ => item
            .ops
            .first()
            .map(Op::diff_kind)
            .unwrap_or(DiffKind::Modified),
    }
}

/// Perceptual checks are the user's (PRD QA): audio changes need a listen.
fn needs_listen(item: &ChangeItem) -> bool {
    item.state == ItemState::Pending && item.command.category() == ToolCategory::Audio
}

/// Where on the timeline an item lives, for seeking.
pub fn item_time(item: &ChangeItem, project: &Project) -> Option<Time> {
    use Command::*;
    match &item.command {
        RippleDelete { range, .. }
        | AddMarker { range, .. }
        | EditMarker { range, .. }
        | AddZoom { range, .. }
        | AddCaption { range, .. }
        | ApplyTitle { range, .. }
        | AddOverlay { range, .. } => Some(range.start),
        Blade { at, .. } | InsertClip { at, .. } | SetPlaylist { at, .. } => Some(*at),
        _ => item.ops.iter().find_map(|op| match op {
            Op::InsertClip { clip, .. } | Op::RemoveClip { clip, .. } => Some(clip.start),
            Op::UpdateClip { before, .. } => project
                .timeline
                .clip(before.id)
                .map(|(_, c)| c.start)
                .or(Some(before.start)),
            Op::InsertMarker { marker } | Op::RemoveMarker { marker } => Some(marker.range.start),
            _ => None,
        }),
    }
}

fn describe(command: &Command) -> String {
    match command {
        Command::RippleDelete { range, .. } => {
            format!("Cut {:.1} s", range.duration().as_secs_f64())
        }
        other => {
            let name = other.name().replace('_', " ");
            let mut chars = name.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

fn warning_text(w: &Warning) -> SharedString {
    match w {
        Warning::TouchesProtected { note, .. } if note.is_empty() => {
            "Touches a protected range".into()
        }
        Warning::TouchesProtected { note, .. } => format!("Touches protected: {note}").into(),
        Warning::NeedsConfirmation { message } => message.clone().into(),
    }
}
