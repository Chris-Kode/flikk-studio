//! The sound pass of analysis: the audio decoded once at 32 kHz and heard
//! by the sound tagger and the sound search model as it streams, then the
//! beat tracker on each stretch of music the tagger found.

use crate::ffmpeg::{self, SOUND_RATE};
use anyhow::Result;
use flikk_core::analysis::{Levels, Music, Sound, SoundVectors};
use flikk_core::time::{FLICKS_PER_SECOND, Time, TimeRange};
use flikk_models::Task as ModelTask;
use flikk_models::beats::{self, BeatTracker};
use flikk_models::sound_search::{self, SoundSearch};
use flikk_models::sound_tagging::{self, Scores, Tagger};
use std::path::Path;

/// Audio either side of a stretch of music the beat tracker also hears,
/// since the tagger's edges are only good to about half a second.
const MUSIC_MARGIN: Time = Time(FLICKS_PER_SECOND);
/// Beats kept past the edges of the music.
const BEAT_SLACK: Time = Time(FLICKS_PER_SECOND / 4);

/// The models of the three tasks, shared by every job.
pub struct SoundModels {
    tagger: Box<dyn Tagger>,
    beats: Box<dyn BeatTracker>,
    search: Box<dyn SoundSearch>,
}

impl Default for SoundModels {
    fn default() -> Self {
        Self {
            tagger: sound_tagging::runner(ModelTask::SoundTagging.model()),
            beats: beats::runner(ModelTask::Beats.model()),
            search: sound_search::runner(ModelTask::SoundSearch.model()),
        }
    }
}

/// The parts of a pass besides the tagger.
#[derive(Clone, Copy, Debug)]
pub struct Pass {
    /// Vectors for sound search.
    pub search: bool,
    /// Beats in the music found.
    pub beats: bool,
}

/// What a pass heard, and what it could not do.
pub struct Heard {
    pub sounds: Vec<Sound>,
    pub music: Vec<Music>,
    pub vectors: Option<SoundVectors>,
    pub problems: Vec<String>,
}

impl SoundModels {
    /// Hear audio stream `stream` of `path`. `levels` are the 10 ms levels
    /// of the same stream, for the edges of short sounds; `hearing` and
    /// `beating` report the two halves' progress. The tagger is required;
    /// without the sound search model or the beat tracker the pass goes on,
    /// noting why. `pass` leaves either out. Fails once `cancel` says so.
    #[allow(clippy::too_many_arguments)]
    pub fn hear(
        &self,
        path: &Path,
        stream: u32,
        duration: Option<Time>,
        levels: Option<&Levels>,
        pass: Pass,
        hearing: &(dyn Fn(f32) + Sync),
        beating: &(dyn Fn(f32) + Sync),
        cancel: &(dyn Fn() -> bool + Sync),
    ) -> Result<Heard> {
        let mut problems = Vec::new();
        let (tagged, searched) = (self.tagger.windows(), self.search.windows());
        let mut tags = Slider::new(tagged.len, tagged.step);
        let mut clips = Slider::new(searched.len, searched.step);
        let mut scores: Vec<Scores> = Vec::new();
        // Until the sound search model fails.
        let mut vectors = pass
            .search
            .then(|| SoundVectors::new(searched.step_time(), searched.len_time()));
        let mut pending_tags = Vec::new();
        let mut pending_clips = Vec::new();
        let mut run = |tags_left: &mut Vec<Vec<f32>>, clips_left: &mut Vec<Vec<f32>>, all: bool| {
            while tags_left.len() >= tagged.batch || (all && !tags_left.is_empty()) {
                let n = tags_left.len().min(tagged.batch);
                let batch: Vec<Vec<f32>> = tags_left.drain(..n).collect();
                scores.extend(self.tagger.score(&batch)?);
            }
            while let Some(found) = vectors.as_mut()
                && (clips_left.len() >= searched.batch || (all && !clips_left.is_empty()))
            {
                let n = clips_left.len().min(searched.batch);
                let batch: Vec<Vec<f32>> = clips_left.drain(..n).collect();
                let windows: Vec<&[f32]> = batch.iter().map(Vec::as_slice).collect();
                match self.search.embed(&windows) {
                    Ok(embedded) => embedded.iter().for_each(|v| found.push(v)),
                    Err(e) => {
                        problems.push(format!("sound search: {e:#}"));
                        vectors = None;
                    }
                }
            }
            if vectors.is_none() {
                clips_left.clear();
            }
            anyhow::Ok(())
        };
        let mut total = 0usize;
        ffmpeg::stream_audio(
            path,
            stream,
            SOUND_RATE,
            None,
            duration,
            hearing,
            cancel,
            &mut |samples| {
                total += samples.len();
                tags.push(samples, &mut pending_tags);
                if pass.search {
                    clips.push(samples, &mut pending_clips);
                }
                run(&mut pending_tags, &mut pending_clips, false)
            },
        )?;
        tags.finish(&mut pending_tags, 0);
        // A last vector only for a window that is mostly there.
        clips.finish(&mut pending_clips, SOUND_RATE as usize);
        run(&mut pending_tags, &mut pending_clips, true)?;

        let end = Time::from_secs_f64(total as f64 / SOUND_RATE as f64);
        let (sounds, mut music) = self.tagger.read(&scores, end, levels);
        let all: i64 = music.iter().map(|m| m.range.duration().0).sum();
        let mut done = 0;
        for m in music.iter_mut().filter(|_| pass.beats) {
            if cancel() {
                anyhow::bail!("cancelled");
            }
            beating(done as f32 / all.max(1) as f32);
            done += m.range.duration().0;
            let stretch = TimeRange::new(
                (m.range.start - MUSIC_MARGIN).max(Time::ZERO),
                (m.range.end + MUSIC_MARGIN).min(end),
            );
            match self.beats_of(path, stream, stretch, cancel) {
                Ok((beats, downbeats)) => {
                    // The tracker carries the beat on a little past where
                    // the music is.
                    let near = TimeRange::new(m.range.start - BEAT_SLACK, m.range.end + BEAT_SLACK);
                    m.beats = beats.into_iter().filter(|t| near.contains(*t)).collect();
                    m.downbeats = downbeats
                        .into_iter()
                        .filter(|t| near.contains(*t))
                        .collect();
                }
                Err(e) => {
                    problems.push(format!("beats: {e:#}"));
                    break;
                }
            }
        }
        beating(1.0);
        Ok(Heard {
            sounds,
            music,
            vectors,
            problems,
        })
    }

