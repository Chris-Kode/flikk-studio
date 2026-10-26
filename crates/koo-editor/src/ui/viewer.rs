//! Program monitor: `EditorState::display_project()` at the playhead, so a
//! previewed change set shows its draft. Paused frames come from
//! `MediaService::frame` off-thread (latest request wins); playback streams
//! frames and mixes audio in background threads. Like the render, every
//! video and overlay layer is composited bottom-up with its grade and
//! transform, and captions and titles are drawn in their export styles.

mod audio;
mod grade;
mod player;

use super::timeline::glyph::{Glyph, glyph};
use super::timeline::{
    NextCut, NextFrame, PrevCut, PrevFrame, ShuttleForward, ShuttleReverse, ShuttleStop,
};
use crate::app::TogglePlay;
use crate::state::EditorState;
use audio::AudioOut;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use koo_core::memory::{CaptionPosition, CaptionStyle};
use koo_core::model::{ClipContent, Project, TrackKind};
use koo_core::{ChangeSetId, ClipId, Time};
use koo_deliver::captions;
use player::{Look, Picture, Player, layers_at};
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
    /// Items in the previewed change set, which grows without a version.
    draft: usize,
    width: u32,
}

/// A paused frame request: width, and per layer its clip, source frame
/// index and look.
type Request = (u32, Vec<(ClipId, i64, Look)>);

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
    /// Layers on screen, bottom-up.
    pictures: Vec<Picture>,
    /// Paused frame last asked for.
    requested: Option<Request>,
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
            pictures: Vec::new(),
            requested: None,
            generation: 0,
            retired: Vec::new(),
            playback: None,
            frame_width: Rc::new(Cell::new(640)),
            _observe,
        }
    }

    fn show(&mut self, pictures: Vec<Picture>) {
        for old in std::mem::replace(&mut self.pictures, pictures) {
            if !self
                .pictures
                .iter()
                .any(|p| Arc::ptr_eq(&p.image, &old.image))
            {
                self.retired.push(old.image);
            }
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
            draft: state
                .preview
                .and_then(|id| state.bus.change_set(id))
                .map_or(0, |set| set.items.len()),
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
        let pictures = self.playback.as_mut().and_then(|p| {
            p.last = playhead;
            p.player.set_target(playhead);
            p.player.take()
        });
        if let Some(pictures) = pictures {
            self.show(pictures);
            self.requested = None;
        }
    }

    /// Paused (or shuttling): fetch every layer under the playhead.
    fn request_frame(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let Some(media) = state.services.media.clone() else {
            self.show(Vec::new());
            return;
        };
        let project = state.display_project();
        let t = state.playhead;
        let seq_rate = project.timeline.settings.rate;
        let width = self.frame_width.get();
        let layers: Vec<_> = layers_at(project, t)
            .into_iter()
            .map(|l| {
                (
                    l.clip.id,
                    l.item.into_owned(),
                    l.source,
                    Look::of(l.clip, t, project),
                )
            })
            .collect();
        let key = (
            width,
            layers
                .iter()
                .map(|(id, item, source, look)| {
                    let rate = item
                        .info
                        .as_ref()
                        .and_then(|i| i.video.as_ref())
                        .map(|v| v.rate)
                        .unwrap_or(seq_rate);
                    (*id, source.to_frame(rate), look.clone())
                })
                .collect(),
        );
        if self.requested.as_ref() == Some(&key) {
            return;
        }
        self.requested = Some(key);
        self.generation += 1;
        if layers.is_empty() {
            self.show(Vec::new());
            return;
        }
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let pictures = cx
                .background_executor()
                .spawn(async move {
                    layers
                        .iter()
                        .filter_map(|(id, item, source, look)| {
                            let frame = media.frame(item, *source, width).ok()?;
                            player::picture(*id, &frame, look)
                        })
                        .collect()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Latest request wins: a newer one is on its way.
                if this.generation == generation {
                    this.show(pictures);
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
        let source = layers_at(project, t).into_iter().last().and_then(|l| {
            let (at, item) = (l.source, l.item);
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

/// Where a block of text sits, as the burned-in ASS styles place it.
#[derive(Clone, Copy)]
enum Anchor {
    Top,
    Middle,
    Bottom,
    /// Left-aligned, 6% in and 12% up.
    LowerThird,
}

/// Text drawn over the picture at the playhead, styled like the render
/// (`koo_deliver::captions::to_ass`): sizes in 1080p units, bold white
/// with a black outline and a half-transparent shadow.
struct Lettering {
    lines: Vec<SharedString>,
    font: SharedString,
    size: f32,
    outline: f32,
    shadow: f32,
    anchor: Anchor,
}

fn lettering(project: &Project, styles: &[CaptionStyle], t: Time) -> Vec<Lettering> {
    let mut out = Vec::new();
    for track in project.timeline.tracks.iter().filter(|t| !t.mix.mute) {
        let Some(clip) = track.clip_at(t) else {
            continue;
        };
        match (&clip.content, track.kind) {
            (ClipContent::Title { text, template }, _) => {
                let lower = template.contains("lower");
                out.push(Lettering {
                    lines: text.lines().map(|l| l.to_string().into()).collect(),
                    font: "Inter".into(),
                    size: if lower { 54. } else { 96. },
                    outline: if lower { 2. } else { 3. },
                    shadow: 0.,
                    anchor: if lower {
                        Anchor::LowerThird
                    } else {
                        Anchor::Middle
                    },
                });
            }
            (ClipContent::Caption { text, .. }, TrackKind::Caption) if !text.trim().is_empty() => {
                let style = captions::style_for(track, styles);
                out.push(Lettering {
                    lines: captions::wrap(text, &style)
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                    font: style.font.clone().into(),
                    size: style.size,
                    outline: style.outline,
                    shadow: style.shadow,
                    anchor: match style.position {
                        CaptionPosition::Top => Anchor::Top,
                        CaptionPosition::Middle => Anchor::Middle,
                        CaptionPosition::Bottom => Anchor::Bottom,
                    },
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
        // Position and scale are placed at paint time; grade, rotation and
        // opacity are already in the pixels.
        let layers: Vec<_> = self
            .pictures
            .iter()
            .filter_map(|p| {
                let (_, c) = project.timeline.clip(p.clip)?;
                Some((p.clone(), c.transform_at(t - c.start)))
            })
            .collect();
        let letters = lettering(project, &state.presets.caption_styles, t);
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
                    for (p, tf) in layers {
                        // Fit the source, grow by the rotation, then scale.
                        let base = fit(frame, p.aspect, px(0.));
                        let k = tf.scale.max(0.);
                        let scaled = size(
                            base.size.width * (p.grown.0 * k),
                            base.size.height * (p.grown.1 * k),
                        );
                        let center = frame.center()
                            + point(
                                frame.size.width * tf.position[0],
                                frame.size.height * tf.position[1],
                            );
                        let rect = Bounds::new(
                            point(center.x - scaled.width / 2., center.y - scaled.height / 2.),
                            scaled,
                        );
                        let _ =
                            window.paint_image(rect, rect, Corners::default(), p.image, 0, false);
                    }
                    for l in &letters {
                        paint_text(l, frame, window, cx);
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
    player::to_image(frame, None)
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

fn paint_text(l: &Lettering, frame: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let (w, h) = (frame.size.width, frame.size.height);
    let k = f32::from(h) / 1080.;
    let font_size = px((l.size * k).max(4.));
    let line_h = font_size * 1.2;
    let block = line_h * l.lines.len() as f32;
    let margin = h * 0.07;
    let top = match l.anchor {
        Anchor::Top => frame.top() + margin,
        Anchor::Middle => frame.center().y - block / 2.,
        Anchor::Bottom => frame.bottom() - margin - block,
        Anchor::LowerThird => frame.bottom() - h * 0.12 - block,
    };
    let mut font = window.text_style().font();
    font.family = l.font.clone();
    font.weight = FontWeight::BOLD;
    let outline = l.outline * k;
    let shadow = l.shadow * k;
    // Outline as copies of the line around the glyphs.
    let ring: Vec<Point<Pixels>> = if outline > 0. {
        let steps = if outline > 2. { 16 } else { 8 };
        (0..steps)
            .map(|i| {
                let a = i as f32 / steps as f32 * std::f32::consts::TAU;
                point(px(a.cos() * outline), px(a.sin() * outline))
            })
            .collect()
    } else {
        Vec::new()
    };
    for (i, text) in l.lines.iter().enumerate() {
        let line = shape(window, text, font.clone(), font_size, white());
        let x = match l.anchor {
            Anchor::LowerThird => frame.origin.x + w * 0.06,
            _ => frame.center().x - line.width / 2.,
        };
        let origin = point(x, top + line_h * i as f32);
        if shadow > 0. {
            let dark = shape(window, text, font.clone(), font_size, black().opacity(0.5));
            let _ = dark.paint(
                origin + point(px(shadow), px(shadow)),
                line_h,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
        if !ring.is_empty() {
            let dark = shape(window, text, font.clone(), font_size, black());
            for offset in &ring {
                let _ = dark.paint(origin + *offset, line_h, TextAlign::Left, None, window, cx);
            }
        }
        let _ = line.paint(origin, line_h, TextAlign::Left, None, window, cx);
    }
}

fn shape(
    window: &mut Window,
    text: &SharedString,
    font: Font,
    font_size: Pixels,
    color: Hsla,
) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text.clone(), font_size, &[run], None)
}
