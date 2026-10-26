//! Edit workspace: transform with per-property keyframe toggles at the
//! playhead, and the clip's keyframes (PRD E5).

use super::param::{Key, format_value};
use super::{InspectorPanel, clip_time, property_of, section};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::Time;
use koo_core::command::Command;
use koo_core::model::{Clip, Keyframe, Property};

const TRANSFORM: [Key; 5] = [
    Key::PosX,
    Key::PosY,
    Key::Scale,
    Key::Rotation,
    Key::Opacity,
];

impl InspectorPanel {
    pub(super) fn edit_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(clip) = self.primary(cx) else {
            return self.sequence_view(cx);
        };
        let state = self.state.read(cx);
        let playhead = state.playhead;
        let frame = state.bus.project().timeline.settings.rate.frame_duration();
        let at = clip_time(&clip, playhead);
        let inside = clip.range().contains(playhead);

        let mut rows = Vec::new();
        for key in TRANSFORM {
            let property = property_of(key).unwrap();
            let value = value_at(&clip, property, at);
            let row = self.param(key, value, window, cx).row(cx);
            let toggle = self.key_toggle(&clip, property, at, frame, inside, cx);
            rows.push(row.child(toggle));
        }
        let mut frames: Vec<Keyframe> = clip.keyframes.clone();
        frames.sort_by_key(|k| (k.at, k.property as u8));
        let transform = section("Transform", cx).children(rows);
        let list = (!frames.is_empty()).then(|| self.keyframe_list(&clip, frames, at, frame, cx));
        v_flex()
            .gap_4()
            .child(transform)
            .children(list)
            .into_any_element()
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
        let (clip_id, value) = (clip.id, value_at(clip, property, at));
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
                            at,
                        }
                    } else {
                        Command::SetKeyframe {
                            clip: clip_id,
                            property,
                            at,
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
        section("Keyframes", cx).children(frames.into_iter().enumerate().map(|(i, k)| {
            let current = (k.at - at).0.abs() < frame.0 / 2 + 1;
            let key = transform_key(k.property);
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
                                    at: time,
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

fn transform_key(p: Property) -> Key {
    match p {
        Property::PositionX => Key::PosX,
        Property::PositionY => Key::PosY,
        Property::Scale => Key::Scale,
        Property::Rotation => Key::Rotation,
        Property::Opacity => Key::Opacity,
    }
}

/// Property value at clip-relative `at`: the base transform, or a linear
/// read of its keyframes (easing is the core stream's evaluator's job).
fn value_at(clip: &Clip, property: Property, at: Time) -> f32 {
    let t = &clip.transform;
    let base = match property {
        Property::PositionX => t.position[0],
        Property::PositionY => t.position[1],
        Property::Scale => t.scale,
        Property::Rotation => t.rotation_deg,
        Property::Opacity => t.opacity,
    };
    let mut keys: Vec<&Keyframe> = clip
        .keyframes
        .iter()
        .filter(|k| k.property == property)
        .collect();
    keys.sort_by_key(|k| k.at);
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
        return base;
    };
    if at <= first.at {
        return first.value;
    }
    if at >= last.at {
        return last.value;
    }
    for pair in keys.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a.at <= at && at <= b.at {
            let span = (b.at - a.at).0.max(1) as f32;
            let f = (at - a.at).0 as f32 / span;
            return a.value + (b.value - a.value) * f;
        }
    }
    base
}
