//! Program monitor: `EditorState::display_project()` at the playhead, so a
//! previewed change set shows its draft. The picture is the program frame
//! from `koo_core::compose`, the same compositor the render and the agent
//! use (effects, transitions, generators, blend modes, grades, transforms):
//! paused frames are composed off-thread from single decodes (one at a
//! time, the latest request wins), playback composes from streams and
//! mixes audio in background threads. Composed frames are kept
//! ([`cache`]), and a playhead that rests renders the next seconds ahead
//! into them. Captions and titles are drawn on top in their export styles.

mod audio;
mod cache;
mod player;

use super::lettering::{self, Lettering, Place};
use super::timeline::glyph::{Glyph, glyph};
use super::timeline::{
    NextCut, NextFrame, PrevCut, PrevFrame, ShuttleForward, ShuttleReverse, ShuttleStop,
};
use crate::app::TogglePlay;
use crate::state::EditorState;
use audio::AudioOut;
use cache::{FrameCache, SharedCache};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use koo_core::compose::{self, Stills};
use koo_core::memory::CaptionStyle;
use koo_core::model::{ClipContent, Project, TrackKind};
use koo_core::query::topmost_video_clip;
use koo_core::services::{PixelOrder, Services};
use koo_core::{ChangeSetId, Time};
use koo_deliver::captions;
use player::Player;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::time::Duration;

/// Largest frame width requested from the media engine.
const MAX_FRAME_WIDTH: u32 = 1280;
const CONTROLS_HEIGHT: f32 = 30.;
/// A playhead move larger than this between renders is a seek, in seconds.
const SEEK_JUMP: f64 = 0.35;
/// How long the playhead rests before the frames after it render ahead.
const AHEAD_DELAY: Duration = Duration::from_millis(500);
/// Seconds rendered ahead of a resting playhead.
const AHEAD_SECONDS: f64 = 4.0;

/// What a running playback was started from; a change restarts it.
#[derive(Clone, Copy, PartialEq)]
struct PlayKey {
    version: u64,
    preview: Option<ChangeSetId>,
    /// Items in the previewed change set, which grows without a version.
    draft: usize,
    width: u32,
}

/// A paused frame: what it shows and the sequence frame index.
#[derive(Clone, Copy, PartialEq)]
struct Request {
    key: PlayKey,
    frame: i64,
}

/// A paused frame to compose off the UI thread.
struct Job {
    project: Arc<Project>,
    services: Services,
    request: Request,
    t: Time,
}

struct Playback {
    key: PlayKey,
    player: Player,
    audio: AudioOut,
    project: Arc<Project>,
    /// Playhead at the previous render, to tell seeks from playback.
    last: Time,
}

pub struct ViewerPanel {
    state: Entity<EditorState>,
    focus: FocusHandle,
    /// The program picture on screen.
    picture: Option<Arc<RenderImage>>,
    /// Paused frame last asked for.
    requested: Option<Request>,
    /// The project paused frames are composed from, by key.
    snapshot: Option<(PlayKey, Arc<Project>)>,
    /// A paused frame is being composed.
    composing: bool,
    /// The newest paused frame asked for while composing.
    pending: Option<Job>,
    /// Images replaced since the last paint, to free from the sprite atlas.
    retired: Vec<Arc<RenderImage>>,
    playback: Option<Playback>,
    /// Width of the program image in device px, from the last layout.
    frame_width: Rc<Cell<u32>>,
    /// Composed frames, shared with the playback threads.
    cache: SharedCache,
    /// Playback preview resolution, kept across playbacks (see
    /// `player::Quality`).
    shift: Arc<AtomicU8>,
    /// Frames after the resting playhead being rendered into `cache`.
    ahead: Option<(Request, Player)>,
    _observe: Subscription,
}

