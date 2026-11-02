//! Program monitor: `EditorState::display_project()` at the playhead, so a
//! previewed change set shows its draft. The picture is the program frame
//! from `koo_core::compose`, the same compositor the render and the agent
//! use (effects, transitions, generators, blend modes, grades, transforms):
//! paused frames are composed off-thread from single decodes (one at a
//! time, the latest request wins), playback composes from streams and
//! mixes audio in background threads. Composed frames are kept
//! ([`cache`]), and a playhead that rests renders the next seconds ahead
//! into them. Captions and titles are part of the frame, lettered as the
//! render burns them in (`koo_deliver::Letterer`).
//!
//! A pool item opened here ([`EditorState::source_picture`]) shows in place
//! of the program, through the same path, with its own position and no
//! editing on the frame.
//!
//! Paused, the frame's layout ([`compose::layout`]) lets the user pick an
//! item and transform it on screen ([`gizmo`]); Shift-click adds or
//! removes items, and several move, scale and turn as one. The view zooms (Fit or a
//! fixed size, scroll with Ctrl/Cmd) and pans (scroll or middle-drag), so
//! handles outside the frame stay in reach. The corner-pin toggle makes a
//! picture's corners move one by one; mask mode shows and edits the
//! selected picture's masks ([`masks`]), its picture whole with what they
//! cut away tinted (unless that is turned off). With the subject tool,
//! hovering finds what a click would keep off the UI thread, the newest
//! hover winning; frames are composed again as subjects' mattes are made.

pub(crate) mod audio;
mod cache;
mod gizmo;
mod masks;
mod player;

use super::timeline::glyph::{Glyph, glyph};
use super::timeline::{
    NextCut, NextFrame, PrevCut, PrevFrame, ShuttleForward, ShuttleReverse, ShuttleStop,
};
use crate::app::TogglePlay;
use crate::state::{EditorState, Source};
use crate::ui::media_pool::{item_name, seek_bar};
use audio::AudioOut;
use cache::{FrameCache, SharedCache};
use gizmo::{Gizmo, Screen};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::compose::{self, MaskView, Stills};
use koo_core::model::{ClipContent, MediaItem, MediaKind, Project, TrackKind};
use koo_core::query::topmost_video_clip;
use koo_core::services::{PixelOrder, Services};
use koo_core::subject::SubjectPrompt;
use koo_core::{ChangeSetId, Time};
use player::Player;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::time::Duration;

/// Largest frame width requested from the media engine, fitted.
const MAX_FRAME_WIDTH: u32 = 1280;
/// Largest frame width requested when zoomed in.
const MAX_ZOOMED_WIDTH: u32 = 3840;
/// Fixed view sizes: device pixels per sequence pixel.
const ZOOMS: [f32; 4] = [0.25, 0.5, 1.0, 2.0];
/// How far one Ctrl/Cmd-scroll step zooms.
const ZOOM_STEP: f32 = 1.15;

actions!(
    viewer,
    [
        NudgeItemLeft,
        NudgeItemRight,
        NudgeItemUp,
        NudgeItemDown,
        NudgeItemLeftMore,
        NudgeItemRightMore,
        NudgeItemUpMore,
        NudgeItemDownMore,
        Deselect,
        RemoveMask,
    ]
);

/// Arrow keys move the item the viewer has picked (context `Gizmo`, set
/// while one is), instead of stepping frames.
fn bind_keys(cx: &mut App) {
    static BOUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if BOUND.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let g = Some("Gizmo");
    cx.bind_keys([
        KeyBinding::new("left", NudgeItemLeft, g),
        KeyBinding::new("right", NudgeItemRight, g),
        KeyBinding::new("up", NudgeItemUp, g),
        KeyBinding::new("down", NudgeItemDown, g),
        KeyBinding::new("shift-left", NudgeItemLeftMore, g),
        KeyBinding::new("shift-right", NudgeItemRightMore, g),
        KeyBinding::new("shift-up", NudgeItemUpMore, g),
        KeyBinding::new("shift-down", NudgeItemDownMore, g),
        KeyBinding::new("escape", Deselect, g),
        KeyBinding::new("delete", RemoveMask, g),
        KeyBinding::new("backspace", RemoveMask, g),
    ]);
}
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
    /// An uncommitted edit being dragged.
    live: u64,
    /// The caption styles lettering is drawn in.
    lettering: u64,
    width: u32,
    /// How masks show: tinted while they are edited.
    masks: MaskView,
    /// The pool item shown in place of the program.
    source: Option<koo_core::MediaId>,
}

