//! The editor's Settings dialog, opened from the top bar (or cmd-,) and
//! from the chat panel's gear: its pages down the left, the open page on
//! the right, and a footer to open the user-data folder or close.

mod entries;
mod integrations;
mod on_device;
mod storage;

use crate::state::{Check, EditorState};
use crate::ui::format::size as size_label;
use crate::ui::kit::{Button, DialogExt as _, Icon, IconName, h_flex, theme as t, v_flex};
use crate::ui::settings::SettingsPanel;
use entries::EntriesTab;
pub use entries::Focus;
use flikk_core::paths;
use flikk_core::skills::Kind;
use flikk_models::Status;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use integrations::IntegrationsTab;
pub(crate) use on_device::transcription_sizes;
use storage::StorageTab;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Agent,
    Skills,
    Styles,
    Integrations,
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
        SettingsTab::Skills,
        SettingsTab::Styles,
        SettingsTab::Integrations,
        SettingsTab::Storage,
    ];

    fn label(self) -> &'static str {
        match self {
            SettingsTab::Agent => "Agent",
            SettingsTab::Skills => "Skills",
            SettingsTab::Styles => "Styles",
            SettingsTab::Integrations => "Integrations",
            SettingsTab::Storage => "Storage",
        }
    }

    fn icon(self) -> IconName {
        match self {
            SettingsTab::Agent => IconName::MessageSquareText,
            SettingsTab::Skills => IconName::SquareSlash,
            SettingsTab::Styles => IconName::Palette,
            SettingsTab::Integrations => IconName::Plug,
            SettingsTab::Storage => IconName::HardDrive,
        }
    }
}

pub struct EditorSettings {
    state: Entity<EditorState>,
    tab: SettingsTab,
    agent: Entity<SettingsPanel>,
    skills: Entity<EntriesTab>,
    styles: Entity<EntriesTab>,
    integrations: Entity<IntegrationsTab>,
    storage: Entity<StorageTab>,
    _observe: Vec<Subscription>,
}

