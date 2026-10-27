//! Levels from the playback mixer to the Mixer panel, lock-free: the mixer
//! thread stores each block's peak and a running RMS per audio track and
//! for the master; the panel reads them every frame while playing.

use koo_core::TrackId;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// A level older than this reads as silence (playback stopped).
const STALE: Duration = Duration::from_millis(250);

static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

fn now_ms() -> u64 {
    EPOCH.elapsed().as_millis() as u64
}

/// One meter: linear peak since the last read, RMS, and when it was set.
#[derive(Default)]
pub struct Level {
    peak: AtomicU32,
    rms: AtomicU32,
    at: AtomicU64,
}

impl Level {
    /// From the mixer thread, once per block.
    pub fn store(&self, peak: f32, rms: f32) {
        // Non-negative floats order like their bits.
        self.peak
            .fetch_max(peak.max(0.).to_bits(), Ordering::Relaxed);
        self.rms.store(rms.max(0.).to_bits(), Ordering::Relaxed);
        self.at.store(now_ms(), Ordering::Relaxed);
    }

    /// Peak since the last call, and the current RMS; zero when stale.
    pub fn take(&self) -> (f32, f32) {
        let peak = f32::from_bits(self.peak.swap(0, Ordering::Relaxed));
        let at = self.at.load(Ordering::Relaxed);
        if now_ms().saturating_sub(at) > STALE.as_millis() as u64 {
            return (0., 0.);
        }
        (peak, f32::from_bits(self.rms.load(Ordering::Relaxed)))
    }
}

type Levels = Mutex<HashMap<Option<TrackId>, Arc<Level>>>;

static LEVELS: LazyLock<Levels> = LazyLock::new(Default::default);

/// The meter of `track`, or of the master for `None`.
pub fn level(track: Option<TrackId>) -> Arc<Level> {
    let mut levels = LEVELS.lock().unwrap_or_else(|e| e.into_inner());
    levels.entry(track).or_default().clone()
}

/// What a strip draws: a falling bar and a peak hold, both linear.
#[derive(Clone, Copy, Default)]
pub struct Display {
    pub peak: f32,
    pub rms: f32,
    pub hold: f32,
    hold_at: Option<Instant>,
    last: Option<Instant>,
}

/// Fall rate of the bar, dB per second, and how long a peak holds.
const FALL_DB_PER_S: f32 = 24.;
const HOLD: Duration = Duration::from_millis(1500);

impl Display {
    /// Fold in a reading.
    pub fn update(&mut self, (peak, rms): (f32, f32)) {
        let now = Instant::now();
        let dt = self.last.map_or(0., |l| (now - l).as_secs_f32());
        self.last = Some(now);
        let fall = 10f32.powf(-FALL_DB_PER_S * dt / 20.);
        self.peak = peak.max(self.peak * fall);
        self.rms = rms;
        let held = self.hold_at.is_some_and(|at| now - at < HOLD);
        if peak >= self.hold || !held {
            self.hold = if held { self.hold.max(peak) } else { peak };
            self.hold_at = Some(now);
        }
    }

    pub fn is_silent(&self) -> bool {
        self.peak < 1e-5 && self.hold < 1e-5
    }
}

/// Meter scale: dBFS → 0..1 of the bar, -60 dB at the bottom.
pub fn position(linear: f32) -> f32 {
    if linear <= 0. {
        return 0.;
    }
    ((20. * linear.log10() + 60.) / 60.).clamp(0., 1.)
}
