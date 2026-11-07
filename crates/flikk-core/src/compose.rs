//! The program-frame compositor: what the timeline shows at a time, as one
//! picture. The viewer (paused and playing), the export and the agent's
//! view tools all call [`program`] (renders [`render_program`], which fails
//! where [`program`] leaves out a generator it could not draw), so effects,
//! transitions, generators and blend modes look the same everywhere.
//!
//! Layers are drawn bottom-up from the unmuted video, overlay and title
//! tracks (generators on title tracks are pictures). On each track, the
//! enabled clip under the playhead, or the transition covering it:
//!
//! 1. **Picture**: a media frame at the clip's source time
//!    ([`Clip::to_source`], so speed and ramps are honoured; slowed clips
//!    show the moment between two source frames as their
//!    [`Interpolation`] says: the two mixed, or the optical flow frame
//!    from [`Services::interpolation`] once made), an overlay asset, or a
//!    generator ([`FxService::generate`] for GLSL, [`HtmlService::frame`]
//!    for HTML) laid out at the output size. A stabilized media frame is
//!    moved, turned and zoomed inside its own frame by the correction of
//!    its source frame ([`crate::stabilize::correction`]).
//! 2. **Grade**: the clip's, then for media the timeline's
//!    ([`crate::grade`]).
//! 3. **Effects**: the clip's enabled effect stack ([`FxService::apply`])
//!    at clip-relative time. Effects that read depth get the source
//!    frame's [`DepthMap`] beside the picture ([`FxBatch::apply_depth`]);
//!    until it is made they are left out. Effects run at the output size: the picture is
//!    first resampled to its fitted size in the output frame. Programs
//!    measure `iResolution` in sequence pixels whatever size they run at,
//!    so a blur radius means the same in a small viewer and in any render.
//! 4. **Masks**: the clip's masks, drawn in the picture's own uv
//!    ([`crate::mask::raster`]), multiply its alpha ([`FxBatch::mask`]).
//!    Subject masks are drawn from their mattes at the source time
//!    ([`subject_mattes`]), refined onto the decoded frame's edges; person
//!    masks from theirs ([`person_mattes`]), eased onto the same frame's
//!    edges by their `edge` amount; depth masks from the same depth map as
//!    the effects. With
//!    [`MaskView::Tinted`] the picture stays whole and what the masks cut
//!    away is tinted over it instead, for editing them.
//! 5. **Transform** and **blend**: fitted inside the canvas (as the export
//!    graph does), stretched and scaled, skewed and rotated clockwise about
//!    its anchor, which lands at the centre plus `position` × canvas size
//!    ([`placement`]), cropped, flipped, faded by opacity and
//!    blended with its [`BlendMode`] ([`FxService::composite`], or
//!    [`composite_cpu`] without a GPU engine).
//!
//! Titles and captions are lettering, drawn over every picture by
//! [`Services::lettering`] when it is wired (the viewer and the agent's
//! views, and renders on an FFmpeg without libass) and transformed like
//! pictures; other renders burn them in with FFmpeg instead.
//!
//! A transition renders each side's layer onto its own transparent canvas,
//! blends them with [`FxService::transition`] at the transition's progress
//! (premultiplied, so fading from or to nothing does not darken), and
//! draws the result over the tracks below. Sides read their sources past
//! the clip edges; [`Pictures`] holds the edge frame when a source runs
//! out.
//!
//! Every step of a frame runs in one [`FxBatch`]: with the GPU engine each
//! source picture is uploaded once and the frame is read back once. Without
//! a GPU engine, or when the GPU fails mid-frame, the frame is composed on
//! the CPU: a transition is a cross-dissolve and effects and GLSL
//! generators are skipped.
//!
//! [`FxService::generate`]: crate::services::FxService::generate
//! [`FxService::apply`]: crate::services::FxService::apply
//! [`FxService::composite`]: crate::services::FxService::composite
//! [`FxService::transition`]: crate::services::FxService::transition
//! [`HtmlService::frame`]: crate::services::HtmlService::frame

mod batch;
mod cpu;
mod layout;
mod pictures;

pub use cpu::{blend_channel, composite_cpu};
pub use layout::{GroupMove, Shape, group_bounds, layout};
pub use pictures::{Pictures, Stills, Streams, frame_length, overlay_item};

use crate::depth::{self, DepthMap};
use crate::fx::{self, FxAsset, FxInstance, FxKind, FxLanguage, PlacedTransition};
use crate::grade;
use crate::ids::{ClipId, MaskId};
use crate::model::{
    BlendMode, Clip, ClipContent, Grade, MediaItem, MediaKind, Project, Property, Track, TrackKind,
    Transform,
};
use crate::person::PersonMatte;
use crate::services::{
    Between, Frame, FxBatch, FxCall, Lettered, PendingFrame, PersonRef, PixelOrder, Placement,
    ServiceError, ServiceResult, Services, SubjectRef, Surface,
};
use crate::speed::{BETWEEN_STEPS, Interpolation};
use crate::subject::Matte;
use crate::time::{Time, TimeRange};
use batch::CpuBatch;
use std::sync::Arc;

