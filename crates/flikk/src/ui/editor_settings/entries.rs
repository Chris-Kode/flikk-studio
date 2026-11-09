//! Settings → Skills and Settings → Styles: the saved ones down the left,
//! the open one on the right. Typing saves after a pause; a name change
//! saves when the field is left or Enter is pressed, since it renames the
//! file.

use crate::state::EditorState;
use crate::ui::kit::{
    Button, DialogExt as _, EmptyState, Icon, IconName, Input, ScrollableElement as _, Textarea,
    h_flex, theme as t, v_flex,
};
use flikk_core::skills::{self, Draft, Entry, Kind};
use gpui_kit::base::input::{InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::time::Duration;

/// Quiet time after the last keystroke before the text is saved.
const SETTLE: Duration = Duration::from_millis(600);

/// The label column of the editor's rows.
const LABEL_W: f32 = 84.;

/// What the editor on the right holds.
#[derive(Clone, PartialEq, Eq)]
enum Open {
    Nothing,
    /// A saved entry, by id.
    Entry(String),
    /// One being written, saved once it has a valid name.
    New,
}

/// Where the page opens.
pub enum Focus {
    /// The entry shown last, or the first.
    Last,
    /// A saved entry, by id.
    Entry(String),
    /// A new, empty one.
    New,
}

pub struct EntriesTab {
    state: Entity<EditorState>,
    kind: Kind,
    entries: Vec<Entry>,
    open: Open,
    name: Entity<InputState>,
    description: Entity<InputState>,
    instructions: Entity<TextareaState>,
    error: Option<String>,
    /// Text typed into the open entry that is still settling; saved when it
    /// fires, or at once when the page moves on.
    pending: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EntriesTab {
    pub fn new(
        state: Entity<EditorState>,
        kind: Kind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (name_hint, instructions_hint) = match kind {
            Kind::Skill => ("tighten-intro", "Steps the agent follows when you run it"),
            Kind::Style => (
                "Documentary",
                "How edits should feel: pace, music, captions",
            ),
        };
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(name_hint));
        let description = cx.new(|cx| InputState::new(window, cx).placeholder("One line"));
        let instructions =
            cx.new(|cx| TextareaState::new(window, cx).placeholder(instructions_hint));
        let _subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.subscribe_in(&name, window, |this, _, event, window, cx| match event {
                InputEvent::Blur | InputEvent::PressEnter { .. } => this.save_name(window, cx),
                _ => {}
            }),
            cx.subscribe_in(&description, window, |this, _, event, window, cx| {
                this.text_event(event, window, cx)
            }),
            cx.subscribe_in(&instructions, window, |this, _, event, window, cx| {
                this.text_event(event, window, cx)
            }),
        ];
        Self {
            state,
            kind,
            entries: skills::list(kind),
            open: Open::Nothing,
            name,
            description,
            instructions,
            error: None,
            pending: None,
            _subscriptions,
        }
    }

    /// How many are saved, for the sidebar.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Read the files again and open `focus`. Called whenever the page is
    /// shown, so changes made by the agent or by hand show up.
    pub fn show(&mut self, focus: Focus, window: &mut Window, cx: &mut Context<Self>) {
        self.flush(window, cx);
        self.entries = skills::list(self.kind);
        let open = match focus {
            Focus::New => Open::New,
            Focus::Entry(id) if self.entry(&id).is_some() => Open::Entry(id),
            Focus::Entry(_) | Focus::Last => match &self.open {
                Open::Entry(id) if self.entry(id).is_some() => self.open.clone(),
                Open::New => Open::New,
                _ => self.first(),
            },
        };
        self.fill(open, window, cx);
    }

    fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    fn open_entry(&self) -> Option<&Entry> {
        match &self.open {
            Open::Entry(id) => self.entry(id),
            _ => None,
        }
    }

    fn first(&self) -> Open {
        self.entries
            .first()
            .map_or(Open::Nothing, |e| Open::Entry(e.id.clone()))
    }

    /// Put `open` into the fields.
    fn fill(&mut self, open: Open, window: &mut Window, cx: &mut Context<Self>) {
        self.pending = None;
        self.error = None;
        let draft = match &open {
            Open::Entry(id) => self.entry(id).map(Draft::from).unwrap_or_default(),
            _ => Draft::default(),
        };
        let focus_name = open == Open::New;
        self.open = open;
        self.name
            .update(cx, |i, cx| i.set_value(draft.name, window, cx));
        self.description
            .update(cx, |i, cx| i.set_value(draft.description, window, cx));
        self.instructions
            .update(cx, |i, cx| i.set_value(draft.instructions, window, cx));
        if focus_name {
            self.name.update(cx, |i, cx| i.focus(window, cx));
        }
        cx.notify();
    }

    fn select(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.flush(window, cx);
        self.fill(Open::Entry(id), window, cx);
    }

    fn new_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush(window, cx);
        self.fill(Open::New, window, cx);
    }

    fn text_event(&mut self, event: &InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            InputEvent::Change => {
                let timer = cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(SETTLE).await;
                    let _ = this.update_in(cx, |this, window, cx| this.flush(window, cx));
                });
                self.pending = Some(timer);
            }
            InputEvent::Blur => self.flush(window, cx),
            _ => {}
        }
    }

    /// Save settling text into the entry it was typed in. A new one waits
    /// for its name.
    fn flush(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.take().is_none() {
            return;
        }
        if let Some(entry) = self.open_entry() {
            let name = entry.name.clone();
            self.write(name, window, cx);
        }
    }

    /// Save with the name field's text: renames a saved entry, or saves a
    /// new one for the first time.
    fn save_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_owned();
        match &self.open {
            Open::Nothing => {}
            Open::New if name.is_empty() => {}
            Open::New => self.write(name, window, cx),
            Open::Entry(_) if self.open_entry().is_some_and(|e| e.name == name) => {}
            Open::Entry(_) => self.write(name, window, cx),
        }
    }

    /// Write the fields under `name`, unless nothing changed.
    fn write(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.pending = None;
        let existing = self.open_entry();
        let draft = Draft {
            name,
            description: self.description.read(cx).value().to_string(),
            instructions: self.instructions.read(cx).value().to_string(),
            by_agent: existing.is_some_and(|e| e.by_agent),
        };
        if existing.is_some_and(|e| Draft::from(e) == draft) {
            return;
        }
        let previous = existing.map(|e| e.id.clone());
        match skills::save(self.kind, previous.as_deref(), &draft) {
            Ok(saved) => {
                self.error = None;
                self.entries = skills::list(self.kind);
                let renamed = previous.as_deref() != Some(saved.id.as_str());
                self.open = Open::Entry(saved.id.clone());
                // The style picked in chat follows its rename.
                if renamed && self.kind == Kind::Style {
                    self.follow_rename(previous, saved.id, cx);
                }
                // The name field shows the name as saved (`/` dropped,
                // spaces tidied).
                if self.name.read(cx).value().as_ref() != saved.name {
                    self.name
                        .update(cx, |i, cx| i.set_value(saved.name, window, cx));
                }
            }
            Err(message) => self.error = Some(message),
        }
        cx.notify();
    }

    fn follow_rename(&self, from: Option<String>, to: String, cx: &mut Context<Self>) {
        let Some(from) = from else {
            return;
        };
        self.state.update(cx, |s, cx| {
            if s.settings.style.as_deref() == Some(from.as_str()) {
                let mut settings = s.settings.clone();
                settings.style = Some(to);
                s.set_settings(settings, cx);
            }
        });
    }

    fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.open_entry().cloned() else {
            return;
        };
        let this = cx.entity();
        let kind = self.kind;
        let title = match kind {
            Kind::Skill => format!("Delete /{}?", entry.name),
            Kind::Style => format!("Delete {}?", entry.name),
        };
        window.open_confirm(cx, move |dialog, _, _| {
            let (this, id) = (this.clone(), entry.id.clone());
            dialog
                .title(title.clone())
                .destructive()
                .confirm("Delete", move |window, cx| {
                    this.update(cx, |this, cx| this.delete(&id, window, cx));
                    true
                })
        });
    }

    fn delete(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        skills::delete(self.kind, id);
        if self.kind == Kind::Style {
            self.state.update(cx, |s, cx| {
                if s.settings.style.as_deref() == Some(id) {
                    let mut settings = s.settings.clone();
                    settings.style = None;
                    s.set_settings(settings, cx);
                }
            });
        }
        self.entries = skills::list(self.kind);
        let open = self.first();
        self.fill(open, window, cx);
    }

    fn render_list(&self, in_chat: Option<&str>, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.entries.iter().enumerate().map(|(ix, entry)| {
            let on = self.open == Open::Entry(entry.id.clone());
            let id = entry.id.clone();
            let label = match self.kind {
                Kind::Skill => div()
                    .font_family(t::mono())
                    .text_size(px(11.))
                    .child(format!("/{}", entry.name)),
                Kind::Style => div().child(entry.name.clone()),
            };
            h_flex()
                .id(("entry", ix))
                .flex_none()
                .h(px(30.))
                .gap(px(8.))
                .px(px(8.))
                .rounded(t::RADIUS)
                .cursor_pointer()
                .text_color(if on { t::T1 } else { t::T2 })
                .when(on, |el| el.bg(t::AC_WASH))
                .when(!on, |el| el.hover(|s| s.bg(t::HV).text_color(t::T1)))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.select(id.clone(), window, cx)),
                )
                .child(label.flex_1().min_w_0().truncate())
                .when(in_chat == Some(entry.id.as_str()), |el| {
                    el.child(div().text_size(px(10.)).text_color(t::T3).child("In chat"))
                })
                .when(entry.by_agent, |el| {
                    el.child(
                        div()
                            .id(("by-agent", ix))
                            .tooltip(crate::ui::kit::Tooltip::text("Made by the agent"))
                            .child(
                                Icon::new(IconName::MessageSquareText)
                                    .size(px(12.))
                                    .color(t::T3),
                            ),
                    )
                })
        });
        v_flex()
            .id("entries")
            .w(px(184.))
            .flex_none()
            .h_full()
            .gap(px(2.))
            .pt(px(8.))
            .pr(px(8.))
            .border_r_1()
            .border_color(t::LN)
            .children(rows)
            .when(self.open == Open::New, |el| {
                el.child(
                    h_flex()
                        .flex_none()
                        .h(px(30.))
                        .px(px(8.))
                        .rounded(t::RADIUS)
                        .bg(t::AC_WASH)
                        .text_color(t::T3)
                        .child(match self.kind {
                            Kind::Skill => "New skill",
                            Kind::Style => "New style",
                        }),
                )
            })
            .overflow_y_scrollbar()
    }

    fn render_editor(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let field_row = |label: &'static str, field: AnyElement| {
            h_flex()
                .flex_none()
                .h(px(34.))
                .gap(px(12.))
                .child(
                    div()
                        .w(px(LABEL_W))
                        .flex_none()
                        .text_color(t::T2)
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(field))
        };
        let name = Input::new(&self.name).when(self.kind == Kind::Skill, |input| {
            input.prefix(div().font_family(t::mono()).child("/"))
        });
        let edited = self
            .open_entry()
            .and_then(|e| chrono::DateTime::from_timestamp(e.edited, 0))
            .map(|at| {
                format!(
                    "Edited {}",
                    at.with_timezone(&chrono::Local).format("%-d %b")
                )
            });
        let footer = h_flex()
            .flex_none()
            .h(px(22.))
            .mt(px(10.))
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.))
                    .when_some(self.error.clone(), |el, error| {
                        el.text_color(t::ERR_TX).child(error)
                    })
                    .when(self.error.is_none(), |el| {
                        el.text_color(t::T3).children(edited)
                    }),
            )
            .when(matches!(self.open, Open::Entry(_)), |el| {
                el.child(
                    Button::new("entry-delete")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Trash)
                        .label("Delete")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.confirm_delete(window, cx)),
                        ),
                )
            });
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .pt(px(8.))
            .pl(px(16.))
            .child(field_row("Name", name.into_any_element()))
            .when(self.kind == Kind::Skill, |el| {
                el.child(field_row(
                    "Description",
                    Input::new(&self.description).into_any_element(),
                ))
            })
            .child(
                div()
                    .flex_none()
                    .mt(px(12.))
                    .mb(px(6.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t::T3)
                    .child("Instructions"),
            )
            .child(Textarea::new(&self.instructions).flex_1().min_h(px(120.)))
            .child(footer)
    }
}

