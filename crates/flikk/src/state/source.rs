//! A pool item or library sound played on its own: sounds from the pool and
//! the library, pictures in the viewer. It is the item as the one clip of a
//! scratch project, so it goes through the same compositor and audio mixer
//! as the program, and the timeline, its playhead and the open project stay
//! as they are. Timeline playback closes it, and playing it stops the
//! timeline; seeking the timeline closes a picture.

use super::{EditorState, PLAY_TICK};
use crate::ui::viewer::audio::AudioOut;
use flikk_core::model::{Clip, ClipContent, MediaItem, MediaKind, Project, Track, TrackKind};
use flikk_core::{MediaId, Time};
use gpui_kit::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often a sound's position is redrawn; pictures follow the program's
/// clock.
const SOUND_TICK: Duration = Duration::from_millis(100);
/// Length of the clip a still is shown from.
const STILL: f64 = 1.0;

pub struct Source {
    item: MediaItem,
    project: Arc<Project>,
    /// Where it last started or paused.
    from: Time,
    playing: Option<Playing>,
}

struct Playing {
    since: Instant,
    _audio: AudioOut,
    _tick: Task<()>,
}

impl Source {
    /// Whether this is `item`. Library items have no id of their own, so
    /// the file tells.
    pub fn is(&self, item: &MediaItem) -> bool {
        self.item.path == item.path
    }

    pub fn item(&self) -> &MediaItem {
        &self.item
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn is_playing(&self) -> bool {
        self.playing.is_some()
    }

    /// Whether it runs at all; a still doesn't.
    pub fn can_play(&self) -> bool {
        length(&self.item).is_some()
    }

    /// The last frame's start: past it there is no picture to show.
    fn last(&self) -> Time {
        let settings = &self.project.timeline.settings;
        let frame = Time::from_frame(1, settings.rate);
        (self.project.timeline.duration() - frame).max(Time::ZERO)
    }

    pub fn position(&self) -> Time {
        let played = self.playing.as_ref().map_or(Time::ZERO, |p| {
            Time::from_secs_f64(p.since.elapsed().as_secs_f64())
        });
        (self.from + played).min(self.last())
    }

    /// How far it has played, 0..1.
    pub fn fraction(&self) -> f32 {
        let length = self.project.timeline.duration().0.max(1) as f64;
        (self.position().0 as f64 / length) as f32
    }
}

/// How long `item` runs; `None` for a still or when unknown.
fn length(item: &MediaItem) -> Option<Time> {
    let length = item.info.as_ref()?.duration;
    (item.kind != MediaKind::Image && length > Time::ZERO).then_some(length)
}

/// `item` as the one clip of a project of its own, in its own frame size
/// and rate.
fn scratch(item: &MediaItem) -> Arc<Project> {
    let mut project = Project::new("Source");
    project.timeline.tracks.clear();
    let info = item.info.as_ref();
    if let Some(video) = info.and_then(|i| i.video.as_ref()) {
        let settings = &mut project.timeline.settings;
        settings.width = video.width;
        settings.height = video.height;
        settings.rate = video.rate;
    }
    let length = length(item).unwrap_or(Time::from_secs_f64(STILL));
    let clip = Clip::new(
        Time::ZERO,
        length,
        ClipContent::Media {
            media: item.id,
            source_in: Time::ZERO,
            audio_stream: None,
        },
    );
    let mut track = |kind, name| {
        let mut track = Track::new(kind, name);
        track.clips.push(clip.clone());
        project.timeline.tracks.push(track);
    };
    if item.kind != MediaKind::Audio {
        track(TrackKind::Video, "V1");
    }
    if item.kind != MediaKind::Image && info.is_none_or(|i| !i.audio.is_empty()) {
        track(TrackKind::Audio, "A1");
    }
    project.media.push(item.clone());
    Arc::new(project)
}

impl EditorState {
    /// The source showing in the viewer in place of the program.
    pub fn source_picture(&self) -> Option<&Source> {
        let source = self.source.as_ref();
        source.filter(|s| s.item.kind != MediaKind::Audio)
    }

