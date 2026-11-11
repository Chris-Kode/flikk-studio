//! Change-set card (PRD AG4–AG6), fixed above the composer, one per set:
//! title with its diff pills, the review list with per-item selection,
//! Preview / Review / Reject / Accept, and the outcome once resolved.

use super::ChatPanel;
use crate::ui::format::clock_truncated;
use crate::ui::kit::{
    Badge, Button, Checkbox, Dot, Icon, IconName, ScrollableElement as _, Tone, Tooltip, h_flex,
    theme as t, v_flex,
};
use flikk_core::changeset::{ChangeItem, ChangeSet, ChangeSetState, ItemState};
use flikk_core::check::{Finding, Severity};
use flikk_core::command::{Command, ToolCategory, Warning};
use flikk_core::model::Project;
use flikk_core::ops::{DiffKind, Op};
use flikk_core::{ChangeItemId, ChangeSetId, Time};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};

/// Per-card UI state, keyed by change set.
#[derive(Default)]
pub struct Review {
    pub open: bool,
    /// Pending items the user unticked.
    pub excluded: HashSet<ChangeItemId>,
    /// Item to scroll into view on the next paint (picked on the timeline).
    pub reveal: Option<ChangeItemId>,
    /// The review list's scroll position.
    pub scroll: ScrollHandle,
}

pub type Reviews = HashMap<ChangeSetId, Review>;

impl ChatPanel {
    /// The chat's change sets with proposals in them, each once however
    /// many turns grew it; a resolved one until the user's next message.
    pub(super) fn render_change_sets(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let sets: Vec<ChangeSetId> = self
            .state
            .read(cx)
            .bus
            .chat_change_sets()
            .filter(|s| !s.items.is_empty())
            .filter(|s| s.state != ChangeSetState::Resolved || self.shown.contains(&s.id))
            .map(|s| s.id)
            .collect();
        self.shown.extend(sets.iter().copied());
        sets.into_iter()
            .map(|id| self.render_change_set(id, cx))
            .collect()
    }