impl ViewerPanel {
    pub fn new(state: Entity<EditorState>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let _observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            focus: cx.focus_handle(),
            picture: None,
            requested: None,
            snapshot: None,
            composing: false,
            pending: None,
            retired: Vec::new(),
            playback: None,
            frame_width: Rc::new(Cell::new(640)),
            cache: FrameCache::shared(),
            shift: Arc::new(AtomicU8::new(0)),
            ahead: None,
            _observe,
        }
    }

    fn show(&mut self, picture: Option<Arc<RenderImage>>) {
        if let Some(old) = std::mem::replace(&mut self.picture, picture) {
            self.retired.push(old);
        }
    }

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
            width: self.frame_width.get(),
        }
    }

    /// Start, follow or stop the playback threads, and pick up their frames.
    fn sync_playback(&mut self, cx: &mut Context<Self>) {
        let key = self.key(cx);
        let state = self.state.read(cx);
        let media = state.services.media.clone();
        let realtime = state.playing && state.play_speed == 1.0;
        let (Some(media), true) = (media, realtime) else {
            self.playback = None;
            return;
        };
        self.ahead = None;
        let playhead = state.playhead;
        // A seek while playing restarts the audio at the new position; video
        // follows the playhead on its own.
        let jumped = self
            .playback
            .as_ref()
            .is_some_and(|p| (playhead - p.last).0.abs() > Time::from_secs_f64(SEEK_JUMP).0);
        if self.playback.as_ref().is_none_or(|p| p.key != key) {
            let project = Arc::new(state.display_project().clone());
            let player = Player::start(
                state.services.clone(),
                project.clone(),
                playhead,
                key,
                self.cache.clone(),
                self.shift.clone(),
            );
            let audio = AudioOut::start(media, project.clone(), playhead);
            self.playback = Some(Playback {
                key,
                player,
                audio,
                project,
                last: playhead,
            });
        } else if let (Some(p), true) = (self.playback.as_mut(), jumped) {
            p.audio = AudioOut::start(media, p.project.clone(), playhead);
        }
        let picture = self.playback.as_mut().and_then(|p| {
            p.last = playhead;
            p.player.set_target(playhead);
            p.player.take()
        });
        if let Some(picture) = picture {
            self.show(Some(picture));
            self.requested = None;
        }
    }

    /// Paused (or shuttling): compose the frame under the playhead.
    fn request_frame(&mut self, cx: &mut Context<Self>) {
        let key = self.key(cx);
        let state = self.state.read(cx);
        let t = state.playhead;
        let rate = state.display_project().timeline.settings.rate;
        let request = Request {
            key,
            frame: t.to_frame(rate),
        };
        if self.requested == Some(request) {
            return;
        }
        self.requested = Some(request);
        if self.ahead.as_ref().is_some_and(|(r, _)| r.key != key) {
            self.ahead = None;
        }
        let project = match &self.snapshot {
            Some((k, project)) if *k == key => project.clone(),
            _ => {
                let project = Arc::new(state.display_project().clone());
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
            self.pending = None;
            self.show(Some(picture));
            self.render_ahead_later(request, cx);
            return;
        }
        let job = Job {
            project,
            services: state.services.clone(),
            request,
            t: Time::from_frame(request.frame, rate),
        };
        if self.composing {
            self.pending = Some(job);
        } else {
            self.compose(job, cx);
        }
    }

    fn compose(&mut self, job: Job, cx: &mut Context<Self>) {
        self.composing = true;
        let request = job.request;
        let cache = self.cache.clone();
        cx.spawn(async move |this, cx| {
            let picture = cx
                .background_executor()
                .spawn(async move {
                    let width = job.request.key.width;
                    let size = compose::size_for_width(&job.project, width);
                    let mut stills = Stills::new(job.services.media.clone(), width);
                    let frame = compose::submit_program(
                        &job.project,
                        job.t,
                        size,
                        &job.services,
                        &mut stills,
                        PixelOrder::Bgra,
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
                this.composing = false;
                // Show progress toward the latest request, but nothing older
                // than a frame already shown from the cache.
                let current = this.requested == Some(request);
                if this.playback.is_none() && (current || this.pending.is_some()) {
                    this.show(picture);
                }
                // Latest request wins: compose the newest one asked for meanwhile.
                if let Some(next) = this.pending.take() {
                    this.compose(next, cx);
                } else if current {
                    this.render_ahead_later(request, cx);
                }
                cx.notify();
            });
        })
        .detach();
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
                let state = this.state.read(cx);
                let Some((_, project)) = this.snapshot.clone().filter(|(k, _)| *k == request.key)
                else {
                    return;
                };
                let rate = project.timeline.settings.rate;
                let end = project.timeline.duration().to_frame(rate);
                let span = (AHEAD_SECONDS * rate.num as f64 / rate.den.max(1) as f64) as i64;
                let frames = request.frame + 1..(request.frame + 1 + span).min(end);
                let player = Player::render_ahead(
                    state.services.clone(),
                    project,
                    frames,
                    request.key,
                    this.cache.clone(),
                );
                this.ahead = Some((request, player));
            });
        })
        .detach();
    }

    fn controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let project = state.display_project();
        let rate = project.timeline.settings.rate;
        let t = state.playhead;
        let theme = cx.theme();
        let (fg, muted, border) = (theme.foreground, theme.muted_foreground, theme.border);
        let source = topmost_video_clip(&project.timeline, t).and_then(|(_, clip)| {
            let at = clip.to_source(t)?;
            let item = project.media(clip.media()?)?;
            let rate = item
                .info
                .as_ref()
                .and_then(|i| i.video.as_ref())
                .map(|v| v.rate)
                .unwrap_or(rate);
            let name = item.path.file_name()?.to_string_lossy().into_owned();
            Some((at.timecode(rate), name))
        });
        let playing = state.playing;
        let entity = self.state.clone();
        h_flex()
            .flex_none()
            .h(px(CONTROLS_HEIGHT))
            .px_2()
            .gap_2()
            .border_t_1()
            .border_color(border)
            .text_xs()
            .child(
                Button::new("viewer-play")
                    .ghost()
                    .xsmall()
                    .tooltip(if playing {
                        "Pause (Space)"
                    } else {
                        "Play (Space)"
                    })
                    .child(glyph(
                        if playing { Glyph::Pause } else { Glyph::Play },
                        px(13.),
                        fg,
                    ))
                    .on_click(move |_, _, cx| entity.update(cx, |s, cx| s.toggle_play(cx))),
            )
            .child(div().text_color(fg).child(t.timecode(rate)))
            .child(div().flex_1())
            .children(source.map(|(tc, name)| {
                h_flex()
                    .gap_2()
                    .min_w_0()
                    .text_color(muted)
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(name),
                    )
                    .child(div().flex_none().child(tc))
            }))
    }
}

