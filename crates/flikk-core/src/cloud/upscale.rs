//! Upscaling: a video in the pool made again at a larger resolution by a
//! provider. What each upscale model takes ([`UpscaleModel`]), and how it
//! reaches the resolution asked for ([`Upscaling`]), so a request is
//! checked before it costs anything.

use super::{CloudError, CloudResult, GenerateRequest, resolution_lines};
use crate::model::{MediaItem, MediaKind};
use crate::time::Time;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The video an upscale starts from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpscaleSource {
    pub path: PathBuf,
    /// What the pool calls it.
    pub name: String,
    /// Its picture, in pixels.
    pub width: u32,
    pub height: u32,
    /// How long it runs, when known.
    #[serde(default)]
    pub length: Option<Time>,
    /// It has sound, which the upscaled video keeps.
    #[serde(default)]
    pub sound: bool,
}

impl UpscaleSource {
    /// `item` as an upscale's source; `None` unless it is a video whose
    /// picture size is known.
    pub fn of(item: &MediaItem) -> Option<Self> {
        if item.kind != MediaKind::Video {
            return None;
        }
        let info = item.info.as_ref()?;
        let video = info
            .video
            .as_ref()
            .filter(|v| v.width > 0 && v.height > 0)?;
        let name = item.name.clone().unwrap_or_else(|| {
            item.path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        });
        Some(Self {
            path: item.path.clone(),
            name,
            width: video.width,
            height: video.height,
            length: Some(info.duration).filter(|d| *d > Time::ZERO),
            sound: !info.audio.is_empty(),
        })
    }

    /// The lines on its short side: 1080 for 1920 × 1080 and 1080 × 1920.
    pub fn lines(&self) -> u32 {
        self.width.min(self.height)
    }
}

/// How a model is told how much larger to make a video.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scale {
    /// Asked for the resolution itself.
    Resolution,
    /// Asked for a factor from `least` to `most`; one under `least` is
    /// raised to it, which makes the video larger than asked.
    Factor { least: f64, most: f64 },
    /// Asked for one of these whole factors, smallest first: the smallest
    /// reaching the resolution is used.
    Steps(&'static [u32]),
}

/// What one upscale model can do, so a request is checked before it costs
/// anything.
#[derive(Clone, Debug, PartialEq)]
pub struct UpscaleModel {
    pub name: &'static str,
    /// The resolutions it makes, smallest first, as the editor names them
    /// ("1080p", "2k", "4k"): the lines on the short side.
    pub targets: &'static [&'static str],
    pub scale: Scale,
    /// The longest video it takes, in seconds.
    pub longest: Option<u32>,
    /// The largest video it takes: the lines on its short side.
    pub largest: Option<u32>,
    /// The most pixels a frame it makes may have.
    pub most_pixels: Option<u64>,
}

/// How a model makes one upscale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Upscaling {
    /// The resolution made, one of the model's targets.
    pub target: &'static str,
    /// How much larger each side is made: 2.0 doubles them.
    pub factor: f64,
}

impl UpscaleModel {
    /// How it makes `source` at `resolution` ("4k", in any case), or why it
    /// can't.
    pub fn upscaling(&self, source: &UpscaleSource, resolution: &str) -> CloudResult<Upscaling> {
        let name = self.name;
        let Some(target) = self
            .targets
            .iter()
            .find(|t| t.eq_ignore_ascii_case(resolution))
            .copied()
        else {
            return refuse(format!(
                "{name} makes {} video, not {resolution}",
                self.targets.join(", ")
            ));
        };
        let lines = resolution_lines(target).unwrap_or(0) as f64;
        let short = source.lines();
        if short == 0 {
            return refuse(format!("The size of {} isn't known", source.name));
        }
        if let (Some(longest), Some(length)) = (self.longest, source.length) {
            let secs = length.as_secs_f64();
            if secs > longest as f64 + 0.05 {
                return refuse(format!(
                    "{name} upscales videos of {longest} s at most; this runs {secs:.1} s"
                ));
            }
        }
        if let Some(largest) = self.largest
            && short > largest
        {
            return refuse(format!(
                "{name} upscales videos of {largest}p at most; this is {short}p"
            ));
        }
        if short as f64 >= lines {
            return refuse(format!(
                "This video is {short}p already; pick a resolution above it"
            ));
        }
        let needed = lines / short as f64;
        let factor = match self.scale {
            Scale::Resolution => needed,
            Scale::Factor { least, most } => {
                if needed > most + 1e-6 {
                    return refuse(too_far(name, most, short));
                }
                needed.max(least)
            }
            Scale::Steps(steps) => {
                let reaching = steps
                    .iter()
                    .map(|s| *s as f64)
                    .find(|s| short as f64 * s >= lines - 0.5);
                match reaching {
                    Some(step) => step,
                    None => {
                        let most = steps.last().copied().unwrap_or(1) as f64;
                        return refuse(too_far(name, most, short));
                    }
                }
            }
        };
        if let Some(most) = self.most_pixels {
            let pixels = source.width as f64 * source.height as f64 * factor * factor;
            if pixels > most as f64 {
                return refuse(format!(
                    "{name} makes frames of {:.1} megapixels at most; {target} from this video is larger",
                    most as f64 / 1e6
                ));
            }
        }
        Ok(Upscaling {
            target,
            factor: (factor * 1000.0).round() / 1000.0,
        })
    }

    /// The resolutions it can make `source` at, smallest first.
    pub fn targets_for(&self, source: &UpscaleSource) -> Vec<&'static str> {
        self.targets
            .iter()
            .copied()
            .filter(|t| self.upscaling(source, t).is_ok())
            .collect()
    }

    /// What `source` is made at when no resolution is asked for: 1080p
    /// where it can be, else the smallest it can be made at.
    pub fn default_target(&self, source: &UpscaleSource) -> Option<&'static str> {
        let targets = self.targets_for(source);
        targets
            .iter()
            .find(|t| t.eq_ignore_ascii_case("1080p"))
            .or(targets.first())
            .copied()
    }

    /// How it makes `request`, at its resolution or the default one; why
    /// it can't, before any money is spent on it.
    pub fn plan(&self, request: &GenerateRequest) -> CloudResult<Upscaling> {
        let GenerateRequest::Upscale { source, resolution } = request else {
            return Err(CloudError::Unavailable(format!(
                "{} only makes videos larger",
                self.name
            )));
        };
        match resolution {
            Some(resolution) => self.upscaling(source, resolution),
            None => match self.default_target(source) {
                Some(target) => self.upscaling(source, target),
                None => match self.targets.last() {
                    // Says why even the largest can't be made.
                    Some(largest) => self.upscaling(source, largest),
                    None => refuse(format!("{} makes no video", self.name)),
                },
            },
        }
    }

    /// Why it can't make `request`, before any money is spent on it.
    pub fn check(&self, request: &GenerateRequest) -> CloudResult<()> {
        match request {
            GenerateRequest::Upscale { .. } => self.plan(request).map(|_| ()),
            _ => Ok(()),
        }
    }
}

fn too_far(name: &str, most: f64, short: u32) -> String {
    format!(
        "{name} makes videos {most}× larger at most: from {short}p, up to {:.0}p",
        short as f64 * most
    )
}

fn refuse<T>(message: String) -> CloudResult<T> {
    Err(CloudError::BadPrompt {
        message,
        suggestion: None,
    })
}
