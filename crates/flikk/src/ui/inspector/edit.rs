//! The Edit inspector: the selected transition, the selected clip's
//! properties (see `clip.rs`); hidden when nothing is selected.
//! Picture clips get transform, compositing, crop and corner pin, each row
//! with keyframe navigation at the playhead, and the clip's keyframes
//! (PRD E5).

use super::param::{Key, format_value};
use super::{
    InspectorPanel, View, clip_time, group, label, panel_header, property_of, row, select_field,
};
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, DropdownMenu as _, EmptyState, IconName, Keyframe, KeyframeNav, PopupMenuItem,
    Segmented, h_flex,
};
use flikk_core::Time;
use flikk_core::command::Command;
use flikk_core::model::{BlendMode, Clip, Fit, Keyframe as ClipKeyframe, Property, Transform};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Corner pin rows: a corner's name and its x and y keys.
const PIN: [(&str, Key, Key); 4] = [
    ("Top left", Key::Pin(0), Key::Pin(1)),
    ("Top right", Key::Pin(2), Key::Pin(3)),
    ("Bottom right", Key::Pin(4), Key::Pin(5)),
    ("Bottom left", Key::Pin(6), Key::Pin(7)),
];

/// The clip and playhead a transform row edits at.
struct At<'a> {
    clip: &'a Clip,
    at: Time,
    frame: Time,
    inside: bool,
}

impl At<'_> {
    fn keyed(&self, p: Property) -> bool {
        self.clip
            .keyframes
            .iter()
            .any(|k| k.property == p && (k.at - self.at).0.abs() < self.frame.0 / 2 + 1)
    }
}