/// How masks show in a program frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MaskView {
    /// Cut away, as rendered.
    #[default]
    Cut,
    /// The picture whole, what the masks of a clip (or with `None`, of
    /// every clip) cut away tinted over it.
    Tinted(Option<ClipId>),
}

/// Colour and strength of [`MaskView::Tinted`].
const TINT: [u8; 3] = [255, 40, 72];
const TINT_ALPHA: f32 = 0.55;

/// The program at timeline time `t`, `size` pixels, on black.
pub fn program(
    project: &Project,
    t: Time,
    size: (u32, u32),
    services: &Services,
    pictures: &mut dyn Pictures,
) -> Frame {
    program_with(project, t, size, services, pictures, MaskView::Cut)
}

/// [`program`] with masks shown as `view` says.
pub fn program_with(
    project: &Project,
    t: Time,
    size: (u32, u32),
    services: &Services,
    pictures: &mut dyn Pictures,
    view: MaskView,
) -> Frame {
    let size = (size.0.max(1), size.1.max(1));
    let (frame, _) = render(
        project,
        services,
        pictures,
        size,
        PixelOrder::Rgba,
        view,
        |ctx| ctx.program(t),
    );
    pictures.finish();
    frame.unwrap_or_else(|| black(size))
}

/// [`program`] for a render, which must not leave a clip out: fails with
/// why when a generator could not be drawn (an HTML clip whose browser did
/// not start, a GLSL program the GPU did not run), where [`program`] draws
/// the frame without it.
pub fn render_program(
    project: &Project,
    t: Time,
    size: (u32, u32),
    services: &Services,
    pictures: &mut dyn Pictures,
) -> Result<Frame, String> {
    let size = (size.0.max(1), size.1.max(1));
    let (frame, failed) = render(
        project,
        services,
        pictures,
        size,
        PixelOrder::Rgba,
        MaskView::Cut,
        |ctx| ctx.program(t),
    );
    pictures.finish();
    match failed {
        Some(e) => Err(e),
        None => Ok(frame.unwrap_or_else(|| black(size))),
    }
}

/// The subject mask `mask` of `clip`, as [`SubjectService`] sees it: on
/// media from the pool, and enabled.
///
/// [`SubjectService`]: crate::services::SubjectService
pub fn subject_ref<'a>(
    project: &'a Project,
    clip: &'a Clip,
    mask: &'a crate::mask::Mask,
) -> Option<SubjectRef<'a>> {
    let ClipContent::Media { media, .. } = &clip.content else {
        return None;
    };
    let subject = mask.picks()?;
    let item = project.media(*media)?;
    let (a, b) = (clip.to_source(clip.start)?, clip.to_source(clip.end())?);
    Some(SubjectRef {
        mask: mask.id,
        item,
        subject,
        range: TimeRange::new(a.min(b), a.max(b)),
    })
}

/// The mattes of `clip`'s enabled subject masks at timeline time `t`, those
/// made so far.
pub fn subject_mattes(
    project: &Project,
    services: &Services,
    clip: &Clip,
    t: Time,
) -> Vec<(MaskId, Matte)> {
    let Some(service) = &services.subjects else {
        return Vec::new();
    };
    let Some(source) = clip.to_source(t) else {
        return Vec::new();
    };
    clip.masks
        .iter()
        .filter(|m| m.enabled)
        .filter_map(|m| {
            let subject = subject_ref(project, clip, m)?;
            Some((m.id, service.matte(subject, source)?))
        })
        .collect()
}

/// The person mask `mask` of `clip`, as [`SubjectService`] sees it: on
/// media from the pool.
///
/// [`SubjectService`]: crate::services::SubjectService
pub fn person_ref<'a>(
    project: &'a Project,
    clip: &'a Clip,
    mask: &'a crate::mask::Mask,
) -> Option<PersonRef<'a>> {
    let ClipContent::Media { media, .. } = &clip.content else {
        return None;
    };
    if !mask.is_person() {
        return None;
    }
    let item = project.media(*media)?;
    let (a, b) = (clip.to_source(clip.start)?, clip.to_source(clip.end())?);
    Some(PersonRef {
        mask: mask.id,
        item,
        range: TimeRange::new(a.min(b), a.max(b)),
    })
}

