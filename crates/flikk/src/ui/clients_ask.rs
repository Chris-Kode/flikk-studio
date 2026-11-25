//! The question asked before Flikk does what another app asked of it over
//! MCP (`Desk::asks`): starting a project with an agent, messaging an
//! agent, saving or closing a project. Cancel denies them all, Accept
//! carries them out, and Always accept also stops asking. It follows the
//! list as more arrive; the window opens and closes it with the list.

use crate::desk::Desk;
use crate::ui::kit::{DialogExt as _, DialogId, theme as t, v_flex};
use gpui_kit::*;

/// Open the question; its id closes it once the list is empty.
pub fn open(desk: &Entity<Desk>, window: &mut Window, cx: &mut App) -> DialogId {
    let desk = desk.clone();
    window.open_dialog_id(cx, move |dialog, _, cx| {
        let rows = desk.read(cx).asks().iter().map(|ask| {
            v_flex()
                .py(px(6.))
                .gap(px(2.))
                .border_b_1()
                .border_color(t::LN)
                .child(div().truncate().child(ask.title.clone()))
                .children(ask.detail.clone().map(|detail| {
                    div()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(t::T3)
                        .child(detail)
                }))
        });
        let rows: Vec<_> = rows.collect();
        let (accept, always, deny) = (desk.clone(), desk.clone(), desk.clone());
        dialog
            .title("Do what another app asks?")
            .w(px(420.))
            .child(v_flex().text_size(px(12.)).children(rows))
            .secondary("Always accept", move |window, cx| {
                always.update(cx, |desk, cx| desk.always_accept(window, cx));
                true
            })
            .confirm("Accept", move |window, cx| {
                accept.update(cx, |desk, cx| desk.answer_asks(true, window, cx));
                true
            })
            .on_cancel(move |window, cx| {
                deny.update(cx, |desk, cx| desk.answer_asks(false, window, cx))
            })
    })
}
