//! Program monitor: `EditorState::display_project()` at the playhead, so a
//! previewed change set shows its draft. The picture is the program frame
//! from `flikk_core::compose`, the same compositor the render and the agent
//! use (effects, transitions, generators, blend modes, grades, transforms):
//! paused frames are composed off-thread from single decodes (one at a
//! time, the latest request wins), playback composes from streams and
//! mixes audio in background threads. Composed frames are kept
//! ([`cache`]), and a playhead that rests renders the next seconds ahead
//! into them. Frames the background render keeps on disk
//! ([`crate::render_cache`]) are read instead of composed, when large
//! enough and masks are not being edited. Captions and titles are part of the frame, lettered as the
//! render burns them in (`flikk_deliver::Letterer`).
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
mod chrome;
mod focus;
mod frames;
mod gizmo;
pub(crate) mod latest;
mod masks;
mod player;
mod pointer;
mod subjects;

use super::timeline::{ClearInOut, MarkIn, MarkOut, NextCut, NextFrame, PrevCut, PrevFrame};
use super::transport;
use crate::state::EditorState;
use crate::ui::kit::{Icon, IconName, Panel, h_flex, theme as t, v_flex};
use crate::ui::mixer::meter;
use audio::AudioOut;
use cache::{FrameCache, SharedCache};
use flikk_core::agent::Workspace;
use flikk_core::command::InsertMode;
use flikk_core::compose::MaskView;
use flikk_core::model::{Project, TrackKind};
use flikk_core::query::audible_clips_at;
use flikk_core::{ChangeSetId, Time};
use frames::Job;
use gizmo::{Gizmo, Screen};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use latest::Latest;
use player::Player;
use pointer::{HeldClick, Press};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::time::Duration;
use subjects::PreviewAsk;

/// A decoded frame as a GPUI image, for other panels' thumbnails.
pub(crate) use player::to_image;

/// What Reframe around subjects asks the agent.
const REFRAME: &str = "Reframe the footage for this frame: fill it, and pan each clip \
(with keyframes where the subject moves) so its subject stays in shot.";

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
        InsertSource,
        OverwriteSource,
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
        // Three-point editing from the source in the viewer.
        KeyBinding::new("i", MarkIn, Some("Viewer")),
        KeyBinding::new("o", MarkOut, Some("Viewer")),
        KeyBinding::new("alt-x", ClearInOut, Some("Viewer")),
        KeyBinding::new(",", InsertSource, Some("Viewer")),
        KeyBinding::new(".", OverwriteSource, Some("Viewer")),
    ]);
}
const TRANSPORT_HEIGHT: f32 = 44.;
/// Where the meter turns hot (−12 dBFS) and loud (−3.6 dBFS), as a
/// fraction of its height.
const HOT: f32 = 0.8;
const LOUD: f32 = 0.94;
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
    source: Option<flikk_core::MediaId>,
}

