//! Window layout: workspace switcher on top, the workspace's panels on the
//! left, the agent chat docked on the right in every workspace (PRD AG1).

use crate::state::EditorState;
use crate::ui::{
    chat::ChatPanel, code_editor::CodeEditorPanel, deliver::DeliverPanel,
    inspector::InspectorPanel, library::LibraryPanel, media_pool::MediaPoolPanel,
    timeline::TimelinePanel, transcript::TranscriptPanel, viewer::ViewerPanel,
};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::*;
use koo_core::agent::Workspace;

actions!(koo, [Undo, Redo, TogglePlay, Quit]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-z", Undo, None),
        KeyBinding::new("ctrl-z", Undo, None),
        KeyBinding::new("cmd-shift-z", Redo, None),
        KeyBinding::new("ctrl-shift-z", Redo, None),
        KeyBinding::new("ctrl-y", Redo, None),
        KeyBinding::new("space", TogglePlay, Some("Timeline")),
        KeyBinding::new("space", TogglePlay, Some("Viewer")),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
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
    deliver: Entity<DeliverPanel>,
    chat: Entity<ChatPanel>,
    library: Entity<LibraryPanel>,
    code_editor: Entity<CodeEditorPanel>,
    left_tab: LeftTab,
    _observe: Subscription,
}

impl KooEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(EditorState::new);
        if crate::demo::enabled() {
            state.update(cx, crate::demo::seed);
        }
        let _observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            timeline: cx.new(|cx| TimelinePanel::new(state.clone(), window, cx)),
            viewer: cx.new(|cx| ViewerPanel::new(state.clone(), window, cx)),
            media_pool: cx.new(|cx| MediaPoolPanel::new(state.clone(), window, cx)),
            inspector: cx.new(|cx| InspectorPanel::new(state.clone(), window, cx)),
            transcript: cx.new(|cx| TranscriptPanel::new(state.clone(), window, cx)),
            deliver: cx.new(|cx| DeliverPanel::new(state.clone(), window, cx)),
            chat: cx.new(|cx| ChatPanel::new(state.clone(), window, cx)),
            library: cx.new(|cx| LibraryPanel::new(state.clone(), window, cx)),
            code_editor: cx.new(|cx| CodeEditorPanel::new(state.clone(), window, cx)),
            left_tab: match crate::demo::fx_view().as_deref() {
                Some("library" | "code") => LeftTab::Effects,
                _ => LeftTab::Media,
            },
            state,
            _observe,
        }
    }

    /// Project name; opens recent projects and "New project".
    fn project_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        let name = state.read(cx).bus.project().name.clone();
        Button::new("project")
            .ghost()
            .small()
            .label(name)
            .dropdown_menu(move |menu, _, cx| {
                let current = state.read(cx).bus.project().id;
                let new = state.clone();
                let mut menu =
                    menu.item(PopupMenuItem::new("New project").on_click(move |_, _, cx| {
                        new.update(cx, |s, cx| s.new_project(cx));
                    }));
                let projects = state.read(cx).projects();
                if !projects.is_empty() {
                    menu = menu.item(PopupMenuItem::separator());
                }
                for p in projects {
                    let state = state.clone();
                    menu = menu.item(
                        PopupMenuItem::new(p.name)
                            .checked(p.id == current)
                            .on_click(move |_, _, cx| {
                                state.update(cx, |s, cx| s.open_project(p.id, cx))
                            }),
                    );
                }
                menu
            })
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
                                .size(px(380.))
                                .child(self.transcript.clone()),
                        )
                        .child(resizable_panel().child(self.viewer.clone())),
                )
                .into_any_element(),
            Workspace::Color | Workspace::Audio => self
                .over_timeline(
                    "grade",
                    h_resizable("grade-top")
                        .child(resizable_panel().child(self.viewer.clone()))
                        .child(
                            resizable_panel()
                                .size(px(320.))
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(
                // Project menu on the left; workspaces stay centred.
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
                    .child(self.workspace_tabs(cx)),
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
