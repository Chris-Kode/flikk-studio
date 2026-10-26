//! The program-frame compositor: what the timeline shows at a time, as one
//! picture. The viewer (paused and playing), the export and the agent's
//! view tools all call [`program`], so effects, transitions, generators and
//! blend modes look the same everywhere.
//!
//! Layers are drawn bottom-up from the unmuted video, overlay and title
//! tracks (titles and captions themselves are lettering, drawn by the
//! viewer and burned in by the export; generators on title tracks are
//! pictures). On each track, the enabled clip under the playhead, or the
//! transition covering it:
//!
//! 1. **Picture**: a media frame at the clip's source time
//!    ([`Clip::to_source`], so speed is honoured), an overlay asset, or a
//!    generator ([`FxService::generate`] for GLSL, [`HtmlService::frame`]
//!    for HTML) laid out at the sequence size.
//! 2. **Grade**: the clip's, then for media the timeline's
//!    ([`crate::grade`]).
//! 3. **Effects**: the clip's enabled effect stack ([`FxService::apply`])
//!    at clip-relative time. Effects run in sequence pixels: the picture is
//!    first resampled to its fitted size in the sequence frame, so a blur
//!    radius means the same in the viewer and in any render.
//! 4. **Transform** and **blend**: fitted inside the canvas (as the export
//!    graph does), scaled, rotated clockwise about its centre, offset by
//!    `position` × canvas size, cropped, flipped, faded by opacity and
//!    blended with its [`BlendMode`] ([`FxService::composite`], or
//!    [`composite_cpu`] without a GPU engine).
//!
//! A transition renders each side's layer onto its own transparent canvas,
//! blends them with [`FxService::transition`] at the transition's progress
//! (premultiplied, so fading from or to nothing does not darken), and
//! draws the result over the tracks below. Sides read their sources past
//! the clip edges; [`Pictures`] holds the edge frame when a source runs
//! out. Without a GPU engine a transition is a cross-dissolve and effects
//! and GLSL generators are skipped.

mod cpu;
mod pictures;

pub use cpu::{blend_channel, composite_cpu};
pub use pictures::{Pictures, Stills, Streams, overlay_item};

use crate::fx::{self, FxAsset, FxInstance, FxKind, FxLanguage, PlacedTransition};
use crate::grade;
use crate::model::{
    BlendMode, Clip, ClipContent, Grade, MediaKind, Project, Property, Track, TrackKind, Transform,
};
use crate::services::{CompositeLayer, Frame, FxCall, Placement, Services};
use crate::time::{Time, TimeRange};

/// The program at timeline time `t`, `size` pixels, on black.
pub fn program(
    project: &Project,
    t: Time,
    size: (u32, u32),
    services: &Services,
    pictures: &mut dyn Pictures,
) -> Frame {
    let size = (size.0.max(1), size.1.max(1));
    let mut ctx = Ctx {
        project,
        services,
        pictures,
        size,
    };
    let mut layers: Vec<(Frame, Placement)> = Vec::new();
    for track in picture_tracks(project) {
        let placed = fx::transitions(track);
        if let Some(pt) = placed
            .iter()
            .find(|p| p.transition.fx.enabled && p.range.contains(t))
        {
            layers.extend(ctx.transition(pt, t));
        } else if let Some(clip) = track.clip_at(t).filter(|c| c.enabled) {
            layers.extend(ctx.layer(clip, t));
        }
    }
    let frame = ctx.composite(size, [0, 0, 0, 255], &layers);
    ctx.pictures.finish();
    frame
}

/// What `clip` shows at timeline time `t` before its transform: its
/// picture, graded, through the first `effects` entries of its effect
/// stack (all when `None`). Generators without effects are rendered at
/// `size`. `None` when the clip has no picture (audio, titles, captions)
/// or it could not be made.
pub fn clip_picture(
    project: &Project,
    clip: &Clip,
    t: Time,
    size: (u32, u32),
    services: &Services,
    pictures: &mut dyn Pictures,
    effects: Option<usize>,
) -> Option<Frame> {
    let mut ctx = Ctx {
        project,
        services,
        pictures,
        size: (size.0.max(1), size.1.max(1)),
    };
    let frame = ctx.picture(clip, t, effects);
    ctx.pictures.finish();
    frame
}

