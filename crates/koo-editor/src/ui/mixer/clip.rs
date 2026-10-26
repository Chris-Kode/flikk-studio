//! A clip's sound for the inspector: volume with a keyframe toggle at the
//! playhead (volume automation), and its effect stack with parameters.
//! The inspector embeds it and sets the clip each render.

use super::controls::{self, ControlHost, Controls, Ctl, Range, Target};
use super::stack;
use crate::state::EditorState;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::slider::Slider;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use koo_core::command::Command;
use koo_core::model::{Clip, Property};
use koo_core::{ClipId, Time};

pub struct ClipAudioFx {
    state: Entity<EditorState>,
    controls: Controls,
    clip: Option<ClipId>,
    _observe: Subscription,
}

impl ControlHost for ClipAudioFx {
    fn controls(&mut self) -> &mut Controls {
        &mut self.controls
    }

    fn editor(&self) -> &Entity<EditorState> {
        &self.state
    }
}

impl ClipAudioFx {
    pub fn new(state: Entity<EditorState>, cx: &mut Context<Self>) -> Self {
        let _observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            controls: Controls::default(),
            clip: None,
            _observe,
        }
    }

    pub fn set_clip(&mut self, clip: Option<ClipId>, cx: &mut Context<Self>) {
        if self.clip != clip {
            self.clip = clip;
            cx.notify();
        }
    }

    /// The volume keyframe at the playhead, within half a frame.
    fn key_at(clip: &Clip, playhead: Time, half_frame: Time) -> Option<Time> {
        let rel = controls::clip_time(clip, playhead);
        clip.keyframes
            .iter()
            .filter(|k| k.property == Property::Volume)
            .find(|k| (k.at - rel).0.abs() <= half_frame.0)
            .map(|k| k.at)
    }

    fn toggle_key(state: &Entity<EditorState>, clip: Option<ClipId>, cx: &mut App) {
        let editor = state.clone();
        let state = state.read(cx);
        let (project, playhead) = (state.bus.project(), state.playhead);
        let Some((_, clip)) = clip.and_then(|id| project.timeline.clip(id)) else {
            return;
        };
        let half = Time(project.timeline.settings.rate.frame_duration().0 / 2);
        let command = match Self::key_at(clip, playhead, half) {
            Some(at) => Command::RemoveKeyframe {
                clip: clip.id,
                property: Property::Volume,
                at: clip.start + at,
            },
            None => {
                let rel = controls::clip_time(clip, playhead);
                Command::SetKeyframe {
                    clip: clip.id,
                    property: Property::Volume,
                    at: clip.start + rel,
                    value: clip.value_at(Property::Volume, rel),
                    ease: Default::default(),
                }
            }
        };
        editor.update(cx, |s, cx| {
            s.execute(command, cx);
        });
    }
}

fn header(title: &'static str, actions: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .h(px(22.))
        .justify_between()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(title)
        .child(actions)
}

/// Keyframe diamond, filled when a key sits at the playhead.
fn diamond(filled: bool, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |b, _, window, _| {
            let c = b.center();
            let r = b.size.width.min(b.size.height) / 2. - px(1.);
            let mut path = if filled {
                PathBuilder::fill()
            } else {
                PathBuilder::stroke(px(1.2))
            };
            path.move_to(point(c.x, c.y - r));
            path.line_to(point(c.x + r, c.y));
            path.line_to(point(c.x, c.y + r));
            path.line_to(point(c.x - r, c.y));
            path.close();
            if let Ok(p) = path.build() {
                window.paint_path(p, color);
            }
        },
    )
    .size(px(10.))
    .flex_none()
}

