//! The selected clip's properties (see `clip.rs`), or the selected
//! transition; for picture clips, transform, composite and crop with
//! per-property keyframe toggles at the playhead, and the clip's keyframes
//! (PRD E5).

use super::param::{Key, format_value};
use super::transition::labeled;
use super::{InspectorPanel, clip_time, property_of, section};
use crate::ui::library::{FxGlyph, fx_glyph};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::Time;
use koo_core::command::Command;
use koo_core::model::{BlendMode, Clip, Keyframe, Property};

const TRANSFORM: [Key; 6] = [
    Key::PosX,
    Key::PosY,
    Key::Scale,
    Key::ScaleX,
    Key::ScaleY,
    Key::Rotation,
];
const SHAPE: [Key; 4] = [Key::SkewX, Key::SkewY, Key::AnchorX, Key::AnchorY];
const CROP: [Key; 4] = [Key::Crop(0), Key::Crop(1), Key::Crop(2), Key::Crop(3)];

impl InspectorPanel {
    /// The selected clip's properties for its kind, or the sequence format
    /// when nothing is selected.
    pub(super) fn edit_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Shown until another clip is selected (the timeline may keep the
        // transition picked while clips are clicked).
        let state = self.state.read(cx);
        let transition = state.selected_transition.filter(|(clip, _)| {
            state.selection.clips.is_empty() || state.selection.clips.contains(clip)
        });
        if let Some((clip, edge)) = transition
            && let Some(view) = self.transition_view(clip, edge, window, cx)
        {
            return view;
        }
        match self.primary_on_track(cx) {
            Some((track, clip)) => self.clip_view(track, clip, window, cx),
            None => self.sequence_view(cx),
        }
    }

    /// Transform with keyframe toggles, then the clip's keyframes. Lettering
    /// (captions, titles) has no crop, flip or blend mode: the render cannot
    /// burn them in.
    pub(super) fn transform_view(
        &mut self,
        clip: &Clip,
        lettering: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let clip = clip.clone();
        let state = self.state.read(cx);
        let playhead = state.playhead;
        let frame = state.bus.project().timeline.settings.rate.frame_duration();
        let at = clip_time(&clip, playhead);
        let inside = clip.range().contains(playhead);

        let mut keyed_rows = |keys: &[Key], this: &mut Self, cx: &mut Context<Self>| {
            keys.iter()
                .map(|&key| {
                    let property = property_of(key).unwrap();
                    let value = clip.value_at(property, at);
                    let row = this.param(key, value, window, cx).row(cx);
                    let toggle = this.key_toggle(&clip, property, at, frame, inside, cx);
                    row.child(toggle)
                })
                .collect::<Vec<_>>()
        };
        let transform_rows = keyed_rows(&TRANSFORM, self, cx);
        let shape_rows = keyed_rows(&SHAPE, self, cx);
        let opacity = keyed_rows(&[Key::Opacity], self, cx);
        let crop_rows = keyed_rows(&CROP, self, cx);
        let mut frames: Vec<Keyframe> = clip.keyframes.clone();
        frames.sort_by_key(|k| (k.at, k.property as u8));
        let transform = section("Transform", cx)
            .children(transform_rows)
            .children(shape_rows)
            .when(!lettering, |s| s.child(self.flip_row(&clip, cx)));
        let composite = section("Composite", cx)
            .children(opacity)
            .when(!lettering, |s| s.child(self.blend_row(&clip, cx)));
        let crop = (!lettering).then(|| section("Crop", cx).children(crop_rows));
        let list = (!frames.is_empty()).then(|| self.keyframe_list(&clip, frames, at, frame, cx));
        v_flex()
            .gap_4()
            .child(transform)
            .child(composite)
            .children(crop)
            .children(list)
            .into_any_element()
    }

