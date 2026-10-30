//! The top bar's notice button and the list it drops down: the warnings and
//! errors in `EditorState::notices`, each with its own dismiss button.

use crate::notices::{Level, Notice, NoticeAction};
use crate::state::EditorState;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;

type OnAction = Rc<dyn Fn(NoticeAction, &mut Window, &mut App)>;

/// The button, `None` without notices; its icon takes the colour of the
/// most serious one. `on_action` runs a notice's own button.
pub fn button(
    state: &Entity<EditorState>,
    open: bool,
    on_open_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
    on_action: impl Fn(NoticeAction, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Option<Popover> {
    let notices = &state.read(cx).notices;
    let (icon, color) = style(notices.level()?, cx);
    let count = notices.len();
    let state = state.clone();
    let on_action: OnAction = Rc::new(on_action);
    Some(
        Popover::new("notices")
            .anchor(Anchor::TopRight)
            .open(open)
            .on_open_change(on_open_change)
            .trigger(
                Button::new("notices-button")
                    .ghost()
                    .small()
                    .icon(Icon::new(icon).text_color(color))
                    .when(count > 1, |b| b.label(count.to_string()))
                    .tooltip("Notifications"),
            )
            .content(move |_, _, cx| list(&state, &on_action, cx)),
    )
}

fn style(level: Level, cx: &App) -> (IconName, Hsla) {
    match level {
        Level::Warning => (IconName::TriangleAlert, cx.theme().warning),
        Level::Error => (IconName::CircleAlert, cx.theme().red),
    }
}

fn list(state: &Entity<EditorState>, on_action: &OnAction, cx: &App) -> Div {
    let notices: Vec<Notice> = state.read(cx).notices.iter().cloned().collect();
    let theme = cx.theme();
    let (border, muted) = (theme.border, theme.muted_foreground);
    v_flex()
        .w(px(340.))
        .gap_3()
        .children(notices.into_iter().enumerate().map(|(ix, notice)| {
            let (icon, color) = style(notice.level, cx);
            let dismiss = {
                let (state, key) = (state.clone(), notice.key.clone());
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    state.update(cx, |s, cx| s.dismiss_notice(&key, cx))
                }
            };
            v_flex()
                .gap_1()
                .when(ix > 0, |el| el.pt_3().border_t_1().border_color(border))
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(icon).small().text_color(color).flex_none())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .font_medium()
                                .child(notice.title),
                        )
                        .child(
                            Button::new(("notice-dismiss", ix))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("Dismiss")
                                .on_click(dismiss),
                        ),
                )
                .child(
                    v_flex()
                        .pl_6()
                        .gap_1()
                        .text_xs()
                        .when(!notice.items.is_empty(), |el| {
                            el.child(v_flex().children(notice.items.into_iter().map(|item| {
                                h_flex()
                                    .gap_1p5()
                                    .child(div().text_color(muted).child("•"))
                                    .child(item)
                            })))
                        })
                        .children(
                            notice
                                .detail
                                .map(|detail| div().text_color(muted).child(detail)),
                        )
                        .children(notice.action.map(|action| {
                            let on_action = on_action.clone();
                            h_flex().pt_1().child(
                                Button::new(("notice-action", ix))
                                    .small()
                                    .outline()
                                    .label(match action {
                                        NoticeAction::OpenModels => "Download…",
                                        NoticeAction::Relink => "Relink…",
                                    })
                                    .on_click(move |_, window, cx| on_action(action, window, cx)),
                            )
                        })),
                )
        }))
}
