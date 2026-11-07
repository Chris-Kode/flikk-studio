//! Audio clip waveforms, fetched off-thread from `MediaService::waveform` and
//! cached per source range. Peaks are fetched at a fixed density and
//! resampled when drawn, so zooming never refetches.

use flikk_core::model::{Clip, ClipContent, Project};
use flikk_core::services::MediaService;
use flikk_core::{MediaId, Time, TimeRange};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Peaks per second of source.
const DENSITY: f64 = 60.;
const MAX_BUCKETS: usize = 30_000;
/// How long a failed waveform waits before it is fetched again.
const RETRY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    media: MediaId,
    stream: u32,
    range: TimeRange,
}

enum Slot {
    InFlight,
    Ready(Arc<Vec<f32>>),
    /// The request failed then (media offline or still being read).
    Failed(Instant),
}

#[derive(Default)]
pub struct Waveforms {
    peaks: HashMap<Key, Slot>,
}

/// What a request needs, taken from the project on the UI thread.
pub struct Request {
    pub key: Key,
    pub item: flikk_core::model::MediaItem,
    pub buckets: usize,
}

impl Waveforms {
    /// Cached peaks for an audio clip; on a miss, returns the request to run.
    pub fn lookup(
        &mut self,
        clip: &Clip,
        project: &Project,
    ) -> (Option<Arc<Vec<f32>>>, Option<Request>) {
        let ClipContent::Media {
            media,
            source_in,
            audio_stream,
        } = &clip.content
        else {
            return (None, None);
        };
        let Some(item) = project.media(*media) else {
            return (None, None);
        };
        // Stream indices are container indices; default to the first audio stream.
        let stream = audio_stream
            .or_else(|| item.info.as_ref()?.audio.first().map(|a| a.index))
            .unwrap_or(0);
        let key = Key {
            media: *media,
            stream,
            range: clip
                .source_range()
                .unwrap_or(TimeRange::new(*source_in, *source_in + clip.duration)),
        };
        match self.peaks.get(&key) {
            Some(Slot::Ready(peaks)) => return (Some(peaks.clone()), None),
            Some(Slot::InFlight) => return (None, None),
            Some(Slot::Failed(at)) if at.elapsed() < RETRY => return (None, None),
            Some(Slot::Failed(_)) | None => {}
        }
        self.peaks.insert(key, Slot::InFlight);
        let buckets =
            ((key.range.duration().as_secs_f64() * DENSITY) as usize).clamp(16, MAX_BUCKETS);
        (
            None,
            Some(Request {
                key,
                item: item.clone(),
                buckets,
            }),
        )
    }

    pub fn insert(&mut self, key: Key, peaks: Option<Vec<f32>>) {
        let slot = match peaks {
            Some(peaks) => Slot::Ready(Arc::new(peaks)),
            None => Slot::Failed(Instant::now()),
        };
        self.peaks.insert(key, slot);
    }
}

impl Request {
    /// Blocking; run on a background thread.
    pub fn run(&self, media: &dyn MediaService) -> Option<Vec<f32>> {
        media
            .waveform(&self.item, self.key.stream, self.key.range, self.buckets)
            .ok()
    }
}

/// Peak of `peaks` (spanning `clip_len`) over the clip-relative `[from, to)`.
pub fn peak(peaks: &[f32], clip_len: Time, from: Time, to: Time) -> f32 {
    if peaks.is_empty() || clip_len.0 <= 0 {
        return 0.;
    }
    let n = peaks.len() as f64;
    let len = clip_len.0 as f64;
    let a = ((from.0 as f64 / len) * n).floor().max(0.) as usize;
    let b = (((to.0 as f64 / len) * n).ceil() as usize).clamp(a + 1, peaks.len());
    peaks
        .get(a.min(peaks.len() - 1)..b)
        .map(|s| s.iter().copied().fold(0., f32::max))
        .unwrap_or(0.)
}