    /// Mirror toggles; not animatable.
    fn flip_row(&self, clip: &Clip, cx: &mut Context<Self>) -> Div {
        let muted = cx.theme().muted_foreground;
        let flip = clip.transform.flip;
        let toggle = |axis: usize, cx: &mut Context<Self>| {
            let (glyph, tip) = match axis {
                0 => (FxGlyph::FlipH, "Flip horizontal"),
                _ => (FxGlyph::FlipV, "Flip vertical"),
            };
            let (id, transform) = (clip.id, clip.transform.clone());
            Button::new(("flip", axis))
                .ghost()
                .xsmall()
                .toggled(flip[axis])
                .tooltip(tip)
                .child(fx_glyph(glyph, px(13.), muted))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let mut transform = transform.clone();
                    transform.flip[axis] = !transform.flip[axis];
                    this.run(
                        vec![Command::SetTransform {
                            clip: id,
                            transform,
                        }],
                        "set_transform",
                        cx,
                    );
                }))
        };
        labeled(
            "Flip",
            h_flex().gap_1().child(toggle(0, cx)).child(toggle(1, cx)),
            cx,
        )
    }

    /// Composite mode dropdown.
    fn blend_row(&self, clip: &Clip, cx: &mut Context<Self>) -> Div {
        let (id, transform) = (clip.id, clip.transform.clone());
        let current = transform.blend;
        let panel = cx.entity();
        labeled(
            "Blend",
            Button::new("blend")
                .outline()
                .xsmall()
                .w_full()
                .label(current.label())
                .dropdown_caret(true)
                .dropdown_menu(move |mut menu, _, _| {
                    for mode in BlendMode::ALL {
                        let (panel, transform) = (panel.clone(), transform.clone());
                        menu = menu.item(
                            PopupMenuItem::new(mode.label())
                                .checked(mode == current)
                                .on_click(move |_, _, cx| {
                                    let mut transform = transform.clone();
                                    transform.blend = mode;
                                    panel.update(cx, |this, cx| {
                                        this.run(
                                            vec![Command::SetTransform {
                                                clip: id,
                                                transform,
                                            }],
                                            "set_transform",
                                            cx,
                                        )
                                    })
                                }),
                        );
                    }
                    menu
                }),
            cx,
        )
    }

    /// Filled when a keyframe sits at the playhead, outlined when the
    /// property is animated elsewhere.
    fn key_toggle(
        &self,
        clip: &Clip,
        property: Property,
        at: Time,
        frame: Time,
        inside: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let keyed = clip
            .keyframes
            .iter()
            .any(|k| k.property == property && (k.at - at).0.abs() < frame.0 / 2 + 1);
        let animated = clip.keyframes.iter().any(|k| k.property == property);
        let color = if animated {
            theme.primary
        } else {
            theme.muted_foreground
        };
        // Commands take timeline time; keyframes store clip-relative time.
        let (clip_id, value, when) = (clip.id, clip.value_at(property, at), clip.start + at);
        div()
            .id(SharedString::from(format!("kf-{property:?}")))
            .flex_none()
            .size(px(18.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .when(inside, |d| {
                d.cursor_pointer().hover(|s| s.bg(theme.secondary_hover))
            })
            .when(!inside, |d| d.opacity(0.4))
            .child(
                div()
                    .size(px(7.))
                    .rounded(px(1.5))
                    .border_1()
                    .border_color(color)
                    .when(keyed, |d| d.bg(color)),
            )
            .tooltip(|window, cx| Tooltip::new("Keyframe").build(window, cx))
            .when(inside, |d| {
                d.on_click(cx.listener(move |this, _, _, cx| {
                    let command = if keyed {
                        Command::RemoveKeyframe {
                            clip: clip_id,
                            property,
                            at: when,
                        }
                    } else {
                        Command::SetKeyframe {
                            clip: clip_id,
                            property,
                            at: when,
                            value,
                            ease: Default::default(),
                        }
                    };
                    this.run(vec![command], "keyframe", cx);
                }))
            })
    }

    fn keyframe_list(
        &self,
        clip: &Clip,
        frames: Vec<Keyframe>,
        at: Time,
        frame: Time,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (muted, active, hover) = (theme.muted_foreground, theme.list_active, theme.list_hover);
        let (clip_id, start) = (clip.id, clip.start);
        let keyed = frames
            .into_iter()
            .filter_map(|k| Some((transform_key(k.property)?, k)));
        section("Keyframes", cx).children(keyed.enumerate().map(|(i, (key, k))| {
            let current = (k.at - at).0.abs() < frame.0 / 2 + 1;
            let spec = key.spec();
            let (property, time) = (k.property, k.at);
            h_flex()
                .id(("keyframe", i))
                .h(px(26.))
                .px_1()
                .gap_2()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .when(current, |d| d.bg(active))
                .when(!current, |d| d.hover(|s| s.bg(hover)))
                .child(div().w(px(64.)).text_color(muted).child(spec.label))
                .child(div().flex_1().child(k.at.to_string()))
                .child(format_value(k.value * spec.factor, spec.decimals))
                .child(
                    Button::new(("remove-keyframe", i))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.run(
                                vec![Command::RemoveKeyframe {
                                    clip: clip_id,
                                    property,
                                    at: start + time,
                                }],
                                "remove_keyframe",
                                cx,
                            );
                        })),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.seek(start + time, cx));
                }))
        }))
    }

    /// Nothing selected: the sequence format, read-only for now.
    fn sequence_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let project = self.project(cx);
        let s = &project.timeline.settings;
        let fps = format!("{:.2}", s.rate.fps());
        let fps = fps.trim_end_matches('0').trim_end_matches('.');
        let line = format!(
            "{}×{} · {} fps · {} kHz · {}",
            s.width,
            s.height,
            fps,
            s.sample_rate / 1000,
            crate::ui::media_pool::clock(project.timeline.duration()),
        );
        let muted = cx.theme().muted_foreground;
        section("Sequence", cx)
            .child(div().text_xs().text_color(muted).child(line))
            .into_any_element()
    }
}

/// The inspector control a keyframed property belongs to, if it has one.
fn transform_key(p: Property) -> Option<Key> {
    Some(match p {
        Property::PositionX => Key::PosX,
        Property::PositionY => Key::PosY,
        Property::Scale => Key::Scale,
        Property::ScaleX => Key::ScaleX,
        Property::ScaleY => Key::ScaleY,
        Property::Rotation => Key::Rotation,
        Property::SkewX => Key::SkewX,
        Property::SkewY => Key::SkewY,
        Property::AnchorX => Key::AnchorX,
        Property::AnchorY => Key::AnchorY,
        Property::Opacity => Key::Opacity,
        Property::CropLeft => Key::Crop(0),
        Property::CropTop => Key::Crop(1),
        Property::CropRight => Key::Crop(2),
        Property::CropBottom => Key::Crop(3),
        _ => return None,
    })
}
