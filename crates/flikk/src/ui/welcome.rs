//! The home page the app opens on, and comes back to from the editor: New
//! project, Open…, a drop zone that starts a project from footage, the
//! agents at work on the open projects ([`board`]) and the last few recent
//! projects. Once a project is open the page is the agents': they take its
//! width, New project and Open… move into their header with the recent
//! projects as a menu, and a drop anywhere on the page starts a project.
//! While a model is missing, the setup list takes the agents' and recent
//! projects' place and no project opens: the models to download as one,
//! the transcription size, the agent CLIs found, and FFmpeg and a browser
//! when they are not.

mod board;

pub(crate) use board::name_from;

use crate::desk::Desk;
use crate::projects;
use crate::state::{EditorState, SetupStatus};
use crate::ui::kit::{
    Button, DropdownMenu as _, Icon, IconName, Input, Kbd, PopupMenuItem, Progress,
    ScrollableElement as _, Segmented, Spinner, Tooltip, h_flex, theme as t, v_flex,
};
use flikk_core::agent::Backend;
use flikk_models::Task as ModelTask;
use flikk_store::project_file::{self, EXTENSION};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, SystemTime};

/// How often the setup list redraws while the models download.
const DOWNLOAD_POLL: Duration = Duration::from_millis(250);

/// How many recent projects the page lists: the search reaches the rest.
const RECENT_SHOWN: usize = 5;

/// Where Claude Code is installed from, when no agent CLI is found.
const AGENT_INSTALL: &str = "https://code.claude.com/docs/en/setup";
/// Where FFmpeg is downloaded from.
const FFMPEG_INSTALL: &str = "https://ffmpeg.org/download.html";
/// Where a browser for HTML titles is downloaded from.
const BROWSER_INSTALL: &str = "https://www.google.com/chrome/";

/// What the page asks the window to do.
pub enum WelcomeEvent {
    /// Go on with the project that is already open.
    Continue,
    /// Start a new project from these files and folders.
    Create(Vec<PathBuf>),
    /// Start a new project with these files and folders whose agent works
    /// on `prompt`, staying on the page.
    Start {
        prompt: String,
        footage: Vec<PathBuf>,
    },
    /// Another project is being started or opened: footage held from an
    /// earlier `Create` (whose dialog was cancelled) must not go into it.
    ForgetFootage,
}

/// What Up, Down and Enter move over and open.
#[derive(Clone, Copy, PartialEq)]
enum Row {
    /// An open project, by its state: Enter opens it in the editor.
    Session(EntityId),
    /// The project a crash left unsaved changes in, open underneath.
    Recovered,
    Recent(usize),
}

/// A recent project as the page lists it.
struct Recent {
    path: PathBuf,
    name: String,
    /// Last saved.
    saved: Option<SystemTime>,
    /// "16:9 · 1080p".
    format: Option<String>,
    duration: Option<String>,
    missing: bool,
}

impl Recent {
    fn read(path: PathBuf) -> Self {
        let name = project_file::name_of(&path);
        let saved = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let file = project_file::read(&path).ok();
        let format = file.as_ref().map(|f| {
            let s = &f.project.timeline.settings;
            format!("{} · {}p", aspect(s.width, s.height), s.width.min(s.height))
        });
        let duration = file
            .as_ref()
            .map(|f| crate::ui::format::clock_padded(f.project.timeline.duration()));
        Self {
            missing: !path.exists(),
            path,
            name,
            saved,
            format,
            duration,
        }
    }
}

/// The agent CLIs looked for on the setup list.
enum Agents {
    /// Not looked for yet: the setup list has not shown.
    Unchecked,
    /// Asked under this number; a later ask drops its answer.
    Checking(u64),
    /// Those found, with the version each reports.
    Found(Vec<(Backend, String)>),
}

pub struct Welcome {
    desk: Entity<Desk>,
    agents: Agents,
    /// Numbers each look for the agent CLIs.
    agents_asked: u64,
    /// Redraws the setup list while the models download.
    download_poll: Option<Task<()>>,
    search: Entity<InputState>,
    recent: Vec<Recent>,
    /// The row Up and Down last moved to, for Enter: the first one until
    /// then, and when it no longer shows.
    selected: Option<Row>,
    /// The open project enlarged on the page (see `board`).
    enlarged: Option<EntityId>,
    /// Files from outside held over the drop zone.
    dropping: Option<Vec<PathBuf>>,
    /// What the next agent is asked in, made when the agents first show
    /// (see `board`), and its subscription.
    composer: Option<(Entity<crate::ui::chat::Composer>, Subscription)>,
    /// A message whose project was not made after all: it goes back into
    /// the composer on the next draw.
    restore: Option<String>,
    focus: FocusHandle,
    _search: Subscription,
    _desk: Subscription,
    /// Redraws with the project in the editor, whose setup, status and
    /// unsaved changes the page shows.
    _state: (EntityId, Subscription),
}

