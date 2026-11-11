//! The welcome page the app opens on: New project, Open…, a drop zone that
//! starts a project from footage, and the recent projects. While a model
//! is missing, the setup list takes the recent projects' place and no
//! project opens: the models to download as one, the transcription size,
//! the agent CLIs found, and FFmpeg and a browser when they are not.

use crate::projects;
use crate::state::{EditorState, SetupStatus};
use crate::ui::kit::{
    Button, Icon, IconName, Input, Kbd, Progress, Segmented, Spinner, Tooltip, h_flex, theme as t,
    v_flex,
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
    /// Another project is being started or opened: footage held from an
    /// earlier `Create` (whose dialog was cancelled) must not go into it.
    ForgetFootage,
}

/// A row of the list.
#[derive(Clone, Copy, PartialEq)]
enum Row {
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
    /// Asked under this number; a later ask drops its answer.
    Checking(u64),
    /// Those found, with the version each reports.
    Found(Vec<(Backend, String)>),
}

pub struct Welcome {
    state: Entity<EditorState>,
    agents: Agents,
    /// Numbers each look for the agent CLIs.
    agents_asked: u64,
    /// Redraws the setup list while the models download.
    download_poll: Option<Task<()>>,
    search: Entity<InputState>,
    recent: Vec<Recent>,
    /// Index into the visible rows, for Up, Down and Enter.
    selected: usize,
    /// Files from outside held over the drop zone.
    dropping: Option<Vec<PathBuf>>,
    focus: FocusHandle,
    _search: Subscription,
    _state: Subscription,
}

impl EventEmitter<WelcomeEvent> for Welcome {}