/// The mattes of `clip`'s enabled person masks at timeline time `t`, those
/// made so far.
pub fn person_mattes(
    project: &Project,
    services: &Services,
    clip: &Clip,
    t: Time,
) -> Vec<(MaskId, PersonMatte)> {
    let Some(service) = &services.subjects else {
        return Vec::new();
    };
    let Some(source) = clip.to_source(t) else {
        return Vec::new();
    };
    clip.masks
        .iter()
        .filter(|m| m.enabled)
        .filter_map(|m| {
            let person = person_ref(project, clip, m)?;
            Some((m.id, service.person(person, source)?))
        })
        .collect()
}

/// The call of every HTML generator [`program`] draws at `t`, in the order
/// it draws them.
pub fn html_calls(project: &Project, t: Time, mut each: impl FnMut(&FxCall<'_>)) {
    let s = &project.timeline.settings;
    let canvas = (s.width.max(1), s.height.max(1));
    let mut generator = |clip: &Clip| {
        let ClipContent::Generator { fx: instance } = &clip.content else {
            return;
        };
        let Some(asset) = fx::asset(project, &instance.fx) else {
            return;
        };
        if asset.kind != FxKind::Generator || asset.language != FxLanguage::Html {
            return;
        }
        let local = t - clip.start;
        let tf = clip.transform_at(local);
        if tf.opacity <= 0.0 || tf.scale <= 0.0 || tf.stretch.iter().any(|s| *s <= 0.0) {
            return;
        }
        each(&FxCall {
            asset,
            values: &instance.values_at(asset, local),
            time: local.as_secs_f64(),
            duration: clip.duration.as_secs_f64(),
            canvas,
        });
    };
    for track in picture_tracks(project) {
        let placed = fx::transitions(track);
        if let Some(pt) = placed
            .iter()
            .find(|p| p.transition.fx.enabled && p.range.contains(t))
        {
            pt.from.into_iter().chain(pt.to).for_each(&mut generator);
        } else if let Some(clip) = track.clip_at(t).filter(|c| c.enabled) {
            generator(clip);
        }
    }
}

/// [`program`] in two halves, for playback: pictures are decoded, uploaded
/// and the frame submitted now; [`Pending::wait`] reads it back in `order`,
/// so the next frame can be prepared while the GPU works on this one.
pub fn submit_program<'a>(
    project: &Project,
    t: Time,
    size: (u32, u32),
    services: &'a Services,
    pictures: &mut dyn Pictures,
    order: PixelOrder,
    view: MaskView,
) -> Pending<'a> {
    let size = (size.0.max(1), size.1.max(1));
    let draw = |ctx: &mut Ctx<'_, '_>| ctx.program(t);
    let submitted = services.fx.as_ref().and_then(|engine| {
        let mut ctx = Ctx {
            project,
            services,
            pictures: &mut *pictures,
            size,
            batch: engine.batch(),
            programs: true,
            view,
            guide: None,
            depth: None,
            failed: None,
        };
        let surface = draw(&mut ctx)?;
        ctx.batch
            .submit(surface, order)
            .map_err(|e| eprintln!("flikk-core: GPU frame failed, composing on the CPU: {e}"))
            .ok()
    });
    let frame = submitted.unwrap_or_else(|| {
        let (frame, _) = render_cpu(project, services, pictures, size, order, view, draw);
        Box::new(frame.unwrap_or_else(|| black(size)))
    });
    pictures.finish();
    Pending { frame, size }
}

/// A program frame from [`submit_program`] on its way back.
pub struct Pending<'a> {
    frame: Box<dyn PendingFrame + 'a>,
    size: (u32, u32),
}

impl Pending<'_> {
    /// The frame; black when the GPU failed after it was submitted.
    pub fn wait(self) -> Frame {
        self.frame.wait().unwrap_or_else(|e| {
            eprintln!("flikk-core: GPU frame failed: {e}");
            black(self.size)
        })
    }
}

/// What `clip` shows at timeline time `t` before its transform: its
/// picture, graded, through the first `effects` entries of its effect
/// stack (all when `None`). Generators, and pictures with effects, are
/// fitted inside `size`. `None` when the clip has no picture (audio,
/// titles, captions) or it could not be made.
pub fn clip_picture(
    project: &Project,
    clip: &Clip,
    t: Time,
    size: (u32, u32),
    services: &Services,
    pictures: &mut dyn Pictures,
    effects: Option<usize>,
) -> Option<Frame> {
    let size = (size.0.max(1), size.1.max(1));
    let view = MaskView::Cut;
    let (frame, _) = render(
        project,
        services,
        pictures,
        size,
        PixelOrder::Rgba,
        view,
        |ctx| ctx.picture(clip, t, effects),
    );
    pictures.finish();
    frame
}