impl Focusable for ViewerPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Captions and titles at `t`, as the render draws them.
fn lettering(project: &Project, styles: &[CaptionStyle], t: Time) -> Vec<Lettering> {
    let mut out = Vec::new();
    for track in project.timeline.tracks.iter().filter(|t| !t.mix.mute) {
        let Some(clip) = track.clip_at(t) else {
            continue;
        };
        match (&clip.content, track.kind) {
            (ClipContent::Title { text, template }, _) => {
                out.push(Lettering::title(text, template.contains("lower")));
            }
            (ClipContent::Caption { text, words, .. }, TrackKind::Caption)
                if !text.trim().is_empty() =>
            {
                let style = captions::style_for(track, styles);
                let duration = clip.duration.as_secs_f64();
                let at = (t - clip.start).as_secs_f64();
                let starts = captions::word_starts(text, words, duration);
                out.push(Lettering {
                    lines: captions::wrap(text, &style)
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                    pose: captions::pose_at(&style, duration, at),
                    spoken: captions::spoken(&starts, at),
                    style: style.into_owned(),
                    place: Place::Caption,
                });
            }
            _ => {}
        }
    }
    out
}

impl Render for ViewerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
        self.sync_playback(cx);
        if self.playback.is_none() {
            self.request_frame(cx);
        }

        let state = self.state.read(cx);
        let project = state.display_project();
        let t = state.playhead;
        let settings = project.timeline.settings.clone();
        let aspect = settings.width.max(1) as f32 / settings.height.max(1) as f32;
        let picture = self.picture.clone();
        let letters = lettering(project, &state.caption_styles(), t);
        let bg = cx.theme().background;
        let width_cell = self.frame_width.clone();

        let program = canvas(
            move |bounds, window, _| {
                let frame = fit(bounds, aspect, px(8.));
                let device = f32::from(frame.size.width) * window.scale_factor();
                // Round so small resizes don't refetch.
                let width = ((device / 160.).ceil() * 160.) as u32;
                width_cell.set(width.clamp(160, MAX_FRAME_WIDTH));
                frame
            },
            move |bounds, frame, window, cx| {
                window.paint_quad(fill(bounds, bg));
                window.paint_quad(fill(frame, black()));
                window.with_content_mask(Some(ContentMask { bounds: frame }), |window| {
                    if let Some(image) = picture {
                        let _ =
                            window.paint_image(frame, frame, Corners::default(), image, 0, false);
                    }
                    let k = f32::from(frame.size.height) / 1080.;
                    for l in &letters {
                        lettering::paint(l, frame, k, window, cx);
                    }
                });
            },
        )
        .flex_1()
        .min_h_0()
        .w_full();

        v_flex()
            .size_full()
            .bg(bg)
            .track_focus(&self.focus)
            .key_context("Viewer")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
            )
            .on_action(cx.listener(|this, a: &TogglePlay, _, cx| {
                super::timeline::transport(&this.state, a, cx)
            }))
            .on_action(cx.listener(|this, a: &ShuttleForward, _, cx| {
                super::timeline::transport(&this.state, a, cx)
            }))
            .on_action(cx.listener(|this, a: &ShuttleReverse, _, cx| {
                super::timeline::transport(&this.state, a, cx)
            }))
            .on_action(cx.listener(|this, a: &ShuttleStop, _, cx| {
                super::timeline::transport(&this.state, a, cx)
            }))
            .on_action(cx.listener(|this, a: &PrevFrame, _, cx| {
                super::timeline::transport(&this.state, a, cx)
            }))
            .on_action(cx.listener(|this, a: &NextFrame, _, cx| {
                super::timeline::transport(&this.state, a, cx)
            }))
            .on_action(cx.listener(|this, a: &PrevCut, _, cx| {
                super::timeline::transport(&this.state, a, cx)
            }))
            .on_action(cx.listener(|this, a: &NextCut, _, cx| {
                super::timeline::transport(&this.state, a, cx)
            }))
            .child(program)
            .child(self.controls(cx))
    }
}

/// A decoded frame as a GPUI image, for other panels' thumbnails.
pub(crate) fn frame_image(frame: &koo_core::services::Frame) -> Option<Arc<RenderImage>> {
    player::to_image(frame)
}

/// The largest rect of `aspect` centered in `bounds`, inset by `pad`.
fn fit(bounds: Bounds<Pixels>, aspect: f32, pad: Pixels) -> Bounds<Pixels> {
    let w = (bounds.size.width - pad * 2.).max(px(0.));
    let h = (bounds.size.height - pad * 2.).max(px(0.));
    let (fw, fh) = if f32::from(w) / f32::from(h).max(1.) > aspect {
        (h * aspect, h)
    } else {
        (w, w / aspect)
    };
    Bounds::new(
        point(
            bounds.origin.x + (bounds.size.width - fw) / 2.,
            bounds.origin.y + (bounds.size.height - fh) / 2.,
        ),
        size(fw, fh),
    )
}