impl Welcome {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search projects"));
        let _search = cx.subscribe(&search, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.selected = 0;
                cx.notify();
            }
        });
        let recent = crate::persistence::recent()
            .into_iter()
            .map(Recent::read)
            .collect();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let _state = cx.observe(&state, |_, _, cx| cx.notify());
        let mut this = Self {
            state,
            agents: Agents::Checking(0),
            agents_asked: 0,
            download_poll: None,
            search,
            recent,
            selected: 0,
            dropping: None,
            focus,
            _search,
            _state,
        };
        if this.state.read(cx).projects_wait() {
            this.check_agents(cx);
        }
        this
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

    /// The rows matching the search, in order.
    fn visible(&self, cx: &App) -> Vec<Row> {
        let query = self.search.read(cx).value().trim().to_lowercase();
        let s = self.state.read(cx);
        if s.projects_wait() {
            return Vec::new();
        }
        let recovered = s.file().is_none()
            && s.is_dirty()
            && s.bus.project().name.to_lowercase().contains(&query);
        let recent = (0..self.recent.len()).filter(|&i| {
            let r = &self.recent[i];
            query.is_empty()
                || r.name.to_lowercase().contains(&query)
                || r.path.to_string_lossy().to_lowercase().contains(&query)
        });
        recovered
            .then_some(Row::Recovered)
            .into_iter()
            .chain(recent.map(Row::Recent))
            .collect()
    }

    fn pick(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        match row {
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
        } else if self.state.read(cx).file() == Some(r.path.as_path()) {
            cx.emit(WelcomeEvent::Continue);
        } else {
            projects::open(&self.state, r.path.clone(), window, cx);
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
        let state = self.state.clone();
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
        if self.state.read(cx).projects_wait() {
            cx.notify();
            return;
        }
        let paths = paths.paths().to_vec();
        if let [file] = paths.as_slice()
            && file.extension().is_some_and(|e| e == EXTENSION)
        {
            cx.emit(WelcomeEvent::ForgetFootage);
            projects::open(&self.state, file.clone(), window, cx);
        } else if !paths.is_empty() {
            cx.emit(WelcomeEvent::Create(paths));
        }
        cx.notify();
    }

    fn key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.visible(cx);
        // As the list shows it: rows can go without a search, as the
        // recovered one does once its project is saved.
        self.selected = self.selected.min(rows.len().saturating_sub(1));
        match ev.keystroke.key.as_str() {
            "down" if !rows.is_empty() => {
                self.selected = (self.selected + 1).min(rows.len() - 1);
            }
            "up" => self.selected = self.selected.saturating_sub(1),
            "enter" => {
                if let Some(&row) = rows.get(self.selected) {
                    self.pick(row, window, cx);
                }
                return;
            }
            "/" if !self.search.read(cx).focus_handle(cx).is_focused(window) => {
                self.search.update(cx, |s, cx| s.focus(window, cx));
                cx.stop_propagation();
            }
            _ => return,
        }
        cx.notify();
    }

    /// New project and Open…, held while the models are missing.
    fn actions(&self, waiting: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let big = |b: Button| {
            b.h(px(38.))
                .w_full()
                .justify_start()
                .px(px(14.))
                .text_size(px(13.))
        };
        v_flex()
            .gap(px(8.))
            .child(big(Button::new("welcome-new")
                .when(!waiting, |b| b.primary())
                .disabled(waiting)
                .icon(IconName::Plus)
                .label("New project")
                .child(div().flex_1())
                .kbd(format!("{} N", crate::app::MOD))
                .on_click(cx.listener(|this, _, window, cx| {
                    cx.emit(WelcomeEvent::ForgetFootage);
                    projects::new_project(&this.state, window, cx)
                }))))
            .child(big(Button::new("welcome-open")
                .disabled(waiting)
                .icon(IconName::FolderOpen)
                .label("Open…")
                .child(div().flex_1())
                .kbd(format!("{} O", crate::app::MOD))
                .on_click(cx.listener(|this, _, window, cx| {
                    cx.emit(WelcomeEvent::ForgetFootage);
                    projects::open_dialog(&this.state, window, cx)
                }))))
    }

    fn drop_zone(&self, waiting: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let dropping = self.dropping.as_ref().filter(|_| !waiting);
        let (title, hint): (SharedString, SharedString) = match dropping {
            Some(paths) => {
                let hint = match paths.as_slice() {
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
                };
                ("Drop to create a project".into(), hint.into())
            }
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
            .on_drag_move(
                cx.listener(|this, ev: &DragMoveEvent<ExternalPaths>, _, cx| {
                    let waiting = this.state.read(cx).projects_wait();
                    let over = (!waiting && ev.bounds.contains(&ev.event.position))
                        .then(|| ev.drag(cx).paths().to_vec());
                    if over != this.dropping {
                        this.dropping = over;
                        cx.notify();
                    }
                }),
            )
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
        let name = self.state.read(cx).bus.project().name.clone();
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

    fn recent(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.visible(cx);
        let selected = self.selected.min(rows.len().saturating_sub(1));
        v_flex()
            .flex_1()
            .min_w_0()
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
                            .child("Recent"),
                    )
                    .child(
                        div()
                            .font_family(t::mono())
                            .text_size(px(11.))
                            .text_color(t::T3)
                            .child(rows.len().to_string()),
                    )
                    .child(div().flex_1())
                    .child(
                        div().w(px(220.)).child(
                            Input::new(&self.search)
                                .search()
                                .kbd("/")
                                .bg(t::P)
                                .rounded(t::RADIUS),
                        ),
                    ),
            )
            .child({
                let mut list = v_flex().gap(px(2.));
                for (n, &row) in rows.iter().enumerate() {
                    list = list.child(match row {
                        Row::Recovered => self.recovered_row(n == selected, cx).into_any_element(),
                        Row::Recent(ix) => self.row(ix, n == selected, cx).into_any_element(),
                    });
                }
                if rows.is_empty() {
                    list = list.child(
                        div()
                            .px(px(10.))
                            .py(px(8.))
                            .text_color(t::T3)
                            .child("No recent projects"),
                    );
                }
                list
            })
    }

    /// The setup list: one row each for the models, the transcription
    /// size and the agent, then FFmpeg and a browser when missing.
    fn setup(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.state.read(cx);
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
        let s = self.state.read(cx);
        let labels: Vec<&str> = ModelTask::ALL.iter().map(|t| t.label()).collect();
        let count = div()
            .id("welcome-models-count")
            .font_family(t::mono())
            .text_size(px(11.))
            .text_color(t::T3)
            .child(format!("{} models", labels.len()))
            .tooltip(Tooltip::text(labels.join(", ")));
        let state = self.state.clone();
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
        let s = self.state.read(cx);
        let downloading = matches!(s.setup_status(), SetupStatus::Downloading { .. });
        let picked = ModelTask::Transcription.model();
        let sizes: Vec<_> = crate::ui::editor_settings::transcription_sizes().collect();
        let selected = sizes
            .iter()
            .position(|(_, m)| m.id == picked.id)
            .unwrap_or(0);
        let items = sizes.iter().map(|(label, model)| {
            let bytes = model.files.iter().map(|f| f.size).sum();
            format!("{label} · {}", crate::ui::format::size(bytes))
        });
        let mut choice = Segmented::new("welcome-transcription")
            .field()
            .items(items)
            .selected(selected);
        for ix in 0..sizes.len() {
            choice = choice.disabled_item(ix, downloading && ix != selected);
        }
        let state = self.state.clone();
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
            Agents::Checking(_) => Spinner::new().size(px(12.)).into_any_element(),
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.dropping.is_some() && !cx.has_active_drag() {
            self.dropping = None;
        }
        let waiting = self.state.read(cx).projects_wait();
        if waiting && self.download_poll.is_none() && self.state.read(cx).models.busy() {
            self.download_poll = Some(crate::ui::repaint_while(
                cx,
                DOWNLOAD_POLL,
                |this, cx| this.state.read(cx).models.busy(),
                |this| this.download_poll = None,
            ));
        }
        let version = env!("CARGO_PKG_VERSION");
        div()
            .id("welcome")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .relative()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(t::G)
            .child(glow())
            .child(
                h_flex()
                    .w(px(1080.))
                    .max_w_full()
                    .mt(px(-20.))
                    .px(px(24.))
                    .items_stretch()
                    .gap(px(56.))
                    .child(
                        v_flex()
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
                            .child(self.drop_zone(waiting, cx)),
                    )
                    .map(|page| {
                        if waiting {
                            page.child(self.setup(cx))
                        } else {
                            page.child(self.recent(cx))
                        }
                    }),
            )
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
