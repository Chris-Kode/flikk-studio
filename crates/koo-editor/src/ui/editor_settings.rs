//! The editor's Settings dialog, opened from the top bar (or cmd-,) and
//! from the chat panel's gear: its pages down the left, the open page on
//! the right, and a footer to open the user-data folder or close.

mod analysis;
mod integrations;
mod models;
mod storage;

use crate::state::{Check, EditorState};
use crate::ui::kit::{Button, DialogExt as _, Icon, IconName, Kbd, h_flex, theme as t, v_flex};
use crate::ui::settings::SettingsPanel;
use analysis::AnalysisTab;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use integrations::IntegrationsTab;
use koo_core::paths;
use koo_models::Status;
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

    fn icon(self) -> IconName {
        match self {
            SettingsTab::Agent => IconName::MessageSquareText,
            SettingsTab::Integrations => IconName::Plug,
            SettingsTab::Analysis => IconName::ScanSearch,
            SettingsTab::Models => IconName::Box,
            SettingsTab::Storage => IconName::HardDrive,
        }
    }
}

pub struct EditorSettings {
    state: Entity<EditorState>,
    tab: SettingsTab,
    agent: Entity<SettingsPanel>,
    integrations: Entity<IntegrationsTab>,
    analysis: Entity<AnalysisTab>,
    models: Entity<ModelsTab>,
    storage: Entity<StorageTab>,
    _observe: Vec<Subscription>,
}

impl EditorSettings {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let models = cx.new(|cx| ModelsTab::new(state.clone(), window, cx));
        let storage = cx.new(StorageTab::new);
        Self {
            // The sidebar follows the downloads and the sizes measured.
            _observe: vec![
                cx.observe(&state, |_, _, cx| cx.notify()),
                cx.observe(&models, |_, _, cx| cx.notify()),
                cx.observe(&storage, |_, _, cx| cx.notify()),
            ],
            tab: SettingsTab::Agent,
            agent: cx.new(|cx| SettingsPanel::new(state.clone(), window, cx)),
            integrations: cx.new(|cx| IntegrationsTab::new(state.clone(), window, cx)),
            analysis: cx.new(|cx| AnalysisTab::new(state.clone(), cx)),
            models,
            storage,
            state,
        }
    }

    /// Open the dialog on `tab`, or the page shown last, unless a dialog
    /// is already open.
    pub fn open(this: &Entity<Self>, tab: Option<SettingsTab>, window: &mut Window, cx: &mut App) {
        if window.has_active_dialog(cx) {
            return;
        }
        // Measured now for the sidebar's total, whichever page opens.
        this.update(cx, |this, cx| {
            this.tab = tab.unwrap_or(this.tab);
            this.storage.update(cx, |s, cx| s.measure(cx));
            cx.notify();
        });
        let view = this.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .close_button(false)
                .w(px(860.))
                .h(px(620.))
                .p_0()
                .gap_0()
                .child(view.clone())
        });
    }

    fn show(&mut self, tab: SettingsTab, cx: &mut Context<Self>) {
        self.tab = tab;
        if tab == SettingsTab::Storage {
            self.storage.update(cx, |s, cx| s.measure(cx));
        }
        cx.notify();
    }

    /// What a page's sidebar row shows after its label: a provider whose
    /// key failed, the downloads' progress, the space used.
    fn trailing(&self, tab: SettingsTab, cx: &App) -> Option<AnyElement> {
        let s = self.state.read(cx);
        match tab {
            SettingsTab::Integrations => koo_cloud::PROVIDERS
                .iter()
                .any(|&p| matches!(s.provider_status(p).check, Check::Failed(_)))
                .then(|| {
                    div()
                        .size(px(6.))
                        .rounded_full()
                        .bg(t::WARN)
                        .into_any_element()
                }),
            SettingsTab::Models => {
                let fractions: Vec<f32> = s
                    .model_tasks
                    .iter()
                    .filter_map(|task| match s.models.status(task.model()) {
                        Status::Downloading(f) => Some(f),
                        _ => None,
                    })
                    .collect();
                (!fractions.is_empty()).then(|| {
                    let fraction = fractions.iter().sum::<f32>() / fractions.len() as f32;
                    meta(format!("{:.0} %", fraction * 100.))
                        .text_color(t::AC)
                        .into_any_element()
                })
            }
            SettingsTab::Storage => self
                .storage
                .read(cx)
                .total()
                .map(|bytes| meta(size_label(bytes)).into_any_element()),
            _ => None,
        }
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w(px(190.))
            .flex_none()
            .h_full()
            .gap(px(2.))
            .px(px(8.))
            .py(px(10.))
            .border_r_1()
            .border_color(t::LN)
            .bg(t::INSET)
            .children(SettingsTab::ALL.iter().enumerate().map(|(ix, &tab)| {
                let on = tab == self.tab;
                h_flex()
                    .id(("settings-tab", ix))
                    .h(px(30.))
                    .gap(px(8.))
                    .px(px(8.))
                    .rounded(t::RADIUS)
                    .cursor_pointer()
                    .text_color(if on { t::T1 } else { t::T2 })
                    .when(on, |el| el.bg(t::HV))
                    .when(!on, |el| el.hover(|s| s.bg(t::HV).text_color(t::T1)))
                    .on_click(cx.listener(move |this, _, _, cx| this.show(tab, cx)))
                    .child(Icon::new(tab.icon()).size(px(14.)).color(if on {
                        t::AC
                    } else {
                        t::T2
                    }))
                    .child(div().flex_1().child(tab.label()))
                    .children(self.trailing(tab, cx))
            }))
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
        let header = h_flex()
            .flex_none()
            .h(px(44.))
            .gap(px(8.))
            .pl(px(16.))
            .pr(px(10.))
            .border_b_1()
            .border_color(t::LN)
            .child(
                div()
                    .flex_1()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Settings"),
            )
            .child(div().text_color(t::T3).child(Kbd::new("Esc")))
            .child(
                Button::new("settings-close")
                    .ghost()
                    .small()
                    .icon(IconName::X)
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            );
        let footer = h_flex()
            .flex_none()
            .h(px(52.))
            .gap(px(8.))
            .pl(px(16.))
            .pr(px(14.))
            .border_t_1()
            .border_color(t::LN)
            .bg(t::INSET)
            .child(
                Button::new("settings-folder")
                    .ghost()
                    .icon(IconName::FolderOpen)
                    .label("Open settings folder")
                    .on_click(|_, _, cx| cx.open_with_system(&paths::home())),
            )
            .child(div().flex_1())
            .child(
                Button::new("settings-done")
                    .primary()
                    .label("Done")
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            );
        v_flex()
            .size_full()
            .text_size(px(12.))
            .text_color(t::T1)
            .child(header)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(self.render_nav(cx))
                    .child(div().flex_1().min_w_0().h_full().child(page)),
            )
            .child(footer)
    }
}

