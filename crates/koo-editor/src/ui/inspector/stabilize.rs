//! A video clip's stabilization (see `koo_core::stabilize`): a switch,
//! and while it is on the smoothness, Smooth or Locked, and the crop.
//! Turning it on starts measuring the camera's path, shown under the
//! title until it is done.

use super::param::Key;
use super::transition::labeled;
use super::{InspectorPanel, section_with};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, h_flex};
use gpui_kit::*;
use koo_core::command::Command;
use koo_core::model::{Clip, MediaKind};
use koo_core::stabilize::{MotionStatus, Stabilization, StabilizeCrop, StabilizeMode};

impl InspectorPanel {
    /// `None` unless `clip` shows a video from the pool.
    pub(super) fn stabilize_view(
        &mut self,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let state = self.state.read(cx);
        let item = state.bus.project().media(clip.media()?)?.clone();
        if item.kind != MediaKind::Video {
            return None;
        }
        let stabilizer = state.services.stabilizer.clone();
        let settings = clip.stabilization.clone();
        let id = clip.id;
        let set = move |s: Stabilization| Command::SetStabilization {
            clips: vec![id],
            stabilization: s,
        };
        let toggle = {
            let (settings, item, stabilizer) = (settings.clone(), item.clone(), stabilizer.clone());
            Switch::new("stabilize")
                .small()
                .checked(settings.enabled)
                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                    if *on && let Some(s) = &stabilizer {
                        s.measure(&item);
                    }
                    let s = Stabilization {
                        enabled: *on,
                        ..settings.clone()
                    };
                    this.run(vec![set(s)], "set_stabilization", cx);
                }))
        };
        let view = section_with("Stabilize", toggle, cx);
        if !settings.enabled {
            return Some(view);
        }
        let theme = cx.theme();
        let status = stabilizer.as_ref().map(|s| s.status(&item));
        let progress = match &status {
            Some(MotionStatus::Queued) => Some(0.0),
            Some(MotionStatus::Measuring(f)) => Some(*f),
            _ => None,
        }
        .map(|fraction| {
            div()
                .h(px(2.))
                .w_full()
                .rounded_sm()
                .bg(theme.secondary)
                .child(div().h_full().w(relative(fraction)).bg(theme.primary))
        });
        let error = match status {
            Some(MotionStatus::Failed(e)) => Some(
                div()
                    .text_xs()
                    .text_color(theme.red)
                    .child(SharedString::from(e)),
            ),
            _ => None,
        };
        let choice = |label: &'static str,
                      options: Vec<(&'static str, bool, Stabilization)>,
                      cx: &mut Context<Self>| {
            let buttons = options
                .into_iter()
                .enumerate()
                .map(|(ix, (name, selected, s))| {
                    Button::new(SharedString::from(format!("stabilize-{label}-{ix}")))
                        .ghost()
                        .xsmall()
                        .selected(selected)
                        .label(name)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !selected {
                                this.run(vec![set(s.clone())], "set_stabilization", cx);
                            }
                        }))
                });
            labeled(label, h_flex().gap_1().children(buttons), cx)
        };
        let modes = StabilizeMode::ALL
            .iter()
            .map(|&mode| {
                let s = Stabilization {
                    mode,
                    ..settings.clone()
                };
                (mode.label(), mode == settings.mode, s)
            })
            .collect();
        let crops = StabilizeCrop::ALL
            .iter()
            .map(|&crop| {
                let s = Stabilization {
                    crop,
                    ..settings.clone()
                };
                (crop.label(), crop == settings.crop, s)
            })
            .collect();
        let mode = choice("Mode", modes, cx);
        let crop = choice("Crop", crops, cx);
        let smoothness = (settings.mode == StabilizeMode::Smooth).then(|| {
            self.param(Key::Smoothness, settings.smoothness, window, cx)
                .row(cx)
        });
        Some(
            view.children(progress)
                .children(error)
                .child(mode)
                .children(smoothness)
                .child(crop),
        )
    }
}
