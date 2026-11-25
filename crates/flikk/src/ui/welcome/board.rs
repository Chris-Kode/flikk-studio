//! The agents on the home page: a prompt that starts a new project with an
//! agent at work on it, and the open projects side by side, each as it is
//! in the editor: its picture, and its agent's chat with the thread, the
//! requests waiting on the user and the composer (style, skills, model and
//! thinking level). A tile's header saves its project, renders it, opens it
//! in the editor and closes it.

use super::{Welcome, WelcomeEvent, aspect, meta};
use crate::desk::Session;
use crate::projects;
use crate::state::AgentStatus;
use crate::ui::kit::{
    Button, DialogExt as _, Icon, IconName, Input, Spinner, h_flex, theme as t, v_flex,
};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;

/// The gap between tiles, and between the tiles and what is around them.
const GAP: f32 = 12.;
/// The least a tile is high: its picture and a few lines of chat over the
/// composer.
const TILE_MIN_HEIGHT: f32 = 460.;
/// The part of a tile's height its picture takes.
const PICTURE: f32 = 0.34;

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

    /// Ask the window for a new project whose agent starts on the prompt,
    /// with the footage picked for it. The prompt stays until the project
    /// is made (see [`Self::agent_started`]): New project can be cancelled.
    fn start(&mut self, cx: &mut Context<Self>) {
        let prompt = self.prompt.read(cx).value().trim().to_owned();
        if prompt.is_empty() || self.state(cx).read(cx).projects_wait() {
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

    /// Open `session`'s project in the editor.
    fn enter(&mut self, session: &Session, cx: &mut Context<Self>) {
        cx.emit(WelcomeEvent::ForgetFootage);
        let state = session.state.clone();
        self.desk.update(cx, |desk, cx| desk.enter(&state, cx));
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

    /// A tile's header: how its agent stands, the project's name and
    /// format, then Save, Render, Open and Close.
    fn tile_header(
        &self,
        ix: usize,
        session: &Session,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let s = session.state.read(cx);
        let project = s.bus.project();
        let glyph = match s.agent_status() {
            AgentStatus::Working => Spinner::new().size(px(14.)).into_any_element(),
            AgentStatus::Waiting => Icon::new(IconName::MessageCircleQuestionMark)
                .color(t::T1)
                .into_any_element(),
            AgentStatus::Failed => Icon::new(IconName::TriangleAlert)
                .color(t::WARN)
                .into_any_element(),
            AgentStatus::Idle => Icon::new(IconName::Clapperboard)
                .color(t::T3)
                .into_any_element(),
        };
        let edited = s.is_dirty();
        let format = &project.timeline.settings;
        let format = format!(
            "{} · {}p",
            aspect(format.width, format.height),
            format.width.min(format.height)
        );
        let duration = crate::ui::format::clock_padded(project.timeline.duration());
        let (open, save, render, close) = (
            session.clone(),
            session.state.clone(),
            session.deliver.clone(),
            session.state.clone(),
        );
        h_flex()
            .flex_none()
            .h(t::PANEL_HEADER)
            .pl(px(10.))
            .pr(px(4.))
            .gap(px(8.))
            .border_b_1()
            .border_color(t::LN)
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
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t::T1)
                    .child(project.name.clone()),
            )
            .when(edited, |row| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(t::T3)
                        .child("Edited"),
                )
            })
            .child(div().flex_1())
            .child(meta(format))
            .child(meta(duration).text_color(t::T2))
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(2.))
                    .child(
                        Button::new(("session-save", ix))
                            .ghost()
                            .small()
                            .icon(IconName::Save)
                            .tooltip("Save")
                            .disabled(!edited)
                            .on_click(move |_, window, cx| projects::save(&save, window, cx)),
                    )
                    .child(
                        Button::new(("session-render", ix))
                            .ghost()
                            .small()
                            .icon(IconName::Upload)
                            .tooltip("Render")
                            .on_click(move |_, window, cx| {
                                let deliver = render.clone();
                                window.open_dialog(cx, move |dialog, _, _| {
                                    dialog
                                        .title("Render")
                                        .w(px(440.))
                                        .child(div().h(px(560.)).child(deliver.clone()))
                                })
                            }),
                    )
                    .child(
                        Button::new(("session-open", ix))
                            .ghost()
                            .small()
                            .icon(IconName::Maximize2)
                            .tooltip("Open in the editor")
                            .on_click(cx.listener(move |this, _, _, cx| this.enter(&open, cx))),
                    )
                    .child(
                        Button::new(("session-close", ix))
                            .ghost()
                            .small()
                            .icon(IconName::X)
                            .tooltip("Close")
                            .on_click(move |_, window, cx| projects::close(&close, window, cx)),
                    ),
            )
    }

    /// An open project as it is in the editor: its picture over its
    /// agent's chat.
    fn session_tile(
        &self,
        ix: usize,
        session: &Session,
        size: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        v_flex()
            .flex_none()
            .w(size.width)
            .h(size.height)
            .overflow_hidden()
            .rounded(px(6.))
            .border_1()
            .border_color(t::LN)
            .bg(t::P)
            .child(self.tile_header(ix, session, cx))
            .child(
                div()
                    .flex_none()
                    .h(size.height * PICTURE)
                    .border_b_1()
                    .border_color(t::LN)
                    .child(session.viewer.clone()),
            )
            .child(div().flex_1().min_h_0().child(session.chat.clone()))
            .into_any_element()
    }

    /// The agents: the prompt that starts one, then the open projects
    /// side by side. One or two fill the window's height; more make rows
    /// of two or three, half as high, and the page scrolls.
    pub(super) fn board(
        &mut self,
        sessions: &[Session],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if std::mem::take(&mut self.clear_prompt) {
            self.prompt
                .update(cx, |prompt, cx| prompt.set_value("", window, cx));
        }
        let working = self.desk.read(cx).agents_working(cx);
        let size = tile_size(sessions.len(), window);
        let tiles: Vec<_> = (sessions.iter().enumerate())
            .map(|(ix, session)| self.session_tile(ix, session, size, cx))
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
            .when(!tiles.is_empty(), |board| {
                board.child(
                    div()
                        .mt(px(4.))
                        .flex()
                        .flex_wrap()
                        .gap(px(GAP))
                        .children(tiles),
                )
            })
    }
}

/// How wide the page's two columns are beside the tiles: the actions on
/// the left, the gap after them and the page's padding.
pub(super) const BESIDE_TILES: f32 = 340. + 32. + 2. * 24.;

/// The size of each of `count` tiles in the window: two or three in a
/// row, filling the height in one row or two.
fn tile_size(count: usize, window: &Window) -> Size<Pixels> {
    let viewport = window.viewport_size();
    let columns = match count {
        0 | 1 => 1,
        2..=4 => 2,
        _ => 3,
    };
    let rows = if count > columns { 2 } else { 1 };
    let gaps = |n: usize| px(GAP * (n as f32 - 1.));
    // The scrollbar's lane stays free, so the last column never wraps.
    let across = viewport.width - px(BESIDE_TILES) - px(12.);
    // Over the tiles: the bars, the page's padding, "Agents" and the prompt.
    let above = t::TITLE_BAR + t::STATUS_BAR + px(2. * 24. + 32. + 8. + 26. + 12.);
    let down = viewport.height - above;
    size(
        ((across - gaps(columns)) / columns as f32).max(px(340.)),
        ((down - gaps(rows)) / rows as f32).max(px(TILE_MIN_HEIGHT)),
    )
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
