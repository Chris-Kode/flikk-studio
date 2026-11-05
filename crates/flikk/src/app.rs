//! Window layout: the title bar (project menu, workspace switcher, undo,
//! notices, Settings, Render), the workspace's panels, the agent chat
//! docked on their right in every workspace (PRD AG1) or folded to a rail,
//! and the status bar.

use crate::notices::NoticeAction;
use crate::projects::{self, NewProject, OpenProject, ProjectSettings, SaveProject, SaveProjectAs};
use crate::state::EditorState;
use crate::ui::caption_style::{CaptionStylePanel, Target};
use crate::ui::editor_settings::{EditorSettings, SettingsTab};
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, DropdownMenu as _, IconName, Indicator, Kbd, Panel, PanelHeader, PanelTabs,
    PopupMenuItem, Progress, ScrollableElement as _, Segmented, h_flex, h_resizable,
    resizable_panel, v_resizable,
};
use crate::ui::{
    chat::ChatPanel,
    code_editor::CodeEditorPanel,
    deliver::DeliverPanel,
    inspector::InspectorPanel,
    library::LibraryPanel,
    media_pool::MediaPoolPanel,
    mixer::MixerPanel,
    scopes::ScopesPanel,
    timeline::TimelinePanel,
    transcript::TranscriptPanel,
    viewer::ViewerPanel,
    welcome::{Welcome, WelcomeEvent},
};
use flikk_core::agent::Workspace;
use flikk_core::changeset::ChangeSetState;
use flikk_core::{ProjectId, Rate, TimeRange};
use flikk_deliver::RenderState;
use gpui_kit::base::{InteractiveElementExt as _, ResizablePanel};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;
use std::time::Duration;

