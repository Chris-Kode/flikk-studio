//! Where everything the program shows at a time lands, for picking items
//! in the viewer and for the agent's `get_layout`: each layer's placement,
//! where it rests before its transform, and the part of it with anything
//! drawn (the opaque part of a logo, the lower third inside a full-frame
//! generator).

use super::{
    Pictures, is_lettering, lettering_layer, lettering_tracks, overlay_item, picture_tracks,
    placement,
};
use crate::fx::{self, FxKind, FxLanguage};
use crate::ids::{ClipId, TrackId};
use crate::model::{Clip, ClipContent, MediaKind, Project};
use crate::services::{Frame, FxCall, Placement, Services};
use crate::time::Time;

/// Width generators are drawn at to find what they cover.
const PROBE_WIDTH: u32 = 256;
/// Alpha (0..255) above which a pixel counts as drawn.
const DRAWN: u8 = 8;

/// One layer drawn at a time.
#[derive(Clone, Debug)]
pub struct Shape {
    pub track: TrackId,
    pub clip: ClipId,
    /// Where it lands, in canvas pixels.
    pub placement: Placement,
    /// Where its picture rests before the transform: centre and size, in
    /// canvas pixels.
    pub rest_center: [f32; 2],
    pub rest_size: [f32; 2],
    /// The part of the picture with anything drawn, cropped, as
    /// `[u0, v0, u1, v1]` in the placement's (flipped) uv.
    pub content: [f32; 4],
    /// A caption or title.
    pub lettering: bool,
    /// The canvas it was laid out on.
    pub canvas: (u32, u32),
}

impl Shape {
    /// Corners of the drawn part on the canvas: top-left, top-right,
    /// bottom-right, bottom-left.
    pub fn corners(&self) -> [[f32; 2]; 4] {
        let [u0, v0, u1, v1] = self.content;
        [[u0, v0], [u1, v0], [u1, v1], [u0, v1]].map(|uv| self.placement.to_canvas(uv))
    }

    /// Axis-aligned bounds of the drawn part: `[x0, y0, x1, y1]`.
    pub fn bounds(&self) -> [f32; 4] {
        self.corners().iter().fold(
            [f32::MAX, f32::MAX, f32::MIN, f32::MIN],
            |[x0, y0, x1, y1], [x, y]| [x0.min(*x), y0.min(*y), x1.max(*x), y1.max(*y)],
        )
    }

    /// Whether canvas point `p` falls on the drawn part.
    pub fn contains(&self, p: [f32; 2]) -> bool {
        let [u0, v0, u1, v1] = self.content;
        self.placement
            .to_picture(p)
            .is_some_and(|[u, v]| u >= u0 && u <= u1 && v >= v0 && v <= v1)
    }
}

/// Every layer the program shows at `t` on a `canvas`, topmost first:
/// titles and captions (with [`Services::lettering`]), then pictures. A
/// clip under a transition is laid out as itself.
pub fn layout(
    project: &Project,
    t: Time,
    canvas: (u32, u32),
    services: &Services,
    pictures: &mut dyn Pictures,
) -> Vec<Shape> {
    let canvas = (canvas.0.max(1), canvas.1.max(1));
    let mut shapes = Vec::new();
    for track in picture_tracks(project) {
        let Some(clip) = track.clip_at(t).filter(|c| c.enabled) else {
            continue;
        };
        shapes.extend(picture_shape(
            project, track.id, clip, t, canvas, services, pictures,
        ));
    }
    for track in lettering_tracks(project) {
        let Some(clip) = track
            .clip_at(t)
            .filter(|c| c.enabled && is_lettering(track, c))
        else {
            continue;
        };
        if let Some((l, placement)) = lettering_layer(services, track, clip, t, canvas, Some(1.0)) {
            shapes.push(Shape {
                track: track.id,
                clip: clip.id,
                placement,
                rest_center: l.center,
                rest_size: l.size,
                content: [0.0, 0.0, 1.0, 1.0],
                lettering: true,
                canvas,
            });
        }
    }
    pictures.finish();
    shapes.reverse();
    shapes
}