impl EventEmitter<WelcomeEvent> for Welcome {}

impl Welcome {
    pub fn new(desk: Entity<Desk>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search projects"));
        let _search = cx.subscribe(&search, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.selected = None;
                cx.notify();
            }
        });
        let recent = crate::persistence::recent()
            .into_iter()
            .map(Recent::read)
            .collect();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let _desk = cx.observe(&desk, |this, _, cx| {
            this.follow_editor(cx);
            cx.notify();
        });
        let state = desk.read(cx).editor().state.clone();
        let _state = (
            state.entity_id(),
            cx.observe(&state, |_, _, cx| cx.notify()),
        );
        Self {
            desk,
            agents: Agents::Unchecked,
            agents_asked: 0,
            download_poll: None,
            search,
            recent,
            selected: None,
            enlarged: None,
            dropping: None,
            composer: None,
            restore: None,
            focus,
            _search,
            _desk,
            _state,
        }
    }

    /// The state of the project in the editor: what the page asks about
    /// the app as a whole (the models, the engines) it asks there.
    fn state(&self, cx: &App) -> Entity<EditorState> {
        self.desk.read(cx).editor().state.clone()
    }

    /// Another project took the editor: redraw with that one.
    fn follow_editor(&mut self, cx: &mut Context<Self>) {
        let state = self.state(cx);
        if self._state.0 != state.entity_id() {
            self._state = (
                state.entity_id(),
                cx.observe(&state, |_, _, cx| cx.notify()),
            );
        }
    }

    /// The page shows again, back from the editor: list the recent
    /// projects as they are now and take the keys.
    pub fn shown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut listed = std::mem::take(&mut self.recent);
        self.recent = crate::persistence::recent()
            .into_iter()
            .map(|path| {
                // A file not saved since it was read is not read again.
                let saved = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                let same = |r: &Recent| r.path == path && r.saved == saved && !r.missing;
                match listed.iter().position(same) {
                    Some(ix) => listed.swap_remove(ix),
                    None => Recent::read(path),
                }
            })
            .collect();
        self.selected = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Look for Claude and Codex, and their versions, off the UI thread.
    fn check_agents(&mut self, cx: &mut Context<Self>) {
        self.agents_asked += 1;
        let asked = self.agents_asked;
        self.agents = Agents::Checking(asked);
        cx.notify();
        let found = cx.background_executor().spawn(async move {
            [Backend::Claude, Backend::Codex]
                .into_iter()
                .filter_map(|b| Some((b, flikk_agent::backend::installed_version(b)?)))
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let found = found.await;
            let _ = this.update(cx, |this, cx| {
                if matches!(this.agents, Agents::Checking(at) if at == asked) {
                    this.agents = Agents::Found(found);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Whether open projects show on the page. It is then theirs: the
    /// recent projects and their search make way.
    fn projects_open(&self, cx: &App) -> bool {
        !self.ordered(cx).is_empty()
    }

    /// The rows the page shows, in order: the open projects, the recovered
    /// one, then the last few recent ones the search matches. With
    /// projects open no recent one shows; the recovered one does, having
    /// no other way in.
    fn visible(&self, cx: &App) -> Vec<Row> {
        let s = self.state(cx).read(cx);
        if s.projects_wait() {
            return Vec::new();
        }
        let sessions = self.ordered(cx);
        let searchable = sessions.is_empty();
        // A search left in the field finds nothing while it is not shown.
        let query = match searchable {
            true => self.search.read(cx).value().trim().to_lowercase(),
            false => String::new(),
        };
        // Once gone into, it shows with the open projects instead.
        let recovered = s.file().is_none()
            && s.is_dirty()
            && !s.is_listed()
            && s.bus.project().name.to_lowercase().contains(&query);
        let listed = if searchable { self.recent.len() } else { 0 };
        let recent = (0..listed)
            .filter(|&i| {
                let r = &self.recent[i];
                query.is_empty()
                    || r.name.to_lowercase().contains(&query)
                    || r.path.to_string_lossy().to_lowercase().contains(&query)
            })
            .take(RECENT_SHOWN);
        let sessions = sessions.iter().map(|s| Row::Session(s.state.entity_id()));
        sessions
            .chain(recovered.then_some(Row::Recovered))
            .chain(recent.map(Row::Recent))
            .collect()
    }

    /// Where in `rows` Up, Down and Enter are.
    fn selected(&self, rows: &[Row]) -> usize {
        let selected = self
            .selected
            .and_then(|row| rows.iter().position(|r| *r == row));
        selected.unwrap_or(0)
    }

    /// The open project Enter opens, while the page has the keys.
    fn selected_session(&self, window: &Window, cx: &App) -> Option<EntityId> {
        if !self.focus.is_focused(window) {
            return None;
        }
        let rows = self.visible(cx);
        match rows.get(self.selected(&rows))? {
            Row::Session(id) => Some(*id),
            Row::Recovered | Row::Recent(_) => None,
        }
    }

    fn pick(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        match row {
            Row::Session(id) => {
                let open = |s: &crate::desk::Session| s.state.entity_id() == id;
                if let Some(session) = self.ordered(cx).into_iter().find(open) {
                    self.enter(&session, cx);
                }
            }
            Row::Recovered => cx.emit(WelcomeEvent::Continue),
            Row::Recent(ix) => self.open(ix, window, cx),
        }
    }

    fn open(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.recent.get(ix) else {
            return;
        };
        cx.emit(WelcomeEvent::ForgetFootage);
        // The list is read once; the file can have gone since.
        if r.missing || !r.path.exists() {
            self.locate(ix, window, cx);
        } else if self.state(cx).read(cx).file() == Some(r.path.as_path()) {
            cx.emit(WelcomeEvent::Continue);
        } else {
            projects::open(&self.state(cx), r.path.clone(), window, cx);
        }
    }

    /// Open the recent project saved at `path`, while it is still listed.
    fn open_recent(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.recent.iter().position(|r| r.path == path) {
            self.open(ix, window, cx);
        }
    }

    /// Find a project that moved: open the file picked instead and, once it
    /// opened, forget where it was.
    fn locate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let old = self.recent[ix].path.clone();
        cx.emit(WelcomeEvent::ForgetFootage);
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        let state = self.state(cx);
        window
            .spawn(cx, async move |cx| {
                let Ok(Ok(Some(paths))) = paths.await else {
                    return;
                };
                if let Some(path) = paths.into_iter().next() {
                    let _ = cx.update(|window, cx| {
                        projects::open(&state, path.clone(), window, cx);
                        // Unsaved changes ask first: the entry then stays.
                        if state.read(cx).file() == Some(path.as_path()) {
                            crate::persistence::forget_recent(&old);
                        }
                    });
                }
            })
            .detach();
    }

    fn drop_files(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        self.dropping = None;
        if self.state(cx).read(cx).projects_wait() {
            cx.notify();
            return;
        }
        let paths = paths.paths().to_vec();
        if let [file] = paths.as_slice()
            && file.extension().is_some_and(|e| e == EXTENSION)
        {
            cx.emit(WelcomeEvent::ForgetFootage);
            projects::open(&self.state(cx), file.clone(), window, cx);
        } else if !paths.is_empty() {
            cx.emit(WelcomeEvent::Create(paths));
        }
        cx.notify();
    }

    fn key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.take_board_key(ev, window, cx) {
            cx.stop_propagation();
            return;
        }
        // The keys are the rows' while the page or the search has them: a
        // chat and the prompt keep their own.
        let search = self.search.read(cx).focus_handle(cx).is_focused(window);
        if !search && !self.focus.is_focused(window) {
            return;
        }
        let rows = self.visible(cx);
        let at = self.selected(&rows);
        match ev.keystroke.key.as_str() {
            "down" => self.selected = rows.get(at + 1).or(rows.last()).copied(),
            "up" => self.selected = rows.get(at.saturating_sub(1)).copied(),
            "enter" => {
                if let Some(&row) = rows.get(at) {
                    self.pick(row, window, cx);
                }
                return;
            }
            "/" if !search && !self.projects_open(cx) => {
                self.search.update(cx, |s, cx| s.focus(window, cx));
                cx.stop_propagation();
            }
            _ => return,
        }
        cx.notify();
    }

    fn new_button(&self, cx: &mut Context<Self>) -> Button {
        Button::new("welcome-new")
            .icon(IconName::Plus)
            .label("New project")
            .kbd(format!("{} N", crate::app::MOD))
            .on_click(cx.listener(|this, _, window, cx| {
                cx.emit(WelcomeEvent::ForgetFootage);
                projects::new_project(&this.state(cx), window, cx)
            }))
    }

    fn open_button(&self, cx: &mut Context<Self>) -> Button {
        Button::new("welcome-open")
            .icon(IconName::FolderOpen)
            .label("Open…")
            .kbd(format!("{} O", crate::app::MOD))
            .on_click(cx.listener(|this, _, window, cx| {
                cx.emit(WelcomeEvent::ForgetFootage);
                projects::open_dialog(&this.state(cx), window, cx)
            }))
    }

    /// The last few recent projects as a menu beside Open…, where their
    /// list does not show.
    fn recent_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let recent: Vec<_> = (self.recent.iter().take(RECENT_SHOWN))
            .map(|r| (r.path.clone(), r.name.clone()))
            .collect();
        let page = cx.entity().downgrade();
        Button::new("welcome-recent")
            .icon(IconName::ChevronDown)
            .tooltip("Recent")
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                if recent.is_empty() {
                    return menu.item(PopupMenuItem::new("No recent projects").disabled(true));
                }
                recent.iter().fold(menu, |menu, (path, name)| {
                    let (page, path) = (page.clone(), path.clone());
                    menu.item(
                        PopupMenuItem::new(name.clone()).on_click(move |_, window, cx| {
                            let _ = page.update(cx, |page, cx| page.open_recent(&path, window, cx));
                        }),
                    )
                })
            })
    }

    /// New project and Open… as the page's side shows them, large and
    /// held while the models are missing.
    fn actions(&self, waiting: bool, cx: &mut Context<Self>) -> impl IntoElement {
        // The shortcut sits at the button's far end.
        let big = |b: Button| {
            b.h(px(38.))
                .w_full()
                .justify_start()
                .px(px(14.))
                .text_size(px(13.))
                .child(div().flex_1())
                .disabled(waiting)
        };
        v_flex()
            .gap(px(8.))
            // The page's primary action, unless the models' download is.
            .child(big(self.new_button(cx).when(!waiting, |b| b.primary())))
            .child(big(self.open_button(cx)))
    }

    /// Files from outside are held over the element this listens on, or
    /// left it: hold them for the drop, while a project can be made.
    fn drag_moved(
        &mut self,
        ev: &DragMoveEvent<ExternalPaths>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let waiting = self.state(cx).read(cx).projects_wait();
        let over = (!waiting && ev.bounds.contains(&ev.event.position))
            .then(|| ev.drag(cx).paths().to_vec());
        if over != self.dropping {
            self.dropping = over;
            cx.notify();
        }
    }

    fn drop_zone(&self, waiting: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let dropping = self.dropping.as_ref().filter(|_| !waiting);
        let (title, hint): (SharedString, SharedString) = match dropping {
            Some(paths) => ("Drop to create a project".into(), drop_hint(paths).into()),
            None => (
                "Drop a folder of footage".into(),
                "MOV · MP4 · WAV · PNG · .flikk".into(),
            ),
        };
        let on = dropping.is_some();
        div()
            .id("welcome-drop")
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .flex_1()
            .min_h(px(210.))
            .mt(px(16.))
            .rounded(t::RADIUS_LG)
            .border_1()
            .border_dashed()
            .border_color(if on { t::AC } else { t::LN_CTL })
            .bg(if on {
                t::alpha(t::AC, 0.06).into()
            } else {
                pattern_slash(t::alpha(t::T1, 0.012), 8., 8.)
            })
            .when(waiting, |zone| zone.opacity(0.5))
            .on_drag_move(cx.listener(Self::drag_moved))
            .on_drop(cx.listener(Self::drop_files))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(44.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(if on { t::AC } else { t::LN })
                    .bg(if on { t::AC_WASH } else { t::R1 })
                    .text_color(if on { t::AC } else { t::T2 })
                    .child(Icon::new(IconName::FolderDown).size(px(20.))),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(title),
            )
            .child(
                div()
                    .font_family(t::mono())
                    .text_size(px(10.))
                    .text_color(t::T3)
                    .child(hint),
            )
    }

    /// The frame every row shares: tile, name over detail, trailing column.
    fn frame(
        id: ElementId,
        selected: bool,
        tile: impl IntoElement,
        name: Div,
        detail: Div,
        trailing: impl IntoElement,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(14.))
            .py(px(8.))
            .px(px(10.))
            .rounded(px(6.))
            .border_1()
            .border_color(t::alpha(t::T1, if selected { 0.05 } else { 0. }))
            .when(selected, |row| row.bg(t::HV))
            .when(!selected, |row| row.hover(|s| s.bg(t::R1)))
            .cursor_pointer()
            .text_size(px(11.))
            .text_color(t::T3)
            .child(tile)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        name.text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .truncate(),
                    )
                    .child(detail.text_size(px(11.)).truncate()),
            )
            .child(trailing)
    }

    fn tile(icon: IconName) -> Div {
        div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(104.))
            .h(px(58.5))
            .rounded(t::RADIUS_SM)
            .border_1()
            .border_color(t::alpha(t::T1, 0.06))
            .bg(t::R1)
            .text_color(t::T3)
            .child(Icon::new(icon).size(px(18.)))
    }

    fn recovered_row(&self, selected: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.state(cx).read(cx).bus.project().name.clone();
        Self::frame(
            "welcome-recovered".into(),
            selected,
            Self::tile(IconName::ArchiveRestore),
            div().text_color(t::T1).child(name),
            h_flex()
                .gap(px(4.))
                .text_color(t::WARN)
                .child(Icon::new(IconName::TriangleAlert).size(px(11.)))
                .child("Unsaved changes recovered"),
            h_flex()
                .gap(px(6.))
                .text_size(px(10.))
                .child("Continue")
                .when(selected, |row| row.child(Kbd::new("Enter"))),
        )
        .on_click(cx.listener(|_, _, _, cx| cx.emit(WelcomeEvent::Continue)))
    }

    fn row(&self, ix: usize, selected: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let r = &self.recent[ix];
        let tile = Self::tile(IconName::Clapperboard).children(r.duration.clone().map(|d| {
            div()
                .absolute()
                .right(px(3.))
                .bottom(px(3.))
                .px(px(3.))
                .rounded(px(2.))
                .bg(t::alpha(t::hex(0x000000), 0.67))
                .font_family(t::mono())
                .text_size(px(9.))
                .text_color(t::T1)
                .child(d)
        }));
        let path = div().font_family(t::mono()).child(home(&r.path));
        let detail = if r.missing {
            h_flex()
                .gap(px(6.))
                .child(
                    h_flex()
                        .gap(px(4.))
                        .text_color(t::WARN)
                        .child(Icon::new(IconName::Unlink).size(px(11.)))
                        .child("Not found"),
                )
                .child(path)
        } else {
            h_flex().child(path)
        };
        let trailing = if r.missing {
            v_flex()
                .items_end()
                .gap(px(3.))
                .child(
                    Button::new(("welcome-locate", ix))
                        .ghost()
                        .small()
                        .label("Locate…")
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.locate(ix, window, cx)),
                        ),
                )
                .children(r.saved.map(|s| div().child(when(s))))
        } else {
            v_flex()
                .items_end()
                .gap(px(3.))
                .children(r.saved.map(|s| div().text_color(t::T2).child(when(s))))
                .child(
                    h_flex()
                        .gap(px(6.))
                        .text_size(px(10.))
                        .children(r.format.clone())
                        .when(selected, |row| row.child(Kbd::new("Enter"))),
                )
        };
        let name = div()
            .text_color(if r.missing { t::T2 } else { t::T1 })
            .child(r.name.clone());
        Self::frame(
            ("welcome-recent", ix).into(),
            selected,
            tile,
            name,
            detail,
            trailing,
        )
        .on_click(cx.listener(move |this, _, window, cx| this.open(ix, window, cx)))
    }

    /// "Recent", how many rows show and the search.
    fn recent_header(&self, shown: usize) -> impl IntoElement {
        h_flex()
            .h(px(32.))
            .px(px(10.))
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Recent"),
            )
            .child(meta(shown.to_string()))
            .child(div().flex_1())
            .child(
                div().w(px(220.)).child(
                    Input::new(&self.search)
                        .search()
                        .kbd("/")
                        .bg(t::P)
                        .rounded(t::RADIUS),
                ),
            )
    }

    /// The recent projects under their header and search. With projects
    /// open there is no section: a recovered project's row stands alone,
    /// and without one nothing shows.
    fn recent(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let rows = self.visible(cx);
        let section = !self.projects_open(cx);
        let selected = self.selected(&rows);
        let mut list = v_flex().gap(px(2.));
        let mut listed = 0;
        for (n, &row) in rows.iter().enumerate() {
            let row = match row {
                // The agents show it.
                Row::Session(_) => continue,
                Row::Recovered => self.recovered_row(n == selected, cx).into_any_element(),
                Row::Recent(ix) => self.row(ix, n == selected, cx).into_any_element(),
            };
            list = list.child(row);
            listed += 1;
        }
        if listed == 0 && !section {
            return None;
        }
        if listed == 0 {
            list = list.child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .text_color(t::T3)
                    .child("No recent projects"),
            );
        }
        Some(
            v_flex()
                .flex_none()
                .gap(px(8.))
                .when(section, |recent| recent.child(self.recent_header(listed)))
                .child(list),
        )
    }

    /// The setup list: one row each for the models, the transcription
    /// size and the agent, then FFmpeg and a browser when missing.
    fn setup(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.state(cx).read(cx);
        let ffmpeg = s
            .services
            .media
            .is_none()
            .then(|| setup_row("FFmpeg", install_button("welcome-ffmpeg", FFMPEG_INSTALL)));
        let browser = s.services.html.is_none().then(|| {
            setup_row(
                "Browser",
                install_button("welcome-browser", BROWSER_INSTALL),
            )
            .id("welcome-browser-row")
            .tooltip(Tooltip::text("For HTML titles"))
            .into_any_element()
        });
        v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(8.))
            .child(
                h_flex().h(px(32.)).px(px(10.)).child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child("Setup"),
                ),
            )
            .child(
                v_flex()
                    .px(px(10.))
                    .child(self.models_row(cx))
                    .child(self.transcription_row(cx))
                    .child(self.agent_row(cx))
                    .children(ffmpeg.map(IntoElement::into_any_element))
                    .children(browser),
            )
    }

    /// Every model as one download: its total size and Download, then its
    /// progress with Cancel, or why it failed with Retry.
    fn models_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.state(cx).read(cx);
        let labels: Vec<&str> = ModelTask::ALL.iter().map(|t| t.label()).collect();
        let count = div()
            .id("welcome-models-count")
            .font_family(t::mono())
            .text_size(px(11.))
            .text_color(t::T3)
            .child(format!("{} models", labels.len()))
            .tooltip(Tooltip::text(labels.join(", ")));
        let state = self.state(cx);
        let control = match s.setup_status() {
            SetupStatus::Complete => Icon::new(IconName::CircleCheck)
                .size(px(14.))
                .color(t::OK)
                .into_any_element(),
            SetupStatus::Missing { bytes } => h_flex()
                .gap(px(10.))
                .child(meta(crate::ui::format::size(bytes)))
                .child(
                    Button::new("welcome-download")
                        .primary()
                        .icon(IconName::Download)
                        .label("Download")
                        .on_click(move |_, _, cx| state.update(cx, |s, cx| s.download_setup(cx))),
                )
                .into_any_element(),
            SetupStatus::Downloading { fraction } => h_flex()
                .gap(px(8.))
                .child(Progress::new().value(fraction).w(px(160.)))
                .child(
                    meta(format!("{:.0} %", fraction * 100.))
                        .w(px(32.))
                        .flex()
                        .justify_end(),
                )
                .child(
                    Button::new("welcome-cancel")
                        .ghost()
                        .xsmall()
                        .icon(IconName::X)
                        .tooltip("Cancel")
                        .on_click(move |_, _, cx| state.update(cx, |s, cx| s.cancel_setup(cx))),
                )
                .into_any_element(),
            SetupStatus::Failed { message } => h_flex()
                .gap(px(8.))
                .child(
                    h_flex()
                        .id("welcome-download-failed")
                        .min_w_0()
                        .max_w(px(240.))
                        .gap(px(6.))
                        .text_size(px(11.))
                        .text_color(t::T2)
                        .child(Icon::new(IconName::CircleAlert).size(px(14.)).color(t::ERR))
                        .child(div().min_w_0().truncate().child(message.clone()))
                        .tooltip(Tooltip::text(message)),
                )
                .child(
                    Button::new("welcome-retry")
                        .primary()
                        .label("Retry")
                        .on_click(move |_, _, cx| state.update(cx, |s, cx| s.download_setup(cx))),
                )
                .into_any_element(),
        };
        setup_row("Models", h_flex().gap(px(12.)).child(count).child(control))
    }

    /// Whisper small or large, each with its size. Held while the models
    /// download, so the total stays what is fetched.
    fn transcription_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.state(cx).read(cx);
        let downloading = matches!(s.setup_status(), SetupStatus::Downloading { .. });
        let picked = s.picked_model(ModelTask::Transcription);
        let sizes: Vec<_> = crate::ui::editor_settings::transcription_sizes().collect();
        let selected = sizes
            .iter()
            .position(|(_, m)| m.id == picked.id)
            .unwrap_or(0);
        let items = sizes.iter().map(|(label, model)| {
            let bytes = crate::state::full_size(model);
            format!("{label} · {}", crate::ui::format::size(bytes))
        });
        let mut choice = Segmented::new("welcome-transcription")
            .field()
            .items(items)
            .selected(selected);
        for ix in 0..sizes.len() {
            choice = choice.disabled_item(ix, downloading && ix != selected);
        }
        let state = self.state(cx);
        setup_row(
            "Transcription",
            choice.on_click(move |ix, _, cx| {
                if let Some((_, model)) = crate::ui::editor_settings::transcription_sizes().nth(*ix)
                {
                    state.update(cx, |s, cx| {
                        s.pick_model(ModelTask::Transcription, model, cx)
                    });
                }
            }),
        )
    }

    /// The agent CLIs found and their versions; when neither is, where to
    /// install one and Check again. It holds no project back.
    fn agent_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let control = match &self.agents {
            Agents::Unchecked | Agents::Checking(_) => {
                Spinner::new().size(px(12.)).into_any_element()
            }
            Agents::Found(found) if found.is_empty() => h_flex()
                .gap(px(8.))
                .child(install_button("welcome-agent", AGENT_INSTALL))
                .child(
                    Button::new("welcome-agent-check")
                        .label("Check again")
                        .on_click(cx.listener(|this, _, _, cx| this.check_agents(cx))),
                )
                .into_any_element(),
            Agents::Found(found) => h_flex()
                .gap(px(12.))
                .children(found.iter().map(|(backend, version)| {
                    h_flex()
                        .gap(px(6.))
                        .text_size(px(11.))
                        .text_color(t::T2)
                        .child(backend.label())
                        .child(meta(version.clone()))
                }))
                .child(Icon::new(IconName::CircleCheck).size(px(14.)).color(t::OK))
                .into_any_element(),
        };
        setup_row("Agent", control)
    }
}