/// A paused frame: what it shows and the sequence frame index.
#[derive(Clone, Copy, PartialEq)]
struct Request {
    key: PlayKey,
    frame: i64,
}

/// Mouse down on the frame.
struct Press {
    /// The items under it, topmost first.
    under: Vec<koo_core::ClipId>,
    /// Alt: a click picks the next item under the mouse.
    alt: bool,
    /// Shift on a selected item: a click takes it out of the selection.
    deselect: Option<koo_core::ClipId>,
}

/// A subject preview to find: the clip, its media and the prompt.
struct PreviewAsk {
    clip: koo_core::ClipId,
    item: MediaItem,
    prompt: SubjectPrompt,
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
    audio: Option<AudioOut>,
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
    /// Picking and transforming items on the frame.
    gizmo: Gizmo,
    /// Drawing and editing the selected picture's masks.
    masks: masks::MaskEditor,
    /// The subjects' version the cached frames were composed at.
    subjects_seen: u64,
    /// A subject preview is being found.
    previewing: bool,
    /// The newest one asked for meanwhile.
    next_preview: Option<PreviewAsk>,
    /// The mouse is over a picture with the subject tool, so a preview
    /// that arrives is shown.
    hovering: bool,
    /// A layout is being made for this frame.
    laying_out: Option<Request>,
    /// Device pixels per sequence pixel; `None` fits the frame to the panel.
    zoom: Option<f32>,
    /// Offset of the frame from the panel centre while zoomed.
    pan: Point<Pixels>,
    /// A middle-drag pan: where it started and the pan then.
    panning: Option<(Point<Pixels>, Point<Pixels>)>,
    /// Mouse down on the frame.
    pressed: Option<Press>,
    /// A click that came before the frame was laid out, picked once it is.
    pending_pick: Option<Point<Pixels>>,
    /// The frame on screen from the last layout.
    frame_rect: Rc<Cell<Bounds<Pixels>>>,
    /// Device pixels per logical pixel at the last layout.
    scale_factor: Rc<Cell<f32>>,
    _observe: Subscription,
}

