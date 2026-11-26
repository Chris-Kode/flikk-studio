//! The agents on the home page: a composer that starts a new project with
//! an agent at work on its message, and the open projects, those that wait
//! for the user first. A project shows as a tile, as it is in the editor
//! (its picture, and its agent's chat with the thread, the requests
//! waiting on the user and the composer), or as a row: one line with how
//! its agent stands and what it last said. A few projects are all tiles;
//! among more, those with nothing under way are rows. A click on a
//! project's bar enlarges it under the rows of them all, and another shows
//! them all again. The bar opens the project in the editor, and its menu
//! saves, renders and closes it.

use super::{Welcome, WelcomeEvent, aspect, meta};
use crate::desk::Session;
use crate::projects;
use crate::state::{AgentStatus, EditorState};
use crate::ui::chat::{Composer, ComposerEvent};
use crate::ui::kit::{
    Button, DialogExt as _, DropdownMenu as _, Icon, IconName, Kbd, PopupMenuItem, Spinner,
    Tooltip, h_flex, theme as t, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;

/// The gap between tiles, and between the tiles and what is around them.
const GAP: f32 = 12.;
/// The least a tile is wide and high: its picture and a few lines of chat
/// over the composer.
const TILE_MIN: Size<f32> = Size {
    width: 340.,
    height: 460.,
};
/// How high the composer is before a message grows it.
const COMPOSER: f32 = 80.;
/// The part of a tile's height its picture takes, over its chat.
const PICTURE: f32 = 0.34;
/// From this width a tile shows its picture beside its chat, not over it.
const BESIDE_FROM: f32 = 880.;
/// How wide the chat is beside the picture.
const CHAT_BESIDE: f32 = 460.;
/// How high a row is, and the gap between two.
const ROW: f32 = 30.;
const ROW_GAP: f32 = 4.;
/// From this many open projects, those with nothing under way are rows.
const ROWS_FROM: usize = 3;
/// The scrollbar's lane beside the projects: free, so the last tile of a
/// line never wraps.
const SCROLLBAR: f32 = 12.;
/// The page's padding, around the agents.
pub(super) const PAGE_PADDING: f32 = 24.;

/// Where a project's agent stands for the user, the most pressing first:
/// the order the open projects show in.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Standing {
    /// Something waits for the user: an approval, a question, or changes
    /// to review.
    Waiting,
    Failed,
    Working,
    Idle,
}

impl Standing {
    fn of(s: &EditorState) -> Self {
        match s.agent_status() {
            _ if crate::desk::waits_in(s).is_some() => Self::Waiting,
            AgentStatus::Waiting => Self::Waiting,
            AgentStatus::Failed => Self::Failed,
            AgentStatus::Working => Self::Working,
            AgentStatus::Idle => Self::Idle,
        }
    }
}

/// How the open projects are laid out.
enum Layout<'a> {
    /// One enlarged, under the rows of them all.
    Enlarged(&'a Session),
    /// The first `tiles` of them side by side, over the rows of the rest.
    Grid { tiles: usize },
}