impl Render for Welcome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.dropping.is_some() && !cx.has_active_drag() {
            self.dropping = None;
        }
        let waiting = self.state(cx).read(cx).projects_wait();
        if waiting && matches!(self.agents, Agents::Unchecked) {
            self.check_agents(cx);
        }
        if waiting && self.download_poll.is_none() && self.state(cx).read(cx).models.busy() {
            self.download_poll = Some(crate::ui::repaint_while(
                cx,
                DOWNLOAD_POLL,
                |this, cx| this.state(cx).read(cx).models.busy(),
                |this| this.download_poll = None,
            ));
        }
        let sessions = self.ordered(cx);
        // With projects open the page is theirs: see `agents_page`.
        let wide = !sessions.is_empty();
        let page = match wide {
            true => self.agents_page(&sessions, window, cx).into_any_element(),
            false => self.start_page(waiting, window, cx).into_any_element(),
        };
        let dropping = wide && self.dropping.is_some();
        div()
            .id("welcome")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .relative()
            .size_full()
            .bg(t::G)
            .child(glow())
            // Centred while it fits; with many agents and recent projects
            // the page is taller than the window and scrolls.
            .child(
                div()
                    .id("welcome-page")
                    .size_full()
                    .overflow_y_scrollbar()
                    .child(
                        div()
                            .flex()
                            .justify_center()
                            .min_h_full()
                            .map(|centre| match wide {
                                // A chat's composer keeps what is dropped on it.
                                true => centre
                                    .on_drag_move(cx.listener(Self::drag_moved))
                                    .on_drop(cx.listener(Self::drop_files)),
                                false => centre.items_center(),
                            })
                            .child(page),
                    ),
            )
            .when(dropping, |welcome| welcome.child(drop_outline()))
    }
}

