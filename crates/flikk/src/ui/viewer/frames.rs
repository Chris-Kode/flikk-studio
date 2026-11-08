//! The program picture: playback, paused frames composed off the UI thread
//! (newest request wins), their layout for picking, and frames rendered
//! ahead of a resting playhead.

use super::audio::AudioOut;
use super::player::{self, Player};
use super::{AHEAD_DELAY, AHEAD_SECONDS, PlayKey, Playback, Request, SEEK_JUMP, ViewerPanel};
use crate::render_cache::{self, Globals};
use flikk_core::Time;
use flikk_core::compose::{self, MaskView, Stills};
use flikk_core::model::Project;
use flikk_core::services::{PixelOrder, Services};
use gpui_kit::*;
use std::sync::Arc;

/// A paused frame to compose off the UI thread.
pub(super) struct Job {
    project: Arc<Project>,
    services: Services,
    request: Request,
    /// Its number among the paused frames asked for.
    seq: u64,
    t: Time,
    disk: Option<Globals>,
}

impl ViewerPanel {
    /// What the picture depends on besides the playhead.
    fn key(&self, cx: &App) -> PlayKey {
        let state = self.state.read(cx);
        PlayKey {
            version: state.bus.version(),
            preview: state.preview,
            draft: state
                .preview
                .and_then(|id| state.bus.change_set(id))
                .map_or(0, |set| set.items.len()),
            live: state.live_version,
            lettering: state.lettering_version(),
            width: self.frame_width.get(),
            masks: match state.selection.clips.first() {
                Some(clip) if self.masks.on && self.masks.tinted && self.editable(state) => {
                    MaskView::Tinted(Some(*clip))
                }
                _ => MaskView::Cut,
            },
            source: state.source_picture().map(|s| s.item().id),
        }
    }

    /// What frames on disk share, when they can stand in for `key`'s:
    /// the program, its masks not tinted.
    fn disk(&self, key: PlayKey, cx: &App) -> Option<Globals> {
        (key.masks == MaskView::Cut && key.source.is_none())
            .then(|| render_cache::globals(self.state.read(cx)))
    }

    /// Start, follow or stop the playback threads, and pick up their frames.
    pub(super) fn sync_playback(&mut self, cx: &mut Context<Self>) {
        let key = self.key(cx);
        let disk = self.disk(key, cx);
        let state = self.state.read(cx);
        let media = state.services.media.clone();
        let source = state.source_picture().is_some();
        let realtime = state.monitor_playing() && (source || state.play_speed() == 1.0);
        let (Some(media), true) = (media, realtime) else {
            self.playback = None;
            return;
        };
        self.ahead = None;
        // A paused frame still waiting is not composed once playback runs.
        self.composing.drop_next();
        self.held_click = None;
        let playhead = state.monitor_time();
        // A seek while playing restarts the audio at the new position; video
        // follows the playhead on its own.
        let jumped = self
            .playback
            .as_ref()
            .is_some_and(|p| (playhead - p.last).0.abs() > Time::from_secs_f64(SEEK_JUMP).0);
        if self.playback.as_ref().is_none_or(|p| p.key != key) {
            let project = Arc::new(state.monitor_project().clone());
            let player = Player::start(
                state.services.clone(),
                project.clone(),
                playhead,
                key,
                self.cache.clone(),
                self.shift.clone(),
                disk,
            );
            // A source plays its own sound.
            let audio = (!source).then(|| AudioOut::start(media, project.clone(), playhead));
            self.playback = Some(Playback {
                key,
                player,
                audio,
                project,
                last: playhead,
            });
        } else if let (Some(p), true, false) = (self.playback.as_mut(), jumped, source) {
            p.audio = Some(AudioOut::start(media, p.project.clone(), playhead));
        }
        let picture = self.playback.as_mut().and_then(|p| {
            p.last = playhead;
            p.player.set_target(playhead);
            p.player.take()
        });
        if let Some(picture) = picture {
            // Newer than any paused frame still composing.
            self.frames += 1;
            self.on_screen = (self.frames, None);
            self.show(Some(picture));
            self.requested = None;
        }
    }

    /// Paused (or shuttling): compose the frame under the playhead.
    pub(super) fn request_frame(&mut self, cx: &mut Context<Self>) {
        let key = self.key(cx);
        let disk = self.disk(key, cx);
        let state = self.state.read(cx);
        let t = state.monitor_time();
        let rate = state.monitor_project().timeline.settings.rate;
        let request = Request {
            key,
            frame: t.to_frame(rate),
        };
        if self.requested == Some(request) {
            return;
        }
        self.requested = Some(request);
        self.frames += 1;
        let seq = self.frames;
        // A click held for another frame's layout no longer applies.
        if self
            .held_click
            .as_ref()
            .is_some_and(|c| c.request != request)
        {
            self.held_click = None;
        }
        if self.ahead.as_ref().is_some_and(|(r, _)| r.key != key) {
            self.ahead = None;
        }
        let project = match &self.snapshot {
            Some((k, project)) if *k == key => project.clone(),
            _ => {
                let project = Arc::new(state.monitor_project().clone());
                self.snapshot = Some((key, project.clone()));
                project
            }
        };
        let cached = self
            .cache
            .lock()
            .ok()
            .and_then(|mut c| c.get(key, request.frame));
        if let Some(picture) = cached {
            // Whatever is still composing is older: don't show it.
            self.composing.drop_next();
            self.show_frame(seq, request, Some(picture));
            self.render_ahead_later(request, cx);
            self.lay_out(request, cx);
            return;
        }
        let job = Job {
            project,
            services: state.services.clone(),
            request,
            seq,
            t: Time::from_frame(request.frame, rate),
            disk,
        };
        if let Some(job) = self.composing.queue(job) {
            self.compose(job, cx);
        }
    }

