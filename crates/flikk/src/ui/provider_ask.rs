//! The question asked before new media is analyzed with the provider
//! picked to transcribe, which costs money: the items waiting
//! (`EditorState::provider_asks`), Cancel and Analyze. It follows the list
//! as more items arrive; the window opens and closes it with the list.

use crate::state::EditorState;
use crate::ui::kit::{DialogExt as _, DialogId, h_flex, theme as t, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Rows listed before the rest are counted.
const SHOWN: usize = 8;

/// Open the question; its id closes it once the list is empty.
pub fn open(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) -> DialogId {
    let state = state.clone();
    window.open_dialog_id(cx, move |dialog, _, cx| {
        let s = state.read(cx);
        let provider = s.online_transcription();
        let name = provider.map_or("the provider", |p| p.name);
        let keyed = provider.is_some_and(|p| s.provider_status(p).has_key());
        let items: Vec<_> = (s.provider_asks.iter())
            .filter_map(|id| s.bus.project().media(*id))
            .collect();
        let rows = items.iter().take(SHOWN).map(|item| {
            let duration = item.info.as_ref().map(|i| i.duration);
            h_flex()
                .h(px(26.))
                .gap(px(12.))
                .border_b_1()
                .border_color(t::LN)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(crate::ui::format::item_name(item)),
                )
                .children(duration.map(|d| meta(crate::ui::format::clock_rounded(d))))
        });
        let more = items.len().saturating_sub(SHOWN);
        let (yes, no) = (state.clone(), state.clone());
        dialog
            .title(format!("Analyze with {name}?"))
            .w(px(420.))
            .child(
                v_flex()
                    .text_size(px(12.))
                    .children(rows)
                    .children((more > 0).then(|| meta(format!("{more} more")).pt(px(6.)))),
            )
            .when(!keyed, |dialog| {
                dialog.note(format!("Add the {name} key in Settings → Integrations"))
            })
            .confirm("Analyze", move |_, cx| {
                yes.update(cx, |s, cx| s.analyze_asked_with_provider(cx));
                true
            })
            .confirm_disabled(!keyed || items.is_empty())
            .on_cancel(move |_, cx| no.update(cx, |s, cx| s.skip_provider_asks(cx)))
    })
}

/// Quiet mono meta text: durations, counts.
fn meta(text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .font_family(t::mono())
        .text_size(px(11.))
        .text_color(t::T3)
        .child(text.into())
}