impl Welcome {
    /// The page with no project open: the name, New project, Open… and the
    /// drop zone on its side, beside the prompt that starts an agent and
    /// the recent projects, or the setup list while the models are missing.
    fn start_page(
        &mut self,
        waiting: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let version = env!("CARGO_PKG_VERSION");
        let side = v_flex()
            .w(px(340.))
            .flex_none()
            .child(
                div()
                    .text_size(px(30.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Flikk"),
            )
            .child(
                div()
                    .mt(px(4.))
                    .font_family(t::mono())
                    .text_color(t::T3)
                    .child(version),
            )
            .child(div().mt(px(32.)).child(self.actions(waiting, cx)))
            .child(self.drop_zone(waiting, cx));
        let lists = match waiting {
            true => self.setup(cx).into_any_element(),
            false => v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(24.))
                .child(self.board(&[], window, cx))
                .children(self.recent(cx))
                .into_any_element(),
        };
        h_flex()
            .w(px(1080.))
            .max_w_full()
            .p(px(board::PAGE_PADDING))
            .items_stretch()
            .gap(px(56.))
            .child(side)
            .child(lists)
    }

    /// The page with projects open, which is their agents': they take its
    /// width, their header starts and opens projects, and a recovered
    /// project keeps its row under them.
    fn agents_page(
        &mut self,
        sessions: &[crate::desk::Session],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        v_flex()
            .w_full()
            .max_w_full()
            .p(px(board::PAGE_PADDING))
            .gap(px(24.))
            .child(self.board(sessions, window, cx))
            .children(self.recent(cx))
    }
}

/// The dashed line around the page while files are held over it: a drop
/// anywhere starts a project.
fn drop_outline() -> impl IntoElement {
    div()
        .absolute()
        .inset(px(8.))
        .rounded(t::RADIUS_LG)
        .border_1()
        .border_dashed()
        .border_color(t::AC)
}

/// What is held over a drop: the one file or folder by name, a folder with
/// how many files it holds, else how many items.
fn drop_hint(paths: &[PathBuf]) -> String {
    match paths {
        [one] => {
            let name = one
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match std::fs::read_dir(one) {
                Ok(dir) => format!("{name} · {} files", dir.count()),
                Err(_) => name,
            }
        }
        many => format!("{} items", many.len()),
    }
}

/// The faint light falling from the top centre of the page.
fn glow() -> impl IntoElement {
    static GLOW: LazyLock<Arc<RenderImage>> = LazyLock::new(|| {
        // An ellipse 60 % wide and 50 % high around the top centre, from
        // `#16161A` to the window colour at 70 % of the way out.
        let (w, h) = (160usize, 100usize);
        let (from, to) = (t::hex(0x16161A), t::G);
        let mut bgra = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let dx = (x as f32 + 0.5) / w as f32 - 0.5;
                let dy = (y as f32 + 0.5) / h as f32;
                let d = ((dx / 0.6).powi(2) + (dy / 0.5).powi(2)).sqrt();
                let k = (d / 0.7).min(1.);
                let mix = |a: f32, b: f32| ((a + (b - a) * k) * 255.).round() as u8;
                bgra.extend_from_slice(&[
                    mix(from.b, to.b),
                    mix(from.g, to.g),
                    mix(from.r, to.r),
                    255,
                ]);
            }
        }
        let buffer =
            image::RgbaImage::from_raw(w as u32, h as u32, bgra).expect("buffer matches its size");
        Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
    });
    canvas(
        |_, _, _| {},
        |bounds, _, window, _| {
            let _ = window.paint_image(bounds, bounds, Corners::default(), GLOW.clone(), 0, false);
        },
    )
    .absolute()
    .inset_0()
}