/// A page: its title (with `trailing` beside it) over its sections,
/// scrolling.
pub(crate) fn page(title: &'static str, trailing: Option<AnyElement>) -> Div {
    v_flex()
        .size_full()
        .px(px(24.))
        .pt(px(18.))
        .pb(px(24.))
        .child(
            h_flex()
                .flex_none()
                .h(px(26.))
                .mb(px(10.))
                .justify_between()
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .children(trailing),
        )
}

/// A group's heading over its rows; its children after the title sit at
/// the right.
pub(crate) fn section_label(title: &'static str) -> Div {
    h_flex()
        .flex_none()
        .justify_between()
        .min_h(px(20.))
        .mb(px(6.))
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(t::T3)
        .child(title)
}

/// One row of a page: its label in a column `label_w` wide on the left,
/// its controls after it.
pub(crate) fn row(label: impl Into<SharedString>, label_w: f32) -> Div {
    h_flex()
        .flex_none()
        .h(px(40.))
        .gap(px(12.))
        .border_b_1()
        .border_color(t::LN)
        .child(div().w(px(label_w)).flex_none().child(label.into()))
}

/// Small mono meta text: sizes, percentages, paths.
fn meta(text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .font_family(t::mono())
        .text_size(px(10.))
        .text_color(t::T3)
        .child(text.into())
}

/// "885 KB", "148 MB", "1.1 GB", as the files' hosts count them.
fn size_label(bytes: u64) -> String {
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