    fn render_change_set(&mut self, id: ChangeSetId, cx: &mut Context<Self>) -> AnyElement {
        let state = self.state.read(cx);
        let Some(set) = state.bus.change_set(id).cloned() else {
            return div().into_any_element();
        };
        let previewing = state.preview == Some(id);
        let project = state.bus.project().clone();
        let focused = state
            .focused_change_item()
            .filter(|(set, _)| *set == id)
            .map(|(_, item)| item);
        let review = self.reviews.entry(id).or_default();
        let open = review.open;
        let excluded = review.excluded.clone();
        let resolved = set.state == ChangeSetState::Resolved;
        let list = open.then(|| self.render_items(&set, &project, focused, cx));

        let review_button = Button::new(("review", id_hash(id)))
            .label("Review")
            .ghost()
            .small()
            .selected(open)
            .on_click(cx.listener(move |this, _, _, cx| {
                let r = this.reviews.entry(id).or_default();
                r.open = !r.open;
                this.remeasure(cx);
            }));
        let card = v_flex()
            .flex_none()
            .w_full()
            .overflow_hidden()
            .rounded(t::RADIUS_LG)
            .border_1()
            .bg(t::R1);
        if resolved {
            return card
                .border_color(t::LN)
                .children(list.map(|list| div().border_b_1().border_color(t::LN).child(list)))
                .child(
                    h_flex()
                        .h(px(34.))
                        .pl(px(10.))
                        .pr(px(6.))
                        .gap(px(8.))
                        .child(outcome(&set))
                        .child(review_button),
                )
                .into_any_element();
        }

        // The agent is still adding to it this turn.
        let working = set.state == ChangeSetState::Open;
        let total = set.items.len();
        // While reviewing, the primary action takes the ticked items.
        let chosen = set
            .pending_items()
            .filter(|i| !open || !excluded.contains(&i.id))
            .count();
        let footer = h_flex()
            .gap(px(4.))
            .px(px(8.))
            .py(px(6.))
            .border_t_1()
            .border_color(t::LN)
            .child(
                Button::new(("preview", id_hash(id)))
                    // The eye shows the state, like a track's visibility.
                    .icon(if previewing {
                        IconName::Eye
                    } else {
                        IconName::EyeOff
                    })
                    .label(if previewing { "Previewing" } else { "Preview" })
                    .ghost()
                    .small()
                    .selected(previewing)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |s, cx| {
                            let next = (s.preview != Some(id)).then_some(id);
                            s.set_preview(next, cx)
                        })
                    })),
            )
            .child(div().flex_1())
            .child(review_button)
            .child(
                Button::new(("reject", id_hash(id)))
                    .label("Reject")
                    .small()
                    .disabled(working)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |s, cx| {
                            if s.preview == Some(id) {
                                s.set_preview(None, cx);
                            }
                            s.reject(id, None, cx)
                        })
                    })),
            )
            .child(
                Button::new(("accept", id_hash(id)))
                    .label(if chosen == total {
                        "Accept all".to_string()
                    } else {
                        format!("Accept {chosen}")
                    })
                    .primary()
                    .small()
                    .disabled(chosen == 0 || working)
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
                    })),
            );
        card.border_color(if previewing {
            t::AC
        } else {
            t::alpha(t::AC, 0.33)
        })
        .child(self.render_set_header(&set, open, &excluded, cx))
        .children(list)
        .child(footer)
        .into_any_element()
    }

    /// Title, diff pills and, while reviewing, the tick-all toggle.
    fn render_set_header(
        &self,
        set: &ChangeSet,
        open: bool,
        excluded: &HashSet<ChangeItemId>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = set.id;
        let total = set.items.len();
        let found = set.findings.checked();
        let warnings = set.items.iter().filter(|i| !i.warnings.is_empty()).count();
        let listen = set.items.iter().filter(|i| needs_listen(i)).count();
        let cut: f64 = set
            .items
            .iter()
            .filter_map(|i| match &i.command {
                Command::RippleDelete { range, .. } => Some(range.duration().as_secs_f64()),
                _ => None,
            })
            .sum();
        let count = |icon: IconName, color: Rgba, n: usize| {
            h_flex()
                .flex_none()
                .gap(px(3.))
                .text_size(px(11.))
                .text_color(color)
                .child(Icon::new(icon).size(px(11.)))
                .child(n.to_string())
        };
        h_flex()
            .h(px(34.))
            .pl(px(10.))
            .pr(px(8.))
            .gap(px(8.))
            .border_b_1()
            .border_color(t::LN)
            .child(
                h_flex()
                    .flex_none()
                    .justify_center()
                    .size(px(20.))
                    .rounded(t::RADIUS)
                    .bg(t::alpha(t::AC, 0.15))
                    .child(Icon::new(IconName::ListChecks).size(px(12.)).color(t::AC)),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(set.title.clone()),
            )
            .when(warnings > 0, |el| {
                el.child(count(IconName::TriangleAlert, t::WARN, warnings))
            })
            .when(listen > 0, |el| {
                el.child(
                    div()
                        .id(("listen", id_hash(id)))
                        .child(count(IconName::Mic, t::T2, listen))
                        .tooltip(Tooltip::text("Needs a listen")),
                )
            })
            .when(!found.is_empty(), |el| {
                el.child(
                    h_flex()
                        .id(("findings", id_hash(id)))
                        .flex_none()
                        .gap(px(4.))
                        .child(finding_dot(worst(found)))
                        .child(
                            div()
                                .font_family(t::mono())
                                .text_size(px(11.))
                                .text_color(t::T3)
                                .child(found.len().to_string()),
                        )
                        .tooltip(Tooltip::text(if found.len() == 1 {
                            "1 finding".to_string()
                        } else {
                            format!("{} findings", found.len())
                        })),
                )
            })
            .child(div().flex_1())
            .when(cut > 0., |el| {
                el.child(Badge::new(format!("−{cut:.1} s")).err().mono())
            })
            .when(!open, |el| {
                el.child(
                    Badge::new(format!(
                        "{total} {}",
                        if total == 1 { "change" } else { "changes" }
                    ))
                    .mono(),
                )
            })
            .when(open, |el| {
                let all = excluded.is_empty();
                el.child(
                    Button::new(("toggle-all", id_hash(id)))
                        .label(if all { "None" } else { "All" })
                        .ghost()
                        .xsmall()
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
    }

    fn render_items(
        &mut self,
        set: &ChangeSet,
        project: &Project,
        focused: Option<ChangeItemId>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = set.id;
        let review = self.reviews.entry(id).or_default();
        let excluded = review.excluded.clone();
        let reveal = review.reveal.take();
        let scroll = review.scroll.clone();
        let mut items: Vec<_> = set
            .items
            .iter()
            .enumerate()
            .map(|(ix, item)| (ix, item, item_time(item, project)))
            .collect();
        items.sort_by_key(|(ix, _, at)| (at.is_none(), *at, *ix));
        if let Some(row) = reveal.and_then(|r| items.iter().position(|(_, i, _)| i.id == r)) {
            scroll.scroll_to_item(row);
        }
        let found: Vec<AnyElement> = set
            .findings
            .checked()
            .iter()
            .enumerate()
            .map(|(ix, f)| self.render_finding(ix, f, cx))
            .collect();
        let rows = items.into_iter().map(|(ix, item, at)| {
            let item_id = item.id;
            let tint = match diff_kind(item) {
                DiffKind::Added => t::OK,
                DiffKind::Removed => t::ERR,
                DiffKind::Modified => t::WARN,
            };
            let reason = if ix == 0 && item.reason == set.title {
                describe(&item.command)
            } else {
                item.reason.clone()
            };
            let pending = item.state == ItemState::Pending;
            let ticked = pending && !excluded.contains(&item_id);
            let conflicted = item.state == ItemState::Conflicted;
            let warning = item.warnings.first().map(warning_text);
            h_flex()
                .id(("item", ix))
                .relative()
                .flex_none()
                .h(px(28.))
                .px(px(10.))
                .gap(px(8.))
                .cursor_pointer()
                .hover(|el| el.bg(t::HV))
                .when(focused == Some(item_id), |el| {
                    el.bg(t::alpha(t::AC, 0.08)).child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(px(2.))
                            .bg(t::AC),
                    )
                })
                .child(if pending {
                    Checkbox::new(("item-check", ix))
                        .checked(ticked)
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
                    state_icon(item.state).into_any_element()
                })
                .child(
                    div()
                        .flex_none()
                        .size(px(6.))
                        .rounded_full()
                        .bg(tint)
                        .when(pending && !ticked, |el| el.opacity(0.5)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(if ticked {
                            t::T1
                        } else if pending {
                            t::T3
                        } else {
                            t::T2
                        })
                        .child(reason),
                )
                .when(needs_listen(item), |el| {
                    el.child(Icon::new(IconName::Mic).size(px(12.)).color(t::T2))
                })
                .when_some(warning, |el, text| {
                    el.child(
                        div()
                            .id(("item-warning", ix))
                            .child(
                                Icon::new(IconName::TriangleAlert)
                                    .size(px(12.))
                                    .color(t::WARN),
                            )
                            .tooltip(Tooltip::text(text)),
                    )
                })
                .when(conflicted, |el| {
                    el.child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(t::ERR)
                            .child("Conflict"),
                    )
                })
                .when_some(at, |el, at| {
                    el.child(
                        div()
                            .flex_none()
                            .font_family(t::mono())
                            .text_size(px(10.))
                            .text_color(if focused == Some(item_id) {
                                t::T2
                            } else {
                                t::T3
                            })
                            .child(clock_truncated(at)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.seek(at, cx))
                    }))
                })
        });
        let findings = (!found.is_empty()).then(|| {
            v_flex()
                .flex_none()
                .mt(px(4.))
                .pt(px(4.))
                .border_t_1()
                .border_color(t::LN)
                .children(found)
        });
        div()
            .relative()
            .child(
                v_flex()
                    .id(("items", id_hash(id)))
                    .max_h(px(280.))
                    .py(px(4.))
                    .text_size(px(12.))
                    .children(rows)
                    .children(findings)
                    .track_scroll(&scroll)
                    .overflow_y_scroll(),
            )
            .vertical_scrollbar(&scroll)
            .into_any_element()
    }

    /// One finding the set brings in: its dot, what it is and where;
    /// clicking it moves the playhead there.
    fn render_finding(&self, ix: usize, f: &Finding, cx: &mut Context<Self>) -> AnyElement {
        let at = f.range.start;
        h_flex()
            .id(("finding", ix))
            .flex_none()
            .h(px(28.))
            .px(px(10.))
            .gap(px(8.))
            .cursor_pointer()
            .hover(|el| el.bg(t::HV))
            // In the column of the items' checkboxes.
            .child(
                h_flex()
                    .flex_none()
                    .justify_center()
                    .size(px(14.))
                    .child(finding_dot(f.severity)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(t::T2)
                    .child(f.note.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(t::mono())
                    .text_size(px(10.))
                    .text_color(t::T3)
                    .child(clock_truncated(at)),
            )
            .on_click(
                cx.listener(move |this, _, _, cx| this.state.update(cx, |s, cx| s.seek(at, cx))),
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

/// What came of a resolved set, in its colour.
fn outcome(set: &ChangeSet) -> impl IntoElement {
    let count = |state| set.items.iter().filter(|i| i.state == state).count();
    let (accepted, conflicted, total) = (
        count(ItemState::Accepted),
        count(ItemState::Conflicted),
        set.items.len(),
    );
    let (icon, color, text) = if accepted == total {
        (IconName::CircleCheck, t::OK, "Accepted".to_string())
    } else if accepted == 0 && conflicted == 0 {
        (IconName::CircleX, t::T2, "Rejected".into())
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
        (IconName::CircleCheck, t::OK, text)
    };
    h_flex()
        .flex_1()
        .min_w_0()
        .gap(px(5.))
        .text_size(px(11.))
        .text_color(color)
        .child(Icon::new(icon).size(px(13.)))
        .child(div().min_w_0().truncate().child(text))
}

/// A finding's dot: `WARN` for what a viewer will notice, quiet for what
/// may be meant.
pub(crate) fn finding_dot(severity: Severity) -> Dot {
    Dot(match severity {
        Severity::Warning => Tone::Warn,
        Severity::Notice => Tone::Neutral,
    })
}

/// The gravest severity among `found`.
fn worst(found: &[Finding]) -> Severity {
    found
        .iter()
        .map(|f| f.severity)
        .min()
        .unwrap_or(Severity::Notice)
}

fn state_icon(state: ItemState) -> impl IntoElement {
    let (icon, color) = match state {
        ItemState::Accepted => (IconName::Check, t::OK),
        ItemState::Rejected => (IconName::X, t::T3),
        ItemState::Conflicted => (IconName::CircleAlert, t::ERR),
        ItemState::Pending => (IconName::Minus, t::T3),
    };
    h_flex()
        .flex_none()
        .justify_center()
        .size(px(14.))
        .child(Icon::new(icon).size(px(12.)).color(color))
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
fn item_time(item: &ChangeItem, project: &Project) -> Option<Time> {
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