/// A paused frame: what it shows and the sequence frame index.
#[derive(Clone, Copy, PartialEq)]
struct Request {
    key: PlayKey,
    frame: i64,
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
    /// A paused frame is being composed, with the newest one asked for
    /// meanwhile.
    composing: Latest<Job>,
    /// Paused frames asked for so far, which numbers them.
    frames: u64,
    /// The number of the picture on screen, and its paused frame (none
    /// for playback's).
    on_screen: (u64, Option<Request>),
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
    /// A subject preview is being found, with the newest one asked for
    /// meanwhile.
    previewing: Latest<PreviewAsk>,
    /// The mouse is over a picture with the subject tool, so a preview
    /// that arrives is shown.
    hovering: bool,
    /// A layout is being made for this frame, with its number.
    laying_out: Option<(Request, u64)>,
    /// Layouts asked for so far, which numbers them.
    layouts: u64,
    /// Device pixels per sequence pixel; `None` fits the frame to the panel.
    zoom: Option<f32>,
    /// Offset of the frame from the panel centre while zoomed.
    pan: Point<Pixels>,
    /// A middle-drag pan: where it started and the pan then.
    panning: Option<(Point<Pixels>, Point<Pixels>)>,
    /// Mouse down on the frame.
    pressed: Option<Press>,
    /// A click that came before the frame was laid out, applied once it is.
    held_click: Option<HeldClick>,
    /// The frame on screen from the last layout.
    frame_rect: Rc<Cell<Bounds<Pixels>>>,
    /// Device pixels per logical pixel at the last layout.
    scale_factor: Rc<Cell<f32>>,
    /// The pool item shown last, which Source brings back.
    last_source: Option<flikk_core::MediaId>,
    /// The master's left and right meters.
    meters: [meter::Display; 2],
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
            composing: Latest::Idle,
            frames: 0,
            on_screen: (0, None),
            retired: Vec::new(),
            playback: None,
            frame_width: Rc::new(Cell::new(640)),
            cache: FrameCache::shared(),
            shift: Arc::new(AtomicU8::new(0)),
            ahead: None,
            gizmo: Gizmo::default(),
            masks: masks::MaskEditor::default(),
            subjects_seen: 0,
            previewing: Latest::Idle,
            hovering: false,
            laying_out: None,
            layouts: 0,
            zoom: None,
            pan: Point::default(),
            panning: None,
            pressed: None,
            held_click: None,
            frame_rect: Rc::new(Cell::new(Bounds::default())),
            scale_factor: Rc::new(Cell::new(1.0)),
            last_source: None,
            meters: Default::default(),
            _observe,
        }
    }

    fn show(&mut self, picture: Option<Arc<RenderImage>>) {
        if let Some(old) = std::mem::replace(&mut self.picture, picture) {
            self.retired.push(old);
        }
    }

    /// Show paused frame number `seq` (`request`), unless a newer one is on
    /// screen.
    fn show_frame(&mut self, seq: u64, request: Request, picture: Option<Arc<RenderImage>>) {
        if seq > self.on_screen.0 {
            self.on_screen = (seq, Some(request));
            self.show(picture);
        }
    }

    /// Whether the frame can be edited: the program, paused.
    fn editable(&self, state: &EditorState) -> bool {
        !state.is_playing() && state.source_picture().is_none()
    }

    /// Where the sequence is on screen.
    fn screen(&self, cx: &App) -> Screen {
        let s = &self.state.read(cx).bus.project().timeline.settings;
        Screen {
            frame: self.frame_rect.get(),
            sequence: (s.width.max(1), s.height.max(1)),
        }
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
        // Before playback starts from a drag's live preview.
        self.drop_stale_drag(cx);
        self.masks
            .forget_lost_pen(self.state.read(cx).bus.project());
        self.sync_subjects(cx);
        self.sync_playback(cx);
        if self.playback.is_none() {
            self.request_frame(cx);
        }

        let picked = self.picked(cx);
        let state = self.state.read(cx);
        if let Some(source) = state.source_picture() {
            self.last_source = Some(source.item().id);
        }
        // The mixer shows the master in Audio; Deliver has no meter.
        let metered = matches!(
            state.workspace,
            Workspace::Edit | Workspace::Captions | Workspace::Color
        );
        if metered {
            for (display, level) in self.meters.iter_mut().zip(audio::channels()) {
                display.update(level.take());
            }
            if self.meters.iter().any(|m| !m.is_silent()) {
                window.request_animation_frame();
            }
        }
        let program = state.source_picture().is_none();
        let project = state.monitor_project();
        let settings = project.timeline.settings.clone();
        let sequence = (settings.width.max(1), settings.height.max(1));
        let aspect = sequence.0 as f32 / sequence.1 as f32;
        let t = state.monitor_time();
        let audio_only = program
            && !project
                .timeline
                .tracks
                .iter()
                .any(|tr| tr.kind != TrackKind::Audio && tr.clip_at(t).is_some())
            && !audible_clips_at(&project.timeline, t).is_empty();
        let label = (!state.monitor_playing() && !audio_only)
            .then(|| self.frame_label(state))
            .flatten();
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
        let masking = self.masks.on && program;
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
        let width_cell = self.frame_width.clone();
        let rect_cell = self.frame_rect.clone();
        let scale_cell = self.scale_factor.clone();
        let (zoom, pan) = (self.zoom, self.pan);
        let entity = cx.entity();

        let program_view = canvas(
            move |bounds, window, _| {
                let sf = window.scale_factor();
                let frame = match zoom {
                    None => fit(bounds, aspect, px(12.)),
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
            move |bounds, frame, window, cx| {
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    window.paint_quad(fill(frame, black()));
                    if let Some(image) = picture {
                        let _ =
                            window.paint_image(frame, frame, Corners::default(), image, 0, false);
                    }
                    if let Some(overlay) = &overlay {
                        let screen = Screen { frame, sequence };
                        gizmo::paint(overlay, &screen, window);
                    }
                    if let Some(overlay) = &mask_overlay {
                        let screen = Screen { frame, sequence };
                        masks::paint(overlay, &screen, window);
                    }
                    if let Some(text) = label {
                        let run = TextRun {
                            len: text.len(),
                            font: font(t::mono()),
                            color: t::alpha(t::ON_AC, 0.7).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let line = window.text_system().shape_line(text, px(10.), &[run], None);
                        let at = point(
                            frame.origin.x.max(bounds.origin.x) + px(10.),
                            frame.origin.y.max(bounds.origin.y) + px(8.),
                        );
                        let _ = line.paint(at, px(14.), TextAlign::Left, None, window, cx);
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
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
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
            .child(program_view)
            .when(audio_only, |d| {
                d.child(
                    v_flex()
                        .absolute()
                        .inset_0()
                        .items_center()
                        .justify_center()
                        .gap(px(6.))
                        .text_size(px(11.))
                        .text_color(t::T2)
                        .child(Icon::new(IconName::AudioLines).size(px(18.)).color(t::T3))
                        .child("Audio only"),
                )
            });

        let focused = self.focus.contains_focused(window, cx);
        div()
            .size_full()
            .key_context("Viewer")
            .map(|d| transport::with_shared_handlers(d, &self.state))
            .on_action(cx.listener(|this, _: &PrevFrame, _, cx| {
                if !this.state.update(cx, |s, cx| s.step_source(false, cx)) {
                    transport::prev_frame(&this.state, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &NextFrame, _, cx| {
                if !this.state.update(cx, |s, cx| s.step_source(true, cx)) {
                    transport::next_frame(&this.state, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &InsertSource, _, cx| {
                edit_source(&this.state, InsertMode::Insert, cx)
            }))
            .on_action(cx.listener(|this, _: &OverwriteSource, _, cx| {
                edit_source(&this.state, InsertMode::Overwrite, cx)
            }))
            .on_action(cx.listener(|this, _: &PrevCut, _, cx| transport::prev_cut(&this.state, cx)))
            .on_action(cx.listener(|this, _: &NextCut, _, cx| transport::next_cut(&this.state, cx)))
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
                // Mid-drag, Escape only drops the drag.
                if this.masks.cancel_drag() {
                    this.state.update(cx, |s, cx| s.show_live(&[], cx));
                    cx.notify();
                    return;
                }
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
                Panel::new().focused(focused).child(
                    v_flex()
                        .size_full()
                        .min_h_0()
                        .track_focus(&self.focus)
                        .when(picked, |d| d.key_context("Gizmo"))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
                        )
                        .child(self.header(cx))
                        .when(masking, |d| d.child(self.mask_tools(cx)))
                        .child(
                            h_flex()
                                .flex_1()
                                .min_h_0()
                                .w_full()
                                .bg(t::G)
                                .child(stage)
                                .when(metered, |d| d.child(meter(&self.meters))),
                        )
                        .child(self.transport(cx)),
                ),
            )
    }
}

/// The master's meter beside the frame: a bar per side, rising through
/// the hot and loud zones, with its peak hold and the louder side's held
/// peak in dBFS under it.
fn meter(sides: &[meter::Display; 2]) -> impl IntoElement {
    let hold = sides.iter().map(|d| d.hold).fold(0., f32::max);
    let (readout, color) = if hold < 1e-5 {
        ("\u{2212}\u{221e}".to_string(), t::T3)
    } else {
        let db = 20. * hold.log10();
        let sign = if db < -0.05 { "\u{2212}" } else { "+" };
        let color = match meter::position(hold) {
            _ if hold >= 1.0 => t::ERR,
            p if p >= LOUD => t::WARN,
            _ => t::T3,
        };
        (format!("{sign}{:.1}", db.abs()), color)
    };
    let bar = |d: &meter::Display| {
        let level = meter::position(d.peak);
        let held = meter::position(d.hold);
        let zone = |from: f32, to: f32, color: Rgba| {
            let h = (level.min(to) - from).max(0.);
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(relative(from))
                .h(relative(h))
                .bg(color)
        };
        let hold_color = match held {
            _ if d.hold >= 1.0 => t::ON_AC,
            p if p >= HOT => t::WARN,
            _ => t::OK,
        };
        div()
            .relative()
            .w(px(6.))
            .h_full()
            .rounded(px(2.))
            .overflow_hidden()
            .bg(t::R1)
            .child(zone(0., HOT, t::OK))
            .child(zone(HOT, LOUD, t::WARN))
            .child(zone(LOUD, 1., t::ERR))
            .when(held > 0., |b| {
                b.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(relative(1. - held))
                        .h(px(2.))
                        .bg(hold_color),
                )
            })
    };
    v_flex()
        .flex_none()
        .w(px(22.))
        .h_full()
        .pt(px(14.))
        .pb(px(10.))
        .mr(px(12.))
        .items_center()
        .gap(px(4.))
        .child(
            h_flex()
                .flex_1()
                .min_h_0()
                .gap(px(3.))
                .children(sides.iter().map(bar)),
        )
        .child(
            div()
                .font_family(t::mono())
                .text_size(px(9.))
                .text_color(color)
                .whitespace_nowrap()
                .child(readout),
        )
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

/// Edit the source in the viewer into the timeline at the playhead.
fn edit_source(state: &Entity<EditorState>, mode: InsertMode, cx: &mut App) {
    state.update(cx, |s, cx| {
        if let Some(id) = s.source_picture().map(|p| p.item().id) {
            s.edit_at_playhead(&[id], mode, cx);
        }
    });
}
