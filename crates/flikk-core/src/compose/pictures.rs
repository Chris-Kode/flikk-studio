//! Where layer pictures come from: single decodes for stills, the agent and
//! frame export ([`Stills`]), or long-lived decoders that follow a moving
//! playhead for playback and render ([`Streams`]).

use crate::ids::{ClipId, MediaId};
use crate::model::{MediaItem, MediaKind};
use crate::services::{Frame, FrameStream, MediaService, ServiceError, ServiceResult};
use crate::time::{Rate, Time};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

/// Source pictures for the compositor.
pub trait Pictures {
    /// The frame of `item` showing at source time `t`, for the layer of
    /// `clip`. Before the first and after the last frame of the source,
    /// that edge frame (transitions read past the clip's handles).
    fn frame(&mut self, clip: ClipId, item: &MediaItem, t: Time) -> Option<Frame>;
    /// The frame showing at source time `t` and the one after it, with
    /// their source times, for frames between them (smooth slow motion).
    fn pair(
        &mut self,
        clip: ClipId,
        item: &MediaItem,
        t: Time,
    ) -> Option<((Time, Frame), (Time, Frame))> {
        let fd = frame_length(item)?;
        let a = Time(t.max(Time::ZERO).0 / fd.0 * fd.0);
        let b = a + fd;
        // Read mid-frame, clear of rounding at the edges.
        let half = Time(fd.0 / 2);
        let fa = self.frame(clip, item, a + half)?;
        let fb = self
            .frame(clip, item, b + half)
            .unwrap_or_else(|| fa.clone());
        Some(((a, fa), (b, fb)))
    }
    /// Called after each composed frame.
    fn finish(&mut self) {}
    /// Why a source stopped giving frames partway, once: a render fails
    /// with it rather than hold the last frame.
    fn failure(&mut self) -> Option<String> {
        None
    }
}

/// One [`MediaService::frame`] per picture (cached by the media engine),
/// at most `max_width` wide. Without a media engine there are none.
pub struct Stills {
    media: Option<Arc<dyn MediaService>>,
    max_width: u32,
}

impl Stills {
    pub fn new(media: Option<Arc<dyn MediaService>>, max_width: u32) -> Self {
        Self { media, max_width }
    }
}

impl Pictures for Stills {
    fn frame(&mut self, _: ClipId, item: &MediaItem, t: Time) -> Option<Frame> {
        self.media.as_ref()?.frame(item, t, self.max_width).ok()
    }
}

/// A video's frame length, from its probed rate.
pub fn frame_length(item: &MediaItem) -> Option<Time> {
    if item.kind == MediaKind::Image {
        return None;
    }
    let fd = item.info.as_ref()?.video.as_ref()?.rate.frame_duration();
    (fd > Time::ZERO).then_some(fd)
}

/// Larger forward jumps than this reopen a stream instead of decoding up.
const MAX_SKIP: f64 = 1.5;
/// Frames a stream decodes ahead of the compositor.
const READ_AHEAD: usize = 4;

struct Open {
    path: PathBuf,
    stream: Box<dyn FrameStream>,
    /// Source time of the last frame read.
    position: Option<Time>,
    frame: Option<Frame>,
    /// The frame read before `frame`, and its source time: frames between
    /// two source frames read both again and again.
    prev: Option<(Time, Frame)>,
    /// The decoder has no more frames: hold the last one.
    ended: bool,
}

/// One [`MediaService::playback`] stream per layer, read in step with the
/// requested source times: frames are decoded and dropped when the source
/// runs faster than the requests (speed above 1) and held when it runs
/// slower; a backward or long forward jump reopens the stream. Streams of
/// layers a composed frame did not use are closed. Each stream is read on
/// its own thread, [`READ_AHEAD`] frames ahead, so decoding overlaps
/// composing.
pub struct Streams {
    media: Option<Arc<dyn MediaService>>,
    max_width: u32,
    /// Frame length assumed for sources without probe data.
    fallback: Time,
    open: HashMap<ClipId, Open>,
    used: HashSet<ClipId>,
    /// The first read that failed since [`Pictures::failure`] was asked.
    failed: Option<String>,
}