    /// What the viewer shows.
    pub fn monitor_project(&self) -> &Project {
        self.source_picture()
            .map_or(self.display_project(), |s| s.project())
    }

    pub fn monitor_time(&self) -> Time {
        self.source_picture()
            .map_or(self.playhead, Source::position)
    }

    pub fn monitor_playing(&self) -> bool {
        self.source_picture()
            .map_or(self.playing, Source::is_playing)
    }

    /// Whether `item` is a sound that can play now.
    pub fn can_listen(&self, item: &MediaItem) -> bool {
        item.kind == MediaKind::Audio
            && length(item).is_some()
            && self.services.media.is_some()
            && !self.offline.contains(&item.id)
    }

    /// Show pool item `media` in the viewer, paused at its start.
    pub fn open_source(&mut self, media: MediaId, cx: &mut Context<Self>) {
        let Some(item) = self.bus.project().media(media).cloned() else {
            return;
        };
        if self.offline.contains(&media) {
            self.status = Some("That file is offline".into());
            cx.notify();
            return;
        }
        if self.playing {
            self.set_playing(false, cx);
        }
        self.source = Some(Source {
            project: scratch(&item),
            item,
            from: Time::ZERO,
            playing: None,
        });
        cx.notify();
    }

    /// Play `item`, or pause it when it is playing.
    pub fn toggle_source(&mut self, item: &MediaItem, cx: &mut Context<Self>) {
        match &mut self.source {
            Some(s) if s.is(item) && s.is_playing() => {
                s.from = s.position();
                s.playing = None;
                cx.notify();
            }
            Some(s) if s.is(item) => {
                let from = s.from;
                self.play_source(item, from, cx);
            }
            _ => self.play_source(item, Time::ZERO, cx),
        }
    }

    /// Go to `fraction` (0..1) of `item`, playing unless it was paused
    /// there.
    pub fn seek_source(&mut self, item: &MediaItem, fraction: f32, cx: &mut Context<Self>) {
        let Some(length) = length(item) else {
            return;
        };
        let at = Time((length.0 as f64 * fraction.clamp(0., 1.) as f64) as i64);
        match &mut self.source {
            Some(s) if s.is(item) && !s.is_playing() => {
                s.from = at.min(s.last());
                cx.notify();
            }
            _ => self.play_source(item, at, cx),
        }
    }

    pub fn close_source(&mut self, cx: &mut Context<Self>) {
        if self.source.take().is_some() {
            cx.notify();
        }
    }

    fn play_source(&mut self, item: &MediaItem, from: Time, cx: &mut Context<Self>) {
        let Some(media) = self.services.media.clone() else {
            return;
        };
        if length(item).is_none() {
            return;
        }
        if self.playing {
            self.set_playing(false, cx);
        }
        let project = match &self.source {
            Some(s) if s.is(item) => s.project.clone(),
            _ => scratch(item),
        };
        let tick = if item.kind == MediaKind::Audio {
            SOUND_TICK
        } else {
            PLAY_TICK
        };
        let clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(tick).await;
                let alive = this.update(cx, |this, cx| this.tick_source(cx));
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        });
        let mut source = Source {
            item: item.clone(),
            project,
            from,
            playing: None,
        };
        // From the end, it starts over.
        if from >= source.last() {
            source.from = Time::ZERO;
        }
        source.playing = Some(Playing {
            since: Instant::now(),
            _audio: AudioOut::start(media, source.project.clone(), source.from),
            _tick: clock,
        });
        self.source = Some(source);
        cx.notify();
    }

    /// Redraw the position. At the end a sound closes and a picture stays
    /// on its last frame. Whether to go on.
    fn tick_source(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(source) = self.source.as_mut().filter(|s| s.is_playing()) else {
            return false;
        };
        if source.position() < source.last() {
            cx.notify();
            return true;
        }
        if source.item.kind == MediaKind::Audio {
            self.close_source(cx);
        } else {
            source.from = source.last();
            source.playing = None;
            cx.notify();
        }
        false
    }
}
