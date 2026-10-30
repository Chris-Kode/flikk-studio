//! Window layout: workspace switcher on top, between the project menu and
//! the notice and Settings buttons; the workspace's panels on the left, the
//! agent chat docked on the right in every workspace (PRD AG1).

use crate::notices::NoticeAction;
use crate::projects::{self, NewProject, OpenProject, SaveProject, SaveProjectAs};
use crate::state::EditorState;
use crate::ui::caption_style::{CaptionStylePanel, Target};
use crate::ui::editor_settings::{EditorSettings, SettingsTab};
use crate::ui::{
    chat::ChatPanel, code_editor::CodeEditorPanel, deliver::DeliverPanel,
    inspector::InspectorPanel, library::LibraryPanel, media_pool::MediaPoolPanel,
    mixer::MixerPanel, scopes::ScopesPanel, timeline::TimelinePanel, transcript::TranscriptPanel,
    viewer::ViewerPanel,
};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, IconName};
use gpui_kit::*;
use koo_core::agent::Workspace;

actions!(koo, [Undo, Redo, TogglePlay, OpenSettings, Quit]);

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

/// What the Edit workspace's left panel shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LeftTab {
    Media,
    Effects,
}

pub struct KooEditor {
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
    /// The notice list is dropped down.
    notices_open: bool,
    /// Unsaved changes as last told to the window.
    edited: bool,
    _observe: Subscription,
}