/// The picture `draw` makes, read back from the GPU engine's batch, or
/// composed on the CPU without one or when the GPU fails; and the first
/// clip that pass could not draw ([`Ctx::failed`]).
fn render(
    project: &Project,
    services: &Services,
    pictures: &mut dyn Pictures,
    size: (u32, u32),
    order: PixelOrder,
    view: MaskView,
    draw: impl Fn(&mut Ctx<'_, '_>) -> Option<Surface>,
) -> (Option<Frame>, Option<String>) {
    if let Some(engine) = &services.fx {
        let mut ctx = Ctx {
            project,
            services,
            pictures: &mut *pictures,
            size,
            batch: engine.batch(),
            programs: true,
            view,
            guide: None,
            depth: None,
            failed: None,
        };
        let Some(surface) = draw(&mut ctx) else {
            return (None, ctx.failed);
        };
        match ctx.batch.submit(surface, order).and_then(|p| p.wait()) {
            Ok(frame) => return (Some(frame), ctx.failed),
            Err(e) => eprintln!("flikk-core: GPU frame failed, composing on the CPU: {e}"),
        }
    }
    render_cpu(project, services, pictures, size, order, view, draw)
}

/// The picture `draw` makes without the GPU engine, and the first clip it
/// could not draw.
fn render_cpu(
    project: &Project,
    services: &Services,
    pictures: &mut dyn Pictures,
    size: (u32, u32),
    order: PixelOrder,
    view: MaskView,
    draw: impl Fn(&mut Ctx<'_, '_>) -> Option<Surface>,
) -> (Option<Frame>, Option<String>) {
    let mut ctx = Ctx {
        project,
        services,
        pictures,
        size,
        batch: Box::new(CpuBatch::default()),
        programs: false,
        view,
        guide: None,
        depth: None,
        failed: None,
    };
    let Some(surface) = draw(&mut ctx) else {
        return (None, ctx.failed);
    };
    let frame = ctx.batch.submit(surface, order).and_then(|p| p.wait()).ok();
    (frame, ctx.failed)
}

/// [`TINT`] where `raster` (a mask raster) cuts the picture away, clear
/// where it keeps it.
fn cut_tint(raster: &Frame) -> Frame {
    let rgba: Vec<u8> = raster
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let cut = 1.0 - p[3] as f32 / 255.0;
            let a = (cut * TINT_ALPHA * 255.0).round() as u8;
            [TINT[0], TINT[1], TINT[2], a]
        })
        .collect();
    Frame {
        rgba: Arc::from(rgba),
        ..*raster
    }
}

/// A black `size` frame.
fn black((w, h): (u32, u32)) -> Frame {
    composite_cpu(w, h, [0, 0, 0, 255], &[])
}

/// Whether `range` uses anything the FFmpeg graph cannot draw exactly:
/// effects, generators, transitions, non-normal blend modes, crop, flip,
/// stretch, skew, an anchor off the centre, a corner pin, masks,
/// stabilization or retimed media.
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
                        || is_free_form(tf)
                        || tf.is_pinned()
                        || crate::mask::any_enabled(&c.masks)
                        || crate::stabilize::applies(c)
                        || c.keyframes.iter().any(|k| {
                            Property::PIN.contains(&k.property)
                                || matches!(
                                    k.property,
                                    Property::CropLeft
                                        | Property::CropTop
                                        | Property::CropRight
                                        | Property::CropBottom
                                        | Property::ScaleX
                                        | Property::ScaleY
                                        | Property::SkewX
                                        | Property::SkewY
                                        | Property::AnchorX
                                        | Property::AnchorY
                                )
                        })
                        || (c.media().is_some() && c.is_retimed())
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

/// Steepest slant drawn, in degrees; beyond it a picture is nearly flat.
pub const MAX_SKEW: f32 = 80.0;

/// Where a `picture`-sized frame with transform `tf` lands on a `canvas`:
/// fitted to it by `tf.fit` and centred, then transformed
/// ([`placement_from`]), as in the export graph.
pub fn placement(tf: &Transform, picture: (u32, u32), canvas: (u32, u32)) -> Placement {
    let (pw, ph) = (picture.0.max(1) as f32, picture.1.max(1) as f32);
    let (cw, ch) = (canvas.0 as f32, canvas.1 as f32);
    let fit = tf.fit.scale(picture, canvas) as f32;
    placement_from(tf, [cw / 2.0, ch / 2.0], [pw * fit, ph * fit], canvas)
}

