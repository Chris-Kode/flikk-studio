//! The subject tool: which subjects the selected picture has, previews of
//! what a click would keep while hovering, and background removal.

use super::{ViewerPanel, masks};
use flikk_core::Time;
use flikk_core::compose;
use flikk_core::model::{ClipContent, MediaItem, MediaKind};
use flikk_core::subject::SubjectPrompt;
use gpui_kit::*;

/// A subject preview to find: the clip, its media and the prompt.
pub(super) struct PreviewAsk {
    clip: flikk_core::ClipId,
    item: MediaItem,
    prompt: SubjectPrompt,
}

impl ViewerPanel {
    /// Frames composed before subjects' mattes were made go, so the
    /// paused frame is composed again with them.
    pub(super) fn sync_subjects(&mut self, cx: &App) {
        let version = self.state.read(cx).subjects_version;
        if version == self.subjects_seen {
            return;
        }
        self.subjects_seen = version;
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
        self.requested = None;
        self.ahead = None;
    }

    /// What subject editing needs for `shape`'s clip at the playhead.
    pub(super) fn subjects(&self, shape: &compose::Shape, cx: &App) -> masks::Subjects {
        let state = self.state.read(cx);
        let project = state.bus.project();
        let Some((_, clip)) = project.timeline.clip(shape.clip) else {
            return masks::Subjects::default();
        };
        let frame = self.subject_media(shape.clip, cx).and_then(|item| {
            let rate = item
                .info
                .as_ref()
                .and_then(|i| i.video.as_ref())
                .map(|v| v.rate)
                .unwrap_or_default();
            let at = match item.kind {
                MediaKind::Image => Time::ZERO,
                _ => clip
                    .to_source(state.playhead)?
                    .max(Time::ZERO)
                    .floor_to_frame(rate),
            };
            Some((at, rate))
        });
        masks::Subjects {
            frame,
            mattes: compose::subject_mattes(project, &state.services, clip, state.playhead),
            people: compose::person_mattes(project, &state.services, clip, state.playhead),
        }
    }

    /// The media under `clip` when it is a picture from the pool: what a
    /// subject mask can pick in.
    fn subject_media(&self, clip: flikk_core::ClipId, cx: &App) -> Option<MediaItem> {
        let project = self.state.read(cx).bus.project();
        let (_, clip) = project.timeline.clip(clip)?;
        let ClipContent::Media { media, .. } = &clip.content else {
            return None;
        };
        project
            .media(*media)
            .filter(|m| m.kind != MediaKind::Audio)
            .cloned()
    }

    /// Show what the subject tool would keep for `ask`, found off the UI
    /// thread; the newest ask wins.
    fn preview_subject(&mut self, ask: PreviewAsk, cx: &mut Context<Self>) {
        let shown = self
            .masks
            .hover
            .as_ref()
            .is_some_and(|h| h.clip == ask.clip && h.prompt == ask.prompt);
        if shown {
            return;
        }
        let Some(ask) = self.previewing.queue(ask) else {
            return;
        };
        let Some(service) = self.state.read(cx).services.subjects.clone() else {
            return;
        };
        self.previewing.start();
        cx.spawn(async move |this, cx| {
            let PreviewAsk { clip, item, prompt } = ask;
            let asked = prompt.clone();
            let outlines = cx
                .background_executor()
                .spawn(async move {
                    if prompt.is_empty() {
                        // Only to encode the frame for the clicks to come.
                        let _ = service.preview(&item, &prompt);
                        return None;
                    }
                    service.preview(&item, &prompt).ok().map(|m| m.outlines())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let next = this.previewing.finish();
                let wanted = this.hovering && this.masks.tool == Some(masks::Tool::Subject);
                if let Some(outlines) = outlines.filter(|_| wanted) {
                    this.masks.hover = Some(masks::Hover {
                        clip,
                        prompt: asked,
                        outlines,
                    });
                }
                if let Some(next) = next {
                    this.preview_subject(next, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// With the subject tool over the selected picture: preview what a
    /// click at `p` would keep.
    pub(super) fn hover_subject(
        &mut self,
        p: Point<Pixels>,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        if self.masks.tool != Some(masks::Tool::Subject) {
            return;
        }
        let screen = self.screen(cx);
        let state = self.state.read(cx);
        let shape = (self.editable(state) && state.preview.is_none())
            .then(|| self.gizmo.active(&state.selection.clips, self.requested))
            .flatten()
            .filter(|s| !s.lettering);
        let ask = shape.and_then(|shape| {
            let subjects = self.subjects(&shape, cx);
            let project = self.state.read(cx).bus.project();
            let prompt = self
                .masks
                .hover_prompt(p, modifiers, &screen, project, &shape, &subjects)?;
            Some(PreviewAsk {
                clip: shape.clip,
                item: self.subject_media(shape.clip, cx)?,
                prompt,
            })
        });
        self.hovering = ask.is_some();
        match ask {
            Some(ask) => self.preview_subject(ask, cx),
            None => {
                self.previewing.drop_next();
                if self.masks.hover.take().is_some() {
                    cx.notify();
                }
            }
        }
    }

    /// Pick the subject tool, when its model is here; start encoding the
    /// frame under the playhead for the first click.
    /// Keep the people in the active clip and cut its background away: a
    /// person mask, or the one it has, selected.
    pub(super) fn remove_background(&mut self, cx: &mut Context<Self>) {
        if !self.state.update(cx, |s, cx| s.people_ready(cx)) {
            return;
        }
        let state = self.state.read(cx);
        let Some(shape) = self
            .gizmo
            .active(&state.selection.clips, self.requested)
            .filter(|s| !s.lettering)
        else {
            return;
        };
        let Some((_, clip)) = state.bus.project().timeline.clip(shape.clip) else {
            return;
        };
        if self.subject_media(shape.clip, cx).is_none() {
            return;
        }
        if let Some(mask) = clip.masks.iter().find(|m| m.is_person()) {
            self.masks.selected = Some(mask.id);
            return;
        }
        let mut masks = clip.masks.clone();
        let mask = flikk_core::mask::Mask::person();
        self.masks.selected = Some(mask.id);
        masks.push(mask);
        let command = flikk_core::command::Command::SetMasks {
            clip: shape.clip,
            masks,
        };
        self.state
            .update(cx, |s, cx| s.edit_batch(vec![command], "set_masks", cx));
    }

    pub(super) fn pick_subject_tool(&mut self, cx: &mut Context<Self>) {
        if !self.state.update(cx, |s, cx| s.subjects_ready(cx)) {
            return;
        }
        self.masks.tool = Some(masks::Tool::Subject);
        let state = self.state.read(cx);
        let shape = self
            .gizmo
            .active(&state.selection.clips, self.requested)
            .filter(|s| !s.lettering);
        let warm = shape.and_then(|shape| {
            let (at, _) = self.subjects(&shape, cx).frame?;
            Some(PreviewAsk {
                clip: shape.clip,
                item: self.subject_media(shape.clip, cx)?,
                prompt: SubjectPrompt {
                    at,
                    ..Default::default()
                },
            })
        });
        if let Some(ask) = warm {
            self.preview_subject(ask, cx);
        }
    }
}