/// Whether `range` uses anything the FFmpeg graph cannot draw exactly:
/// effects, generators, transitions, non-normal blend modes, crop, flip or
/// retimed media.
pub fn needs_compositor(project: &Project, range: TimeRange) -> bool {
    picture_tracks(project).any(|track| {
        fx::transitions(track)
            .iter()
            .any(|p| p.transition.fx.enabled && p.range.overlaps(&range))
            || track
                .clips
                .iter()
                .filter(|c| c.enabled && c.range().overlaps(&range))
                .any(|c| {
                    let tf = &c.transform;
                    c.effects.iter().any(|e| e.enabled)
                        || matches!(c.content, ClipContent::Generator { .. })
                        || tf.blend != BlendMode::Normal
                        || tf.flip.iter().any(|f| *f)
                        || tf.crop.iter().any(|v| *v > 0.0)
                        || c.keyframes.iter().any(|k| {
                            matches!(
                                k.property,
                                Property::CropLeft
                                    | Property::CropTop
                                    | Property::CropRight
                                    | Property::CropBottom
                            )
                        })
                        || (c.media().is_some() && (c.speed - 1.0).abs() > 1e-9)
                })
    })
}

/// Program size for a `width`-pixel-wide picture of the sequence (even
/// sizes).
pub fn size_for_width(project: &Project, width: u32) -> (u32, u32) {
    let s = &project.timeline.settings;
    let h = width as u64 * s.height.max(1) as u64 / s.width.max(1) as u64;
    (width.max(2) & !1, (h as u32).max(2) & !1)
}

/// Where a `picture`-sized frame with transform `tf` lands on a `canvas`:
/// fitted inside it, scaled, centred plus `position` × canvas size, as in
/// the export graph.
pub fn placement(tf: &Transform, picture: (u32, u32), canvas: (u32, u32)) -> Placement {
    let (pw, ph) = (picture.0.max(1) as f32, picture.1.max(1) as f32);
    let (cw, ch) = (canvas.0 as f32, canvas.1 as f32);
    let fit = (cw / pw).min(ch / ph);
    let s = tf.scale.max(0.0);
    Placement {
        center: [
            cw / 2.0 + tf.position[0] * cw,
            ch / 2.0 + tf.position[1] * ch,
        ],
        size: [pw * fit * s, ph * fit * s],
        rotation_deg: tf.rotation_deg,
        crop: tf.crop.map(|c| c.clamp(0.0, 1.0)),
        flip: tf.flip,
        opacity: tf.opacity.clamp(0.0, 1.0),
        blend: tf.blend,
    }
}

/// `picture` fitted inside `canvas`, in whole pixels.
fn fit_size(picture: (u32, u32), canvas: (u32, u32)) -> (u32, u32) {
    let (pw, ph) = (picture.0.max(1) as f64, picture.1.max(1) as f64);
    let fit = (canvas.0 as f64 / pw).min(canvas.1 as f64 / ph);
    (
        ((pw * fit).round() as u32).max(1),
        ((ph * fit).round() as u32).max(1),
    )
}

/// Tracks that can show pictures, bottom-up.
fn picture_tracks(project: &Project) -> impl Iterator<Item = &Track> {
    project.timeline.tracks.iter().rev().filter(|t| {
        matches!(
            t.kind,
            TrackKind::Video | TrackKind::Overlay | TrackKind::Title
        ) && !t.mix.mute
    })
}

struct Ctx<'a> {
    project: &'a Project,
    services: &'a Services,
    pictures: &'a mut dyn Pictures,
    /// Output size.
    size: (u32, u32),
}

