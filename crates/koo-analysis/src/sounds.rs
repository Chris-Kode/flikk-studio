//! The sound pass of analysis: the audio decoded once at 32 kHz and heard
//! by the sound tagger and CLAP's audio tower as it streams, then the beat
//! tracker on each stretch of music the tagger found.

use crate::beats::{BeatModel, Tracker};
use crate::clap::{self, ClapAudio, ClapText, TextFiles};
use crate::download::Download;
use crate::ffmpeg::{self, SOUND_RATE};
use crate::tagger::{self, Scores, Tagger};
use anyhow::Result;
use koo_core::analysis::{Levels, Music, Sound, SoundVectors};
use koo_core::time::{FLICKS_PER_SECOND, Time, TimeRange};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// Audio either side of a stretch of music the beat tracker also hears,
/// since the tagger's edges are only good to about half a second.
const MUSIC_MARGIN: Time = Time(FLICKS_PER_SECOND);
/// Beats kept past the edges of the music.
const BEAT_SLACK: Time = Time(FLICKS_PER_SECOND / 4);

/// The sound models, loaded on first use and shared by every job; each
/// runs one batch at a time.
#[derive(Default)]
pub struct SoundModels {
    tagger: Mutex<Option<(PathBuf, Tagger)>>,
    beats: Mutex<Option<(PathBuf, BeatModel)>>,
    clap: Mutex<Option<(PathBuf, ClapAudio)>>,
    text: Mutex<Option<ClapText>>,
    /// Tagger, beat tracker, CLAP audio, then CLAP text and its tokenizer.
    pub downloads: [Download; 3],
    pub text_downloads: [Download; 3],
}

/// The model files a pass uses; the tagger is required, the others are
/// left out (with the reason) when they could not be had.
pub struct Files {
    pub tagger: PathBuf,
    pub beats: Result<PathBuf>,
    pub clap: Result<PathBuf>,
}

/// What a pass heard, and what it could not do.
pub struct Heard {
    pub sounds: Vec<Sound>,
    pub music: Vec<Music>,
    pub vectors: Option<SoundVectors>,
    pub problems: Vec<String>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `slot`'s model, loading it from `path` when it holds none or another.
fn loaded<'a, T>(
    slot: &'a Mutex<Option<(PathBuf, T)>>,
    path: &Path,
    load: impl FnOnce(&Path) -> Result<T>,
) -> Result<MutexGuard<'a, Option<(PathBuf, T)>>> {
    let mut guard = lock(slot);
    if guard.as_ref().is_none_or(|(p, _)| p != path) {
        *guard = Some((path.to_path_buf(), load(path)?));
    }
    Ok(guard)
}

impl SoundModels {
    /// Hear audio stream `stream` of `path`. `levels` are the 10 ms levels
    /// of the same stream, for the edges of short sounds; `hearing` and
    /// `beating` report the two halves' progress.
    #[allow(clippy::too_many_arguments)]
    pub fn hear(
        &self,
        path: &Path,
        stream: u32,
        duration: Option<Time>,
        levels: Option<&Levels>,
        files: Files,
        hearing: &(dyn Fn(f32) + Sync),
        beating: &(dyn Fn(f32) + Sync),
    ) -> Result<Heard> {
        let mut problems = Vec::new();
        let clap_path = match files.clap {
            Ok(p) => Some(p),
            Err(e) => {
                problems.push(format!("sound search: {e:#}"));
                None
            }
        };
        let mut tags = Slider::new(tagger::WINDOW, tagger::STEP);
        let mut clips = Slider::new(clap::WINDOW, clap::STEP);
        let mut scores: Vec<Scores> = Vec::new();
        let mut vectors = clap_path
            .as_ref()
            .map(|_| SoundVectors::new(clap::STEP_TIME, clap::WINDOW_TIME));
        let mut pending_tags = Vec::new();
        let mut pending_clips = Vec::new();
        let mut run = |tags_left: &mut Vec<Vec<f32>>, clips_left: &mut Vec<Vec<f32>>, all: bool| {
            while tags_left.len() >= tagger::BATCH || (all && !tags_left.is_empty()) {
                let n = tags_left.len().min(tagger::BATCH);
                let batch: Vec<Vec<f32>> = tags_left.drain(..n).collect();
                let mut model = loaded(&self.tagger, &files.tagger, Tagger::load)?;
                scores.extend(model.as_mut().expect("loaded").1.run(&batch)?);
            }
            if let (Some(path), Some(vectors)) = (&clap_path, vectors.as_mut()) {
                while clips_left.len() >= clap::BATCH || (all && !clips_left.is_empty()) {
                    let n = clips_left.len().min(clap::BATCH);
                    let batch: Vec<Vec<f32>> = clips_left.drain(..n).collect();
                    let windows: Vec<&[f32]> = batch.iter().map(Vec::as_slice).collect();
                    let mut model = loaded(&self.clap, path, ClapAudio::load)?;
                    for v in model.as_mut().expect("loaded").1.embed(&windows)? {
                        vectors.push(&v);
                    }
                }
            } else {
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
            &mut |samples| {
                total += samples.len();
                tags.push(samples, &mut pending_tags);
                clips.push(samples, &mut pending_clips);
                run(&mut pending_tags, &mut pending_clips, false)
            },
        )?;
        tags.finish(&mut pending_tags, 0);
        // A last vector only for a window that is mostly there.
        clips.finish(&mut pending_clips, SOUND_RATE as usize);
        run(&mut pending_tags, &mut pending_clips, true)?;

        let end = Time::from_secs_f64(total as f64 / SOUND_RATE as f64);
        let (sounds, mut music) = tagger::read(&scores, end, levels);
        match files.beats {
            Ok(model) => {
                let all: i64 = music.iter().map(|m| m.range.duration().0).sum();
                let mut done = 0;
                for m in music.iter_mut() {
                    beating(done as f32 / all.max(1) as f32);
                    done += m.range.duration().0;
                    let stretch = TimeRange::new(
                        (m.range.start - MUSIC_MARGIN).max(Time::ZERO),
                        (m.range.end + MUSIC_MARGIN).min(end),
                    );
                    match self.beats_of(path, stream, stretch, &model) {
                        Ok((beats, downbeats)) => {
                            // The tracker carries the beat on a little
                            // past where the music is.
                            let near = TimeRange::new(
                                m.range.start - BEAT_SLACK,
                                m.range.end + BEAT_SLACK,
                            );
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
            }
            Err(e) if !music.is_empty() => problems.push(format!("beats: {e:#}")),
            Err(_) => {}
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
        model: &Path,
    ) -> Result<(Vec<Time>, Vec<Time>)> {
        let mut guard = loaded(&self.beats, model, BeatModel::load)?;
        let mut tracker = Tracker::new(&mut guard.as_mut().expect("loaded").1);
        ffmpeg::stream_audio(
            path,
            stream,
            SOUND_RATE,
            Some(stretch),
            None,
            &|_| {},
            &mut |samples| tracker.push(samples),
        )?;
        let (beats, downbeats) = tracker.finish()?;
        let shift = |t: Vec<Time>| t.into_iter().map(|t| t + stretch.start).collect();
        Ok((shift(beats), shift(downbeats)))
    }

    /// The vector of `text`, loading the text tower first if needed.
    pub fn describe(&self, files: &TextFiles, text: &str) -> Result<Vec<f32>> {
        let mut guard = lock(&self.text);
        if guard.is_none() {
            *guard = Some(ClapText::load(files)?);
        }
        guard.as_mut().expect("loaded above").embed(text)
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