actions!(
    flikk,
    [
        Undo,
        Redo,
        TogglePlay,
        OpenSettings,
        OpenAgentSettings,
        ToggleAgent,
        Quit
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-z", Undo, None),
        KeyBinding::new("ctrl-z", Undo, None),
        KeyBinding::new("cmd-shift-z", Redo, None),
        KeyBinding::new("ctrl-shift-z", Redo, None),
        KeyBinding::new("ctrl-y", Redo, None),
        KeyBinding::new("space", TogglePlay, Some("Timeline")),
        KeyBinding::new("space", TogglePlay, Some("Viewer")),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("ctrl-,", OpenSettings, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
    projects::bind_keys(cx);
}

/// The platform's command modifier as shortcut chips spell it.
pub(crate) const MOD: &str = if cfg!(target_os = "macos") {
    "Cmd"
} else {
    "Ctrl"
};

/// The agent chat's width when docked.
const CHAT_WIDTH: f32 = 340.;
/// The folded chat's rail.
const RAIL_WIDTH: f32 = 44.;
/// How often the status bar redraws while a render runs.
const RENDER_POLL: Duration = Duration::from_millis(250);

/// What the Edit workspace's left panel shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LeftTab {
    Media,
    Effects,
}

pub struct Flikk {
    state: Entity<EditorState>,
    timeline: Entity<TimelinePanel>,
    viewer: Entity<ViewerPanel>,
    media_pool: Entity<MediaPoolPanel>,
    inspector: Entity<InspectorPanel>,
    transcript: Entity<TranscriptPanel>,
    caption_style: Entity<CaptionStylePanel>,
    deliver: Entity<DeliverPanel>,
    chat: Entity<ChatPanel>,
    scopes: Entity<ScopesPanel>,
    mixer: Entity<MixerPanel>,
    library: Entity<LibraryPanel>,
    code_editor: Entity<CodeEditorPanel>,
    settings: Entity<EditorSettings>,
    left_tab: LeftTab,
    /// The agent chat is docked, not folded to the rail.
    agent_open: bool,
    /// The notice list is dropped down.
    notices_open: bool,
    /// Unsaved changes as last told to the window.
    edited: bool,
    /// The title bar was pressed: moving the pointer moves the window.
    moving: bool,
    /// The status bar is redrawing for a running render.
    render_poll: bool,
    /// The welcome page, shown at launch until a project is picked, and the
    /// project open underneath it.
    welcome: Option<(Entity<Welcome>, ProjectId)>,
    /// Footage dropped on the welcome page, imported once its new project
    /// is open.
    footage: Option<Vec<PathBuf>>,
    _observe: Subscription,
    _welcome: Option<Subscription>,
}

impl Flikk {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(EditorState::new);
        if crate::demo::enabled() {
            state.update(cx, crate::demo::seed);
        }
        let _observe = cx.observe(&state, |this, _, cx| this.state_changed(cx));
        // Every launch starts on the welcome page, unless the app was
        // started for a file or a demo.
        let welcome =
            (!crate::demo::enabled() && !crate::persistence::launched_with_file()).then(|| {
                let project = state.read(cx).bus.project().id;
                (
                    cx.new(|cx| Welcome::new(state.clone(), window, cx)),
                    project,
                )
            });
        let _welcome = welcome.as_ref().map(|(page, _)| {
            cx.subscribe_in(page, window, |this, _, ev: &WelcomeEvent, window, cx| {
                match ev {
                    WelcomeEvent::Continue => this.welcome = None,
                    WelcomeEvent::Create(paths) => {
                        this.footage = Some(paths.clone());
                        projects::new_project(&this.state, window, cx);
                    }
                }
                cx.notify();
            })
        });
        let closing = state.clone();
        window.on_window_should_close(cx, move |window, cx| {
            projects::should_close(&closing, window, cx)
        });
        if let Some(page) = crate::demo::settings_page().and_then(|p| SettingsTab::named(&p)) {
            cx.defer_in(window, move |this: &mut Self, window, cx| {
                this.open_settings(Some(page), window, cx)
            });
        }
        Self {
            timeline: cx.new(|cx| TimelinePanel::new(state.clone(), window, cx)),
            viewer: cx.new(|cx| ViewerPanel::new(state.clone(), window, cx)),
            media_pool: cx.new(|cx| MediaPoolPanel::new(state.clone(), window, cx)),
            inspector: cx.new(|cx| InspectorPanel::new(state.clone(), window, cx)),
            transcript: cx.new(|cx| TranscriptPanel::new(state.clone(), window, cx)),
            caption_style: cx
                .new(|cx| CaptionStylePanel::new(state.clone(), Target::Active, window, cx)),
            deliver: cx.new(|cx| DeliverPanel::new(state.clone(), window, cx)),
            chat: cx.new(|cx| ChatPanel::new(state.clone(), window, cx)),
            scopes: cx.new(|cx| ScopesPanel::new(state.clone(), window, cx)),
            mixer: cx.new(|cx| MixerPanel::new(state.clone(), window, cx)),
            library: cx.new(|cx| LibraryPanel::new(state.clone(), window, cx)),
            code_editor: cx.new(|cx| CodeEditorPanel::new(state.clone(), window, cx)),
            settings: cx.new(|cx| EditorSettings::new(state.clone(), window, cx)),
            agent_open: true,
            notices_open: false,
            edited: false,
            moving: false,
            render_poll: false,
            left_tab: match crate::demo::fx_view().as_deref() {
                Some("library" | "code") => LeftTab::Effects,
                _ => LeftTab::Media,
            },
            welcome,
            footage: None,
            state,
            _observe,
            _welcome,
        }
    }

    /// Leave the welcome page once another project is open, importing the
    /// footage it was started from.
    fn state_changed(&mut self, cx: &mut Context<Self>) {
        let project = self.state.read(cx).bus.project().id;
        if self.welcome.as_ref().is_some_and(|(_, at)| *at != project) {
            self.welcome = None;
            if let Some(paths) = self.footage.take() {
                if let [folder] = paths.as_slice()
                    && folder.is_dir()
                    && let Some(name) = folder.file_name()
                {
                    let name = name.to_string_lossy().into_owned();
                    self.state.update(cx, |s, _| s.bus.rename_project(&name));
                }
                self.media_pool
                    .update(cx, |pool, cx| pool.import(paths, None, cx));
            }
        }
        cx.notify();
    }

    /// Open Settings on `tab`, or on the page shown last.
    fn open_settings(
        &mut self,
        tab: Option<SettingsTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.notices_open = false;
        cx.notify();
        EditorSettings::open(&self.settings, tab, window, cx);
    }

    /// Project menu on the left, workspaces centred, history, notices,
    /// Settings and Render on the right. On macOS the bar also holds the
    /// window buttons and moves the window.
    fn title_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let macos = cfg!(target_os = "macos");
        let lights = macos && !window.is_fullscreen();
        // Presses on the controls are theirs, not the start of a move.
        let own = |el: Div| el.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        div()
            .id("title-bar")
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .h(t::TITLE_BAR)
            .pl(px(if lights { 76. } else { 6. }))
            .pr(px(12.))
            .border_b_1()
            .border_color(t::LN)
            .bg(t::P)
            .when(macos, |bar| {
                bar.on_double_click(|_, window, _| window.titlebar_double_click())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.moving = true),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.moving = false),
                    )
                    .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, _| {
                        if this.moving && e.pressed_button == Some(MouseButton::Left) {
                            this.moving = false;
                            window.start_window_move();
                        }
                    }))
            })
            .map(|bar| {
                if self.welcome.is_some() {
                    return bar
                        .child(
                            div()
                                .pl(px(6.))
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t::T2)
                                .child("Flikk"),
                        )
                        .child(div().flex_1())
                        .child(own(h_flex().gap(px(2.))).children(self.top_right(cx)));
                }
                bar.child(own(div()).child(self.project_menu(cx)))
                    .child(div().flex_1())
                    .child(own(h_flex().gap(px(2.))).children(self.top_right(cx)))
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(own(div()).child(self.workspace_tabs(cx))),
                    )
            })
    }

    /// Project name, "Edited" while there are unsaved changes; opens the
    /// project file commands and recent projects.
    fn project_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        let s = state.read(cx);
        let name = s.bus.project().name.clone();
        let edited = s.is_dirty();
        Button::new("project")
            .ghost()
            .px(px(6.))
            .text_color(t::T1)
            .label(name)
            .when(edited, |b| {
                b.child(div().size(px(6.)).rounded_full().bg(t::T3)).child(
                    div()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(t::T3)
                        .child("Edited"),
                )
            })
            .trailing_icon(IconName::ChevronDown)
            .dropdown_menu(move |menu, window, cx| {
                let item =
                    |label: &'static str,
                     action: Box<dyn Action>,
                     f: fn(&Entity<EditorState>, &mut Window, &mut App)| {
                        let state = state.clone();
                        PopupMenuItem::new(label)
                            .action(action)
                            .on_click(move |_, window, cx| f(&state, window, cx))
                    };
                let (current, recent) = {
                    let s = state.read(cx);
                    (s.file.clone(), s.recent_projects())
                };
                let mut menu = menu
                    .item(
                        item("New project", Box::new(NewProject), projects::new_project)
                            .icon(IconName::FilePlus),
                    )
                    .item(
                        item("Open…", Box::new(OpenProject), projects::open_dialog)
                            .icon(IconName::FolderOpen),
                    );
                if !recent.is_empty() {
                    let state = state.clone();
                    menu = menu.submenu("Open recent", window, cx, move |mut menu, _, _| {
                        for path in &recent {
                            let (state, path) = (state.clone(), path.clone());
                            let label = flikk_store::project_file::name_of(&path);
                            let checked = current.as_ref() == Some(&path);
                            menu = menu.item(PopupMenuItem::new(label).checked(checked).on_click(
                                move |_, window, cx| {
                                    projects::open(&state, path.clone(), window, cx)
                                },
                            ));
                        }
                        menu
                    });
                }
                menu.separator()
                    .item(item("Save", Box::new(SaveProject), projects::save).icon(IconName::Save))
                    .item(item("Save as…", Box::new(SaveProjectAs), projects::save_as))
                    .separator()
                    .item(
                        item(
                            "Project settings…",
                            Box::new(ProjectSettings),
                            projects::settings,
                        )
                        .icon(IconName::Settings2),
                    )
            })
    }

    fn workspace_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.state.read(cx).workspace;
        let state = self.state.clone();
        Segmented::new("workspaces")
            .items(Workspace::ALL.iter().map(|w| w.label()))
            .selected(
                Workspace::ALL
                    .iter()
                    .position(|w| *w == current)
                    .unwrap_or(0),
            )
            .on_click(move |ix, _, cx| {
                state.update(cx, |s, cx| s.set_workspace(Workspace::ALL[*ix], cx))
            })
    }

    /// Undo and redo, the notices bell, Settings and, outside Deliver,
    /// Render, which goes there.
    fn top_right(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = self.state.read(cx);
        let (can_undo, can_redo) = (s.bus.history().can_undo(), s.bus.history().can_redo());
        let delivering = s.workspace == Workspace::Deliver;
        let this = cx.entity().downgrade();
        let notices = crate::ui::notices::button(
            &self.state,
            self.notices_open,
            {
                let this = this.clone();
                move |open, _, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.notices_open = *open;
                        cx.notify();
                    });
                }
            },
            move |action, window, cx| {
                let _ = this.update(cx, |this, cx| match action {
                    NoticeAction::OpenModels => {
                        this.open_settings(Some(SettingsTab::Integrations), window, cx)
                    }
                    NoticeAction::OpenIntegrations => {
                        this.open_settings(Some(SettingsTab::Integrations), window, cx)
                    }
                    NoticeAction::Relink => crate::relink::relink_folder(&this.state, cx),
                });
            },
            cx,
        );
        let mut items = vec![
            Button::new("undo")
                .ghost()
                .icon(IconName::Undo2)
                .tooltip_kbd("Undo", format!("{MOD} Z"))
                .disabled(!can_undo)
                .on_click(cx.listener(|this, _, _, cx| this.state.update(cx, |s, cx| s.undo(cx))))
                .into_any_element(),
            Button::new("redo")
                .ghost()
                .icon(IconName::Redo2)
                .tooltip_kbd("Redo", format!("Shift {MOD} Z"))
                .disabled(!can_redo)
                .on_click(cx.listener(|this, _, _, cx| this.state.update(cx, |s, cx| s.redo(cx))))
                .into_any_element(),
            div()
                .w(px(1.))
                .h(px(16.))
                .mx(px(6.))
                .bg(t::LN)
                .into_any_element(),
            notices.into_any_element(),
            Button::new("settings")
                .ghost()
                .icon(IconName::Settings)
                .tooltip_kbd("Settings", format!("{MOD} ,"))
                .on_click(cx.listener(|this, _, window, cx| this.open_settings(None, window, cx)))
                .into_any_element(),
        ];
        if self.welcome.is_some() {
            // Nothing to undo or render before a project is picked.
            items.drain(..3);
            return items;
        }
        if !delivering {
            items.push(
                Button::new("render")
                    .primary()
                    .ml(px(8.))
                    .icon(IconName::Upload)
                    .label("Render")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state
                            .update(cx, |s, cx| s.set_workspace(Workspace::Deliver, cx))
                    }))
                    .into_any_element(),
            );
        }
        items
    }

    /// The panels over the timeline, the agent chat beside them.
    fn over_timeline(
        &self,
        id: &'static str,
        panels: Vec<ResizablePanel>,
        timeline: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // One timeline height per workspace, whichever panels sit above.
        let workspace = self.state.read(cx).workspace as usize;
        v_resizable(("over-timeline", workspace))
            .child(resizable_panel().child(self.with_agent(id, panels, cx)))
            .child(
                resizable_panel()
                    .size(px(timeline))
                    .child(self.timeline.clone()),
            )
            .into_any_element()
    }

    /// `panels` side by side, then the docked chat or its rail.
    fn with_agent(
        &self,
        id: &'static str,
        panels: Vec<ResizablePanel>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.agent_open;
        let chat = open.then(|| {
            resizable_panel()
                .size(px(CHAT_WIDTH))
                .fixed()
                .child(self.chat.clone())
        });
        // A group's `children` replaces its panels, so they go in at once.
        h_flex()
            .size_full()
            .child(
                div().flex_1().min_w_0().h_full().child(
                    h_resizable((id, open as usize)).children(panels.into_iter().chain(chat)),
                ),
            )
            .when(!open, |row| row.child(self.rail(cx)))
    }

    /// The folded chat: its button, counting the change sets waiting for
    /// review.
    fn rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pending = pending_reviews(self.state.read(cx));
        div()
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .w(px(RAIL_WIDTH))
            .h_full()
            .pt(px(8.))
            .border_l_1()
            .border_color(t::LN)
            .bg(t::P)
            .child(
                Button::new("agent-rail")
                    .ghost()
                    .relative()
                    .icon(IconName::MessageSquareText)
                    .tooltip("Agent")
                    .when(pending > 0, |b| b.child(Indicator::Count(pending)))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_agent(cx))),
            )
    }

    fn toggle_agent(&mut self, cx: &mut Context<Self>) {
        self.agent_open = !self.agent_open;
        cx.notify();
    }

    /// Edit's left panel: the media pool or the effects library.
    fn edit_left(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.left_tab;
        let (body, actions): (AnyElement, AnyElement) = match tab {
            LeftTab::Media => (
                self.media_pool.clone().into_any_element(),
                self.media_pool.update(cx, |p, cx| p.header_actions(cx)),
            ),
            LeftTab::Effects => (
                self.library.clone().into_any_element(),
                self.library.update(cx, |p, cx| p.header_actions(cx)),
            ),
        };
        Panel::new()
            .child(
                PanelHeader::new().child(actions).leading(
                    PanelTabs::new("edit-left")
                        .items(["Media", "Effects"])
                        .selected(match tab {
                            LeftTab::Media => 0,
                            LeftTab::Effects => 1,
                        })
                        .on_click(cx.listener(|this, ix: &usize, _, cx| {
                            this.left_tab = if *ix == 0 {
                                LeftTab::Media
                            } else {
                                LeftTab::Effects
                            };
                            cx.notify();
                        })),
                ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }

    /// The Captions workspace's caption style, under its header.
    fn caption_style_panel(&self) -> impl IntoElement {
        Panel::new()
            .child(PanelHeader::new().title("Caption style"))
            .child(
                div()
                    .id("caption-style")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(self.caption_style.clone()),
            )
    }

    fn workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        let side = |width: f32| resizable_panel().size(px(width)).fixed();
        match self.state.read(cx).workspace {
            Workspace::Edit => {
                // The code editor takes the inspector's place while open;
                // the inspector shows only with something selected.
                let coding = self.state.read(cx).editing_fx.is_some();
                let right: Option<(&'static str, AnyElement, f32)> = if coding {
                    Some((
                        "edit-code",
                        self.code_editor.clone().into_any_element(),
                        460.,
                    ))
                } else if self.inspector.read(cx).has_subject(cx) {
                    Some(("edit", self.inspector.clone().into_any_element(), 300.))
                } else {
                    None
                };
                let id = right.as_ref().map_or("edit-bare", |(id, _, _)| *id);
                let mut panels = vec![
                    side(260.).child(self.edit_left(cx)),
                    resizable_panel().child(self.viewer.clone()),
                ];
                panels.extend(right.map(|(_, right, width)| side(width).child(right)));
                self.over_timeline(id, panels, 340., cx)
            }
            Workspace::Captions => {
                let panels = vec![
                    side(340.).child(self.transcript.clone()),
                    resizable_panel().child(self.viewer.clone()),
                    side(300.).child(self.caption_style_panel()),
                ];
                self.over_timeline("captions", panels, 340., cx)
            }
            Workspace::Color => {
                let panels = vec![
                    resizable_panel().child(self.viewer.clone()),
                    side(330.).child(self.scopes.clone()),
                    side(320.).child(self.inspector.clone()),
                ];
                self.over_timeline("color", panels, 340., cx)
            }
            Workspace::Audio => {
                let panels = vec![
                    resizable_panel().child(self.viewer.clone()),
                    side(420.).child(self.mixer.clone()),
                    side(300.).child(self.inspector.clone()),
                ];
                self.over_timeline("audio", panels, 300., cx)
            }
            Workspace::Deliver => {
                let panels = vec![
                    side(380.).child(self.deliver.clone()),
                    resizable_panel().child(self.viewer.clone()),
                ];
                self.with_agent("deliver", panels, cx).into_any_element()
            }
        }
    }

    /// What the app can report: the last status message, a running
    /// render, the timeline's format, the background render cache; on the
    /// right, change sets waiting while the chat is docked, the selection
    /// or the timeline's length, and the shortcuts to reach for.
    fn status_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.state.read(cx);
        let project = s.bus.project();
        let format = &project.timeline.settings;
        let rate = format.rate;
        let format = if s.workspace == Workspace::Audio {
            format!("{} kHz", format.sample_rate as f32 / 1000.)
        } else {
            format!("{}×{} · {} fps", format.width, format.height, fps(rate))
        };
        let rendering = s
            .render_queue
            .jobs()
            .into_iter()
            .find_map(|job| match job.state {
                RenderState::Running { fraction } => Some((job.output, fraction)),
                _ => None,
            });
        let cache = cache_fraction(s);
        let pending = if self.agent_open {
            pending_reviews(s)
        } else {
            0
        };
        let selected = project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| s.selection.clips.contains(&c.id))
            .map(|c| c.range())
            .reduce(|a, b| TimeRange::new(a.start.min(b.start), a.end.max(b.end)));
        let place = match selected {
            Some(range) => (
                match s.selection.clips.len() {
                    1 => "Clip".to_owned(),
                    n => format!("{n} clips"),
                },
                format!(
                    "{} → {}",
                    range.start.timecode(rate),
                    range.end.timecode(rate)
                ),
            ),
            None => (
                "Timeline".to_owned(),
                project.timeline.duration().timecode(rate),
            ),
        };
        let status = s.status.clone();
        if rendering.is_some() {
            self.poll_render(cx);
        }

        let mono = |text: String| div().font_family(t::mono()).text_color(t::T2).child(text);
        let hint = |keys: &[&str], what: &'static str| {
            h_flex()
                .gap(px(3.))
                .children(keys.iter().map(|k| Kbd::new(k.to_string())))
                .child(div().ml(px(1.)).child(what))
        };
        h_flex()
            .flex_none()
            .gap(px(16.))
            .h(t::STATUS_BAR)
            .px(px(12.))
            .border_t_1()
            .border_color(t::LN)
            .bg(t::P)
            .text_size(px(11.))
            .text_color(t::T3)
            .whitespace_nowrap()
            .children(status.map(|s| div().min_w_0().truncate().text_color(t::T2).child(s)))
            .children(rendering.map(|(output, fraction)| {
                let name = output
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                h_flex()
                    .gap(px(6.))
                    .child(format!("Rendering {name}"))
                    .child(Progress::new().value(fraction).w(px(60.)).h(px(3.)))
                    .child(mono(format!("{:.0} %", fraction * 100.)))
            }))
            .child(format)
            .children(cache.map(|cached| {
                h_flex()
                    .gap(px(6.))
                    .child("Cache")
                    .child(
                        div()
                            .w(px(60.))
                            .h(px(3.))
                            .rounded_full()
                            .bg(t::R1)
                            .child(div().h_full().w(relative(cached)).rounded_full().bg(t::OK)),
                    )
                    .child(mono(format!("{:.0} %", cached * 100.)))
            }))
            .child(div().flex_1())
            .when(pending > 0, |bar| {
                bar.child(
                    h_flex()
                        .gap(px(6.))
                        .child(div().size(px(6.)).rounded_full().bg(t::AC))
                        .child(match pending {
                            1 => "Agent · 1 change set to review".to_owned(),
                            n => format!("Agent · {n} change sets to review"),
                        }),
                )
            })
            .child(h_flex().gap(px(4.)).child(place.0).child(mono(place.1)))
            .child(hint(&["J", "K", "L"], "shuttle"))
    }

    /// Redraw the status bar until the running render finishes.
    fn poll_render(&mut self, cx: &mut Context<Self>) {
        if self.render_poll {
            return;
        }
        self.render_poll = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(RENDER_POLL).await;
                let busy = this.update(cx, |this, cx| {
                    let busy = this.state.read(cx).render_queue.is_busy();
                    this.render_poll = busy;
                    cx.notify();
                    busy
                });
                if !matches!(busy, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }
}