impl ViewerPanel {
    pub fn new(state: Entity<EditorState>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        bind_keys(cx);
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
            gizmo: Gizmo::default(),
            masks: masks::MaskEditor::default(),
            subjects_seen: 0,
            previewing: false,
            next_preview: None,
            hovering: false,
            laying_out: None,
            zoom: None,
            pan: Point::default(),
            panning: None,
            pressed: None,
            pending_pick: None,
            frame_rect: Rc::new(Cell::new(Bounds::default())),
            scale_factor: Rc::new(Cell::new(1.0)),
            _observe,
        }
    }

    fn show(&mut self, picture: Option<Arc<RenderImage>>) {
        if let Some(old) = std::mem::replace(&mut self.picture, picture) {
            self.retired.push(old);
        }
    }

    /// Whether the frame can be edited: the program, paused.
    fn editable(&self, state: &EditorState) -> bool {
        !state.playing && state.source_picture().is_none()
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
            live: state.live_version,
            lettering: state.lettering_version,
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

    /// Frames composed before subjects' mattes were made go, so the
    /// paused frame is composed again with them.
    fn sync_subjects(&mut self, cx: &App) {
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
    fn subjects(&self, shape: &compose::Shape, cx: &App) -> masks::Subjects {
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
    fn subject_media(&self, clip: koo_core::ClipId, cx: &App) -> Option<MediaItem> {
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
        if self.previewing {
            self.next_preview = Some(ask);
            return;
        }
        let Some(service) = self.state.read(cx).services.subjects.clone() else {
            return;
        };
        self.previewing = true;
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
                this.previewing = false;
                let wanted = this.hovering && this.masks.tool == Some(masks::Tool::Subject);
                if let Some(outlines) = outlines.filter(|_| wanted) {
                    this.masks.hover = Some(masks::Hover {
                        clip,
                        prompt: asked,
                        outlines,
                    });
                }
                if let Some(next) = this.next_preview.take() {
                    this.preview_subject(next, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// With the subject tool over the selected picture: preview what a
    /// click at `p` would keep.
    fn hover_subject(&mut self, p: Point<Pixels>, modifiers: Modifiers, cx: &mut Context<Self>) {
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
                self.next_preview = None;
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
    fn remove_background(&mut self, cx: &mut Context<Self>) {
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
        let mask = koo_core::mask::Mask::person();
        self.masks.selected = Some(mask.id);
        masks.push(mask);
        let command = koo_core::command::Command::SetMasks {
            clip: shape.clip,
            masks,
        };
        self.state
            .update(cx, |s, cx| s.edit_batch(vec![command], "set_masks", cx));
    }

    fn pick_subject_tool(&mut self, cx: &mut Context<Self>) {
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

    /// Start, follow or stop the playback threads, and pick up their frames.
    fn sync_playback(&mut self, cx: &mut Context<Self>) {
        let key = self.key(cx);
        let state = self.state.read(cx);
        let media = state.services.media.clone();
        let source = state.source_picture().is_some();
        let realtime = state.monitor_playing() && (source || state.play_speed == 1.0);
        let (Some(media), true) = (media, realtime) else {
            self.playback = None;
            return;
        };
        self.ahead = None;
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
            self.show(Some(picture));
            self.requested = None;
        }
    }

    /// Paused (or shuttling): compose the frame under the playhead.
    fn request_frame(&mut self, cx: &mut Context<Self>) {
        let key = self.key(cx);
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
            self.pending = None;
            self.show(Some(picture));
            self.render_ahead_later(request, cx);
            self.lay_out(request, cx);
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
        if laid_out || self.laying_out == Some(request) || self.gizmo.dragging() {
            return;
        }
        let Some((_, project)) = self.snapshot.clone().filter(|(k, _)| *k == request.key) else {
            return;
        };
        self.laying_out = Some(request);
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
                if this.laying_out == Some(request) {
                    this.laying_out = None;
                }
                this.gizmo.set_shapes(request, shapes);
                if this.requested == Some(request)
                    && let Some(p) = this.pending_pick.take()
                {
                    let screen = this.screen(cx);
                    let under = this.gizmo.clips_at(p, &screen, Some(request));
                    this.state.update(cx, |s, cx| {
                        s.select_clips(under.first().copied().into_iter().collect(), cx)
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Where the sequence is on screen.
    fn screen(&self, cx: &App) -> Screen {
        let s = &self.state.read(cx).bus.project().timeline.settings;
        Screen {
            frame: self.frame_rect.get(),
            sequence: (s.width.max(1), s.height.max(1)),
        }
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let screen = self.screen(cx);
        let state = self.state.read(cx);
        if !self.editable(state) {
            return;
        }
        let project = state.bus.project();
        let playhead = state.playhead;
        let editable = state.preview.is_none();
        let mut selected = state.selection.clips.clone();
        let request = self.requested;
        let under = self.gizmo.clips_at(ev.position, &screen, request);
        let handle =
            self.gizmo
                .handle_at(ev.position, &screen, project, &selected, playhead, request);
        // Not laid out yet (just edited or seeked): the box just dragged
        // still works; anything else is picked once the layout arrives.
        let laid_out = self
            .gizmo
            .shapes
            .as_ref()
            .is_some_and(|(r, _)| Some(*r) == request);
        if !laid_out && handle.is_none() {
            self.pending_pick = Some(ev.position);
            return;
        }
        self.pending_pick = None;
        if self.masks.on {
            self.mask_press(ev, &screen, under, cx);
            return;
        }
        let alt = ev.modifiers.alt;
        let mut press = Press {
            under: under.clone(),
            alt,
            deselect: None,
        };
        // Shift-click adds the item under the mouse, or takes a selected
        // one out on release (a drag moves them all instead).
        if ev.modifiers.shift
            && let Some(&clip) = under.first()
        {
            if selected.contains(&clip) {
                press.deselect = Some(clip);
            } else {
                selected.push(clip);
                let clips = selected.clone();
                self.state.update(cx, |s, cx| s.select_clips(clips, cx));
            }
            self.pressed = Some(press);
            if editable {
                let state = self.state.read(cx);
                let (project, request) = (state.bus.project(), self.requested);
                if !self.gizmo.begin_group(
                    gizmo::Handle::Body,
                    ev.position,
                    &screen,
                    project,
                    &selected,
                    playhead,
                    request,
                ) && let Some(&first) = selected.first()
                {
                    self.gizmo.begin(
                        gizmo::Handle::Body,
                        first,
                        ev.position,
                        ev.modifiers,
                        &screen,
                        project,
                        playhead,
                        request,
                    );
                }
            }
            cx.notify();
            return;
        }
        self.pressed = Some(press);
        // The group's box takes its handles, and its body over its items
        // or nothing.
        if self.gizmo.grouped(&selected, request)
            && let Some(handle) = handle
            && (handle != gizmo::Handle::Body || under.first().is_none_or(|c| selected.contains(c)))
        {
            if editable {
                let state = self.state.read(cx);
                self.gizmo.begin_group(
                    handle,
                    ev.position,
                    &screen,
                    state.bus.project(),
                    &selected,
                    playhead,
                    request,
                );
            }
            cx.notify();
            return;
        }
        // The selected box's handles come first; its body only when it is
        // the topmost item there, or with Alt (which cycles on release).
        let body =
            |h: gizmo::Handle| h == gizmo::Handle::Body || (h == gizmo::Handle::Anchor && !alt);
        let own = selected
            .first()
            .is_some_and(|c| under.first() == Some(c) || (alt && under.contains(c)));
        let handle = match (handle, under.first()) {
            (Some(handle), _) if !body(handle) || own => handle,
            (_, Some(&clip)) => {
                selected = vec![clip];
                self.state
                    .update(cx, |s, cx| s.select_clips(vec![clip], cx));
                gizmo::Handle::Body
            }
            (Some(handle), None) => handle,
            (None, None) => {
                if !selected.is_empty() {
                    self.state
                        .update(cx, |s, cx| s.select_clips(Vec::new(), cx));
                }
                return;
            }
        };
        if editable && let Some(&clip) = selected.first() {
            let state = self.state.read(cx);
            self.gizmo.begin(
                handle,
                clip,
                ev.position,
                ev.modifiers,
                &screen,
                state.bus.project(),
                playhead,
                request,
            );
        }
        cx.notify();
    }

    /// A press in mask mode: on the selected picture's masks, else picking
    /// another item (no transform).
    fn mask_press(
        &mut self,
        ev: &MouseDownEvent,
        screen: &Screen,
        under: Vec<koo_core::ClipId>,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.read(cx);
        let (playhead, request) = (state.playhead, self.requested);
        let editable = state.preview.is_none();
        let selected = state.selection.clips.clone();
        let shape = self
            .gizmo
            .active(&selected, request)
            .filter(|s| !s.lettering && editable);
        let commands = shape.and_then(|shape| {
            let subjects = self.subjects(&shape, cx);
            let project = self.state.read(cx).bus.project();
            self.masks.press(
                ev.position,
                ev.modifiers,
                screen,
                project,
                &shape,
                playhead,
                &subjects,
            )
        });
        match commands {
            Some(commands) if !commands.is_empty() => {
                self.state
                    .update(cx, |s, cx| s.edit_batch(commands, "set_masks", cx));
            }
            Some(_) => {}
            None => {
                let pick: Vec<_> = under.first().copied().into_iter().collect();
                if pick.first() != selected.first() {
                    self.masks.cancel();
                    self.masks.selected = None;
                    self.state.update(cx, |s, cx| s.select_clips(pick, cx));
                }
            }
        }
        cx.notify();
    }

    /// Remove the picked mask; whether there was one.
    fn remove_mask(&mut self, cx: &mut Context<Self>) -> bool {
        let state = self.state.read(cx);
        if !self.masks.on || state.preview.is_some() {
            return false;
        }
        let Some(shape) = self.gizmo.active(&state.selection.clips, self.requested) else {
            return false;
        };
        let commands = self.masks.remove_selected(state.bus.project(), shape.clip);
        if commands.is_empty() {
            return false;
        }
        self.state
            .update(cx, |s, cx| s.edit_batch(commands, "set_masks", cx));
        true
    }

    fn mouse_moved(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if let Some((from, pan)) = self.panning {
            self.pan = pan + (ev.position - from);
            cx.notify();
            return;
        }
        let screen = self.screen(cx);
        if self.masks.dragging() {
            if ev.dragging() {
                if let Some(commands) = self.masks.drag_to(ev.position, ev.modifiers, &screen) {
                    self.state.update(cx, |s, cx| s.show_live(&commands, cx));
                }
                cx.notify();
            }
            return;
        }
        if self.masks.on {
            if self.gizmo.hover.take().is_some() {
                cx.notify();
            }
            self.hover_subject(ev.position, ev.modifiers, cx);
            return;
        }
        let state = self.state.read(cx);
        let (playhead, request) = (state.playhead, self.requested);
        if self.gizmo.dragging() {
            if !ev.dragging() {
                return;
            }
            let commands = self.gizmo.drag_to(
                ev.position,
                ev.modifiers,
                &screen,
                state.bus.project(),
                playhead,
            );
            if let Some(commands) = commands {
                self.state.update(cx, |s, cx| s.show_live(&commands, cx));
            }
            cx.notify();
            return;
        }
        let selected = &state.selection.clips;
        let handle = (self.editable(state) && state.preview.is_none())
            .then(|| {
                self.gizmo.handle_at(
                    ev.position,
                    &screen,
                    state.bus.project(),
                    selected,
                    playhead,
                    request,
                )
            })
            .flatten();
        if handle != self.gizmo.hover {
            self.gizmo.hover = handle;
            cx.notify();
        }
    }

    fn mouse_up(&mut self, ev: &MouseUpEvent, cx: &mut Context<Self>) {
        if ev.button == MouseButton::Middle {
            self.panning = None;
            return;
        }
        if ev.button != MouseButton::Left {
            return;
        }
        let pressed = self.pressed.take();
        if self.masks.dragging() {
            let commands = self.masks.end();
            self.state.update(cx, |s, cx| {
                if commands.is_empty() {
                    s.show_live(&[], cx);
                } else {
                    s.edit_batch(commands, "set_masks", cx);
                }
            });
            cx.notify();
            return;
        }
        if !self.gizmo.dragging() {
            return;
        }
        let state = self.state.read(cx);
        let (commands, click) = self.gizmo.end(state.bus.project(), state.playhead);
        let selection = state.selection.clips.clone();
        let selected = selection.first().copied();
        let applied = self.state.update(cx, |s, cx| {
            if commands.is_empty() {
                s.show_live(&[], cx);
                false
            } else {
                s.edit_batch(commands, "set_transform", cx).is_some()
            }
        });
        if !applied {
            self.gizmo.cancel();
        }
        // Shift-click on a selected item: out of the selection.
        if click && let Some(clip) = pressed.as_ref().and_then(|p| p.deselect) {
            let rest: Vec<_> = selection.into_iter().filter(|c| *c != clip).collect();
            self.state.update(cx, |s, cx| s.select_clips(rest, cx));
            cx.notify();
            return;
        }
        // Alt-click: the next item under the mouse.
        if click
            && let Some(Press {
                under, alt: true, ..
            }) = pressed
            && under.len() > 1
        {
            let next = selected
                .and_then(|c| under.iter().position(|u| *u == c))
                .map_or(0, |i| (i + 1) % under.len());
            let clip = under[next];
            self.state
                .update(cx, |s, cx| s.select_clips(vec![clip], cx));
        }
        cx.notify();
    }

    fn scrolled(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = ev.delta.pixel_delta(px(20.));
        if ev.modifiers.secondary() {
            let screen = self.screen(cx);
            let current = self
                .zoom
                .unwrap_or_else(|| screen.scale() * self.scale_factor.get());
            // Scrolling up zooms in; a wheel notch is about one step.
            let steps = f32::from(delta.y) / 60.0;
            let zoom = (current * ZOOM_STEP.powf(steps)).clamp(0.05, 8.0);
            // Keep the point under the mouse still.
            let panel = self.frame_rect.get().center() - self.pan;
            let k = zoom / current;
            let offset = ev.position - panel;
            self.pan = point(
                offset.x - (offset.x - self.pan.x) * k,
                offset.y - (offset.y - self.pan.y) * k,
            );
            self.zoom = Some(zoom);
            cx.notify();
        } else if self.zoom.is_some() {
            self.pan += delta;
            cx.notify();
        }
    }

    /// Show the view at `zoom` (`None` fits it), centred.
    fn set_zoom(&mut self, zoom: Option<f32>, cx: &mut Context<Self>) {
        self.zoom = zoom;
        self.pan = Point::default();
        cx.notify();
    }

    /// Move the picked items by `delta` sequence pixels. One picked caption
    /// takes its track along, as when dragged.
    fn nudge(&mut self, delta: [f32; 2], cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        if state.preview.is_some() {
            return;
        }
        let project = state.bus.project();
        let s = &project.timeline.settings;
        let picked = picked_clips(&state.selection.clips, project, state.playhead);
        let alone = picked.len() > 1;
        let mut moved = Vec::new();
        let mut commands = Vec::new();
        for (track, clip) in picked {
            if track.locked {
                continue;
            }
            let tf = gizmo::nudged(clip, state.playhead, delta, (s.width, s.height));
            let lettering = !alone && compose::is_lettering(track, clip);
            commands.extend(gizmo::commit(
                project,
                clip.id,
                state.playhead,
                &tf,
                lettering,
            ));
            moved.push((clip.id, tf));
        }
        if commands.is_empty() {
            return;
        }
        let applied = self
            .state
            .update(cx, |s, cx| s.edit_batch(commands, "set_transform", cx))
            .is_some();
        if applied {
            for (id, tf) in &moved {
                self.gizmo.hold(*id, tf);
            }
        }
    }

    /// Whether an item is picked on the paused frame: a selected clip shows
    /// a picture or lettering under the playhead.
    fn picked(&self, cx: &App) -> bool {
        let state = self.state.read(cx);
        self.editable(state)
            && !picked_clips(&state.selection.clips, state.bus.project(), state.playhead).is_empty()
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

    /// The opened item's controls: its name, a bar to seek on and the way
    /// back to the program.
    fn opened(&self, source: &Source, cx: &App) -> AnyElement {
        let item = source.item().clone();
        let (state, back) = (self.state.clone(), self.state.clone());
        let name = item_name(&item);
        h_flex()
            .flex_1()
            .min_w_0()
            .gap_2()
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(cx.theme().muted_foreground)
                    .child(name),
            )
            .map(|d| {
                if source.can_play() {
                    d.child(seek_bar(
                        source.fraction(),
                        move |f, cx| state.update(cx, |s, cx| s.seek_source(&item, f, cx)),
                        cx,
                    ))
                } else {
                    d.child(div().flex_1())
                }
            })
            .child(
                Button::new("viewer-program")
                    .ghost()
                    .xsmall()
                    .label("Program")
                    .on_click(move |_, _, cx| back.update(cx, |s, cx| s.close_source(cx))),
            )
            .into_any_element()
    }

    fn controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let project = state.monitor_project();
        let rate = project.timeline.settings.rate;
        let t = state.monitor_time();
        let theme = cx.theme();
        let (fg, muted, border) = (theme.foreground, theme.muted_foreground, theme.border);
        let opened = state.source_picture().map(|s| self.opened(s, cx));
        let program = opened.is_none();
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
        let playing = state.monitor_playing();
        let entity = self.state.clone();
        let viewer = cx.entity();
        let zoom = self.zoom;
        let zoom_label = match zoom {
            None => "Fit".to_string(),
            Some(z) => format!("{:.0}%", z * 100.0),
        };
        let safe = self.gizmo.safe_areas;
        let zoom_menu = Button::new("viewer-zoom")
            .ghost()
            .xsmall()
            .label(zoom_label)
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                let v = viewer.clone();
                menu =
                    menu.item(PopupMenuItem::new("Fit").checked(zoom.is_none()).on_click(
                        move |_, _, cx| v.update(cx, |this, cx| this.set_zoom(None, cx)),
                    ));
                for z in ZOOMS {
                    let v = viewer.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!("{:.0}%", z * 100.0))
                            .checked(zoom == Some(z))
                            .on_click(move |_, _, cx| {
                                v.update(cx, |this, cx| this.set_zoom(Some(z), cx))
                            }),
                    );
                }
                menu
            });
        let viewer = cx.entity();
        let safe_toggle = Button::new("viewer-safe")
            .ghost()
            .xsmall()
            .toggled(safe)
            .tooltip("Safe areas")
            .child(glyph(
                Glyph::SafeArea,
                px(13.),
                if safe { fg } else { muted },
            ))
            .on_click(move |_, _, cx| {
                viewer.update(cx, |this, cx| {
                    this.gizmo.safe_areas = !this.gizmo.safe_areas;
                    cx.notify();
                })
            });
        let viewer = cx.entity();
        let masking = self.masks.on;
        let mask_toggle = Button::new("viewer-masks")
            .ghost()
            .xsmall()
            .toggled(masking)
            .tooltip("Masks")
            .child(glyph(
                Glyph::Mask,
                px(13.),
                if masking { fg } else { muted },
            ))
            .on_click(move |_, _, cx| {
                viewer.update(cx, |this, cx| {
                    this.masks.on = !this.masks.on;
                    this.masks.cancel();
                    this.masks.tool = None;
                    if this.masks.on {
                        this.gizmo.corner_pin = false;
                    }
                    cx.notify();
                })
            });
        let tools = masking.then(|| {
            let tool = self.masks.tool;
            let tinted = self.masks.tinted;
            let viewer = cx.entity();
            let tint = {
                let viewer = viewer.clone();
                Button::new("viewer-mask-tint")
                    .ghost()
                    .xsmall()
                    .toggled(tinted)
                    .tooltip("Show what masks cut away")
                    .child(glyph(
                        Glyph::MaskOverlay,
                        px(13.),
                        if tinted { fg } else { muted },
                    ))
                    .on_click(move |_, _, cx| {
                        viewer.update(cx, |this, cx| {
                            this.masks.tinted = !this.masks.tinted;
                            cx.notify();
                        })
                    })
            };
            let person = {
                let viewer = viewer.clone();
                Button::new("viewer-mask-person")
                    .ghost()
                    .xsmall()
                    .tooltip("Remove background")
                    .child(glyph(Glyph::Person, px(13.), muted))
                    .on_click(move |_, _, cx| {
                        viewer.update(cx, |this, cx| {
                            this.masks.cancel();
                            this.masks.tool = None;
                            this.remove_background(cx);
                            cx.notify();
                        })
                    })
            };
            h_flex()
                .gap_1()
                .child(person)
                .children(
                    [
                        (masks::Tool::Subject, Glyph::Subject, "Subject"),
                        (masks::Tool::Rectangle, Glyph::MaskRect, "Rectangle"),
                        (masks::Tool::Ellipse, Glyph::MaskEllipse, "Ellipse"),
                        (masks::Tool::Pen, Glyph::Pen, "Pen"),
                    ]
                    .into_iter()
                    .map(|(t, g, tip)| {
                        let on = tool == Some(t);
                        let viewer = viewer.clone();
                        Button::new(SharedString::from(format!("viewer-mask-{tip}")))
                            .ghost()
                            .xsmall()
                            .toggled(on)
                            .tooltip(tip)
                            .child(glyph(g, px(13.), if on { fg } else { muted }))
                            .on_click(move |_, _, cx| {
                                viewer.update(cx, |this, cx| {
                                    this.masks.cancel();
                                    if this.masks.tool == Some(t) {
                                        this.masks.tool = None;
                                    } else if t == masks::Tool::Subject {
                                        this.pick_subject_tool(cx);
                                    } else {
                                        this.masks.tool = Some(t);
                                    }
                                    cx.notify();
                                })
                            })
                    }),
                )
                .child(tint)
        });
        let viewer = cx.entity();
        let pin = self.gizmo.corner_pin;
        let pin_toggle = Button::new("viewer-corner-pin")
            .ghost()
            .xsmall()
            .toggled(pin)
            .tooltip("Corner pin")
            .child(glyph(
                Glyph::CornerPin,
                px(13.),
                if pin { fg } else { muted },
            ))
            .on_click(move |_, _, cx| {
                viewer.update(cx, |this, cx| {
                    this.gizmo.corner_pin = !this.gizmo.corner_pin;
                    if this.gizmo.corner_pin {
                        this.masks.on = false;
                        this.masks.cancel();
                    }
                    cx.notify();
                })
            });
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
            .children(opened)
            .when(program, |d| {
                d.child(div().flex_1())
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
                    .children(tools)
                    .child(mask_toggle)
                    .child(pin_toggle)
            })
            .child(safe_toggle)
            .child(zoom_menu)
    }
}

impl Focusable for ViewerPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ViewerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
        self.sync_subjects(cx);
        self.sync_playback(cx);
        if self.playback.is_none() {
            self.request_frame(cx);
        }

        let picked = self.picked(cx);
        let state = self.state.read(cx);
        let project = state.monitor_project();
        let settings = project.timeline.settings.clone();
        let sequence = (settings.width.max(1), settings.height.max(1));
        let aspect = sequence.0 as f32 / sequence.1 as f32;
        let picture = self.picture.clone();
        let overlay = self.editable(state).then(|| {
            self.gizmo.overlay(
                state.bus.project(),
                &state.selection.clips,
                state.playhead,
                self.requested,
                state.preview.is_none() && !self.masks.on,
            )
        });
        let mask_overlay = (self.editable(state) && self.masks.on)
            .then(|| {
                let shape = self
                    .gizmo
                    .active(&state.selection.clips, self.requested)
                    .filter(|s| !s.lettering)?;
                let screen = Screen {
                    frame: self.frame_rect.get(),
                    sequence,
                };
                let subjects = self.subjects(&shape, cx);
                Some(self.masks.overlay(
                    state.bus.project(),
                    &shape,
                    state.playhead,
                    &screen,
                    &subjects,
                ))
            })
            .flatten();
        let theme = cx.theme();
        let bg = theme.background;
        let colors = [
            theme.primary,
            theme.magenta,
            theme.muted_foreground.opacity(0.7),
        ];
        let width_cell = self.frame_width.clone();
        let rect_cell = self.frame_rect.clone();
        let scale_cell = self.scale_factor.clone();
        let (zoom, pan) = (self.zoom, self.pan);
        let entity = cx.entity();

        let program = canvas(
            move |bounds, window, _| {
                let sf = window.scale_factor();
                let frame = match zoom {
                    None => fit(bounds, aspect, px(8.)),
                    Some(z) => {
                        let size = size(
                            px(sequence.0 as f32 * z / sf),
                            px(sequence.1 as f32 * z / sf),
                        );
                        let c = bounds.center() + pan;
                        Bounds::new(point(c.x - size.width / 2., c.y - size.height / 2.), size)
                    }
                };
                rect_cell.set(frame);
                scale_cell.set(sf);
                let device = f32::from(frame.size.width) * sf;
                // Round so small resizes don't refetch.
                let width = ((device / 160.).ceil() * 160.) as u32;
                let max = if zoom.is_some() {
                    MAX_FRAME_WIDTH.max(sequence.0.min(MAX_ZOOMED_WIDTH))
                } else {
                    MAX_FRAME_WIDTH
                };
                width_cell.set(width.clamp(160, max));
                frame
            },
            move |bounds, frame, window, _| {
                window.paint_quad(fill(bounds, bg));
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    window.paint_quad(fill(frame, black()));
                    if let Some(image) = picture {
                        let _ =
                            window.paint_image(frame, frame, Corners::default(), image, 0, false);
                    }
                    if let Some(overlay) = &overlay {
                        let screen = Screen { frame, sequence };
                        gizmo::paint(overlay, &screen, colors, window);
                    }
                    if let Some(overlay) = &mask_overlay {
                        let screen = Screen { frame, sequence };
                        masks::paint(overlay, &screen, colors[1], window);
                    }
                });
                let e = entity.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        e.update(cx, |this, cx| this.mouse_moved(ev, cx));
                    }
                });
                let e = entity.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        e.update(cx, |this, cx| this.mouse_up(ev, cx));
                    }
                });
            },
        )
        .size_full();

        let cursor = if self.panning.is_some() {
            CursorStyle::ClosedHand
        } else if self.masks.on && self.masks.tool.is_some() {
            CursorStyle::Crosshair
        } else {
            gizmo::cursor(self.gizmo.hover, self.gizmo.dragging())
        };
        let stage = div()
            .id("viewer-program")
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .cursor(cursor)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    this.mouse_down(ev, window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    if this.zoom.is_some() {
                        this.panning = Some((ev.position, this.pan));
                        cx.notify();
                    }
                }),
            )
            .on_scroll_wheel(
                cx.listener(|this, ev: &ScrollWheelEvent, _, cx| this.scrolled(ev, cx)),
            )
            .child(program);

        v_flex()
            .size_full()
            .bg(bg)
            .key_context("Viewer")
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
            .on_action(cx.listener(|this, _: &NudgeItemLeft, _, cx| this.nudge([-1., 0.], cx)))
            .on_action(cx.listener(|this, _: &NudgeItemRight, _, cx| this.nudge([1., 0.], cx)))
            .on_action(cx.listener(|this, _: &NudgeItemUp, _, cx| this.nudge([0., -1.], cx)))
            .on_action(cx.listener(|this, _: &NudgeItemDown, _, cx| this.nudge([0., 1.], cx)))
            .on_action(cx.listener(|this, _: &NudgeItemLeftMore, _, cx| this.nudge([-10., 0.], cx)))
            .on_action(cx.listener(|this, _: &NudgeItemRightMore, _, cx| this.nudge([10., 0.], cx)))
            .on_action(cx.listener(|this, _: &NudgeItemUpMore, _, cx| this.nudge([0., -10.], cx)))
            .on_action(cx.listener(|this, _: &NudgeItemDownMore, _, cx| this.nudge([0., 10.], cx)))
            .on_action(cx.listener(|this, _: &RemoveMask, _, cx| {
                if !this.remove_mask(cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &Deselect, _, cx| {
                if this.masks.drawing() {
                    this.masks.cancel();
                    cx.notify();
                    return;
                }
                this.gizmo.cancel();
                this.state.update(cx, |s, cx| {
                    s.show_live(&[], cx);
                    s.select_clips(Vec::new(), cx);
                });
            }))
            .child(
                v_flex()
                    .size_full()
                    .track_focus(&self.focus)
                    .when(picked, |d| d.key_context("Gizmo"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
                    )
                    .child(stage)
                    .child(self.controls(cx)),
            )
    }
}

/// The `selected` clips that show a picture or lettering at `playhead`.
fn picked_clips<'a>(
    selected: &[koo_core::ClipId],
    project: &'a Project,
    playhead: Time,
) -> Vec<(&'a koo_core::model::Track, &'a koo_core::model::Clip)> {
    selected
        .iter()
        .filter_map(|id| project.timeline.clip(*id))
        .filter(|(track, clip)| track.kind != TrackKind::Audio && clip.range().contains(playhead))
        .collect()
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