/// `path` with the home folder as `~`.
fn home(path: &Path) -> String {
    let path = path.to_string_lossy();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.into_owned(),
    }
}

/// "Today, 09:42", "Yesterday", "Mon, 18:05", "28 Oct".
fn when(at: SystemTime) -> String {
    use chrono::{DateTime, Local};
    let at: DateTime<Local> = at.into();
    let days = (Local::now().date_naive() - at.date_naive()).num_days();
    match days {
        0 => at.format("Today, %H:%M").to_string(),
        1 => "Yesterday".into(),
        2..=6 => at.format("%a, %H:%M").to_string(),
        _ => at.format("%-d %b").to_string(),
    }
}

/// "16:9", "9:16", "1:1"…
fn aspect(w: u32, h: u32) -> String {
    fn gcd(a: u32, b: u32) -> u32 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let g = gcd(w, h).max(1);
    format!("{}:{}", w / g, h / g)
}

/// A row of the setup list: its label on the left, `control` on the right.
fn setup_row(label: &'static str, control: impl IntoElement) -> Stateful<Div> {
    h_flex()
        .id(label)
        .h(px(44.))
        .gap(px(12.))
        .border_b_1()
        .border_color(t::LN)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .child(label),
        )
        .child(control)
}

/// "Install…", which opens `url` in the browser.
fn install_button(id: &'static str, url: &'static str) -> Button {
    Button::new(id)
        .icon(IconName::ExternalLink)
        .label("Install…")
        .on_click(move |_, _, cx| cx.open_url(url))
}

/// Quiet mono meta text: sizes, percentages, versions.
fn meta(text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .font_family(t::mono())
        .text_size(px(11.))
        .text_color(t::T3)
        .child(text.into())
}