    /// Beats and downbeats in `stretch` of the source, in source time.
    fn beats_of(
        &self,
        path: &Path,
        stream: u32,
        stretch: TimeRange,
        cancel: &(dyn Fn() -> bool + Sync),
    ) -> Result<(Vec<Time>, Vec<Time>)> {
        let (beats, downbeats) = self.beats.track(&mut |sink| {
            let range = Some(stretch);
            ffmpeg::stream_audio(path, stream, SOUND_RATE, range, None, &|_| {}, cancel, sink)
        })?;
        let shift = |t: Vec<Time>| t.into_iter().map(|t| t + stretch.start).collect();
        Ok((shift(beats), shift(downbeats)))
    }

    /// The vector of `text`.
    pub fn describe(&self, text: &str) -> Result<Vec<f32>> {
        self.search.describe(text)
    }
}

/// Windows of `len` samples every `step` over audio that arrives in
/// pieces.
struct Slider {
    len: usize,
    step: usize,
    buf: Vec<f32>,
    /// Sample index of `buf[0]` in the whole stream.
    offset: usize,
    /// Where the next window starts.
    next: usize,
}

impl Slider {
    fn new(len: usize, step: usize) -> Self {
        Self {
            len,
            step,
            buf: Vec::new(),
            offset: 0,
            next: 0,
        }
    }

    /// Add `samples`, moving every window they complete to `out`.
    fn push(&mut self, samples: &[f32], out: &mut Vec<Vec<f32>>) {
        self.buf.extend_from_slice(samples);
        while self.next + self.len <= self.offset + self.buf.len() {
            let at = self.next - self.offset;
            out.push(self.buf[at..at + self.len].to_vec());
            self.next += self.step;
        }
        let drop = (self.next - self.offset).min(self.buf.len());
        self.buf.drain(..drop);
        self.offset += drop;
    }

    /// The windows the end of the stream cuts short, each holding at least
    /// `min` samples (the first window always comes).
    fn finish(&mut self, out: &mut Vec<Vec<f32>>, min: usize) {
        let end = self.offset + self.buf.len();
        while self.next < end {
            let at = self.next - self.offset;
            if self.next > 0 && end - self.next < min {
                break;
            }
            out.push(self.buf[at..].to_vec());
            self.next += self.step;
        }
    }
}