/// Where a picture resting at `center` with `size` (canvas pixels) lands
/// with transform `tf`: stretched and scaled, skewed and rotated clockwise
/// about its anchor, then moved by `position` × canvas size.
pub fn placement_from(
    tf: &Transform,
    center: [f32; 2],
    size: [f32; 2],
    canvas: (u32, u32),
) -> Placement {
    let (cw, ch) = (canvas.0 as f32, canvas.1 as f32);
    let s = tf.scale.max(0.0);
    let drawn = [
        size[0] * s * tf.stretch[0].max(0.0),
        size[1] * s * tf.stretch[1].max(0.0),
    ];
    let mut place = Placement {
        center: [0.0, 0.0],
        size: drawn,
        rotation_deg: tf.rotation_deg,
        skew_deg: tf.skew_deg.map(|d| d.clamp(-MAX_SKEW, MAX_SKEW)),
        crop: tf.crop.map(|c| c.clamp(0.0, 1.0)),
        flip: tf.flip,
        opacity: tf.opacity.clamp(0.0, 1.0),
        blend: tf.blend,
        pin: tf.corner_pin,
    };
    // The anchor stays where it rests (plus `position`); the centre is the
    // anchor's offset from it, scaled and turned with the picture.
    let [a, b, c, d] = place.linear();
    let rest = [tf.anchor[0] * size[0], tf.anchor[1] * size[1]];
    let (ax, ay) = (tf.anchor[0] * drawn[0], tf.anchor[1] * drawn[1]);
    place.center = [
        center[0] + rest[0] + tf.position[0] * cw - (a * ax + b * ay),
        center[1] + rest[1] + tf.position[1] * ch - (c * ax + d * ay),
    ];
    place
}

