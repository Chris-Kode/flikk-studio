//! Fit a music clip to a length (`flikk_core::music_fit`): decoded and
//! planned off the UI thread, applied as one undoable `fit_music` edit.

use super::EditorState;
use flikk_core::ClipId;
use flikk_core::music_fit::{FitOptions, Request, Target};
use gpui_kit::*;

impl EditorState {
    pub fn fit_music(&mut self, clip: ClipId, target: Target, cx: &mut Context<Self>) {
        let request = match Request::new(self.bus.project(), clip, target, FitOptions::default()) {
            Ok(r) => r,
            Err(e) => {
                self.say(e, cx);
                return;
            }
        };
        let services = self.services.clone();
        let task = cx
            .background_executor()
            .spawn(async move { request.fit(&services) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok((command, fit)) => {
                    if this.edit(command, cx).is_some() {
                        let edits = fit.jumps.len();
                        this.status = Some(format!(
                            "Fitted: {edits} edit{}, {:.0}% match",
                            if edits == 1 { "" } else { "s" },
                            fit.quality * 100.0
                        ));
                    }
                }
                Err(e) => {
                    this.say(e, cx);
                }
            });
        })
        .detach();
    }
}
