//! Ready-made looks as tiles lettered in themselves, and the preview of
//! the edited style.

use super::CaptionStylePanel;
use crate::ui::kit::{theme as t, v_flex};
use crate::ui::lettering::{self, Lettering};
use flikk_core::memory::{CaptionHighlight, CaptionStyle};
use flikk_deliver::captions::{Pose, wrap};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const TILE_HEIGHT: f32 = 46.;
const PREVIEW_HEIGHT: f32 = 48.;
/// Largest preview lettering, in pixels per 1080p pixel.
const PREVIEW_ZOOM: f32 = 0.6;

/// `text` in `style` at rest, a word into its highlight; every word shows.
fn letters(style: &CaptionStyle, text: &str) -> Lettering {
    let words = text.split_whitespace().count().max(1);
    let spoken = match style.highlight {
        CaptionHighlight::Reveal => words - 1,
        _ => 1.min(words - 1),
    };
    Lettering {
        lines: wrap(text, style).into_iter().map(Into::into).collect(),
        style: style.clone(),
        pose: Pose::REST,
        spoken: Some(spoken),
    }
}

/// `l` on a backdrop, scaled to fit, at most `zoom`.
fn lettered(l: Lettering, height: f32, zoom: f32) -> Canvas<()> {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            window.paint_quad(fill(bounds, t::BACKDROP).corner_radii(px(6.)));
            let natural = lettering::measure(&l, 1., window);
            let fit = |room: Pixels, need: Pixels| f32::from(room) / f32::from(need).max(1.);
            let k = fit(bounds.size.width * 0.88, natural.width)
                .min(fit(bounds.size.height * 0.84, natural.height))
                .min(zoom);
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                lettering::paint(&l, bounds, k, window, cx)
            });
        },
    )
    .w_full()
    .h(px(height))
}

/// The edited style on `text`.
pub fn preview(style: &CaptionStyle, text: String) -> impl IntoElement {
    lettered(letters(style, &text), PREVIEW_HEIGHT, PREVIEW_ZOOM)
}

impl CaptionStylePanel {
    /// Three across, each named; picking one dresses the edited style in it.
    pub(super) fn looks_page(
        &mut self,
        style: &CaptionStyle,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div().grid().grid_cols(3).gap(px(6.)).children(
            CaptionStyle::looks()
                .into_iter()
                .enumerate()
                .map(|(i, look)| {
                    let worn = style.with_look(&look) == *style;
                    let name = look.name.clone();
                    v_flex()
                        .id(("caption-look", i))
                        .gap(px(3.))
                        .cursor_pointer()
                        .text_size(px(10.))
                        .text_color(if worn { t::T1 } else { t::T3 })
                        .hover(|s| s.text_color(t::T1))
                        .child(
                            div()
                                .rounded(px(6.))
                                .when(worn, |d| {
                                    d.shadow(vec![BoxShadow {
                                        color: t::AC.into(),
                                        offset: point(px(0.), px(0.)),
                                        blur_radius: px(0.),
                                        spread_radius: px(1.5),
                                        inset: false,
                                    }])
                                })
                                .child(lettered(letters(&look, "Aa Bb"), TILE_HEIGHT, 1.)),
                        )
                        .child(div().w_full().text_center().truncate().child(name))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let look = look.clone();
                            this.edit(|s| *s = s.with_look(&look), cx);
                        }))
                }),
        )
    }
}
