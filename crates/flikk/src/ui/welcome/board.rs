//! The agents on the home page: a line that starts a new project with an
//! agent at work on it, and a card for each open project, with what its
//! agent is doing, a field to write to it, Stop and Close. A card opens its
//! project in the editor; the Home button there comes back.
//!
//! Turns started here run unattended (`EditorState::send_unattended`).

use super::{Welcome, WelcomeEvent, aspect, meta};
use crate::projects;
use crate::state::{AgentState, Doing, OpenProject};
use crate::ui::kit::{Button, Icon, IconName, Input, Spinner, h_flex, theme as t, v_flex};
use flikk_core::ProjectId;
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;

/// The field that writes to one open project's agent.
pub(super) struct Reply {
    input: Entity<InputState>,
    _sent: Subscription,
}

impl Welcome {
    /// The field a new agent's first message is written in.
    pub(super) fn prompt_input(window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        cx.new(|cx| InputState::new(window, cx).placeholder("Describe a video"))
    }

    /// Enter in the prompt starts the agent.
    pub(super) fn watch_prompt(
        prompt: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe_in(prompt, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.start(cx);
            }
        })
    }

    /// Whether the keys go to the prompt or a reply field: Enter sends
    /// there, and is not the list's.
    pub(super) fn writing(&self, window: &Window, cx: &App) -> bool {
        let focused =
            |input: &Entity<InputState>| input.read(cx).focus_handle(cx).is_focused(window);
        focused(&self.prompt) || self.replies.values().any(|reply| focused(&reply.input))
    }

    /// Ask the window for a new project whose agent starts on the prompt,
    /// with the footage picked for it. The prompt stays until the project
    /// is made (see [`Self::agent_started`]): New project can be cancelled.
    fn start(&mut self, cx: &mut Context<Self>) {
        let prompt = self.prompt.read(cx).value().trim().to_owned();
        if prompt.is_empty() || self.state.read(cx).projects_wait() {
            return;
        }
        cx.emit(WelcomeEvent::Start {
            prompt,
            footage: self.footage.clone(),
        });
    }

    /// The agent asked for with the prompt started: the prompt and its
    /// footage are its own now.
    pub fn agent_started(&mut self, cx: &mut Context<Self>) {
        self.footage.clear();
        self.clear_prompt = true;
        cx.notify();
    }

    /// Pick the files and folders the next agent's project starts with.
    fn pick_footage(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.footage = paths;
                cx.notify();
            });
        })
        .detach();
    }

    /// Keep a reply field for each open project, and none for a closed
    /// one; empty the prompt once its agent started.
    fn sync_fields(&mut self, open: &[OpenProject], window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.clear_prompt) {
            self.prompt
                .update(cx, |prompt, cx| prompt.set_value("", window, cx));
        }
        self.replies
            .retain(|id, _| open.iter().any(|project| project.id == *id));
        for project in open {
            let id = project.id;
            if self.replies.contains_key(&id) {
                continue;
            }
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Message"));
            let _sent = cx.subscribe_in(
                &input,
                window,
                move |this, input, ev: &InputEvent, window, cx| {
                    if !matches!(ev, InputEvent::PressEnter { .. }) {
                        return;
                    }
                    let text = input.read(cx).value().trim().to_owned();
                    if text.is_empty() || !this.write_to(id, text, cx) {
                        return;
                    }
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                },
            );
            self.replies.insert(id, Reply { input, _sent });
        }
    }

    /// Send `text` to `project`'s agent, unless it is busy. Whether it
    /// went.
    fn write_to(&mut self, project: ProjectId, text: String, cx: &mut Context<Self>) -> bool {
        let busy = (self.state.read(cx).open_projects().iter())
            .any(|open| open.id == project && matches!(open.agent, AgentState::Working(_)));
        if !busy {
            self.state
                .update(cx, |s, cx| s.send_unattended(project, text, cx));
        }
        !busy
    }

    fn enter(&mut self, project: ProjectId, cx: &mut Context<Self>) {
        cx.emit(WelcomeEvent::ForgetFootage);
        self.state.update(cx, |s, cx| s.enter_project(project, cx));
    }

    /// The prompt, with the footage picked for it and Start.
    fn start_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let footage = self.footage.len();
        let mut prompt = Input::new(&self.prompt).bg(t::P).rounded(t::RADIUS);
        if footage > 0 {
            prompt = prompt.suffix(meta(match footage {
                1 => "1 item".to_owned(),
                n => format!("{n} items"),
            }));
        }
        h_flex()
            .gap(px(6.))
            .child(div().flex_1().min_w_0().child(prompt))
            .child(
                Button::new("agent-footage")
                    .ghost()
                    .icon(IconName::Paperclip)
                    .tooltip("Add footage")
                    .accent(footage > 0)
                    .on_click(cx.listener(|this, _, _, cx| this.pick_footage(cx))),
            )
            .child(
                Button::new("agent-start")
                    .icon(IconName::ArrowUp)
                    .label("Start")
                    .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
            )
    }

    /// An open project: what its agent is doing, a field to write to it,
    /// Stop while it works, and Close.
    fn card(&self, ix: usize, open: &OpenProject, cx: &mut Context<Self>) -> AnyElement {
        let id = open.id;
        let working = matches!(open.agent, AgentState::Working(_));
        let glyph = match &open.agent {
            AgentState::Working(_) => Spinner::new().size(px(14.)).into_any_element(),
            AgentState::Asking { .. } => Icon::new(IconName::MessageCircleQuestionMark)
                .color(t::T1)
                .into_any_element(),
            AgentState::Failed { .. } => Icon::new(IconName::TriangleAlert)
                .color(t::WARN)
                .into_any_element(),
            AgentState::Idle { .. } => Icon::new(IconName::Clapperboard)
                .color(t::T3)
                .into_any_element(),
        };
        let line = match &open.agent {
            AgentState::Working(Doing::Thinking) => div().text_color(t::T3).child("Thinking"),
            AgentState::Working(Doing::Tool(tool)) => {
                div().font_family(t::mono()).child(tool.clone())
            }
            AgentState::Working(Doing::Writing(text)) => div().child(text.clone()),
            AgentState::Asking { question, .. } => div().text_color(t::T1).child(question.clone()),
            AgentState::Failed { message } => div().text_color(t::ERR_TX).child(message.clone()),
            AgentState::Idle { reply: Some(reply) } => div().child(reply.clone()),
            AgentState::Idle { reply: None } => div().text_color(t::T3).child("No messages"),
        };
        let choices = match &open.agent {
            AgentState::Asking { choices, .. } => choices.clone(),
            _ => Vec::new(),
        };
        let s = &open.sequence;
        let format = format!("{} · {}p", aspect(s.width, s.height), s.width.min(s.height));
        let head = v_flex()
            .id(("open-project", ix))
            .flex_1()
            .min_w_0()
            .gap(px(4.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.enter(id, cx)))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        h_flex()
                            .flex_none()
                            .w(px(14.))
                            .justify_center()
                            .child(glyph),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t::T1)
                            .child(open.name.clone()),
                    )
                    .when(open.edited, |row| {
                        row.child(div().flex_none().text_color(t::T3).child("Edited"))
                    })
                    .child(div().flex_1())
                    .child(meta(format))
                    .child(meta(crate::ui::format::clock_padded(open.duration)).text_color(t::T2)),
            )
            // Under the name, past the glyph's column.
            .child(div().pl(px(22.)).truncate().text_color(t::T2).child(line));
        v_flex()
            .gap(px(8.))
            .p(px(10.))
            .rounded(px(6.))
            .border_1()
            .border_color(t::LN)
            .bg(t::P)
            .text_size(px(11.))
            .child(
                h_flex().items_start().gap(px(8.)).child(head).child(
                    h_flex()
                        .flex_none()
                        .gap(px(2.))
                        .when(working, |row| {
                            row.child(
                                Button::new(("open-stop", ix))
                                    .ghost()
                                    .small()
                                    .icon(IconName::Square)
                                    .tooltip("Stop")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.update(cx, |s, cx| s.stop_agent(id, cx))
                                    })),
                            )
                        })
                        .child(
                            Button::new(("open-close", ix))
                                .ghost()
                                .small()
                                .icon(IconName::X)
                                .tooltip("Close")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    projects::close(&this.state, id, window, cx)
                                })),
                        ),
                ),
            )
            .when(!choices.is_empty(), |card| {
                card.child(h_flex().pl(px(22.)).gap(px(6.)).flex_wrap().children(
                    choices.into_iter().enumerate().map(|(n, choice)| {
                        let answer = choice.clone();
                        Button::new(("open-choice", ix * 16 + n))
                            .small()
                            .label(choice)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.write_to(id, answer.clone(), cx);
                            }))
                    }),
                ))
            })
            .children(self.replies.get(&id).map(|reply| {
                div()
                    .pl(px(22.))
                    .child(Input::new(&reply.input).small().disabled(working))
            }))
            .into_any_element()
    }

    /// The agents: the line that starts one, then the open projects.
    pub(super) fn board(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.state.read(cx).open_projects();
        self.sync_fields(&open, window, cx);
        let working = self.state.read(cx).agents_working();
        let cards: Vec<_> = (open.iter().enumerate())
            .map(|(ix, project)| self.card(ix, project, cx))
            .collect();
        v_flex()
            .gap(px(8.))
            .child(
                h_flex()
                    .h(px(32.))
                    .px(px(10.))
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Agents"),
                    )
                    .when(working > 0, |row| {
                        row.child(meta(format!("{working} working")))
                    }),
            )
            .child(self.start_row(cx))
            .child(v_flex().gap(px(6.)).children(cards))
    }
}

/// A project name from the prompt its agent starts on: its first words.
pub(crate) fn name_from(prompt: &str, footage: &[PathBuf]) -> String {
    const WORDS: usize = 5;
    const MAX: usize = 40;
    // One folder names the project.
    if let [folder] = footage
        && folder.is_dir()
        && let Some(name) = folder.file_name()
    {
        return name.to_string_lossy().into_owned();
    }
    let line = prompt.lines().next().unwrap_or_default();
    let mut name = String::new();
    for word in line.split_whitespace().take(WORDS) {
        if name.chars().count() + word.chars().count() + 1 > MAX {
            break;
        }
        if !name.is_empty() {
            name.push(' ');
        }
        name.push_str(word);
    }
    let name = flikk_store::project_file::sanitize(&name);
    if name.trim().is_empty() {
        "Untitled".to_owned()
    } else {
        name
    }
}