    fn compose(&mut self, job: Job, cx: &mut Context<Self>) {
        self.composing.start();
        let (request, seq) = (job.request, job.seq);
        let cache = self.cache.clone();
        cx.spawn(async move |this, cx| {
            let picture = cx
                .background_executor()
                .spawn(async move {
                    let width = job.request.key.width;
                    if let Some(globals) = job.disk {
                        let plan = render_cache::plan_for(&job.project, globals);
                        let image = (width <= plan.size.0)
                            .then(|| render_cache::image(&plan, request.frame))
                            .flatten();
                        if let Some(image) = image {
                            if let Ok(mut cache) = cache.lock() {
                                let (w, h) = plan.size;
                                let bytes = w as usize * h as usize * 4;
                                cache.insert(request.key, request.frame, image.clone(), bytes);
                            }
                            return Some(image);
                        }
                    }
                    let size = compose::size_for_width(&job.project, width);
                    let mut stills = Stills::new(job.services.media.clone(), width);
                    let frame = compose::submit_program(
                        &job.project,
                        job.t,
                        size,
                        &job.services,
                        &mut stills,
                        PixelOrder::Bgra,
                        job.request.key.masks,
                    )
                    .wait();
                    let image = player::bgra_image(&frame)?;
                    if let Ok(mut cache) = cache.lock() {
                        let bytes = frame.rgba.len();
                        cache.insert(request.key, request.frame, image.clone(), bytes);
                    }
                    Some(image)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let next = this.composing.finish();
                if this.playback.is_some() {
                    return;
                }
                // Show progress toward the latest request, but nothing older
                // than the picture on screen (a later cache hit, say).
                let current = this.requested == Some(request);
                this.show_frame(seq, request, picture);
                // Latest request wins: compose the newest one asked for meanwhile.
                if let Some(next) = next {
                    this.compose(next, cx);
                } else if current {
                    this.render_ahead_later(request, cx);
                    this.lay_out(request, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Lay out the paused frame `request` (where each item lands) off the UI
    /// thread, for picking and transforming items. Not while dragging: the
    /// drag shows its own box.
    fn lay_out(&mut self, request: Request, cx: &mut Context<Self>) {
        let laid_out = self
            .gizmo
            .shapes
            .as_ref()
            .is_some_and(|(r, _)| *r == request);
        let underway = self.laying_out.is_some_and(|(r, _)| r == request);
        if laid_out || underway || self.gizmo.dragging() {
            return;
        }
        let Some((_, project)) = self.snapshot.clone().filter(|(k, _)| *k == request.key) else {
            return;
        };
        self.layouts += 1;
        let epoch = self.layouts;
        self.laying_out = Some((request, epoch));
        let services = self.state.read(cx).services.clone();
        cx.spawn(async move |this, cx| {
            let shapes = cx
                .background_executor()
                .spawn(async move {
                    let s = &project.timeline.settings;
                    let t = Time::from_frame(request.frame, s.rate);
                    let mut stills = Stills::new(services.media.clone(), request.key.width);
                    compose::layout(&project, t, (s.width, s.height), &services, &mut stills)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.laying_out.is_some_and(|(_, e)| e == epoch) {
                    this.laying_out = None;
                }
                // Layouts can finish out of order: an older one is dropped.
                this.gizmo.set_shapes(request, epoch, shapes);
                this.lay_out_shown(cx);
                this.replay_click(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Lay out the paused frame on screen, if it still needs it (an older
    /// layout finished last, or a drag held it back).
    pub(super) fn lay_out_shown(&mut self, cx: &mut Context<Self>) {
        if let Some(request) = self.requested.filter(|r| self.on_screen.1 == Some(*r)) {
            self.lay_out(request, cx);
        }
    }

    /// Once the playhead has rested on `request` for [`AHEAD_DELAY`], render
    /// the following [`AHEAD_SECONDS`] into the cache.
    fn render_ahead_later(&mut self, request: Request, cx: &mut Context<Self>) {
        if self.ahead.as_ref().is_some_and(|(r, _)| *r == request) {
            return;
        }
        self.ahead = None;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AHEAD_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.requested != Some(request) || this.playback.is_some() {
                    return;
                }
                let Some((_, project)) = this.snapshot.clone().filter(|(k, _)| *k == request.key)
                else {
                    return;
                };
                let rate = project.timeline.settings.rate;
                let end = project.timeline.duration().to_frame(rate);
                let span = (AHEAD_SECONDS * rate.num as f64 / rate.den.max(1) as f64) as i64;
                let frames = request.frame + 1..(request.frame + 1 + span).min(end);
                let disk = this.disk(request.key, cx);
                let state = this.state.read(cx);
                let player = Player::render_ahead(
                    state.services.clone(),
                    project,
                    frames,
                    request.key,
                    this.cache.clone(),
                    disk,
                );
                this.ahead = Some((request, player));
            });
        })
        .detach();
    }
}
