//! Cards for the agent's changes to the user's skills and styles: one
//! waiting on the user above the composer (Deny / Approve, like every
//! request), and one in the thread for a change it was allowed to make,
//! with Open and, for a new one, Undo.

use super::ChatPanel;
use crate::ui::editor_settings::{EditorSettings, Focus};
use crate::ui::kit::{Button, Icon, IconName, h_flex, theme as t, v_flex};
use flikk_core::agent::ActivityEntry;
use flikk_core::skills::{self, Kind};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::Value;

/// The tools whose calls these cards show.
pub(super) fn is_entry_tool(tool: &str) -> bool {
    matches!(
        tool,
        "save_skill" | "save_style" | "delete_skill" | "delete_style"
    )
}

/// A change to one skill or style, as `tools::skills` wrote it up.
struct Change {
    kind: Kind,
    name: String,
    instructions: String,
    /// The instructions it replaces, for an update.
    before: Option<String>,
    deleting: bool,
    created: bool,
}

impl Change {
    fn read(tool: &str, args: &Value) -> Option<Self> {
        let text = |key| args.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(Self {
            kind: match text("kind")?.as_str() {
                "style" => Kind::Style,
                _ => Kind::Skill,
            },
            name: text("name")?,
            instructions: text("instructions").unwrap_or_default(),
            before: text("before"),
            deleting: tool.starts_with("delete_"),
            created: args.get("created").and_then(Value::as_bool) == Some(true),
        })
    }

    fn icon(&self) -> IconName {
        match self.kind {
            Kind::Skill => IconName::SquareSlash,
            Kind::Style => IconName::Palette,
        }
    }

    fn title(&self) -> String {
        match self.kind {
            Kind::Skill => format!("/{}", self.name),
            Kind::Style => self.name.clone(),
        }
    }

    fn verb(&self) -> String {
        let noun = self.kind.noun();
        if self.deleting {
            format!("Delete {noun}")
        } else if self.before.is_some() {
            format!("Update {noun}")
        } else {
            format!("New {noun}")
        }
    }

    /// The saved entry's id, if it is still there.
    fn saved_id(&self) -> Option<String> {
        skills::id_of(self.kind, &self.name).filter(|id| skills::get(self.kind, id).is_some())
    }
}

/// Header, a few lines of what changes, and `footer` when there is one.
fn card(change: &Change, meta: String, footer: Option<Div>) -> Div {
    let lines = preview(change);
    v_flex()
        .w_full()
        .overflow_hidden()
        .rounded(t::RADIUS)
        .border_1()
        .border_color(t::LN)
        .bg(t::G)
        .child(
            h_flex()
                .h(px(32.))
                .px(px(10.))
                .gap(px(8.))
                .when(!lines.is_empty(), |el| el.border_b_1().border_color(t::LN))
                .child(Icon::new(change.icon()).color(t::T2))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .when(change.kind == Kind::Skill, |el| {
                            el.font_family(t::mono()).text_size(px(11.))
                        })
                        .child(change.title()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(t::T3)
                        .child(meta),
                ),
        )
        .when(!lines.is_empty(), |el| {
            el.child(
                v_flex()
                    .px(px(10.))
                    .py(px(8.))
                    .gap(px(2.))
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .children(lines),
            )
        })
        .children(footer.map(|footer| {
            footer
                .px(px(8.))
                .py(px(6.))
                .gap(px(6.))
                .justify_end()
                .border_t_1()
                .border_color(t::LN)
        }))
}

/// For an update, the first line that goes and the first that comes;
/// otherwise the start of the instructions.
fn preview(change: &Change) -> Vec<Div> {
    if change.deleting {
        return Vec::new();
    }
    let line = |text: &str| div().w_full().truncate().child(text.to_owned());
    if let Some(before) = &change.before {
        let old: Vec<&str> = before.lines().filter(|l| !l.trim().is_empty()).collect();
        let new: Vec<&str> = change
            .instructions
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();
        let gone = old.iter().find(|l| !new.contains(l));
        let came = new.iter().find(|l| !old.contains(l));
        let mut out = Vec::new();
        if let Some(gone) = gone {
            out.push(line(gone).text_color(t::T3).line_through());
        }
        if let Some(came) = came {
            out.push(line(came).text_color(t::T1));
        }
        if !out.is_empty() {
            return out;
        }
    }
    change
        .instructions
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(3)
        .enumerate()
        .map(|(i, l)| line(l).text_color(if i == 0 { t::T1 } else { t::T2 }))
        .collect()
}

impl ChatPanel {
    /// A request to change a skill or style, waiting on the user.
    pub(super) fn render_entry_approval(
        &self,
        id: u64,
        tool: &str,
        args: &Value,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let change = Change::read(tool, args)?;
        let deny = Button::new(("deny", id as usize))
            .label("Deny")
            .small()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.update(cx, |s, cx| s.deny_approval(id, cx))
            }));
        let approve = Button::new(("approve", id as usize))
            .label("Approve")
            .small()
            .on_click(
                cx.listener(move |this, _, _, cx| this.state.update(cx, |s, cx| s.approve(id, cx))),
            );
        // A delete can't be taken back: its Approve reads as destructive.
        let approve = if change.deleting {
            approve.destructive()
        } else {
            approve.primary()
        };
        let footer = h_flex().child(deny).child(approve);
        Some(card(&change, change.verb(), Some(footer)).into_any_element())
    }

    /// A save the agent was allowed to make, in the thread.
    pub(super) fn render_saved_entry(
        &self,
        key: (usize, usize),
        entry: &ActivityEntry,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // Only a save that ran records `kind`; one sent for approval shows
        // above the composer instead.
        entry.args.get("kind")?;
        if !entry.ok {
            return None;
        }
        let change = Change::read(&entry.tool, &entry.args)?;
        let saved = change.saved_id();
        let meta = match (&saved, change.created) {
            (None, true) => "Undone".to_owned(),
            (None, false) => "Deleted".to_owned(),
            (Some(_), true) => format!("New {}", change.kind.noun()),
            (Some(_), false) => format!("Updated {}", change.kind.noun()),
        };
        let kind = change.kind;
        let footer = saved.is_some().then(h_flex).map(|footer| {
            footer
                .when(change.created && saved.is_some(), |el| {
                    let id = saved.clone().unwrap_or_default();
                    el.child(
                        Button::new(("entry-undo", key.0 * 64 + key.1))
                            .ghost()
                            .small()
                            .icon(IconName::Undo2)
                            .label("Undo")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                skills::delete(kind, &id);
                                cx.notify();
                            })),
                    )
                })
                .when_some(saved, |el, id| {
                    el.child(
                        Button::new(("entry-open", key.0 * 64 + key.1))
                            .small()
                            .label("Open")
                            .on_click(move |_, window, cx| {
                                EditorSettings::open_entries(
                                    kind,
                                    Focus::Entry(id.clone()),
                                    window,
                                    cx,
                                )
                            }),
                    )
                })
        });
        Some(card(&change, meta, footer).into_any_element())
    }
}
