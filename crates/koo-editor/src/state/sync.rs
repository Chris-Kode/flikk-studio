//! Syncing by sound from the timeline and the media pool: the recordings
//! are heard off the UI thread ([`koo_core::sync::measure_group`]), then
//! the edit is applied as one undo step.

use super::EditorState;
use gpui_kit::*;
use koo_core::sync::{self, Measured, Synced, plan};
use koo_core::{ClipId, MediaId};

impl EditorState {
    /// The recordings among `clips` that can be synced, reference first;
    /// none unless there are two.
    pub fn sync_recordings(&self, clips: &[ClipId]) -> Vec<plan::Recording> {
        let found = plan::recordings(self.bus.project(), clips);
        if found.len() < 2 { Vec::new() } else { found }
    }

    /// Whether syncing `clips` could leave only a recorder's sound.
    pub fn sync_has_recorder(&self, clips: &[ClipId]) -> bool {
        let found = self.sync_recordings(clips);
        found.iter().any(|r| r.sound_only) && found.iter().any(|r| !r.sound_only)
    }

    /// Move the followers among `clips` to the reference by their sound.
    pub fn sync_clips(&mut self, clips: Vec<ClipId>, mute_camera: bool, cx: &mut Context<Self>) {
        let recordings = self.sync_recordings(&clips);
        let Some(media) = self.services.media.clone() else {
            return;
        };
        let project = self.bus.project();
        let sources: Option<Vec<sync::Source>> = recordings
            .iter()
            .map(|r| sync::source(project, r.media, r.stream))
            .collect();
        let Some(sources) = sources.filter(|s| s.len() >= 2) else {
            return;
        };
        self.status = Some("Syncing…".into());
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { sync::measure_group(media.as_ref(), &sources[0], &sources[1..]) });
        cx.spawn(async move |this, cx| {
            let measured = task.await;
            let _ = this.update(cx, |this, cx| {
                let synced = measured.map(|results| {
                    sync::clips_command(this.bus.project(), &recordings, results, mute_camera)
                });
                this.finish_sync(synced, "sync_clips", cx);
            });
        })
        .detach();
    }

    /// Lay the pool items `media` on the timeline from the playhead, in
    /// sync by their sound, each on a track of its own.
    pub fn sync_media(&mut self, media: Vec<MediaId>, mute_camera: bool, cx: &mut Context<Self>) {
        let Some(service) = self.services.media.clone() else {
            return;
        };
        let project = self.bus.project();
        let order = sync::pool_order(project, &media);
        let sources: Option<Vec<sync::Source>> =
            order.iter().map(|m| sync::source(project, *m, 0)).collect();
        let Some(sources) = sources.filter(|s| s.len() >= 2) else {
            return;
        };
        let at = self.playhead;
        self.status = Some("Syncing…".into());
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            sync::measure_group(service.as_ref(), &sources[0], &sources[1..])
        });
        cx.spawn(async move |this, cx| {
            let measured = task.await;
            let _ = this.update(cx, |this, cx| {
                let synced = measured.map(|results| {
                    sync::place_command(this.bus.project(), &order, at, results, mute_camera)
                });
                this.finish_sync(synced, "place_synced", cx);
            });
        })
        .detach();
    }

    fn finish_sync(&mut self, synced: Result<Synced, String>, label: &str, cx: &mut Context<Self>) {
        let Synced { command, results } = match synced {
            Ok(s) => s,
            Err(e) => {
                self.status = Some(format!("Could not sync: {e}"));
                cx.notify();
                return;
            }
        };
        let failed: Vec<String> = results.iter().filter_map(|r| r.clone().err()).collect();
        let matched: Vec<&Measured> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
        let Some(command) = command else {
            self.status = Some(format!("No match · {}", failed.join(" · ")));
            cx.notify();
            return;
        };
        if self.edit_batch(vec![command], label, cx).is_none() {
            // The bus said why.
            return;
        }
        // The timeline shows the rest; only drift corrected and what
        // didn't match are worth a line.
        let mut notes = Vec::new();
        if !failed.is_empty() {
            notes.push(format!(
                "Synced {} of {}",
                matched.len() + 1,
                results.len() + 1
            ));
        }
        if let Some(d) = matched.iter().map(|m| m.drift).find(|d| *d != 0.0) {
            notes.push(format!("drift {:+.4}% corrected", d * 100.0));
        }
        notes.extend(failed);
        self.status = (!notes.is_empty()).then(|| notes.join(" · "));
        cx.notify();
    }
}
