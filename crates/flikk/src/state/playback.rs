//! Program playback: the playhead advancing in real time, or at a shuttle
//! speed (JKL), until the timeline's end (or start, backwards) or a stop.
//! The pool's source plays on its own clock (see `source`).

use super::EditorState;
use flikk_core::Time;
use gpui_kit::*;
use std::time::{Duration, Instant};

/// Playback clock resolution.
pub(super) const PLAY_TICK: Duration = Duration::from_millis(16);

/// Whether the program plays, and how.
pub(super) enum Playback {
    Stopped,
    /// `speed` 1.0 is real time, negative plays backwards; `_clock` advances
    /// the playhead and stops with it.
    Playing {
        speed: f64,
        _clock: Task<()>,
    },
}

impl Playback {
    /// The speed it plays at, 1.0 while stopped: a shuttle's speed ends
    /// with it, so the next play is real time.
    fn speed(&self) -> f64 {
        match self {
            Self::Stopped => 1.0,
            Self::Playing { speed, .. } => *speed,
        }
    }
}

impl EditorState {
    /// The program is playing.
    pub fn is_playing(&self) -> bool {
        matches!(self.playback, Playback::Playing { .. })
    }

    /// Shuttle speed: 1.0 is real time, negative plays backwards (JKL).
    /// 1.0 while stopped.
    pub fn play_speed(&self) -> f64 {
        self.playback.speed()
    }

    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if let Some(item) = self.source_picture().map(|s| s.item().clone()) {
            self.toggle_source(&item, cx);
            return;
        }
        let playing = !self.is_playing();
        self.set_playing(playing, cx);
    }

    /// JKL: `speed` 0 stops; otherwise play at that rate (negative = reverse).
    pub fn shuttle(&mut self, speed: f64, cx: &mut Context<Self>) {
        if speed == 0.0 {
            self.set_playing(false, cx);
        } else {
            // The clock starts over at the new speed.
            self.playback = Playback::Stopped;
            self.play(speed, cx);
        }
    }

    /// Play at the speed playing now (real time from a stop), or stop.
    pub fn set_playing(&mut self, playing: bool, cx: &mut Context<Self>) {
        if playing {
            self.play(self.play_speed(), cx);
        } else {
            self.playback = Playback::Stopped;
            cx.notify();
        }
    }

    /// Play at `speed`, closing the source in the viewer; forwards from the
    /// end starts over. A clock already running goes on.
    fn play(&mut self, speed: f64, cx: &mut Context<Self>) {
        self.close_source(cx);
        let end = self.display_project().timeline.duration();
        if speed > 0.0 && self.playhead >= end {
            self.playhead = Time::ZERO;
        }
        match &mut self.playback {
            Playback::Playing { speed: playing, .. } => *playing = speed,
            Playback::Stopped => {
                let clock = cx.spawn(async move |this, cx| {
                    let mut last = Instant::now();
                    loop {
                        cx.background_executor().timer(PLAY_TICK).await;
                        let alive = this.update(cx, |this, cx| this.tick(&mut last, cx));
                        if !matches!(alive, Ok(true)) {
                            break;
                        }
                    }
                });
                self.playback = Playback::Playing {
                    speed,
                    _clock: clock,
                };
            }
        }
        cx.notify();
    }

    /// Advance the playhead by the wall time since the last tick. A seek in
    /// between (the playhead moved elsewhere) goes on from the new position.
    /// Returns false once playback stopped.
    fn tick(&mut self, last: &mut Instant, cx: &mut Context<Self>) -> bool {
        let Playback::Playing { speed, .. } = self.playback else {
            return false;
        };
        let now = Instant::now();
        let elapsed = now.duration_since(*last).as_secs_f64() * speed;
        *last = now;
        if elapsed == 0.0 {
            return true;
        }
        let end = self.display_project().timeline.duration();
        let t = self.playhead + Time::from_secs_f64(elapsed);
        let done = (speed > 0.0 && t >= end) || (speed < 0.0 && t <= Time::ZERO);
        self.playhead = t.max(Time::ZERO).min(end);
        if done {
            self.playback = Playback::Stopped;
        }
        cx.notify();
        !done
    }
}