impl Render for ClipAudioFx {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let project = state.bus.project();
        let playhead = state.playhead;
        let half = Time(project.timeline.settings.rate.frame_duration().0 / 2);
        let Some(clip) = self
            .clip
            .and_then(|id| project.timeline.clip(id))
            .map(|(_, c)| c.clone())
        else {
            return v_flex();
        };
        let rel = controls::clip_time(&clip, playhead);
        let keyed = Self::key_at(&clip, playhead, half).is_some();
        let automated = clip
            .keyframes
            .iter()
            .any(|k| k.property == Property::Volume);
        let volume = clip.value_at(Property::Volume, rel);
        let slider = controls::knob(
            self,
            Ctl::ClipVolume(clip.id),
            volume,
            Range {
                min: -48.,
                max: 24.,
                log: false,
            },
            window,
            cx,
        );
        let shown = controls::shown(self, &Ctl::ClipVolume(clip.id)).unwrap_or(volume);
        let target = Target::Clip(clip.id);
        let mut effects = Vec::new();
        for (i, fx) in clip.audio.effects.iter().enumerate() {
            let rows = stack::param_rows(self, target, fx, window, cx);
            effects.push((i, fx.clone(), rows));
        }
        let theme = cx.theme();
        let (fg, muted, accent, green) = (
            theme.foreground,
            theme.muted_foreground,
            theme.accent_foreground,
            theme.green,
        );
        let (key_state, key_clip) = (self.state.clone(), self.clip);
        let key_button = div()
            .id("volume-key")
            .size(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .cursor_pointer()
            .hover(|s| s.bg(cx.theme().secondary))
            .tooltip(stack::tip(if keyed {
                "Remove keyframe"
            } else {
                "Add keyframe"
            }))
            .child(diamond(
                keyed,
                if keyed || automated { accent } else { muted },
            ))
            .on_click(move |_, _, cx| Self::toggle_key(&key_state, key_clip, cx));
        let volume_row = h_flex()
            .h(px(28.))
            .gap_2()
            .child(
                div()
                    .w(px(76.))
                    .flex_none()
                    .text_xs()
                    .text_color(muted)
                    .child("Volume"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Slider::new(&slider).w_full()),
            )
            .child(
                div()
                    .w(px(40.))
                    .flex_none()
                    .text_xs()
                    .text_right()
                    .text_color(muted)
                    .child(controls::format(shown, "dB")),
            )
            .child(key_button);

        let list = v_flex()
            .id("clip-fx")
            .gap_2()
            .min_h(px(28.))
            .children(effects.into_iter().map(|(i, fx, rows)| {
                let (on_state, rm_state, id) = (self.state.clone(), self.state.clone(), fx.id);
                v_flex()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .h(px(24.))
                            .gap_1()
                            .text_xs()
                            .text_color(if fx.enabled { fg } else { muted })
                            .child(
                                div()
                                    .id(("clip-fx-on", i))
                                    .size(px(14.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .tooltip(stack::tip(if fx.enabled {
                                        "Bypass"
                                    } else {
                                        "Enable"
                                    }))
                                    .child(div().size(px(6.)).rounded_full().bg(if fx.enabled {
                                        green
                                    } else {
                                        muted.opacity(0.4)
                                    }))
                                    .on_click(move |_, _, cx| {
                                        stack::toggle(&on_state, target, id, cx)
                                    }),
                            )
                            .child(div().flex_1().child(fx.kind.label()))
                            .child(
                                Button::new(("clip-fx-remove", i))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Close)
                                    .tooltip("Remove")
                                    .on_click(move |_, _, cx| {
                                        stack::remove(&rm_state, target, id, cx)
                                    }),
                            ),
                    )
                    .children(rows.iter().map(|r| stack::param_row(r, muted)))
            }));
        let list = stack::drop_target(list, &self.state, target, cx);
        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_0p5()
                    .child(header("Clip", div(), cx))
                    .child(volume_row),
            )
            .child(
                v_flex()
                    .gap_0p5()
                    .child(header(
                        "Effects",
                        stack::add_button("clip-add-fx", &self.state, target),
                        cx,
                    ))
                    .child(list),
            )
    }
}
