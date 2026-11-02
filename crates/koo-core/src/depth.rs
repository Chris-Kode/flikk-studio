//! Depth maps: how far each part of a picture is from the camera, relative
//! to the rest of it, for the depth-aware effects and depth masks.
//!
//! A [`DepthMap`] holds *distance* in 0..1 (0 nearest, 1 farthest) per
//! cell, row by row from the top, in the picture's own uv, at a reduced
//! resolution. It is relative: the model knows what is in front of what,
//! not how many metres away it is, and the map is spread over the
//! picture's own range (see `koo-analysis`' depth service). Distance runs
//! linearly in disparity (inverse depth), which is how a lens defocuses:
//! the blur of a point grows with its disparity's difference from the
//! focus.
//!
//! [`DepthService`] makes the maps, one per source frame, temporally
//! smoothed and kept on disk; the compositor asks it for the frame a clip
//! shows ([`crate::compose`]), and renders make every map of their range
//! first ([`DepthService::complete`]), so the export matches the viewer.
//!
//! GLSL effects that read depth call `float getDepth(vec2 uv)`
//! ([`needs_depth`]); the engine then binds the clip's map beside its
//! picture. Without a map (still being made, or no model) such an effect
//! is left out of the chain, so the picture shows as it is.

use crate::fx::{self, FxAsset, FxKind, FxLanguage};
use crate::model::{Clip, ClipContent, MediaItem, MediaKind, Project, Track};
use crate::services::{Frame, ServiceResult};
use crate::time::{Time, TimeRange};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// What a GLSL program calls to read depth.
pub const DEPTH_FN: &str = "getDepth(";

/// A picture's relative distance, `width` × `height` cells, top row first:
/// 0 nearest, 255 farthest.
#[derive(Clone, Debug, PartialEq)]
pub struct DepthMap {
    pub width: u32,
    pub height: u32,
    pub values: Arc<[u8]>,
}