impl Ctx<'_> {
    fn sequence(&self) -> (u32, u32) {
        let s = &self.project.timeline.settings;
        (s.width.max(1), s.height.max(1))
    }

    /// `clip` at `t`, ready to composite.
    fn layer(&mut self, clip: &Clip, t: Time) -> Option<(Frame, Placement)> {
        let tf = clip.transform_at(t - clip.start);
        if tf.opacity <= 0.0 || tf.scale <= 0.0 {
            return None;
        }
        let picture = self.picture(clip, t, None)?;
        let place = placement(&tf, (picture.width, picture.height), self.size);
        Some((picture, place))
    }

    fn picture(&mut self, clip: &Clip, t: Time, limit: Option<usize>) -> Option<Frame> {
        let local = t - clip.start;
        let effects: Vec<(&FxInstance, &FxAsset)> = clip
            .effects
            .iter()
            .take(limit.unwrap_or(usize::MAX))
            .filter(|e| e.enabled)
            .filter_map(|e| {
                let asset = fx::asset(self.project, &e.fx)?;
                (asset.kind == FxKind::Effect).then_some((e, asset))
            })
            .collect();
        let engine = self.services.fx.clone().filter(|_| !effects.is_empty());
        let picture = match &clip.content {
            ClipContent::Media { media, .. } => {
                let item = self.project.media(*media)?;
                if item.kind == MediaKind::Audio {
                    return None;
                }
                self.pictures.frame(clip.id, item, clip.to_source(t)?)?
            }
            ClipContent::Overlay { asset, .. } => {
                let item = overlay_item(clip.id, asset);
                self.pictures.frame(clip.id, &item, local)?
            }
            ClipContent::Generator { fx } => {
                let size = if engine.is_some() {
                    self.sequence()
                } else {
                    self.size
                };
                self.generate(clip, fx, local, size)?
            }
            ClipContent::Title { .. } | ClipContent::Caption { .. } => return None,
        };
        let timeline = matches!(clip.content, ClipContent::Media { .. })
            .then_some(&self.project.timeline.grade);
        let grades: Vec<&Grade> = std::iter::once(&clip.grade)
            .chain(timeline)
            .filter(|g| !grade::is_neutral(g))
            .collect();
        let picture = if grades.is_empty() {
            picture
        } else {
            grade::graded(&picture, &grades)
        };
        let Some(engine) = engine else {
            return Some(picture);
        };
        let fitted = fit_size((picture.width, picture.height), self.sequence());
        let picture = if (picture.width, picture.height) == fitted {
            picture
        } else {
            let fill = Placement::fill(fitted.0, fitted.1);
            self.composite(fitted, [0; 4], &[(picture, fill)])
        };
        let values: Vec<_> = effects
            .iter()
            .map(|(e, asset)| e.values_at(asset, local))
            .collect();
        let calls: Vec<FxCall> = effects
            .iter()
            .zip(&values)
            .map(|((_, asset), values)| FxCall {
                asset,
                values,
                time: local.as_secs_f64(),
                duration: clip.duration.as_secs_f64(),
                canvas: self.sequence(),
            })
            .collect();
        match engine.apply(&picture, &calls) {
            Ok(frame) => Some(frame),
            // One broken program should not hide the clip: run the others.
            Err(_) => Some(calls.iter().fold(picture, |frame, call| {
                engine
                    .apply(&frame, std::slice::from_ref(call))
                    .unwrap_or(frame)
            })),
        }
    }

    fn generate(
        &self,
        clip: &Clip,
        instance: &FxInstance,
        local: Time,
        (w, h): (u32, u32),
    ) -> Option<Frame> {
        let asset = fx::asset(self.project, &instance.fx)?;
        if asset.kind != FxKind::Generator {
            return None;
        }
        let values = instance.values_at(asset, local);
        let call = FxCall {
            asset,
            values: &values,
            time: local.as_secs_f64(),
            duration: clip.duration.as_secs_f64(),
            canvas: self.sequence(),
        };
        match asset.language {
            FxLanguage::Glsl => self.services.fx.as_ref()?.generate(&call, w, h).ok(),
            FxLanguage::Html => self.services.html.as_ref()?.frame(&call, w, h).ok(),
        }
    }

    /// Both sides of `pt` on their own canvases, blended at `t`.
    fn transition(&mut self, pt: &PlacedTransition<'_>, t: Time) -> Option<(Frame, Placement)> {
        let size = self.size;
        let side = |ctx: &mut Self, clip: Option<&Clip>| -> Frame {
            match clip.and_then(|c| ctx.layer(c, t)) {
                Some((frame, place)) => {
                    let place = Placement {
                        blend: BlendMode::Normal,
                        ..place
                    };
                    cpu::premultiplied(&ctx.composite(size, [0; 4], &[(frame, place)]))
                }
                None => cpu::transparent(size.0, size.1),
            }
        };
        let from = side(self, pt.from);
        let to = side(self, pt.to);
        let local = t - pt.range.start;
        let instance = &pt.transition.fx;
        let blended = fx::asset(self.project, &instance.fx)
            .filter(|a| a.kind == FxKind::Transition)
            .zip(self.services.fx.as_ref())
            .and_then(|(asset, engine)| {
                let values = instance.values_at(asset, local);
                let call = FxCall {
                    asset,
                    values: &values,
                    time: local.as_secs_f64(),
                    duration: pt.range.duration().as_secs_f64(),
                    canvas: self.sequence(),
                };
                engine.transition(&from, &to, &call).ok()
            })
            .unwrap_or_else(|| cpu::mix(&from, &to, pt.progress(t)));
        let blend = pt
            .to
            .or(pt.from)
            .map_or(BlendMode::Normal, |c| c.transform.blend);
        Some((
            cpu::unpremultiplied(&blended),
            Placement {
                blend,
                ..Placement::fill(size.0, size.1)
            },
        ))
    }

    fn composite(
        &self,
        (w, h): (u32, u32),
        background: [u8; 4],
        layers: &[(Frame, Placement)],
    ) -> Frame {
        let refs: Vec<CompositeLayer> = layers
            .iter()
            .map(|(frame, placement)| CompositeLayer {
                frame,
                placement: *placement,
            })
            .collect();
        if let Some(engine) = &self.services.fx
            && let Ok(frame) = engine.composite(w, h, background, &refs)
        {
            return frame;
        }
        composite_cpu(w, h, background, &refs)
    }
}