impl KooEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(EditorState::new);
        if crate::demo::enabled() {
            state.update(cx, crate::demo::seed);
        }
        let _observe = cx.observe(&state, |_, _, cx| cx.notify());
        let closing = state.clone();
        window.on_window_should_close(cx, move |window, cx| {
            projects::should_close(&closing, window, cx)
        });
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
            notices_open: false,
            edited: false,
            left_tab: match crate::demo::fx_view().as_deref() {
                Some("library" | "code") => LeftTab::Effects,
                _ => LeftTab::Media,
            },
            state,
            _observe,
        }
    }

    /// Project name, with a dot while there are unsaved changes; opens
    /// the project file commands and recent projects.
    fn project_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        let s = state.read(cx);
        let name = s.bus.project().name.clone();
        let label = if s.is_dirty() {
            format!("{name} •")
        } else {
            name
        };
        Button::new("project")
            .ghost()
            .small()
            .label(label)
            .dropdown_menu(move |menu, _, cx| {
                let item =
                    |label: &'static str,
                     action: Box<dyn Action>,
                     f: fn(&Entity<EditorState>, &mut Window, &mut App)| {
                        let state = state.clone();
                        PopupMenuItem::new(label)
                            .action(action)
                            .on_click(move |_, window, cx| f(&state, window, cx))
                    };
                let mut menu = menu
                    .item(item(
                        "New Project",
                        Box::new(NewProject),
                        projects::new_project,
                    ))
                    .item(item("Open…", Box::new(OpenProject), projects::open_dialog))
                    .item(item("Save", Box::new(SaveProject), projects::save))
                    .item(item("Save As…", Box::new(SaveProjectAs), projects::save_as));
                let (current, recent) = {
                    let s = state.read(cx);
                    (s.file.clone(), s.recent_projects())
                };
                if !recent.is_empty() {
                    menu = menu.item(PopupMenuItem::separator());
                }
                for path in recent {
                    let state = state.clone();
                    let label = koo_store::project_file::name_of(&path);
                    let checked = current.as_ref() == Some(&path);
                    menu = menu.item(PopupMenuItem::new(label).checked(checked).on_click(
                        move |_, window, cx| projects::open(&state, path.clone(), window, cx),
                    ));
                }
                menu
            })
    }

    fn open_settings(&mut self, tab: SettingsTab, window: &mut Window, cx: &mut Context<Self>) {
        self.notices_open = false;
        cx.notify();
        EditorSettings::open(&self.settings, tab, window, cx);
    }

    /// The notice button, while there are notices, and Settings.
    fn top_right(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.state.read(cx).notices.is_empty() {
            self.notices_open = false;
        }
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
                    NoticeAction::OpenModels => this.open_settings(SettingsTab::Models, window, cx),
                    NoticeAction::Relink => crate::relink::relink_folder(&this.state, cx),
                });
            },
            cx,
        );
        div()
            .flex()
            .items_center()
            .gap_0p5()
            .children(notices)
            .child(
                Button::new("settings")
                    .ghost()
                    .small()
                    .icon(IconName::Settings)
                    .tooltip("Settings")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_settings(SettingsTab::Models, window, cx)
                    })),
            )
    }

    fn workspace_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.state.read(cx).workspace;
        let state = self.state.clone();
        TabBar::new("workspaces")
            .segmented()
            .selected_index(
                Workspace::ALL
                    .iter()
                    .position(|w| *w == current)
                    .unwrap_or(0),
            )
            .children(Workspace::ALL.iter().map(|w| Tab::new().label(w.label())))
            .on_click(move |ix, _, cx| {
                state.update(cx, |s, cx| s.set_workspace(Workspace::ALL[*ix], cx))
            })
    }

    /// Viewer area on top, timeline below: the frame every editing workspace shares.
    fn over_timeline(&self, id: &'static str, top: impl IntoElement) -> impl IntoElement {
        v_resizable(id).child(resizable_panel().child(top)).child(
            resizable_panel()
                .size(px(300.))
                .child(self.timeline.clone()),
        )
    }

    /// Edit's left panel: media pool or effects library under segmented tabs.
    fn edit_left(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.left_tab;
        let border = cx.theme().border;
        let body: AnyElement = match tab {
            LeftTab::Media => self.media_pool.clone().into_any_element(),
            LeftTab::Effects => self.library.clone().into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div().p_1().border_b_1().border_color(border).child(
                    TabBar::new("edit-left")
                        .segmented()
                        .xsmall()
                        .w_full()
                        .selected_index(match tab {
                            LeftTab::Media => 0,
                            LeftTab::Effects => 1,
                        })
                        .child(Tab::new().label("Media").flex_1())
                        .child(Tab::new().label("Effects").flex_1())
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

    fn workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.state.read(cx).workspace {
            Workspace::Edit => {
                // The code editor takes the inspector's place while open.
                let coding = self.state.read(cx).editing_fx.is_some();
                let (id, right, width): (&'static str, AnyElement, f32) = if coding {
                    (
                        "edit-top-code",
                        self.code_editor.clone().into_any_element(),
                        460.,
                    )
                } else {
                    ("edit-top", self.inspector.clone().into_any_element(), 300.)
                };
                self.over_timeline(
                    "edit",
                    h_resizable(id)
                        .child(resizable_panel().size(px(260.)).child(self.edit_left(cx)))
                        .child(resizable_panel().child(self.viewer.clone()))
                        .child(resizable_panel().size(px(width)).child(right)),
                )
                .into_any_element()
            }
            Workspace::Captions => self
                .over_timeline(
                    "captions",
                    h_resizable("captions-top")
                        .child(
                            resizable_panel()
                                .size(px(340.))
                                .child(self.transcript.clone()),
                        )
                        .child(resizable_panel().child(self.viewer.clone()))
                        .child(
                            resizable_panel().size(px(300.)).child(
                                div()
                                    .id("caption-style")
                                    .size_full()
                                    .overflow_y_scrollbar()
                                    .child(div().p_3().child(self.caption_style.clone())),
                            ),
                        ),
                )
                .into_any_element(),
            Workspace::Color => self
                .over_timeline(
                    "color",
                    h_resizable("color-top")
                        .child(resizable_panel().child(self.viewer.clone()))
                        .child(resizable_panel().size(px(340.)).child(self.scopes.clone()))
                        .child(
                            resizable_panel()
                                .size(px(320.))
                                .child(self.inspector.clone()),
                        ),
                )
                .into_any_element(),
            Workspace::Audio => self
                .over_timeline(
                    "audio",
                    h_resizable("audio-top")
                        .child(resizable_panel().child(self.viewer.clone()))
                        .child(resizable_panel().size(px(420.)).child(self.mixer.clone()))
                        .child(
                            resizable_panel()
                                .size(px(300.))
                                .child(self.inspector.clone()),
                        ),
                )
                .into_any_element(),
            Workspace::Deliver => h_resizable("deliver")
                .child(resizable_panel().size(px(380.)).child(self.deliver.clone()))
                .child(resizable_panel().child(self.viewer.clone()))
                .into_any_element(),
        }
    }
}

impl Render for KooEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let edited = self.state.read(cx).is_dirty();
        if edited != self.edited {
            self.edited = edited;
            window.set_window_edited(edited);
        }
        let theme = cx.theme();
        let (bg, fg, border) = (theme.background, theme.foreground, theme.border);
        let status = self.state.read(cx).status.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .on_action(
                cx.listener(|this, _: &Undo, _, cx| this.state.update(cx, |s, cx| s.undo(cx))),
            )
            .on_action(
                cx.listener(|this, _: &Redo, _, cx| this.state.update(cx, |s, cx| s.redo(cx))),
            )
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                this.open_settings(SettingsTab::Models, window, cx)
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
            .on_action(
                cx.listener(|this, _: &Quit, window, cx| projects::quit(&this.state, window, cx)),
            )
            .child(
                // Project menu on the left, notices and Settings on the
                // right; workspaces stay centred.
                div()
                    .relative()
                    .flex()
                    .justify_center()
                    .py_1()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .absolute()
                            .left_2()
                            .top_0()
                            .bottom_0()
                            .flex()
                            .items_center()
                            .child(self.project_menu(cx)),
                    )
                    .child(self.workspace_tabs(cx))
                    .child(
                        div()
                            .absolute()
                            .right_2()
                            .top_0()
                            .bottom_0()
                            .flex()
                            .items_center()
                            .child(self.top_right(cx)),
                    ),
            )
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("root")
                        .child(resizable_panel().child(self.workspace(cx)))
                        .child(resizable_panel().size(px(360.)).child(self.chat.clone())),
                ),
            )
            .children(status.map(|s| {
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .border_t_1()
                    .border_color(border)
                    .child(s)
            }))
    }
}