impl EditorSettings {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let integrations = cx.new(|cx| IntegrationsTab::new(state.clone(), window, cx));
        let storage = cx.new(StorageTab::new);
        let skills = cx.new(|cx| EntriesTab::new(state.clone(), Kind::Skill, window, cx));
        let styles = cx.new(|cx| EntriesTab::new(state.clone(), Kind::Style, window, cx));
        Self {
            // The sidebar follows the downloads and the sizes measured.
            _observe: vec![
                cx.observe(&state, |_, _, cx| cx.notify()),
                cx.observe(&integrations, |_, _, cx| cx.notify()),
                cx.observe(&storage, |_, _, cx| cx.notify()),
                cx.observe(&skills, |_, _, cx| cx.notify()),
                cx.observe(&styles, |_, _, cx| cx.notify()),
            ],
            tab: SettingsTab::Agent,
            agent: cx.new(|cx| SettingsPanel::new(state.clone(), window, cx)),
            skills,
            styles,
            integrations,
            storage,
            state,
        }
    }

    /// Open the dialog on the Skills or Styles page, at `focus`, wherever
    /// it is asked from.
    pub fn open_entries(kind: Kind, focus: Focus, window: &mut Window, cx: &mut App) {
        let Some(this) = cx.try_global::<Handle>().map(|h| h.0.clone()) else {
            return;
        };
        let tab = match kind {
            Kind::Skill => SettingsTab::Skills,
            Kind::Style => SettingsTab::Styles,
        };
        if !window.has_active_dialog(cx) {
            Self::open(&this, Some(tab), window, cx);
        }
        this.update(cx, |this, cx| {
            this.tab = tab;
            this.entries(kind)
                .update(cx, |e, cx| e.show(focus, window, cx));
            cx.notify();
        });
    }

    /// Makes [`Self::open_entries`] reach this dialog.
    pub fn register(this: &Entity<Self>, cx: &mut App) {
        cx.set_global(Handle(this.clone()));
    }

    fn entries(&self, kind: Kind) -> &Entity<EntriesTab> {
        match kind {
            Kind::Skill => &self.skills,
            Kind::Style => &self.styles,
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
            match this.tab {
                SettingsTab::Integrations => {
                    this.integrations.update(cx, |i, cx| i.reopen(window, cx))
                }
                SettingsTab::Skills | SettingsTab::Styles => {
                    let kind = this.tab_kind().unwrap_or(Kind::Skill);
                    this.entries(kind)
                        .update(cx, |e, cx| e.show(Focus::Last, window, cx))
                }
                _ => {}
            }
            cx.notify();
        });
        let view = this.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .close_button(false)
                .w(px(900.))
                .h(px(620.))
                .p_0()
                .gap_0()
                .child(view.clone())
        });
    }

    fn show(&mut self, tab: SettingsTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        match tab {
            SettingsTab::Storage => self.storage.update(cx, |s, cx| s.measure(cx)),
            SettingsTab::Integrations => self.integrations.update(cx, |i, cx| i.reopen(window, cx)),
            SettingsTab::Skills | SettingsTab::Styles => {
                let kind = self.tab_kind().unwrap_or(Kind::Skill);
                self.entries(kind)
                    .update(cx, |e, cx| e.show(Focus::Last, window, cx))
            }
            SettingsTab::Agent => {}
        }
        cx.notify();
    }

    fn tab_kind(&self) -> Option<Kind> {
        match self.tab {
            SettingsTab::Skills => Some(Kind::Skill),
            SettingsTab::Styles => Some(Kind::Style),
            _ => None,
        }
    }

    /// What a page's sidebar row shows after its label: the downloads'
    /// progress and a provider whose key failed, the space used.
    fn trailing(&self, tab: SettingsTab, cx: &App) -> Option<AnyElement> {
        let s = self.state.read(cx);
        match tab {
            SettingsTab::Integrations => {
                let failed = flikk_cloud::PROVIDERS
                    .iter()
                    .any(|&p| matches!(s.provider_status(p).check, Check::Failed { .. }));
                let fractions: Vec<f32> = s
                    .model_tasks
                    .iter()
                    .filter_map(|task| match s.models.status(task.model()) {
                        Status::Downloading(f) => Some(f),
                        _ => None,
                    })
                    .collect();
                let progress = (!fractions.is_empty()).then(|| {
                    let fraction = fractions.iter().sum::<f32>() / fractions.len() as f32;
                    meta(format!("{:.0} %", fraction * 100.)).text_color(t::AC)
                });
                (failed || progress.is_some()).then(|| {
                    h_flex()
                        .gap(px(6.))
                        .children(progress)
                        .when(failed, |el| {
                            el.child(div().size(px(6.)).rounded_full().bg(t::WARN))
                        })
                        .into_any_element()
                })
            }
            SettingsTab::Storage => self
                .storage
                .read(cx)
                .total()
                .map(|bytes| meta(size_label(bytes)).into_any_element()),
            SettingsTab::Skills | SettingsTab::Styles => {
                let kind = if tab == SettingsTab::Skills {
                    Kind::Skill
                } else {
                    Kind::Style
                };
                let count = self.entries(kind).read(cx).count();
                (count > 0).then(|| meta(count.to_string()).into_any_element())
            }
            SettingsTab::Agent => None,
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
                    .on_click(cx.listener(move |this, _, window, cx| this.show(tab, window, cx)))
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
            SettingsTab::Skills => self.skills.clone().into(),
            SettingsTab::Styles => self.styles.clone().into(),
            SettingsTab::Integrations => self.integrations.clone().into(),
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
                    .kbd("Esc")
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

/// The app's Settings dialog, for [`EditorSettings::open_entries`].
struct Handle(Entity<EditorSettings>);

impl Global for Handle {}

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

/// A heading over one of several sections of a page (Integrations' "Use
/// for", "On this Mac"): a step up from [`section_label`], so the
/// sections read apart.
pub(crate) fn group_label(title: &'static str) -> Div {
    section_label(title)
        .h(px(26.))
        .mb(px(4.))
        .text_size(px(12.))
        .text_color(t::T2)
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