impl DepthMap {
    /// Distance (0..1) at `uv` (0..1 across the picture, v down), sampled
    /// bilinearly between cell centres as the GPU does.
    pub fn at(&self, uv: [f32; 2]) -> f32 {
        let (w, h) = (self.width.max(1) as usize, self.height.max(1) as usize);
        if self.values.len() < w * h {
            return 0.0;
        }
        let x = (uv[0].clamp(0.0, 1.0) * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
        let y = (uv[1].clamp(0.0, 1.0) * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let v = |x: usize, y: usize| self.values[y * w + x] as f32 / 255.0;
        let top = v(x0, y0) + (v(x1, y0) - v(x0, y0)) * fx;
        let bottom = v(x0, y1) + (v(x1, y1) - v(x0, y1)) * fx;
        top + (bottom - top) * fy
    }

    /// Distance at `uv` averaged over a small disc (a 30th of the
    /// picture's height), so a click on a fine edge lands on what is
    /// mostly there.
    pub fn around(&self, uv: [f32; 2]) -> f32 {
        let aspect = self.width.max(1) as f32 / self.height.max(1) as f32;
        let r = 1.0 / 30.0;
        let mut taps = vec![self.at(uv)];
        for k in 0..8 {
            let a = k as f32 * std::f32::consts::TAU / 8.0;
            taps.push(self.at([uv[0] + a.cos() * r / aspect, uv[1] + a.sin() * r]));
        }
        // The median: robust to the far side of an edge.
        taps.sort_by(f32::total_cmp);
        taps[taps.len() / 2]
    }

    /// The map as a grey picture (distance in every channel, opaque), as
    /// the GPU engine reads it.
    pub fn to_frame(&self) -> Frame {
        let rgba: Vec<u8> = self.values.iter().flat_map(|v| [*v, *v, *v, 255]).collect();
        Frame {
            width: self.width,
            height: self.height,
            rgba: Arc::from(rgba),
        }
    }
}

/// How far the depth maps of a range are made.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DepthStatus {
    /// Source frames the range needs (with those its smoothing reads),
    /// and how many are made.
    pub frames: usize,
    pub made: usize,
    /// Seconds a frame took lately on this machine.
    pub seconds_per_frame: Option<f32>,
    /// Why making them failed.
    pub error: Option<String>,
}

impl DepthStatus {
    pub fn complete(&self) -> bool {
        self.made >= self.frames
    }
}

/// Depth maps (`koo-analysis`): a depth model run on source frames, kept by
/// fingerprint and model, so a frame is estimated once.
pub trait DepthService: Send + Sync {
    /// Whether the model is here; why not otherwise.
    fn available(&self) -> Result<(), String>;
    /// The map of `item` at source time `t`, smoothed with the frames
    /// around it that are made. Starts making what is missing, that frame
    /// first and then the next seconds, so playback finds them; `None`
    /// until that frame's is made.
    fn depth(&self, item: &MediaItem, t: Time) -> Option<DepthMap>;
    /// How far the maps of `item` over source `range` are made.
    fn status(&self, item: &MediaItem, range: TimeRange) -> DepthStatus;
    /// Make every map `range` of `item` shows, waiting for it (renders).
    /// `progress` gets 0..1.
    fn complete(
        &self,
        item: &MediaItem,
        range: TimeRange,
        progress: &dyn Fn(f32),
        cancel: &AtomicBool,
    ) -> ServiceResult<()>;
    /// Counts maps made, so views know to draw again.
    fn version(&self) -> u64;
}

/// Whether `asset` reads depth: a GLSL effect calling `getDepth`.
pub fn needs_depth(asset: &FxAsset) -> bool {
    asset.language == FxLanguage::Glsl
        && asset.kind == FxKind::Effect
        && asset.code.contains(DEPTH_FN)
}

/// Whether `clip` reads depth at all: an enabled effect that does, or an
/// enabled depth mask.
pub fn clip_needs_depth(project: &Project, clip: &Clip) -> bool {
    clip.effects
        .iter()
        .filter(|e| e.enabled)
        .filter_map(|e| fx::asset(project, &e.fx))
        .any(needs_depth)
        || clip.masks.iter().any(|m| m.enabled && m.reads_depth())
}

/// The media item `clip` (on `track`) shows, and the source range of it
/// that timeline `within` reads, the transitions that play it past its
/// edges included, when it can have depth (video and images from the
/// pool).
pub fn source<'a>(
    project: &'a Project,
    track: &Track,
    clip: &Clip,
    within: TimeRange,
) -> Option<(&'a MediaItem, TimeRange)> {
    let ClipContent::Media { media, .. } = &clip.content else {
        return None;
    };
    let item = project.media(*media)?;
    if item.kind == MediaKind::Audio {
        return None;
    }
    let mut shown = clip.range();
    for p in fx::transitions(track) {
        let sides = [p.from, p.to];
        if sides.iter().flatten().any(|c| c.id == clip.id) {
            shown = TimeRange::new(shown.start.min(p.range.start), shown.end.max(p.range.end));
        }
    }
    let shown = shown.intersect(&within)?;
    let (a, b) = (clip.to_source(shown.start)?, clip.to_source(shown.end)?);
    Some((item, TimeRange::new(a.min(b), a.max(b))))
}

/// The effects on `clip` with a focus distance to pick ([`crate::fx::depth::FOCUS`]
/// on an effect that reads depth), in stack order.
pub fn focus_effects<'a>(project: &Project, clip: &'a Clip) -> Vec<&'a crate::fx::FxInstance> {
    clip.effects
        .iter()
        .filter(|e| {
            fx::asset(project, &e.fx)
                .is_some_and(|a| needs_depth(a) && a.param(fx::depth::FOCUS).is_some())
        })
        .collect()
}

/// The command that focuses `effect` on `clip` at `distance` from timeline
/// time `at`: a keyframe there when its focus has keyframes or `key` asks
/// for one (a rack focus), else its value throughout. `None` when the
/// effect is not on the clip.
pub fn focus_command(
    clip: &Clip,
    effect: crate::ids::EffectId,
    at: Time,
    distance: f32,
    key: bool,
) -> Option<crate::command::Command> {
    use crate::command::Command;
    use crate::fx::ParamValue;
    let instance = clip.effects.iter().find(|e| e.id == effect)?;
    let value = ParamValue::Number((distance.clamp(0.0, 1.0) as f64 * 1000.0).round() / 1000.0);
    let param = fx::depth::FOCUS;
    let keyed = instance.keyframes.iter().any(|k| k.param == param);
    if keyed || key {
        let ease = instance
            .keyframes
            .iter()
            .find(|k| k.param == param && k.at == at - clip.start)
            .map_or_else(Default::default, |k| k.ease);
        return Some(Command::SetEffectKeyframe {
            clip: clip.id,
            effect,
            param: param.into(),
            at,
            value,
            ease,
        });
    }
    let mut changed = instance.clone();
    changed.params.insert(param.into(), value);
    Some(Command::SetEffect {
        clip: clip.id,
        effect: changed,
    })
}