/// Whether `tf` uses anything beyond position, uniform scale, rotation and
/// opacity: stretch, skew or an anchor off the centre.
pub fn is_free_form(tf: &Transform) -> bool {
    tf.stretch.iter().any(|v| (v - 1.0).abs() > 1e-6)
        || tf.skew_deg.iter().any(|v| v.abs() > 1e-6)
        || tf.anchor.iter().any(|v| v.abs() > 1e-6)
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

/// Tracks that can show lettering, bottom-up: captions, then titles, over
/// every picture, as the render burns them in.
fn lettering_tracks(project: &Project) -> impl Iterator<Item = &Track> {
    let shown = |kind: TrackKind| {
        project
            .timeline
            .tracks
            .iter()
            .rev()
            .filter(move |t| t.kind == kind && !t.mix.mute)
    };
    shown(TrackKind::Caption).chain(shown(TrackKind::Title))
}

/// Whether `clip` on `track` is lettering: a title on a title track, or a
/// caption on a caption track.
pub fn is_lettering(track: &Track, clip: &Clip) -> bool {
    matches!(
        (&clip.content, track.kind),
        (ClipContent::Title { .. }, TrackKind::Title)
            | (ClipContent::Caption { .. }, TrackKind::Caption)
    )
}

/// `tf` without what lettering does not draw: the burned-in render cannot
/// crop, flip or blend it, nor pin its corners (ASS's 3D rotations tilt a
/// line but cannot map it onto an arbitrary quad), so the viewer and the
/// agent's views don't either.
pub fn lettering_transform(mut tf: Transform) -> Transform {
    tf.crop = [0.0; 4];
    tf.flip = [false; 2];
    tf.blend = BlendMode::Normal;
    tf.corner_pin = [[0.0; 2]; 4];
    tf
}

/// The caption or title `clip` on `track` shows at `t` on a `canvas`
/// (drawn `zoom` × finer, see [`LetteringService::letter`]), where it rests
/// and where it lands.
///
/// [`LetteringService::letter`]: crate::services::LetteringService::letter
fn lettering_layer(
    services: &Services,
    track: &Track,
    clip: &Clip,
    t: Time,
    canvas: (u32, u32),
    zoom: Option<f32>,
) -> Option<(Lettered, Placement)> {
    if !is_lettering(track, clip) {
        return None;
    }
    let service = services.lettering.as_ref()?;
    let tf = lettering_transform(clip.transform_at(t - clip.start));
    if tf.opacity <= 0.0 || tf.scale <= 0.0 || tf.stretch.iter().any(|s| *s <= 0.0) {
        return None;
    }
    let zoom =
        zoom.unwrap_or_else(|| (tf.scale * tf.stretch[0].max(tf.stretch[1])).clamp(0.25, 4.0));
    let l = service.letter(track, clip, t - clip.start, canvas, zoom)?;
    let place = placement_from(&tf, l.center, l.size, canvas);
    Some((l, place))
}

/// A frame between two source frames.
enum Tween {
    /// Ready to show.
    Made(Frame),
    /// Two frames to mix, by how far from the first.
    Mix(Frame, Frame, f32),
}

struct Ctx<'a, 'b> {
    project: &'a Project,
    services: &'a Services,
    pictures: &'a mut dyn Pictures,
    /// Output size.
    size: (u32, u32),
    /// Borrows the engine only, so a submitted frame can outlive `pictures`.
    batch: Box<dyn FxBatch + 'b>,
    /// Whether `batch` runs programs (effects, transitions, GLSL
    /// generators).
    programs: bool,
    view: MaskView,
    /// The frame last decoded for a clip whose subjects refine onto it.
    guide: Option<Frame>,
    /// The depth map of the frame last decoded for a clip that reads it.
    depth: Option<DepthMap>,
    /// Why the first clip that could not be drawn was left out: a render
    /// fails with it ([`render_program`]), the viewer shows the frame
    /// without the clip.
    failed: Option<String>,
}

impl Ctx<'_, '_> {
    fn sequence(&self) -> (u32, u32) {
        let s = &self.project.timeline.settings;
        (s.width.max(1), s.height.max(1))
    }

    /// Every picture track at `t`, on black.
    fn program(&mut self, t: Time) -> Option<Surface> {
        let project = self.project;
        let mut layers: Vec<(Surface, Placement)> = Vec::new();
        for track in picture_tracks(project) {
            let placed = fx::transitions(track);
            if let Some(pt) = placed
                .iter()
                .find(|p| p.transition.fx.enabled && p.range.contains(t))
            {
                layers.extend(self.transition(pt, t));
            } else if let Some(clip) = track.clip_at(t).filter(|c| c.enabled) {
                layers.extend(self.layer(clip, t));
            }
        }
        for track in lettering_tracks(project) {
            if let Some(clip) = track.clip_at(t).filter(|c| c.enabled) {
                layers.extend(self.lettering(track, clip, t));
            }
        }
        let (w, h) = self.size;
        self.batch.composite(w, h, [0, 0, 0, 255], &layers).ok()
    }

    /// `clip`'s caption or title at `t`, ready to composite.
    fn lettering(&mut self, track: &Track, clip: &Clip, t: Time) -> Option<(Surface, Placement)> {
        let (l, place) = lettering_layer(self.services, track, clip, t, self.size, None)?;
        Some((self.batch.upload(&l.frame).ok()?, place))
    }

    /// `clip` at `t`, ready to composite: its picture, and with
    /// [`MaskView::Tinted`] the tint of what its masks cut away over it.
    fn layer(&mut self, clip: &Clip, t: Time) -> Vec<(Surface, Placement)> {
        let tf = clip.transform_at(t - clip.start);
        if tf.opacity <= 0.0 || tf.scale <= 0.0 || tf.stretch.iter().any(|s| *s <= 0.0) {
            return Vec::new();
        }
        let Some(picture) = self.picture(clip, t, None) else {
            return Vec::new();
        };
        let size = self.batch.size(picture);
        let place = placement(&tf, size, self.size);
        let raster = self.mask_raster(clip, size, t);
        self.guide = None;
        self.depth = None;
        let tinted = match self.view {
            MaskView::Cut => false,
            MaskView::Tinted(only) => only.is_none_or(|c| c == clip.id),
        };
        let Some(raster) = raster else {
            return vec![(picture, place)];
        };
        if tinted {
            let tint = Placement {
                opacity: 1.0,
                blend: BlendMode::Normal,
                ..place
            };
            let cut = cut_tint(&raster);
            return match self.batch.upload(&cut) {
                Ok(cut) => vec![(picture, place), (cut, tint)],
                Err(_) => vec![(picture, place)],
            };
        }
        let masked = self
            .batch
            .upload(&raster)
            .and_then(|mask| self.batch.mask(picture, mask))
            .unwrap_or(picture);
        vec![(masked, place)]
    }

    /// What the masks of `clip` keep of its `size` picture at `t`; `None`
    /// when it has none.
    fn mask_raster(&mut self, clip: &Clip, (w, h): (u32, u32), t: Time) -> Option<Frame> {
        if !crate::mask::any_enabled(&clip.masks) {
            return None;
        }
        let (mw, mh) = crate::mask::raster_size((w, h));
        let aspect = w.max(1) as f32 / h.max(1) as f32;
        let mattes = subject_mattes(self.project, self.services, clip, t);
        let people = person_mattes(self.project, self.services, clip, t);
        let subjects = crate::mask::SubjectMattes {
            mattes: &mattes,
            picture: self.guide.as_ref(),
            people: &people,
            depth: self.depth.as_ref(),
        };
        crate::mask::raster(&clip.masks, t - clip.start, aspect, mw, mh, &subjects)
    }

    fn picture(&mut self, clip: &Clip, t: Time, limit: Option<usize>) -> Option<Surface> {
        let project = self.project;
        self.guide = None;
        self.depth = None;
        let local = t - clip.start;
        let mut effects: Vec<(&FxInstance, &FxAsset)> = clip
            .effects
            .iter()
            .take(limit.unwrap_or(usize::MAX))
            .filter(|e| e.enabled)
            .filter_map(|e| {
                let asset = fx::asset(project, &e.fx)?;
                (asset.kind == FxKind::Effect).then_some((e, asset))
            })
            .collect();
        let timeline =
            matches!(clip.content, ClipContent::Media { .. }).then_some(&project.timeline.grade);
        let grades: Vec<&Grade> = std::iter::once(&clip.grade)
            .chain(timeline)
            .filter(|g| !grade::is_neutral(g))
            .collect();
        let picture = match &clip.content {
            ClipContent::Media { media, .. } => {
                let item = project.media(*media)?;
                if item.kind == MediaKind::Audio {
                    return None;
                }
                let source = clip.to_source(t)?;
                let tween = self.tween(clip, item, t, source);
                let frame = match &tween {
                    Some(Tween::Made(frame) | Tween::Mix(frame, ..)) => frame.clone(),
                    None => self.pictures.frame(clip.id, item, source)?,
                };
                let refines = clip
                    .masks
                    .iter()
                    .any(|m| m.enabled && (m.picks().is_some_and(|s| s.refine) || m.is_person()));
                if refines {
                    self.guide = Some(frame.clone());
                }
                if depth::clip_needs_depth(project, clip) {
                    self.depth = self
                        .services
                        .depth
                        .as_ref()
                        .and_then(|d| d.depth(item, clip.to_source(t)?));
                }
                let between = tween.is_some();
                let picture = match tween {
                    Some(Tween::Mix(a, b, at)) => {
                        let a = self.upload(&a, &grades)?;
                        match self.upload(&b, &grades) {
                            Some(b) => self.batch.mix(a, b, at).unwrap_or(a),
                            None => a,
                        }
                    }
                    _ => self.upload(&frame, &grades)?,
                };
                self.stabilized(clip, t, between, picture)
            }
            ClipContent::Overlay { asset, .. } => {
                let item = overlay_item(clip.id, asset);
                let frame = self.pictures.frame(clip.id, &item, local)?;
                self.upload(&frame, &grades)?
            }
            ClipContent::Generator { fx } => self.generate(clip, fx, local, &grades)?,
            ClipContent::Title { .. } | ClipContent::Caption { .. } => return None,
        };
        if self.depth.is_none() {
            effects.retain(|(_, asset)| !depth::needs_depth(asset));
        }
        if !self.programs || effects.is_empty() {
            return Some(picture);
        }
        let size = self.batch.size(picture);
        let fitted = fit_size(size, self.size);
        let picture = if size == fitted {
            picture
        } else {
            let fill = Placement::fill(fitted.0, fitted.1);
            self.batch
                .composite(fitted.0, fitted.1, [0; 4], &[(picture, fill)])
                .unwrap_or(picture)
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
        let depth = match &self.depth {
            Some(map) if effects.iter().any(|(_, a)| depth::needs_depth(a)) => {
                self.batch.upload(&map.to_frame()).ok()
            }
            _ => None,
        };
        let batch = &mut self.batch;
        let mut run = |surface: Surface, calls: &[FxCall<'_>]| match depth {
            Some(d) => batch.apply_depth(surface, d, calls),
            None => batch.apply(surface, calls),
        };
        match run(picture, &calls) {
            Ok(surface) => Some(surface),
            // One broken program should not hide the clip: run the others.
            Err(_) => Some(calls.iter().fold(picture, |surface, call| {
                run(surface, std::slice::from_ref(call)).unwrap_or(surface)
            })),
        }
    }

    /// `picture` with `clip`'s stabilization at `t` drawn into its own
    /// frame ([`crate::stabilize`]), `between` source frames when it shows
    /// a moment made between them; as it is when there is none (yet).
    fn stabilized(&mut self, clip: &Clip, t: Time, between: bool, picture: Surface) -> Surface {
        let Some(warp) =
            crate::stabilize::correction(self.project, self.services, clip, t, between)
        else {
            return picture;
        };
        let size = self.batch.size(picture);
        let place = placement_from(
            &warp.transform(),
            [size.0 as f32 / 2.0, size.1 as f32 / 2.0],
            [size.0 as f32, size.1 as f32],
            size,
        );
        self.batch
            .composite(size.0, size.1, [0; 4], &[(picture, place)])
            .unwrap_or(picture)
    }

    /// What a slowed `clip` shows at timeline time `t` (source time
    /// `source`) between two source frames, as its [`Interpolation`] says:
    /// `None` to show the frame there as it is.
    fn tween(&mut self, clip: &Clip, item: &MediaItem, t: Time, source: Time) -> Option<Tween> {
        let mode = clip.retime.interpolation;
        if mode == Interpolation::Nearest
            || item.kind == MediaKind::Image
            || clip.speed_at(t - clip.start) >= 1.0 - 1e-9
        {
            return None;
        }
        let ((ta, a), (tb, b)) = self.pictures.pair(clip.id, item, source)?;
        let span = (tb - ta).as_secs_f64();
        if span <= 0.0 {
            return None;
        }
        let at = ((source - ta).as_secs_f64() / span).clamp(0.0, 1.0) as f32;
        let step = (at * BETWEEN_STEPS).round() / BETWEEN_STEPS;
        if step <= 0.0 {
            return Some(Tween::Made(a));
        }
        if step >= 1.0 {
            return Some(Tween::Made(b));
        }
        if mode == Interpolation::OpticalFlow
            && let Some(service) = &self.services.interpolation
        {
            let request = Between {
                item,
                a: (ta, &a),
                b: (tb, &b),
                at: step,
            };
            if let Some(made) = service.between(&request, false) {
                return Some(Tween::Made(made));
            }
        }
        Some(Tween::Mix(a, b, at))
    }

    /// `frame` graded by `grades` (on the CPU), in the batch.
    fn upload(&mut self, frame: &Frame, grades: &[&Grade]) -> Option<Surface> {
        if grades.is_empty() {
            self.batch.upload(frame).ok()
        } else {
            self.batch.upload(&grade::graded(frame, grades)).ok()
        }
    }

    /// `instance` at the output size. A graded GLSL generator is read back
    /// for its grade.
    fn generate(
        &mut self,
        clip: &Clip,
        instance: &FxInstance,
        local: Time,
        grades: &[&Grade],
    ) -> Option<Surface> {
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
        let (w, h) = self.size;
        let frame = match asset.language {
            // Without the GPU engine GLSL generators are left out.
            FxLanguage::Glsl if !self.programs => return None,
            FxLanguage::Glsl if grades.is_empty() => {
                let made = self.batch.generate(&call, w, h);
                return self.drawn(asset, made);
            }
            FxLanguage::Glsl => {
                let made = self.services.fx.as_ref()?.generate(&call, w, h);
                self.drawn(asset, made)?
            }
            FxLanguage::Html => {
                let made = match &self.services.html {
                    Some(html) => html.frame(&call, w, h),
                    None => Err(ServiceError("the HTML engine is unavailable".into())),
                };
                self.drawn(asset, made)?
            }
        };
        self.upload(&frame, grades)
    }

    /// What generator `asset` made, keeping why it made nothing in
    /// [`Ctx::failed`] when it is the first clip left out.
    fn drawn<T>(&mut self, asset: &FxAsset, made: ServiceResult<T>) -> Option<T> {
        made.map_err(|e| {
            let what = match asset.language {
                FxLanguage::Glsl => "GLSL",
                FxLanguage::Html => "HTML",
            };
            self.failed.get_or_insert_with(|| {
                format!("{what} clip “{}” could not be drawn: {e}", asset.name)
            });
        })
        .ok()
    }

    /// Both sides of `pt` on their own canvases, blended at `t`.
    fn transition(&mut self, pt: &PlacedTransition<'_>, t: Time) -> Option<(Surface, Placement)> {
        let from = self.side(pt.from, t)?;
        let to = self.side(pt.to, t)?;
        let local = t - pt.range.start;
        let instance = &pt.transition.fx;
        let canvas = self.sequence();
        let program = fx::asset(self.project, &instance.fx)
            .filter(|a| a.kind == FxKind::Transition)
            .and_then(|asset| {
                let values = instance.values_at(asset, local);
                let call = FxCall {
                    asset,
                    values: &values,
                    time: local.as_secs_f64(),
                    duration: pt.range.duration().as_secs_f64(),
                    canvas,
                };
                self.batch.transition(from, to, &call).ok()
            });
        let blended = match program {
            Some(blended) => blended,
            None => self.batch.mix(from, to, pt.progress(t)).ok()?,
        };
        let blend = pt
            .to
            .or(pt.from)
            .map_or(BlendMode::Normal, |c| c.transform.blend);
        let (w, h) = self.size;
        Some((
            self.batch.unpremultiply(blended).ok()?,
            Placement {
                blend,
                ..Placement::fill(w, h)
            },
        ))
    }

    /// `clip`'s layer at `t` alone on a transparent canvas, premultiplied.
    fn side(&mut self, clip: Option<&Clip>, t: Time) -> Option<Surface> {
        let layers: Vec<(Surface, Placement)> = clip
            .map(|c| self.layer(c, t))
            .unwrap_or_default()
            .into_iter()
            .map(|(picture, place)| {
                let place = Placement {
                    blend: BlendMode::Normal,
                    ..place
                };
                (picture, place)
            })
            .collect();
        let (w, h) = self.size;
        let canvas = self.batch.composite(w, h, [0; 4], &layers).ok()?;
        self.batch.premultiply(canvas).ok()
    }
}
