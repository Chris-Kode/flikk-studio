//! Picking a depth effect's focus: with "Pick focus" armed in the
//! inspector, a click on the clip's picture makes that frame's depth map
//! (off the UI thread) and focuses the effect on the distance there,
//! keyed at the playhead when its focus has keyframes (a rack focus).

use super::ViewerPanel;
use super::gizmo::Screen;
use flikk_core::depth;
use flikk_core::{ClipId, EffectId, Time, TimeRange};
use gpui_kit::*;
use std::sync::atomic::AtomicBool;

impl ViewerPanel {
    /// The click at `p` picks the focus of `effect` on `clip`, when it
    /// lands on the clip's picture; the pick ends either way.
    pub(super) fn pick_focus(
        &mut self,
        (clip, effect): (ClipId, EffectId),
        p: Point<Pixels>,
        screen: &Screen,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            s.depth_pick = None;
            cx.notify();
        });
        let Some(shape) = self.gizmo.active(&[clip], self.requested) else {
            return;
        };
        let Some([u, v]) = shape.placement.to_picture(screen.to_sequence(p)) else {
            return;
        };
        let flip = shape.placement.flip;
        let uv = [
            if flip[0] { 1.0 - u } else { u },
            if flip[1] { 1.0 - v } else { v },
        ];
        if !uv.iter().all(|c| (0.0..=1.0).contains(c)) {
            return;
        }
        let state = self.state.read(cx);
        let playhead = state.playhead;
        let project = state.bus.project();
        let Some((track, c)) = project.timeline.clip(clip) else {
            return;
        };
        let (Some((item, _)), Some(source)) = (
            depth::source(project, track, c, c.range()),
            c.to_source(playhead),
        ) else {
            return;
        };
        let Some(service) = state.services.depth.clone() else {
            return;
        };
        if !c.range().contains(playhead) || service.available().is_err() {
            return;
        }
        let item = item.clone();
        cx.spawn(async move |this, cx| {
            let distance = cx
                .background_executor()
                .spawn(async move {
                    let frame = TimeRange::new(source, source + Time(1));
                    let cancel = AtomicBool::new(false);
                    // The user clicked and waits on it.
                    let asked = flikk_os::gpu::Level::Asked;
                    service
                        .complete(&item, frame, asked, &|_| {}, &cancel)
                        .ok()?;
                    Some(service.depth(&item, source)?.around(uv))
                })
                .await;
            let Some(distance) = distance else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| {
                    let command = s.bus.project().timeline.clip(clip).and_then(|(_, c)| {
                        depth::focus_command(c, effect, playhead, distance, false)
                    });
                    if let Some(command) = command {
                        s.edit(command, cx);
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}