impl InspectorPanel {
    /// The selected transition, the selected clip's properties for its
    /// kind, or an empty state when nothing is selected.
    pub(super) fn edit_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> View {
        // Shown until another clip is selected (the timeline may keep the
        // transition picked while clips are clicked).
        let state = self.state.read(cx);
        let transition = state.inspected_transition();
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

    /// Transform, compositing, crop and corner pin, then the clip's
    /// keyframes. Lettering (captions, titles) has no fit, flip, blend
    /// mode, crop or corner pin: the render cannot burn them in.
    pub(super) fn transform_view(
        &mut self,
        clip: &Clip,
        lettering: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Div> {
        let state = self.state.read(cx);
        let playhead = state.playhead;
        let frame = state.bus.project().timeline.settings.rate.frame_duration();
        let at = At {
            clip,
            at: clip_time(clip, playhead),
            frame,
            inside: clip.range().contains(playhead),
        };
        let mut groups = Vec::new();

        let mut rows = Vec::new();
        if self.is_open("transform") {
            if !lettering {
                rows.push(self.fit_row(clip, cx));
            }
            rows.push(self.keyed_pair(
                "Position",
                [(Key::PosX, "X"), (Key::PosY, "Y")],
                &at,
                window,
                cx,
            ));
            rows.push(self.keyed_row(Key::Scale, &at, window, cx));
            rows.push(self.keyed_pair(
                "Stretch",
                [(Key::ScaleX, "W"), (Key::ScaleY, "H")],
                &at,
                window,
                cx,
            ));
            rows.push(self.keyed_row(Key::Rotation, &at, window, cx));
            rows.push(self.keyed_pair(
                "Skew",
                [(Key::SkewX, "X"), (Key::SkewY, "Y")],
                &at,
                window,
                cx,
            ));
            rows.push(self.keyed_pair(
                "Anchor",
                [(Key::AnchorX, "X"), (Key::AnchorY, "Y")],
                &at,
                window,
                cx,
            ));
            if !lettering {
                rows.push(self.flip_row(clip, cx));
            }
        }
        groups.push(group(self.header("transform", "Transform", cx), rows));

        let mut rows = Vec::new();
        if self.is_open("compositing") {
            rows.push(self.keyed_row(Key::Opacity, &at, window, cx));
            if !lettering {
                rows.push(self.blend_row(clip, cx));
            }
        }
        groups.push(group(self.header("compositing", "Compositing", cx), rows));

        if !lettering {
            let rows: Vec<Div> = if self.is_open("crop") {
                (0..4)
                    .map(|i| self.keyed_row(Key::Crop(i), &at, window, cx))
                    .collect()
            } else {
                Vec::new()
            };
            groups.push(group(self.header("crop", "Crop", cx), rows));
            let rows: Vec<Div> = if self.is_open("pin") {
                PIN.iter()
                    .map(|(name, x, y)| {
                        self.keyed_pair(name, [(*x, "X"), (*y, "Y")], &at, window, cx)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            groups.push(group(self.header("pin", "Corner pin", cx), rows));
        }

        let mut frames: Vec<ClipKeyframe> = clip.keyframes.clone();
        frames.sort_by_key(|k| (k.at, k.property as u8));
        if !frames.is_empty() {
            let rows = if self.is_open("keyframes") {
                self.keyframe_rows(clip, frames, at.at, frame, cx)
            } else {
                Vec::new()
            };
            groups.push(group(self.header("keyframes", "Keyframes", cx), rows));
        }
        groups
    }

    /// One transform key with its keyframe lane.
    fn keyed_row(&mut self, key: Key, at: &At, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let property = property_of(key).unwrap();
        let (keyed, nav) = self.key_nav(at, &[property], cx);
        let value = at.clip.value_at(property, at.at);
        self.param_row(key, value, keyed, Some(nav), window, cx)
    }

    /// Two transform keys on one row, keyframed together.
    fn keyed_pair(
        &mut self,
        title: &'static str,
        keys: [(Key, &'static str); 2],
        at: &At,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let properties = keys.map(|(k, _)| property_of(k).unwrap());
        let (keyed, nav) = self.key_nav(at, &properties, cx);
        let fields = keys.map(|(k, axis)| {
            let p = property_of(k).unwrap();
            (k, at.clip.value_at(p, at.at), axis)
        });
        self.pair_row(title, fields, keyed, Some(nav), window, cx)
    }

    /// Keyframe navigation for `properties` at the playhead: the diamond
    /// adds a keyframe to each (or removes theirs), the arrows seek to the
    /// previous or next keyframe of any of them. Also whether all are keyed
    /// here.
    fn key_nav(&self, at: &At, properties: &[Property], cx: &Context<Self>) -> (bool, AnyElement) {
        let clip = at.clip;
        let keyed: Vec<Property> = properties
            .iter()
            .copied()
            .filter(|p| at.keyed(*p))
            .collect();
        let animated = clip
            .keyframes
            .iter()
            .any(|k| properties.contains(&k.property));
        let state = if keyed.len() == properties.len() {
            Keyframe::On
        } else if !keyed.is_empty() {
            Keyframe::Mixed
        } else if animated {
            Keyframe::Off
        } else {
            Keyframe::None
        };
        // Commands take timeline time; keyframes store clip-relative time.
        let (id, start, when) = (clip.id, clip.start, clip.start + at.at);
        let commands: Vec<Command> = if keyed.is_empty() {
            properties
                .iter()
                .map(|&property| Command::SetKeyframe {
                    clip: id,
                    property,
                    at: when,
                    value: clip.value_at(property, at.at),
                    ease: Default::default(),
                })
                .collect()
        } else {
            keyed
                .iter()
                .map(|&property| Command::RemoveKeyframe {
                    clip: id,
                    property,
                    at: when,
                })
                .collect()
        };
        let half = Time(at.frame.0 / 2);
        let times = clip
            .keyframes
            .iter()
            .filter(|k| properties.contains(&k.property))
            .map(|k| k.at);
        let prev = times.clone().filter(|t| *t < at.at - half).max();
        let next = times.filter(|t| *t > at.at + half).min();
        let panel = cx.entity();
        let seek = |to: Option<Time>| {
            let panel = panel.clone();
            to.map(|to| {
                move |_: &mut Window, cx: &mut App| {
                    panel.update(cx, |this, cx| {
                        this.state.update(cx, |s, cx| s.seek(start + to, cx))
                    })
                }
            })
        };
        let (prev, next) = (seek(prev), seek(next));
        let nav = KeyframeNav::new(SharedString::from(format!("kf-{:?}", properties[0])), state)
            .disabled(!at.inside)
            .on_toggle(move |_, cx| {
                let commands = commands.clone();
                panel.update(cx, |this, cx| this.run(commands, "keyframe", cx))
            })
            .when_some(prev, |nav, f| nav.on_prev(f))
            .when_some(next, |nav, f| nav.on_next(f));
        (state == Keyframe::On, nav.into_any_element())
    }

    /// Fit inside the frame or fill it; not animatable.
    fn fit_row(&self, clip: &Clip, cx: &mut Context<Self>) -> Div {
        let current = clip.transform.fit;
        let (id, transform) = (clip.id, clip.transform.clone());
        row(
            label("Frame"),
            Segmented::new("fit")
                .compact()
                .items(Fit::ALL.map(fit_label))
                .selected(Fit::ALL.iter().position(|f| *f == current).unwrap_or(0))
                .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                    let fit = Fit::ALL[*ix];
                    if fit == current {
                        return;
                    }
                    let transform = Transform {
                        fit,
                        ..transform.clone()
                    };
                    this.run(
                        vec![Command::SetTransform {
                            clip: id,
                            transform,
                        }],
                        "set_transform",
                        cx,
                    );
                })),
            None,
        )
    }

    /// Mirror toggles; not animatable.
    fn flip_row(&self, clip: &Clip, cx: &mut Context<Self>) -> Div {
        let flip = clip.transform.flip;
        let toggle = |axis: usize, cx: &mut Context<Self>| {
            let (icon, tip) = match axis {
                0 => (IconName::FlipHorizontal2, "Flip horizontal"),
                _ => (IconName::FlipVertical2, "Flip vertical"),
            };
            let (id, transform) = (clip.id, clip.transform.clone());
            Button::new(("flip", axis))
                .ghost()
                .xsmall()
                .icon(icon)
                .selected(flip[axis])
                .accent(flip[axis])
                .tooltip(tip)
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
        row(
            label("Flip"),
            h_flex()
                .gap(px(2.))
                .child(toggle(0, cx))
                .child(toggle(1, cx)),
            None,
        )
    }

    /// Composite mode select.
    fn blend_row(&self, clip: &Clip, cx: &mut Context<Self>) -> Div {
        let (id, transform) = (clip.id, clip.transform.clone());
        let current = transform.blend;
        let panel = cx.entity();
        row(
            label("Blend"),
            select_field("blend", current.label()).dropdown_menu(move |mut menu, _, _| {
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
            None,
        )
    }

    /// The clip's keyframes in time order: click one to seek to it.
    fn keyframe_rows(
        &self,
        clip: &Clip,
        frames: Vec<ClipKeyframe>,
        at: Time,
        frame: Time,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let (clip_id, start) = (clip.id, clip.start);
        frames
            .into_iter()
            .filter_map(|k| Some((transform_key(k.property)?, k)))
            .enumerate()
            .map(|(i, (key, k))| {
                let current = (k.at - at).0.abs() < frame.0 / 2 + 1;
                let spec = key.spec();
                let (property, time) = (k.property, k.at);
                h_flex()
                    .id(("keyframe", i))
                    .flex_none()
                    .h(px(26.))
                    .gap(px(6.))
                    .pl(px(22.))
                    .pr(px(10.))
                    .cursor_pointer()
                    .when(current, |d| d.bg(t::AC_WASH))
                    .when(!current, |d| d.hover(|s| s.bg(t::HV)))
                    .child(div().w(px(76.)).flex_none().child(label(spec.label)))
                    .child(
                        div()
                            .flex_1()
                            .font_family(t::mono())
                            .text_size(px(11.))
                            .text_color(t::T2)
                            .child(k.at.to_string()),
                    )
                    .child(
                        div()
                            .font_family(t::mono())
                            .text_size(px(11.))
                            .child(format_value(k.value * spec.factor, spec.decimals)),
                    )
                    .child(
                        Button::new(("remove-keyframe", i))
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .tooltip("Remove")
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
            })
            .collect()
    }

    /// Whether a clip or transition is selected for the inspector to
    /// show; the Edit workspace hides it otherwise.
    pub fn has_subject(&self, cx: &App) -> bool {
        let state = self.state.read(cx);
        let transition = state.inspected_transition().is_some();
        transition || self.primary_on_track(cx).is_some()
    }

    /// Nothing selected; the Edit workspace hides the panel then.
    fn sequence_view(&self, cx: &mut Context<Self>) -> View {
        View::new(
            panel_header(self.project(cx).name.clone()),
            EmptyState::new(IconName::MousePointerClick, "No clip selected").compact(),
        )
    }
}

fn fit_label(fit: Fit) -> &'static str {
    match fit {
        Fit::Contain => "Fit",
        Fit::Cover => "Fill",
    }
}

/// The inspector key a keyframed property belongs to, if it has one.
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
        p => Key::Pin(Property::PIN.iter().position(|q| *q == p)?),
    })
}
