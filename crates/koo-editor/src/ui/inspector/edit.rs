//! The selected clip's properties (see `clip.rs`); for
//! picture clips, transform with per-property keyframe toggles at the
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
    /// The selected clip's properties for its kind, or the sequence format
    /// when nothing is selected.
    pub(super) fn edit_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.primary_on_track(cx) {
            Some((track, clip)) => self.clip_view(track, clip, window, cx),
            None => self.sequence_view(cx),
        }
    }

    /// Transform with keyframe toggles, then the clip's keyframes.
    pub(super) fn transform_view(
        &mut self,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let clip = clip.clone();
        let state = self.state.read(cx);
        let playhead = state.playhead;
        let frame = state.bus.project().timeline.settings.rate.frame_duration();
        let at = clip_time(&clip, playhead);
        let inside = clip.range().contains(playhead);

        let mut rows = Vec::new();
        for key in TRANSFORM {
            let property = property_of(key).unwrap();
            let value = clip.value_at(property, at);
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
        Property::Rotation => Key::Rotation,
        Property::Opacity => Key::Opacity,
        _ => return None,
    })
}
