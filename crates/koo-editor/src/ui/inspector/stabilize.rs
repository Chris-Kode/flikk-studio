//! A video clip's stabilization (see `koo_core::stabilize`): a switch,
//! and while it is on the smoothness, Smooth or Locked, and the crop.
//! Turning it on starts measuring the camera's path, shown under the
//! title until it is done.

use super::param::Key;
use super::{InspectorPanel, error_line, group, label, row, wide};
use crate::ui::kit::{Progress, Segmented, Switch};
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
        let header = self.header("stabilize", "Stabilize", cx).child(toggle);
        if !settings.enabled || !self.is_open("stabilize") {
            return Some(group(header, Vec::<Div>::new()));
        }
        let status = stabilizer.as_ref().map(|s| s.status(&item));
        let progress = match &status {
            Some(MotionStatus::Queued) => Some(0.0),
            Some(MotionStatus::Measuring(f)) => Some(*f),
            _ => None,
        }
        .map(|fraction| wide(Progress::new().value(fraction)));
        let error = match status {
            Some(MotionStatus::Failed(e)) => Some(error_line(e)),
            _ => None,
        };
        let choice = |name: &'static str,
                      options: Vec<(&'static str, bool, Stabilization)>,
                      cx: &mut Context<Self>| {
            let selected = options.iter().position(|(_, on, _)| *on).unwrap_or(0);
            let labels: Vec<&'static str> = options.iter().map(|(l, _, _)| *l).collect();
            let picks: Vec<Stabilization> = options.into_iter().map(|(_, _, s)| s).collect();
            row(
                label(name),
                Segmented::new(SharedString::from(format!("stabilize-{name}")))
                    .compact()
                    .fill()
                    .items(labels)
                    .selected(selected)
                    .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                        if *ix != selected
                            && let Some(s) = picks.get(*ix)
                        {
                            this.run(vec![set(s.clone())], "set_stabilization", cx);
                        }
                    })),
                None,
            )
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
            self.param_row(
                Key::Smoothness,
                settings.smoothness,
                false,
                None,
                window,
                cx,
            )
        });
        let rows: Vec<Div> = progress
            .into_iter()
            .chain(error)
            .chain([mode])
            .chain(smoothness)
            .chain([crop])
            .collect();
        Some(group(header, rows))
    }
}