/// Change sets the agent closed for review.
/// The status bar under the welcome page.
fn welcome_status() -> impl IntoElement {
    h_flex()
        .flex_none()
        .h(t::STATUS_BAR)
        .px(px(12.))
        .gap(px(6.))
        .border_t_1()
        .border_color(t::LN)
        .bg(t::P)
        .text_size(px(11.))
        .text_color(t::T3)
        .child("No project open")
        .child(div().flex_1())
        .child(Kbd::new(format!("{MOD} ,")))
        .child("settings")
}

fn pending_reviews(s: &EditorState) -> usize {
    s.bus
        .change_sets()
        .iter()
        .filter(|c| c.state == ChangeSetState::Pending)
        .count()
}

/// How much of what the background render covers is on disk, `None` while
/// nothing needs it.
fn cache_fraction(s: &EditorState) -> Option<f32> {
    let status = s.render.status()?;
    let (mut total, mut cached) = (0, 0);
    for (range, on_disk) in &status.runs {
        let length = range.duration().as_millis();
        total += length;
        if *on_disk {
            cached += length;
        }
    }
    (total > 0).then(|| cached as f32 / total as f32)
}

/// "25", "29.97".
fn fps(rate: Rate) -> String {
    if rate.den == 1 {
        return rate.num.to_string();
    }
    let text = format!("{:.3}", rate.fps());
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

impl Render for Flikk {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let edited = self.state.read(cx).is_dirty();
        if edited != self.edited {
            self.edited = edited;
            window.set_window_edited(edited);
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t::G)
            .text_color(t::T1)
            .text_size(px(12.))
            .on_action(
                cx.listener(|this, _: &Undo, _, cx| this.state.update(cx, |s, cx| s.undo(cx))),
            )
            .on_action(
                cx.listener(|this, _: &Redo, _, cx| this.state.update(cx, |s, cx| s.redo(cx))),
            )
            .on_action(cx.listener(|this, _: &ToggleAgent, _, cx| this.toggle_agent(cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                this.open_settings(None, window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenAgentSettings, window, cx| {
                this.open_settings(Some(SettingsTab::Agent), window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewProject, window, cx| {
                projects::new_project(&this.state, window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenProject, window, cx| {
                projects::open_dialog(&this.state, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SaveProject, window, cx| {
                projects::save(&this.state, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SaveProjectAs, window, cx| {
                projects::save_as(&this.state, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ProjectSettings, window, cx| {
                if this.welcome.is_none() {
                    projects::settings(&this.state, window, cx)
                }
            }))
            .on_action(
                cx.listener(|this, _: &Quit, window, cx| projects::quit(&this.state, window, cx)),
            )
            .child(self.title_bar(window, cx))
            .map(
                |root| match self.welcome.as_ref().map(|(page, _)| page.clone()) {
                    Some(page) => root
                        .child(div().flex_1().min_h_0().child(page))
                        .child(welcome_status()),
                    None => root
                        .child(div().flex_1().min_h_0().child(self.workspace(cx)))
                        .child(self.status_bar(cx)),
                },
            )
    }
}