/// `clip`'s picture layer at `t`.
fn picture_shape(
    project: &Project,
    track: TrackId,
    clip: &Clip,
    t: Time,
    canvas: (u32, u32),
    services: &Services,
    pictures: &mut dyn Pictures,
) -> Option<Shape> {
    let local = t - clip.start;
    let tf = clip.transform_at(local);
    if tf.opacity <= 0.0 {
        return None;
    }
    // The picture's size, and the part of it drawn when it has alpha.
    let (size, drawn) = match &clip.content {
        ClipContent::Media { media, .. } => {
            let item = project.media(*media)?;
            match item.kind {
                MediaKind::Audio => return None,
                MediaKind::Video => match item.info.as_ref().and_then(|i| i.video.as_ref()) {
                    Some(v) => ((v.width, v.height), None),
                    None => {
                        let frame = pictures.frame(clip.id, item, clip.to_source(t)?)?;
                        ((frame.width, frame.height), None)
                    }
                },
                MediaKind::Image => {
                    let frame = pictures.frame(clip.id, item, clip.to_source(t)?)?;
                    ((frame.width, frame.height), alpha_bounds(&frame))
                }
            }
        }
        ClipContent::Overlay { asset, .. } => {
            let frame = pictures.frame(clip.id, &overlay_item(clip.id, asset), local)?;
            ((frame.width, frame.height), alpha_bounds(&frame))
        }
        ClipContent::Generator { fx: instance } => {
            let asset = fx::asset(project, &instance.fx)?;
            if asset.kind != FxKind::Generator {
                return None;
            }
            let values = instance.values_at(asset, local);
            let s = &project.timeline.settings;
            let call = FxCall {
                asset,
                values: &values,
                time: local.as_secs_f64(),
                duration: clip.duration.as_secs_f64(),
                canvas: (s.width.max(1), s.height.max(1)),
            };
            let w = PROBE_WIDTH;
            let h = ((w as u64 * canvas.1 as u64 / canvas.0 as u64) as u32).max(1);
            let probe = match asset.language {
                FxLanguage::Glsl => services
                    .fx
                    .as_ref()
                    .and_then(|f| f.generate(&call, w, h).ok()),
                FxLanguage::Html => services
                    .html
                    .as_ref()
                    .and_then(|e| e.frame(&call, w, h).ok()),
            };
            (canvas, probe.as_ref().and_then(alpha_bounds))
        }
        ClipContent::Title { .. } | ClipContent::Caption { .. } => return None,
    };
    let place = placement(&tf, size, canvas);
    // Scaled to nothing: not drawn, so nothing to pick.
    place.inverse()?;
    let fit = (canvas.0 as f32 / size.0.max(1) as f32).min(canvas.1 as f32 / size.1.max(1) as f32);
    // Drawn part, in the picture's own uv, cut by the crop, then flipped
    // into the placement's uv.
    let [x0, y0, x1, y1] = drawn.unwrap_or([0.0, 0.0, 1.0, 1.0]);
    let [cl, ct, cr, cb] = place.crop;
    let (mut u0, mut v0, mut u1, mut v1) =
        (x0.max(cl), y0.max(ct), x1.min(1.0 - cr), y1.min(1.0 - cb));
    if u0 >= u1 || v0 >= v1 {
        return None;
    }
    if place.flip[0] {
        (u0, u1) = (1.0 - u1, 1.0 - u0);
    }
    if place.flip[1] {
        (v0, v1) = (1.0 - v1, 1.0 - v0);
    }
    Some(Shape {
        track,
        clip: clip.id,
        placement: place,
        rest_center: [canvas.0 as f32 / 2.0, canvas.1 as f32 / 2.0],
        rest_size: [size.0 as f32 * fit, size.1 as f32 * fit],
        content: [u0, v0, u1, v1],
        lettering: false,
        canvas,
    })
}

/// The part of `frame` with alpha above [`DRAWN`], as `[u0, v0, u1, v1]`;
/// `None` when nothing is drawn.
fn alpha_bounds(frame: &Frame) -> Option<[f32; 4]> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (y, row) in frame.rgba.chunks_exact(w * 4).enumerate().take(h) {
        let mut first = None;
        let mut last = 0;
        for (x, px) in row.as_chunks::<4>().0.iter().enumerate() {
            if px[3] > DRAWN {
                first.get_or_insert(x);
                last = x;
            }
        }
        if let Some(first) = first {
            x0 = x0.min(first);
            x1 = x1.max(last + 1);
            y0 = y0.min(y);
            y1 = y + 1;
        }
    }
    (x0 < x1 && y0 < y1).then(|| {
        [
            x0 as f32 / w as f32,
            y0 as f32 / h as f32,
            x1 as f32 / w as f32,
            y1 as f32 / h as f32,
        ]
    })
}
