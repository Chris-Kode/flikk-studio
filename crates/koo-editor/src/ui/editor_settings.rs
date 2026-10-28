//! The editor's Settings dialog, opened from the top bar (or cmd-,): its
//! tabs down the left, the open tab's page on the right. Agent settings
//! live in the chat panel (`settings.rs`).

mod models;

use crate::state::EditorState;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use models::ModelsTab;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Models,
}

impl SettingsTab {
    const ALL: [SettingsTab; 1] = [SettingsTab::Models];

    fn label(self) -> &'static str {
        match self {
            SettingsTab::Models => "Models",
        }
    }
}

pub struct EditorSettings {
    tab: SettingsTab,
    models: Entity<ModelsTab>,
}

impl EditorSettings {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            tab: SettingsTab::Models,
            models: cx.new(|cx| ModelsTab::new(state, window, cx)),
        }
    }

    /// Open the dialog on `tab`, unless a dialog is already open.
    pub fn open(this: &Entity<Self>, tab: SettingsTab, window: &mut Window, cx: &mut App) {
        if window.has_active_dialog(cx) {
            return;
        }
        this.update(cx, |this, cx| {
            this.tab = tab;
            cx.notify();
        });
        let view = this.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.title("Settings").w(px(780.)).child(view.clone())
        });
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (selected, hover) = (theme.accent, theme.secondary_hover);
        v_flex().w(px(150.)).flex_none().gap_0p5().children(
            SettingsTab::ALL.iter().enumerate().map(|(ix, &tab)| {
                h_flex()
                    .id(("settings-tab", ix))
                    .h(px(30.))
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .when(tab == self.tab, |el| el.bg(selected))
                    .hover(|el| el.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = tab;
                        cx.notify();
                    }))
                    .child(tab.label())
            }),
        )
    }
}

impl Render for EditorSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = match self.tab {
            SettingsTab::Models => self.models.clone(),
        };
        h_flex()
            .h(px(440.))
            .gap_4()
            .items_start()
            .child(self.render_tabs(cx))
            .child(div().flex_1().min_w_0().h_full().child(page))
    }
}
