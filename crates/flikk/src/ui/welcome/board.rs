//! The agents on the home page: a composer that starts a new project with
//! an agent at work on its message, and the open projects side by side, each as it is
//! in the editor: its picture, and its agent's chat with the thread, the
//! requests waiting on the user and the composer (style, skills, model and
//! thinking level). A tile's header saves its project, renders it, opens it
//! in the editor and closes it.

use super::{Welcome, WelcomeEvent, aspect, meta};
use crate::desk::Session;
use crate::projects;
use crate::state::AgentStatus;
use crate::ui::chat::{Composer, ComposerEvent};
use crate::ui::kit::{Button, DialogExt as _, Icon, IconName, Spinner, h_flex, theme as t, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;

/// The gap between tiles, and between the tiles and what is around them.
const GAP: f32 = 12.;
/// The least a tile is high: its picture and a few lines of chat over the
/// composer.
const TILE_MIN_HEIGHT: f32 = 460.;
/// How high the composer is before a message grows it.
const COMPOSER: f32 = 80.;
/// The part of a tile's height its picture takes.
const PICTURE: f32 = 0.34;

impl Welcome {
    /// The composer the next agent's first message is written in: a
    /// chat's own, on the desk's draft session, so the message is written
    /// with a style, skills, a model and thinking level, and attachments.
    fn composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<Composer> {
        if let Some((composer, _)) = &self.composer {
            return composer.clone();
        }
        let draft = self.desk.update(cx, |desk, cx| desk.draft(cx));
        let composer = cx.new(|cx| Composer::new(draft, window, cx));
        composer.update(cx, |composer, cx| {
            composer.set_placeholder("Describe a video, / for skills", window, cx)
        });
        let sent = cx.subscribe(&composer, |this, _, event: &ComposerEvent, cx| {
            let ComposerEvent::Submit(text) = event;
            this.start(text.clone(), cx);
        });
        self.composer = Some((composer.clone(), sent));
        composer
    }

    /// Ask the window for a new project whose agent starts on `prompt`,
    /// with what was attached to the draft: imported into the project, and
    /// attached to its chat.
    fn start(&mut self, prompt: String, cx: &mut Context<Self>) {
        let draft = self.desk.update(cx, |desk, cx| desk.draft(cx));
        let attached = draft.read(cx).attachments().iter();
        let footage = attached.map(|a| a.path.clone()).collect();
        cx.emit(WelcomeEvent::Start { prompt, footage });
    }

    /// The agent asked for with the draft's message started: the next
    /// message is written on a new draft.
    pub fn agent_started(&mut self, cx: &mut Context<Self>) {
        self.composer = None;
        self.desk.update(cx, |desk, cx| desk.forget_draft(cx));
        cx.notify();
    }

    /// New project was cancelled: `prompt` goes back into the composer.
    pub fn restore_prompt(&mut self, prompt: String, cx: &mut Context<Self>) {
        self.restore = Some(prompt);
        cx.notify();
    }

    /// Open `session`'s project in the editor.
    fn enter(&mut self, session: &Session, cx: &mut Context<Self>) {
        cx.emit(WelcomeEvent::ForgetFootage);
        let state = session.state.clone();
        self.desk.update(cx, |desk, cx| desk.enter(&state, cx));
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
        let composer = self.composer(window, cx);
        if let Some(prompt) = self.restore.take() {
            composer.update(cx, |composer, cx| composer.restore(&prompt, window, cx));
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
            .child(composer)
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
    // Over the tiles: the bars, the page's padding, "Agents" and the
    // composer with an empty message.
    let above = t::TITLE_BAR + t::STATUS_BAR + px(2. * 24. + 32. + 8. + COMPOSER + 12.);
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