impl Streams {
    /// Streams at most `max_width` wide; `rate` is the frame length assumed
    /// for sources without probe data.
    pub fn new(media: Option<Arc<dyn MediaService>>, max_width: u32, rate: Rate) -> Self {
        Self {
            media,
            max_width,
            fallback: rate.frame_duration(),
            open: HashMap::new(),
            used: HashSet::new(),
            failed: None,
        }
    }
}

impl Pictures for Streams {
    fn frame(&mut self, clip: ClipId, item: &MediaItem, t: Time) -> Option<Frame> {
        self.used.insert(clip);
        let still = item.kind == MediaKind::Image;
        let fd = item
            .info
            .as_ref()
            .and_then(|i| i.video.as_ref())
            .map_or(self.fallback, |v| v.rate.frame_duration());
        let t = t.max(Time::ZERO);
        let reopen = self.open.get(&clip).is_none_or(|o| {
            o.path != item.path
                || (!still
                    && o.position.is_some_and(|p| {
                        t + fd < p || (!o.ended && t - p > Time::from_secs_f64(MAX_SKIP))
                    }))
        });
        if reopen {
            let stream = self
                .media
                .as_ref()?
                .playback(item, t, self.max_width)
                .ok()?;
            let stream = Box::new(ReadAhead::new(stream));
            self.open.insert(
                clip,
                Open {
                    path: item.path.clone(),
                    stream,
                    position: None,
                    frame: None,
                    prev: None,
                    ended: false,
                },
            );
        }
        let o = self.open.get_mut(&clip)?;
        if let (Some(p), Some((at, f))) = (o.position, &o.prev)
            && *at <= t
            && t < p
        {
            return Some(f.clone());
        }
        while !o.ended && (o.frame.is_none() || o.position.is_some_and(|p| p + fd <= t)) {
            match o.stream.next_frame() {
                Ok(Some((at, f))) => {
                    o.prev = o.position.zip(o.frame.take());
                    o.position = Some(at);
                    o.frame = Some(f);
                }
                Ok(None) => o.ended = true,
                Err(e) => {
                    o.ended = true;
                    self.failed.get_or_insert(e.0);
                }
            }
        }
        o.frame.clone()
    }

    fn finish(&mut self) {
        let used = std::mem::take(&mut self.used);
        self.open.retain(|id, _| used.contains(id));
    }

    fn failure(&mut self) -> Option<String> {
        self.failed.take()
    }
}

/// A pool-less item for an overlay clip's asset, keyed by the clip.
pub fn overlay_item(clip: ClipId, asset: &Path) -> MediaItem {
    let ext = asset
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let still = matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tif" | "tiff" | "tga"
    );
    MediaItem {
        id: MediaId(clip.0),
        path: asset.to_path_buf(),
        kind: if still {
            MediaKind::Image
        } else {
            MediaKind::Video
        },
        fingerprint: None,
        info: None,
        bin: None,
        tags: Vec::new(),
        proxy: None,
        generated: None,
        name: None,
        tone_map: Default::default(),
    }
}

/// A [`FrameStream`] read on its own thread into a bounded queue. The thread
/// ends, closing the stream, once this is dropped.
struct ReadAhead {
    frames: mpsc::Receiver<ServiceResult<Option<(Time, Frame)>>>,
    /// The error the stream ended with, given again on every later read.
    failed: Option<String>,
}

impl ReadAhead {
    fn new(mut stream: Box<dyn FrameStream>) -> Self {
        let (tx, frames) = mpsc::sync_channel(READ_AHEAD);
        let read = move || {
            loop {
                let next = stream.next_frame();
                let more = matches!(next, Ok(Some(_)));
                if tx.send(next).is_err() || !more {
                    break;
                }
            }
        };
        // Without a thread the receiver sees a closed queue: the stream ends.
        let _ = std::thread::Builder::new()
            .name("flikk-read-ahead".into())
            .spawn(read);
        Self {
            frames,
            failed: None,
        }
    }
}

impl FrameStream for ReadAhead {
    fn next_frame(&mut self) -> ServiceResult<Option<(Time, Frame)>> {
        if let Some(e) = &self.failed {
            return Err(ServiceError(e.clone()));
        }
        let next = self.frames.recv().unwrap_or(Ok(None));
        if let Err(e) = &next {
            self.failed = Some(e.0.clone());
        }
        next
    }
}