/// The projects the page marks, the same for every bar.
#[derive(Clone, Copy)]
struct Marks {
    /// The one enlarged.
    enlarged: Option<EntityId>,
    /// The one Enter opens, while the page has the keys.
    selected: Option<EntityId>,
}

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

    /// The open projects the page shows, in the order it shows and numbers
    /// them: those that wait for the user, then the failed, the working
    /// and the idle, the last opened first among equals. None while the
    /// projects wait for the models.
    pub(super) fn ordered(&self, cx: &App) -> Vec<Session> {
        if self.state(cx).read(cx).projects_wait() {
            return Vec::new();
        }
        let mut sessions = self.desk.read(cx).listed(cx);
        sessions.sort_by_key(|session| Standing::of(session.state.read(cx)));
        sessions
    }

    /// Open `session`'s project in the editor.
    pub(super) fn enter(&mut self, session: &Session, cx: &mut Context<Self>) {
        cx.emit(WelcomeEvent::ForgetFootage);
        let state = session.state.clone();
        self.desk.update(cx, |desk, cx| desk.enter(&state, cx));
    }

    /// Enlarge project `id`, or show them all again when it is the one
    /// enlarged.
    fn toggle_enlarged(&mut self, id: EntityId, cx: &mut Context<Self>) {
        self.enlarged = (self.enlarged != Some(id)).then_some(id);
        cx.notify();
    }

    /// The keys the agents take wherever on the page they are typed:
    /// Cmd 1 to 9 enlarges that project, ready for a message, and Escape
    /// shows them all again. Whether `ev` was one of them.
    pub(super) fn take_board_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = ev.keystroke.key.as_str();
        if key == "escape" && self.enlarged.is_some() {
            self.enlarged = None;
            window.focus(&self.focus, cx);
            cx.notify();
            return true;
        }
        if ev.keystroke.modifiers != Modifiers::secondary_key() {
            return false;
        }
        let number = key.parse::<usize>().ok().filter(|n| (1..=9).contains(n));
        let sessions = self.ordered(cx);
        let Some(session) = number.and_then(|n| sessions.get(n - 1)) else {
            return false;
        };
        self.enlarged = Some(session.state.entity_id());
        session
            .chat
            .update(cx, |chat, cx| chat.focus_composer(window, cx));
        cx.notify();
        true
    }

    /// How `sessions` are laid out: the enlarged one alone, else a few all
    /// as tiles, else those whose agent has something under way or for the
    /// user. They come first: see `ordered`.
    fn layout<'a>(&self, sessions: &'a [Session], cx: &App) -> Layout<'a> {
        let enlarged = |s: &&Session| Some(s.state.entity_id()) == self.enlarged;
        if let Some(session) = sessions.iter().find(enlarged) {
            return Layout::Enlarged(session);
        }
        let under_way = |s: &&Session| Standing::of(s.state.read(cx)) != Standing::Idle;
        Layout::Grid {
            tiles: match sessions.len() {
                few if few < ROWS_FROM => few,
                _ => sessions.iter().filter(under_way).count(),
            },
        }
    }

    /// A project in one line: how its agent stands, its name, what `line`
    /// says of it, its format and length, then Open and the menu with
    /// Save, Render and Close. A click enlarges it, or shows every project
    /// again. `n` is its place on the page, from 0.
    fn bar(
        &self,
        n: usize,
        session: &Session,
        line: Option<String>,
        marks: Marks,
        cx: &mut Context<Self>,
    ) -> Div {
        let s = session.state.read(cx);
        let id = session.state.entity_id();
        let key = id.as_u64();
        let project = s.bus.project();
        let standing = Standing::of(s);
        let glyph = match standing {
            Standing::Working => Spinner::new().size(px(14.)).into_any_element(),
            Standing::Waiting => Icon::new(IconName::MessageCircleQuestionMark)
                .color(t::T1)
                .into_any_element(),
            Standing::Failed => Icon::new(IconName::TriangleAlert)
                .color(t::WARN)
                .into_any_element(),
            Standing::Idle => Icon::new(IconName::Clapperboard)
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
        let name = project.name.clone();
        let selected = marks.selected == Some(id);
        let (tip, shortcut) = match marks.enlarged == Some(id) {
            true => ("Show all", Some("Esc".to_owned())),
            false => (
                "Enlarge",
                (n < 9).then(|| format!("{} {}", crate::app::MOD, n + 1)),
            ),
        };
        let said = match line {
            Some(line) => div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(11.))
                .text_color(match standing {
                    Standing::Waiting => t::T1,
                    _ => t::T2,
                })
                .child(line),
            None => div().flex_1(),
        };
        let open = session.clone();
        h_flex()
            .child(
                h_flex()
                    .id(("session-bar", key))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pl(px(10.))
                    .pr(px(6.))
                    .gap(px(8.))
                    .cursor_pointer()
                    .when(selected, |bar| bar.bg(t::HV))
                    .hover(|s| s.bg(t::HV))
                    .active(|s| s.bg(t::PS))
                    .tooltip(Tooltip::with_kbd(tip, shortcut))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_enlarged(id, cx)))
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
                            .child(name),
                    )
                    .when(edited, |bar| {
                        bar.child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(t::T3)
                                .child("Edited"),
                        )
                    })
                    .child(said)
                    .when(selected, |bar| bar.child(Kbd::new("Enter")))
                    .child(meta(format))
                    .child(meta(duration).text_color(t::T2)),
            )
            .child(
                h_flex()
                    .flex_none()
                    .px(px(4.))
                    .gap(px(2.))
                    .child(
                        Button::new(("session-open", key))
                            .ghost()
                            .small()
                            .icon(IconName::Maximize2)
                            .tooltip("Open in the editor")
                            .on_click(cx.listener(move |this, _, _, cx| this.enter(&open, cx))),
                    )
                    .child(session_menu(session, key)),
            )
    }

    /// A project as a row: its bar, with a line of its chat.
    fn session_row(
        &self,
        n: usize,
        session: &Session,
        marks: Marks,
        cx: &mut Context<Self>,
    ) -> Div {
        let line = line(session.state.read(cx));
        let enlarged = marks.enlarged == Some(session.state.entity_id());
        self.bar(n, session, line, marks, cx)
            .flex_none()
            .h(px(ROW))
            .overflow_hidden()
            .rounded(px(6.))
            .border_1()
            .border_color(t::LN)
            .bg(if enlarged { t::AC_WASH } else { t::P })
    }

    /// A project as a tile `size` large: its bar over it as it is in the
    /// editor.
    fn session_tile(
        &self,
        n: usize,
        session: &Session,
        size: Size<Pixels>,
        marks: Marks,
        cx: &mut Context<Self>,
    ) -> Div {
        let bar = self.bar(n, session, None, marks, cx);
        frame(size)
            .child(
                bar.flex_none()
                    .h(t::PANEL_HEADER)
                    .border_b_1()
                    .border_color(t::LN),
            )
            .child(body(session, size))
    }

    /// The agents: the prompt that starts one, then the open projects as
    /// `layout` puts them. Tiles fill the window's height in one row or
    /// two, over their rows; more make the page scroll. With projects
    /// open, the header also starts and opens one, and says what a drop
    /// on the page does.
    pub(super) fn board(
        &mut self,
        sessions: &[Session],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let composer = self.composer(window, cx);
        if let Some(prompt) = self.restore.take() {
            composer.update(cx, |composer, cx| composer.restore(&prompt, window, cx));
        }
        let layout = self.layout(sessions, cx);
        if !matches!(layout, Layout::Enlarged(_)) {
            // The one enlarged was closed.
            self.enlarged = None;
        }
        let marks = Marks {
            enlarged: self.enlarged,
            selected: self.selected_session(window, cx),
        };
        let rows = |this: &Self, from: usize, cx: &mut Context<Self>| {
            let rows = (sessions.iter().enumerate().skip(from))
                .map(|(n, session)| this.session_row(n, session, marks, cx))
                .collect::<Vec<_>>();
            v_flex().gap(px(ROW_GAP)).children(rows)
        };
        let projects = match layout {
            Layout::Enlarged(session) => {
                let size = tile_size(1, sessions.len(), window);
                v_flex()
                    .child(rows(self, 0, cx))
                    .child(frame(size).child(body(session, size)))
            }
            Layout::Grid { tiles } => {
                let size = tile_size(tiles, sessions.len() - tiles, window);
                let shown = (sessions.iter().enumerate().take(tiles))
                    .map(|(n, session)| self.session_tile(n, session, size, marks, cx))
                    .collect::<Vec<_>>();
                v_flex()
                    .when(tiles > 0, |projects| {
                        projects.child(div().flex().flex_wrap().gap(px(GAP)).children(shown))
                    })
                    .when(tiles < sessions.len(), |projects| {
                        projects.child(rows(self, tiles, cx))
                    })
            }
        };
        v_flex()
            .gap(px(8.))
            .child(self.board_header(sessions, cx))
            .child(composer)
            .when(!sessions.is_empty(), |board| {
                board.child(projects.mt(px(4.)).pr(px(SCROLLBAR)).gap(px(GAP)))
            })
    }

    /// "Agents", how many wait for the user and how many work. With
    /// projects open, New project and Open… too, and while files are held
    /// over the page, what dropping them does.
    fn board_header(&self, sessions: &[Session], cx: &mut Context<Self>) -> impl IntoElement {
        let waits = |s: &&Session| Standing::of(s.state.read(cx)) == Standing::Waiting;
        let waiting = sessions.iter().filter(waits).count();
        let working = self.desk.read(cx).agents_working(cx);
        let dropping = (self.dropping.as_ref()).filter(|_| !sessions.is_empty());
        let drop = dropping.map(|paths| {
            h_flex()
                .min_w_0()
                .gap(px(6.))
                .text_color(t::AC)
                .child(Icon::new(IconName::FolderDown))
                .child("Drop to create a project")
                .child(meta(super::drop_hint(paths)).min_w_0().truncate())
        });
        h_flex()
            .h(px(32.))
            .pl(px(10.))
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Agents"),
            )
            .when(waiting > 0, |row| {
                row.child(meta(format!("{waiting} waiting")).text_color(t::T1))
            })
            .when(working > 0, |row| {
                row.child(meta(format!("{working} working")))
            })
            .children(drop)
            .child(div().flex_1())
            .when(!sessions.is_empty(), |row| {
                row.child(self.new_button(cx)).child(
                    h_flex()
                        .gap(px(2.))
                        .child(self.open_button(cx))
                        .child(self.recent_menu(cx)),
                )
            })
    }
}

