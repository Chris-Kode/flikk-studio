//! The title bar's bell and the notifications it drops down: the update,
//! warnings and errors in `EditorState::notices`, each but the update with
//! its own dismiss button.

use crate::notices::{Level, Notice, NoticeAction};
use crate::state::EditorState;
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, EmptyState, Icon, IconName, Indicator, Popover, Tone, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;

type OnAction = Rc<dyn Fn(NoticeAction, &mut Window, &mut App)>;

/// The bell, with a dot in the colour of the most serious notice;
/// `on_action` runs a notice's own button.
pub fn button(
    state: &Entity<EditorState>,
    open: bool,
    on_open_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
    on_action: impl Fn(NoticeAction, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Popover {
    let level = state.read(cx).notices.level();
    let state = state.clone();
    let on_action: OnAction = Rc::new(on_action);
    Popover::new("notices")
        .anchor(Anchor::TopRight)
        .open(open)
        .on_open_change(on_open_change)
        .trigger(
            Button::new("notices-button")
                .ghost()
                .icon(IconName::Bell)
                .tooltip("Notifications")
                .relative()
                .children(level.map(|level| Indicator::Dot(tone(level)))),
        )
        .content(move |_, _, cx| list(&state, &on_action, cx))
}

fn tone(level: Level) -> Tone {
    match level {
        Level::Update => Tone::Accent,
        Level::Warning => Tone::Warn,
    }
}

fn icon(level: Level) -> (IconName, Rgba) {
    match level {
        Level::Update => (IconName::Download, t::AC),
        Level::Warning => (IconName::TriangleAlert, t::WARN),
    }
}

fn list(state: &Entity<EditorState>, on_action: &OnAction, cx: &App) -> Div {
    let notices: Vec<Notice> = state.read(cx).notices.iter().cloned().collect();
    let count = notices.len();
    let dismissable = notices.iter().filter(|n| n.dismissable()).count();
    let dismiss_all = {
        let state = state.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            state.update(cx, |s, cx| {
                let keys: Vec<String> = s
                    .notices
                    .iter()
                    .filter(|n| n.dismissable())
                    .map(|n| n.key.clone())
                    .collect();
                for key in keys {
                    s.dismiss_notice(&key, cx);
                }
            })
        }
    };
    v_flex()
        .w(px(380.))
        .child(
            h_flex()
                .gap(px(6.))
                .h(t::PANEL_HEADER)
                .pl(px(12.))
                .pr(px(6.))
                .border_b_1()
                .border_color(t::LN)
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .child("Notifications")
                .when(count > 0, |el| {
                    el.child(
                        div()
                            .font_family(t::mono())
                            .text_color(t::T3)
                            .child(count.to_string()),
                    )
                })
                .child(div().flex_1())
                .when(dismissable > 1, |el| {
                    el.child(
                        Button::new("notices-dismiss-all")
                            .ghost()
                            .xsmall()
                            .label("Dismiss all")
                            .on_click(dismiss_all),
                    )
                }),
        )
        .when(count == 0, |el| {
            el.child(EmptyState::new(IconName::Bell, "No notifications").compact())
        })
        .children(
            notices
                .into_iter()
                .enumerate()
                .map(|(ix, notice)| row(ix, notice, state, on_action)),
        )
}

fn row(ix: usize, notice: Notice, state: &Entity<EditorState>, on_action: &OnAction) -> Div {
    let (icon, color) = icon(notice.level);
    let dismissable = notice.dismissable();
    let dismiss = {
        let (state, key) = (state.clone(), notice.key.clone());
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            state.update(cx, |s, cx| s.dismiss_notice(&key, cx))
        }
    };
    h_flex()
        .items_start()
        .gap(px(10.))
        .pl(px(14.))
        .pr(px(10.))
        .py(px(12.))
        .when(ix > 0, |el| el.border_t_1().border_color(t::LN))
        .child(
            div()
                .pt(px(1.))
                .child(Icon::new(icon).size(px(16.)).color(color)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().font_weight(FontWeight::MEDIUM).child(notice.title))
                .children(notice.items.into_iter().map(|item| {
                    div()
                        .mt(px(3.))
                        .truncate()
                        .font_family(t::mono())
                        .text_size(px(11.))
                        .text_color(t::T2)
                        .child(item)
                }))
                .children(
                    notice
                        .detail
                        .map(|detail| div().mt(px(3.)).text_color(t::T2).child(detail)),
                )
                .children(notice.action.map(|action| {
                    let on_action = on_action.clone();
                    let (label, icon) = match action {
                        NoticeAction::OpenIntegrations => ("Settings…", None),
                        NoticeAction::Relink => ("Relink…", Some(IconName::Link)),
                        NoticeAction::Update => ("Update", Some(IconName::Download)),
                        NoticeAction::OpenRelease => ("Download…", Some(IconName::ExternalLink)),
                        NoticeAction::Restart => ("Restart", Some(IconName::RotateCcw)),
                    };
                    let mut button = Button::new(("notice-action", ix)).small().label(label);
                    if let Some(icon) = icon {
                        button = button.icon(icon);
                    }
                    if !dismissable {
                        button = button.primary();
                    }
                    h_flex()
                        .mt(px(8.))
                        .child(button.on_click(move |_, window, cx| on_action(action, window, cx)))
                })),
        )
        .when(dismissable, |el| {
            el.child(
                Button::new(("notice-dismiss", ix))
                    .ghost()
                    .xsmall()
                    .icon(IconName::X)
                    .tooltip("Dismiss")
                    .on_click(dismiss),
            )
        })
}
