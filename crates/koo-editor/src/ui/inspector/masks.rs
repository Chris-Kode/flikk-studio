//! A picture clip's masks (see `koo_core::mask`): add a rectangle or an
//! ellipse (paths are drawn with the viewer's pen), and per mask its mode,
//! invert, smoothing, a keyframe toggle for its whole shape at the
//! playhead, removal, and its edge, opacity and box as sliders.

use super::param::{Key, MaskField};
use super::{InspectorPanel, clip_time, section_with};
use crate::ui::timeline::glyph::{Glyph, glyph};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::command::Command;
use koo_core::mask::{Mask, MaskKeyframe, MaskMode, MaskShape};
use koo_core::model::Clip;

/// Turns a clip's mask list into the one to commit.
type Change = dyn Fn(&mut Vec<Mask>);

impl InspectorPanel {
    pub(super) fn masks_view(
        &mut self,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let muted = cx.theme().muted_foreground;
        let add =
            |shape: MaskShape, g: Glyph, tip: &'static str, clip: &Clip, cx: &mut Context<Self>| {
                let (id, masks) = (clip.id, clip.masks.clone());
                Button::new(SharedString::from(format!("add-mask-{tip}")))
                    .ghost()
                    .xsmall()
                    .tooltip(tip)
                    .child(glyph(g, px(13.), muted))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let mut masks = masks.clone();
                        masks.push(Mask::new(shape.clone()));
                        this.run(vec![Command::SetMasks { clip: id, masks }], "set_masks", cx);
                    }))
            };
        let actions = h_flex()
            .gap_1()
            .child(add(
                MaskShape::Rectangle {
                    center: [0.5, 0.5],
                    size: [0.5, 0.5],
                    rotation_deg: 0.0,
                    roundness: 0.0,
                },
                Glyph::MaskRect,
                "Rectangle",
                clip,
                cx,
            ))
            .child(add(
                MaskShape::Ellipse {
                    center: [0.5, 0.5],
                    size: [0.5, 0.5],
                    rotation_deg: 0.0,
                },
                Glyph::MaskEllipse,
                "Ellipse",
                clip,
                cx,
            ));
        let blocks: Vec<Div> = (0..clip.masks.len())
            .map(|i| self.mask_block(clip, i, window, cx))
            .collect();
        section_with("Masks", actions, cx).children(blocks)
    }

    /// One mask: its controls row, then its sliders.
    fn mask_block(
        &mut self,
        clip: &Clip,
        i: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = self.state.read(cx);
        let playhead = state.playhead;
        let frame = state.bus.project().timeline.settings.rate.frame_duration();
        let at = clip_time(clip, playhead);
        let inside = clip.range().contains(playhead);
        let mask = clip.masks[i].clone();
        let now = mask.state_at(at);
        let theme = cx.theme();
        let (muted, primary, hover) =
            (theme.muted_foreground, theme.primary, theme.secondary_hover);
        let (id, masks) = (clip.id, clip.masks.clone());
        // `change` turns the mask list into the one to commit.
        let set = move |change: Box<Change>| {
            let masks = masks.clone();
            move |this: &mut Self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                let mut masks = masks.clone();
                change(&mut masks);
                this.run(vec![Command::SetMasks { clip: id, masks }], "set_masks", cx);
            }
        };
        let kind = match now.shape {
            MaskShape::Rectangle { .. } => "Rectangle",
            MaskShape::Ellipse { .. } => "Ellipse",
            MaskShape::Path { .. } => "Path",
        };
        let panel = cx.entity();
        let current = mask.mode;
        let mode_masks = clip.masks.clone();
        let mode = Button::new(("mask-mode", i))
            .outline()
            .xsmall()
            .label(current.label())
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for mode in MaskMode::ALL {
                    let (panel, masks) = (panel.clone(), mode_masks.clone());
                    menu = menu.item(
                        PopupMenuItem::new(mode.label())
                            .checked(mode == current)
                            .on_click(move |_, _, cx| {
                                let mut masks = masks.clone();
                                masks[i].mode = mode;
                                panel.update(cx, |this, cx| {
                                    this.run(
                                        vec![Command::SetMasks { clip: id, masks }],
                                        "set_masks",
                                        cx,
                                    )
                                })
                            }),
                    );
                }
                menu
            });
        let invert = Button::new(("mask-invert", i))
            .ghost()
            .xsmall()
            .toggled(mask.invert)
            .label("Invert")
            .on_click(cx.listener(set(Box::new(move |m| m[i].invert = !m[i].invert))));
        let smooth = match &mask.state.shape {
            MaskShape::Path { smooth, .. } => Some(
                Button::new(("mask-smooth", i))
                    .ghost()
                    .xsmall()
                    .toggled(*smooth)
                    .label("Smooth")
                    .on_click(cx.listener(set(Box::new(move |m| {
                        let toggle = |shape: &mut MaskShape| {
                            if let MaskShape::Path { smooth, .. } = shape {
                                *smooth = !*smooth;
                            }
                        };
                        toggle(&mut m[i].state.shape);
                        for k in &mut m[i].keyframes {
                            toggle(&mut k.state.shape);
                        }
                    })))),
            ),
            _ => None,
        };
        let keyed = mask
            .keyframes
            .iter()
            .any(|k| (k.at - at).0.abs() < frame.0 / 2 + 1);
        let color = if mask.keyframes.is_empty() {
            muted
        } else {
            primary
        };
        let key_state = now.clone();
        let key = div()
            .id(("mask-key", i))
            .flex_none()
            .size(px(18.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .when(inside, |d| d.cursor_pointer().hover(|s| s.bg(hover)))
            .when(!inside, |d| d.opacity(0.4))
            .child(
                div()
                    .size(px(7.))
                    .rounded(px(1.5))
                    .border_1()
                    .border_color(color)
                    .when(keyed, |d| d.bg(color)),
            )
            .when(inside, |d| {
                d.on_click(cx.listener(set(Box::new(move |m| {
                    let keys = &mut m[i].keyframes;
                    if keyed {
                        keys.retain(|k| (k.at - at).0.abs() > frame.0 / 2);
                    } else {
                        keys.push(MaskKeyframe {
                            at,
                            state: key_state.clone(),
                            ease: Default::default(),
                        });
                        keys.sort_by_key(|k| k.at);
                    }
                }))))
            });
        let remove = Button::new(("mask-remove", i))
            .ghost()
            .xsmall()
            .icon(IconName::Close)
            .on_click(cx.listener(set(Box::new(move |m| {
                m.remove(i);
            }))));
        let header = h_flex()
            .h(px(26.))
            .gap_1()
            .child(div().flex_1().text_xs().text_color(muted).child(kind))
            .child(mode)
            .child(invert)
            .children(smooth)
            .child(key)
            .child(remove);
        let mut fields = vec![MaskField::Feather, MaskField::Expansion, MaskField::Opacity];
        match now.shape {
            MaskShape::Rectangle { .. } => {
                fields.extend([MaskField::Rotation, MaskField::Roundness])
            }
            MaskShape::Ellipse { .. } => fields.push(MaskField::Rotation),
            MaskShape::Path { .. } => {}
        }
        let rows: Vec<Div> = fields
            .into_iter()
            .map(|field| {
                let key = Key::Mask(i, field);
                self.param(key, field.get(&now), window, cx).row(cx)
            })
            .collect();
        v_flex().pt_1().child(header).children(rows)
    }
}