/// The menu of a project's bar: Save, Render… and Close.
fn session_menu(session: &Session, key: u64) -> impl IntoElement + use<> {
    let (state, deliver) = (session.state.clone(), session.deliver.clone());
    Button::new(("session-menu", key))
        .ghost()
        .small()
        .icon(IconName::Ellipsis)
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, cx| {
            let (save, render, close) = (state.clone(), deliver.clone(), state.clone());
            menu.item(
                PopupMenuItem::new("Save")
                    .icon(IconName::Save)
                    .disabled(!state.read(cx).is_dirty())
                    .on_click(move |_, window, cx| projects::save(&save, window, cx)),
            )
            .item(
                PopupMenuItem::new("Render…")
                    .icon(IconName::Upload)
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
            .separator()
            .item(
                PopupMenuItem::new("Close")
                    .icon(IconName::X)
                    .on_click(move |_, window, cx| projects::close(&close, window, cx)),
            )
        })
}

/// What a row says of a project in one line: what waits for the user,
/// else why its agent failed, else the last thing said in its chat.
fn line(s: &EditorState) -> Option<String> {
    if let Some(waits) = crate::desk::waits_in(s) {
        return Some(waits);
    }
    let said = match s.agent_error() {
        Some(error) => error.to_owned(),
        None => {
            let last = s.chat().iter().rev().find(|m| !m.text.trim().is_empty())?;
            crate::ui::chat::plain_text(&last.text, s.bus.project())
        }
    };
    let line = said.lines().find(|line| !line.trim().is_empty())?;
    // What marks a heading, a list or emphasis says nothing in one line.
    let line = line.trim().trim_start_matches(['#', '-', '*', ' ']);
    Some(line.replace("**", "").replace('`', ""))
}

