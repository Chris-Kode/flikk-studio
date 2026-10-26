//! Caption style presets (PRD C2), kept in `EditorState::presets` until
//! style profiles are persisted.

use super::TranscriptPanel;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::memory::{CaptionPosition, CaptionStyle};

const POSITIONS: [CaptionPosition; 3] = [
    CaptionPosition::Top,
    CaptionPosition::Middle,
    CaptionPosition::Bottom,
];

impl TranscriptPanel {
    pub(super) fn edit_style(&mut self, f: impl FnOnce(&mut CaptionStyle), cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let i = s.caption_style;
            if let Some(style) = s.presets.caption_styles.get_mut(i) {
                f(style);
            }
            cx.notify();
        });
    }

    fn add_style(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let mut style = s.active_caption_style();
            style.name = format!("Style {}", s.presets.caption_styles.len() + 1);
            s.presets.caption_styles.push(style);
            s.caption_style = s.presets.caption_styles.len() - 1;
            cx.notify();
        });
    }

    pub(super) fn style_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.read(cx);
        let style = state.active_caption_style();
        let names: Vec<String> = state
            .presets
            .caption_styles
            .iter()
            .map(|s| s.name.clone())
            .collect();
        let active = state.caption_style;
        let focused = self.font.read(cx).focus_handle(cx).is_focused(window);
        if !focused && self.font.read(cx).value().as_ref() != style.font {
            let font = style.font.clone();
            self.font.update(cx, |i, cx| i.set_value(font, window, cx));
        }
        let theme = cx.theme();
        let (muted, border, chip_bg, hover) = (
            theme.muted_foreground,
            theme.border,
            theme.secondary,
            theme.secondary_hover,
        );
        let fg = theme.foreground;
        let presets = h_flex()
            .gap_1()
            .flex_wrap()
            .children(names.into_iter().enumerate().map(|(i, name)| {
                div()
                    .id(("caption-style", i))
                    .px_2()
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .when(i == active, |d| d.bg(chip_bg).text_color(fg))
                    .when(i != active, |d| d.text_color(muted))
                    .hover(move |s| s.bg(hover))
                    .child(name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |s, cx| {
                            s.caption_style = i;
                            cx.notify();
                        });
                    }))
            }))
            .child(
                Button::new("add-caption-style")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .tooltip("New style")
                    .on_click(cx.listener(|this, _, _, cx| this.add_style(cx))),
            );
        let position = POSITIONS
            .iter()
            .position(|p| *p == style.position)
            .unwrap_or(2);
        let uppercase = style.uppercase;
        v_flex()
            .gap_2()
            .p_2()
            .border_t_1()
            .border_color(border)
            .child(presets)
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.font).xsmall()))
                    .child(
                        Button::new("caption-case")
                            .ghost()
                            .xsmall()
                            .icon(IconName::CaseSensitive)
                            .tooltip("Uppercase")
                            .toggled(uppercase)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit_style(|s| s.uppercase = !uppercase, cx)
                            })),
                    )
                    .child(
                        TabBar::new("caption-position")
                            .segmented()
                            .xsmall()
                            .selected_index(position)
                            .child(Tab::new().label("Top"))
                            .child(Tab::new().label("Mid"))
                            .child(Tab::new().label("Bottom"))
                            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                let p = POSITIONS[(*ix).min(2)];
                                this.edit_style(|s| s.position = p, cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(stepper(
                        "Size",
                        style.size as i64,
                        2,
                        12,
                        160,
                        cx,
                        |s, v| s.size = v as f32,
                    ))
                    .child(stepper(
                        "Line",
                        style.max_chars_per_line as i64,
                        1,
                        12,
                        60,
                        cx,
                        |s, v| s.max_chars_per_line = v as u32,
                    ))
                    .child(stepper(
                        "Lines",
                        style.max_lines as i64,
                        1,
                        1,
                        3,
                        cx,
                        |s, v| s.max_lines = v as u32,
                    )),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(stepper(
                        "Outline",
                        style.outline as i64,
                        1,
                        0,
                        12,
                        cx,
                        |s, v| s.outline = v as f32,
                    ))
                    .child(stepper(
                        "Shadow",
                        style.shadow as i64,
                        1,
                        0,
                        12,
                        cx,
                        |s, v| s.shadow = v as f32,
                    )),
            )
    }
}

/// `label  −  value  +`, clamped to `min..=max`.
fn stepper(
    label: &'static str,
    value: i64,
    step: i64,
    min: i64,
    max: i64,
    cx: &mut Context<TranscriptPanel>,
    set: fn(&mut CaptionStyle, i64),
) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    let button = |id: &'static str, icon: IconName, to: i64| {
        Button::new(SharedString::from(format!("{label}-{id}")))
            .ghost()
            .xsmall()
            .icon(icon)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.edit_style(|s| set(s, to.clamp(min, max)), cx)
            }))
    };
    h_flex()
        .gap_0p5()
        .text_xs()
        .child(div().pr_1().text_color(muted).child(label))
        .child(button("dec", IconName::Minus, value - step))
        .child(div().min_w(px(18.)).text_center().child(value.to_string()))
        .child(button("inc", IconName::Plus, value + step))
}
