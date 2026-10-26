//! Program monitor: `EditorState::display_project()` at the playhead, so a
//! previewed change set shows its draft. Paused frames come from
//! `MediaService::frame` off-thread (latest request wins); playback streams
//! frames and mixes audio in background threads. Transforms, captions and
//! titles are applied for preview.

mod audio;
mod player;

use super::timeline::glyph::{Glyph, glyph};
use super::timeline::{NextFrame, PrevFrame, ShuttleForward, ShuttleReverse, ShuttleStop};
use crate::app::TogglePlay;
use crate::state::EditorState;
use audio::AudioOut;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use koo_core::model::{ClipContent, Project, TrackKind};
use koo_core::{ChangeSetId, ClipId, Time};
use player::{Picture, Player, top_clip};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

/// Largest frame width requested from the media engine.
const MAX_FRAME_WIDTH: u32 = 1280;
const CONTROLS_HEIGHT: f32 = 30.;
/// A playhead move larger than this between renders is a seek, in seconds.
const SEEK_JUMP: f64 = 0.35;

/// What a running playback was started from; a change restarts it.
#[derive(Clone, Copy, PartialEq)]
struct PlayKey {
    version: u64,
    preview: Option<ChangeSetId>,
    width: u32,
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
    picture: Option<Picture>,
    /// Paused frame last asked for: clip and source frame index.
    requested: Option<(ClipId, i64, u32)>,
    generation: u64,
    /// Images replaced since the last paint, to free from the sprite atlas.
    retired: Vec<Arc<RenderImage>>,
    playback: Option<Playback>,
    /// Width of the program image in device px, from the last layout.
    frame_width: Rc<Cell<u32>>,
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
            generation: 0,
            retired: Vec::new(),
            playback: None,
            frame_width: Rc::new(Cell::new(640)),
            _observe,
        }
    }

    fn show(&mut self, picture: Option<Picture>) {
        if let Some(old) = std::mem::replace(&mut self.picture, picture) {
            self.retired.push(old.image);
        }
    }

    /// Start, follow or stop the playback threads, and pick up their frames.
    fn sync_playback(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let media = state.services.media.clone();
        let realtime = state.playing && state.play_speed == 1.0;
        let (Some(media), true) = (media, realtime) else {
            self.playback = None;
            return;
        };
        let key = PlayKey {
            version: state.bus.version(),
            preview: state.preview,
            width: self.frame_width.get(),
        };
        let playhead = state.playhead;
        // A seek while playing restarts the audio at the new position; video
        // follows the playhead on its own.
        let jumped = self
            .playback
            .as_ref()
            .is_some_and(|p| (playhead - p.last).0.abs() > Time::from_secs_f64(SEEK_JUMP).0);
        if self.playback.as_ref().is_none_or(|p| p.key != key) {
            let project = Arc::new(state.display_project().clone());
            let player = Player::start(media.clone(), project.clone(), playhead, key.width);
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
            self.show(picture);
            self.requested = None;
        }
    }

    /// Paused (or shuttling): fetch the frame under the playhead.
    fn request_frame(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let Some(media) = state.services.media.clone() else {
            self.show(None);
            return;
        };
        let project = state.display_project();
        let t = state.playhead;
        let Some((clip, item)) = top_clip(project, t) else {
            self.requested = None;
            self.show(None);
            return;
        };
        let Some(source) = clip.to_source(t) else {
            return;
        };
        let rate = item
            .info
            .as_ref()
            .and_then(|i| i.video.as_ref())
            .map(|v| v.rate)
            .unwrap_or(project.timeline.settings.rate);
        let width = self.frame_width.get();
        let key = (clip.id, source.to_frame(rate), width);
        if self.requested == Some(key) {
            return;
        }
        self.requested = Some(key);
        self.generation += 1;
        let generation = self.generation;
        let (item, clip_id) = (item.clone(), clip.id);
        cx.spawn(async move |this, cx| {
            let image = cx
                .background_executor()
                .spawn(async move {
                    media
                        .frame(&item, source, width)
                        .ok()
                        .and_then(|f| player::to_image(&f))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Latest request wins: a newer one is on its way.
                if this.generation == generation {
                    this.show(image.map(|image| Picture {
                        clip: clip_id,
                        image,
                    }));
                    cx.notify();
                }
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
        let source = top_clip(project, t).and_then(|(clip, item)| {
            let at = clip.to_source(t)?;
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

/// Text drawn over the picture at the playhead: titles and captions.
struct Lettering {
    text: SharedString,
    lower_third: bool,
    caption: bool,
}

fn lettering(project: &Project, t: Time) -> Vec<Lettering> {
    let mut out = Vec::new();
    for track in project.timeline.tracks.iter().filter(|t| !t.mix.mute) {
        let Some(clip) = track.clip_at(t) else {
            continue;
        };
        match (&clip.content, track.kind) {
            (ClipContent::Title { text, template }, _) => out.push(Lettering {
                text: text.clone().into(),
                lower_third: template.contains("lower"),
                caption: false,
            }),
            (ClipContent::Caption { text, .. }, TrackKind::Caption) => out.push(Lettering {
                text: text.clone().into(),
                lower_third: false,
                caption: true,
            }),
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
        let transform = self.picture.as_ref().and_then(|p| {
            project.timeline.clip(p.clip).map(|(_, c)| {
                let mut tr = c.transform_at(t - c.start);
                tr.opacity = tr.opacity.clamp(0., 1.);
                tr
            })
        });
        let image = self.picture.as_ref().map(|p| p.image.clone());
        let letters = lettering(project, t);
        let theme = cx.theme();
        let (bg, fg) = (theme.background, theme.foreground);
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
                    if let (Some(image), Some(tf)) = (image, transform) {
                        let size = image.size(0);
                        let img_aspect = size.width.0 as f32 / size.height.0.max(1) as f32;
                        let base = fit(frame, img_aspect, px(0.));
                        let scaled = size_scaled(base.size, tf.scale.max(0.));
                        let center = frame.center()
                            + point(
                                frame.size.width * tf.position[0],
                                frame.size.height * tf.position[1],
                            );
                        let rect = Bounds::new(
                            point(center.x - scaled.width / 2., center.y - scaled.height / 2.),
                            scaled,
                        );
                        let _ = window.paint_image(rect, rect, Corners::default(), image, 0, false);
                        if tf.opacity < 1. {
                            // The program is composited on black.
                            window.paint_quad(fill(rect, black().opacity(1. - tf.opacity)));
                        }
                    }
                    paint_lettering(&letters, frame, fg, window, cx);
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

fn size_scaled(s: Size<Pixels>, k: f32) -> Size<Pixels> {
    size(s.width * k, s.height * k)
}

fn paint_lettering(
    letters: &[Lettering],
    frame: Bounds<Pixels>,
    fg: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    let h = frame.size.height;
    for l in letters {
        let font_size = if l.caption {
            (h * 0.05).max(px(9.))
        } else if l.lower_third {
            (h * 0.045).max(px(9.))
        } else {
            (h * 0.08).max(px(12.))
        };
        let mut font = window.text_style().font();
        if !l.caption {
            font.weight = FontWeight::SEMIBOLD;
        }
        let run = TextRun {
            len: l.text.len(),
            font,
            color: fg,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window
            .text_system()
            .shape_line(l.text.clone(), font_size, &[run], None);
        let line_h = font_size * 1.3;
        let w = line.width;
        let origin = if l.caption {
            point(frame.center().x - w / 2., frame.bottom() - h * 0.1 - line_h)
        } else if l.lower_third {
            point(
                frame.origin.x + frame.size.width * 0.06,
                frame.bottom() - h * 0.24,
            )
        } else {
            point(frame.center().x - w / 2., frame.center().y - line_h / 2.)
        };
        if l.caption || l.lower_third {
            let pad = font_size * 0.4;
            window.paint_quad(
                fill(
                    Bounds::new(
                        origin - point(pad, pad * 0.4),
                        size(w + pad * 2., line_h + pad * 0.8),
                    ),
                    black().opacity(0.55),
                )
                .corner_radii(px(3.)),
            );
        }
        let _ = line.paint(origin, line_h, TextAlign::Left, None, window, cx);
    }
}