/// The box of a tile `size` large.
fn frame(size: Size<Pixels>) -> Div {
    v_flex()
        .flex_none()
        .w(size.width)
        .h(size.height)
        .overflow_hidden()
        .rounded(px(6.))
        .border_1()
        .border_color(t::LN)
        .bg(t::P)
}

/// A project as it is in the editor, filling what is left of a tile
/// `size` large: its picture over its agent's chat, or beside it in a
/// wide tile.
fn body(session: &Session, size: Size<Pixels>) -> Div {
    let (picture, chat) = (div().border_color(t::LN), div());
    if size.width >= px(BESIDE_FROM) {
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .child(
                picture
                    .flex_1()
                    .min_w_0()
                    .border_r_1()
                    .child(session.viewer.clone()),
            )
            .child(
                chat.flex_none()
                    .w(px(CHAT_BESIDE))
                    .child(session.chat.clone()),
            )
    } else {
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                picture
                    .flex_none()
                    .h(size.height * PICTURE)
                    .border_b_1()
                    .child(session.viewer.clone()),
            )
            .child(chat.flex_1().min_h_0().child(session.chat.clone()))
    }
}

/// The size of each of `tiles` tiles in the window, beside `rows` rows:
/// up to three across, filling the height the rows leave in one line or
/// two.
fn tile_size(tiles: usize, rows: usize, window: &Window) -> Size<Pixels> {
    let viewport = window.viewport_size();
    let columns = match tiles {
        0 | 1 => 1,
        2 | 4 => 2,
        _ => 3,
    };
    let lines = if tiles > columns { 2 } else { 1 };
    let gaps = |n: usize| px(GAP * (n as f32 - 1.));
    let across = viewport.width - px(2. * PAGE_PADDING + SCROLLBAR);
    // Over the tiles: the bars, the page's padding, "Agents" and the
    // composer with an empty message.
    let above = t::TITLE_BAR + t::STATUS_BAR + px(2. * PAGE_PADDING + 32. + 8. + COMPOSER + 12.);
    let beside = match rows {
        0 => px(0.),
        rows => px(rows as f32 * ROW + (rows as f32 - 1.) * ROW_GAP + GAP),
    };
    let down = viewport.height - above - beside;
    size(
        ((across - gaps(columns)) / columns as f32).max(px(TILE_MIN.width)),
        ((down - gaps(lines)) / lines as f32).max(px(TILE_MIN.height)),
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
