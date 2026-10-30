//! The editor's Settings dialog, opened from the top bar (or cmd-,) and
//! from the chat panel's gear: its pages down the left, the open page on
//! the right.

mod analysis;
mod integrations;
mod models;
mod storage;

use crate::state::EditorState;
use crate::ui::settings::SettingsPanel;
use analysis::AnalysisTab;
use gpui_kit::component::{ActiveTheme as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use integrations::IntegrationsTab;
use models::ModelsTab;
use storage::StorageTab;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Agent,
    Integrations,
    Analysis,
    Models,
    Storage,
}

impl SettingsTab {
    /// The page labelled `label`, any case.
    pub fn named(label: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|t| t.label().eq_ignore_ascii_case(label))
    }

    const ALL: [SettingsTab; 5] = [
        SettingsTab::Agent,
        SettingsTab::Integrations,
        SettingsTab::Analysis,
        SettingsTab::Models,
        SettingsTab::Storage,
    ];

    fn label(self) -> &'static str {
        match self {
            SettingsTab::Agent => "Agent",
            SettingsTab::Integrations => "Integrations",
            SettingsTab::Analysis => "Analysis",
            SettingsTab::Models => "Models",
            SettingsTab::Storage => "Storage",
        }
    }
}

pub struct EditorSettings {
    tab: SettingsTab,
    agent: Entity<SettingsPanel>,
    integrations: Entity<IntegrationsTab>,
    analysis: Entity<AnalysisTab>,
    models: Entity<ModelsTab>,
    storage: Entity<StorageTab>,
}

impl EditorSettings {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            tab: SettingsTab::Agent,
            agent: cx.new(|cx| SettingsPanel::new(state.clone(), window, cx)),
            integrations: cx.new(|cx| IntegrationsTab::new(state.clone(), window, cx)),
            analysis: cx.new(|cx| AnalysisTab::new(state.clone(), cx)),
            models: cx.new(|cx| ModelsTab::new(state.clone(), window, cx)),
            storage: cx.new(StorageTab::new),
        }
    }

    /// Open the dialog on `tab`, or the page shown last, unless a dialog
    /// is already open.
    pub fn open(this: &Entity<Self>, tab: Option<SettingsTab>, window: &mut Window, cx: &mut App) {
        if window.has_active_dialog(cx) {
            return;
        }
        this.update(cx, |this, cx| this.show(tab.unwrap_or(this.tab), cx));
        let view = this.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.title("Settings").w(px(820.)).child(view.clone())
        });
    }

    fn show(&mut self, tab: SettingsTab, cx: &mut Context<Self>) {
        self.tab = tab;
        if tab == SettingsTab::Storage {
            self.storage.update(cx, |s, cx| s.measure(cx));
        }
        cx.notify();
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
                    .on_click(cx.listener(move |this, _, _, cx| this.show(tab, cx)))
                    .child(tab.label())
            }),
        )
    }
}

impl Render for EditorSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page: AnyView = match self.tab {
            SettingsTab::Agent => self.agent.clone().into(),
            SettingsTab::Integrations => self.integrations.clone().into(),
            SettingsTab::Analysis => self.analysis.clone().into(),
            SettingsTab::Models => self.models.clone().into(),
            SettingsTab::Storage => self.storage.clone().into(),
        };
        h_flex()
            .h(px(500.))
            .gap_4()
            .items_start()
            .child(self.render_tabs(cx))
            .child(div().flex_1().min_w_0().h_full().child(page))
    }
}

/// A page's heading over its rows.
fn section(title: &'static str, cx: &App) -> Div {
    v_flex().gap_1().child(
        div()
            .text_xs()
            .font_medium()
            .text_color(cx.theme().muted_foreground)
            .child(title),
    )
}

/// One row of a page: its label on the left, its controls on the right.
fn row(label: impl Into<SharedString>, cx: &App) -> Div {
    h_flex()
        .flex_none()
        .h(px(40.))
        .gap_3()
        .border_b_1()
        .border_color(cx.theme().border)
        .text_sm()
        .child(div().w(px(130.)).flex_none().child(label.into()))
}

/// "885 KB", "148 MB", "1.1 GB", as the files' hosts count them.
pub(crate) fn size_label(bytes: u64) -> String {
    const KB: f64 = 1e3;
    const MB: f64 = 1e6;
    const GB: f64 = 1e9;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.0} MB", b / MB)
    } else {
        format!("{:.0} KB", (b / KB).max(1.))
    }
}