impl Render for EntriesTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, new_label, icon, empty) = match self.kind {
            Kind::Skill => ("Skills", "New skill", IconName::SquareSlash, "No skills"),
            Kind::Style => ("Styles", "New style", IconName::Palette, "No styles"),
        };
        let this = cx.entity().downgrade();
        let new_button = move |id: &'static str| {
            let this = this.clone();
            Button::new(id)
                .small()
                .icon(IconName::Plus)
                .label(new_label)
                .on_click(move |_, window, cx| {
                    let _ = this.update(cx, |this, cx| this.new_entry(window, cx));
                })
        };
        let body = if self.entries.is_empty() && self.open == Open::Nothing {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .child(EmptyState::new(icon, empty).child(new_button("entries-new-empty")))
                .into_any_element()
        } else {
            let in_chat = self.state.read(cx).settings.style.clone();
            h_flex()
                .flex_1()
                .min_h_0()
                .items_start()
                .child(self.render_list(in_chat.as_deref(), cx))
                .child(self.render_editor(cx))
                .into_any_element()
        };
        let has_any = !self.entries.is_empty() || self.open != Open::Nothing;
        v_flex()
            .size_full()
            .px(px(24.))
            .pt(px(18.))
            .pb(px(16.))
            .child(
                h_flex()
                    .flex_none()
                    .h(px(26.))
                    .mb(px(10.))
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .when(has_any, |el| el.child(new_button("entries-new"))),
            )
            .child(div().flex_none().h(px(1.)).bg(t::LN))
            .child(body)
    }
}
