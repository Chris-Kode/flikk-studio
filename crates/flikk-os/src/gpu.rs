//! GPU work taking turns, and the list of what takes them.
//!
//! Flikk's GPU users sit on separate APIs (wgpu, ONNX Runtime, Whisper,
//! MLX), so they are coordinated here, on the CPU side, before each unit
//! of work: one frame, one inference. A unit is a [`Turn`]. Work comes in
//! three levels:
//!
//! - **watched**: what the user is looking at right now (playback, a paused
//!   or scrubbed frame). It never waits: [`watching`] only marks the GPU as
//!   watched, and the mark lapses [`GRACE`] after the last watched frame;
//! - [`Level::Asked`]: work the user asked for and waits on (an export).
//!   One turn at a time, served in the order asked, so two asked jobs
//!   alternate;
//! - [`Level::Background`]: work nobody waits on (the render cache). One
//!   turn at a time, none while the GPU is watched, and one turn in every
//!   [`BACKGROUND_EVERY`] while asked work runs, so it is never starved.
//!
//! A [`Job`] is the long-lived work that takes many turns. It is listed
//! ([`jobs`]) from [`Job::new`] until it is dropped, so a new kind of GPU
//! work joins the turns and the list just by opening a job. Work too
//! short-lived to deserve a row takes turns with [`turn`] instead.
//!
//! Turns share the GPU out; nothing breaks when two units overlap, so no
//! mistake in taking them can stop work for good. Every turn and job lets
//! go when dropped (a panic, a cancel or an early return never holds the
//! GPU or leaves a row behind); a turn asked for inside another on the
//! same thread is that same turn (a frame whose composing runs a model
//! that takes turns too); and a turn held longer than [`LONGEST_TURN`]
//! (a long unit, or a holder waiting on other GPU work from another
//! thread) no longer keeps the next waiter out.

use crate::sync::{lock, wait_timeout};
use std::cell::Cell;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// How long the GPU stays watched after the last watched frame, so the
/// gaps between frames of a playback don't let background work in.
pub const GRACE: Duration = Duration::from_millis(250);
/// While asked work waits, background work gets one turn in this many.
pub const BACKGROUND_EVERY: u32 = 4;
/// A watched frame longer than this no longer keeps the GPU watched: a
/// guard kept by mistake cannot hold background work off for good.
const LONGEST_WATCHED_FRAME: Duration = Duration::from_secs(2);
/// A turn held longer than this no longer keeps the next waiter out, which
/// takes the turn over: the two overlap, as all GPU work did before turns.
pub const LONGEST_TURN: Duration = Duration::from_secs(5);
/// How often a waiter looks at its cancel and at a watched mark lapsing.
const RECHECK: Duration = Duration::from_millis(50);
/// A job that had turns still shows as running for this long while it
/// waits for the next one, so jobs taking turns don't flicker.
const STILL_RUNNING: Duration = Duration::from_secs(1);

/// Whether someone waits on a job's work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// The user asked for it and waits on it.
    Asked,
    /// Nobody waits on it; it waits while the GPU is watched.
    Background,
}

/// Where a listed job stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// It holds a turn, or had one lately.
    Running,
    /// It has not had a turn yet, or has waited a while for the next.
    Waiting,
}

/// A job as [`jobs`] lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    /// Unique for the session: what a row is keyed by.
    pub id: u64,
    /// What the user reads, such as "Export: Interview.mp4".
    pub label: String,
    pub level: Level,
    pub state: State,
    /// Done so far, 0–1, once the job has said.
    pub progress: Option<f32>,
}

/// Long-lived GPU work, listed until dropped. Not `Clone`: share it by
/// reference.
#[must_use = "a job is listed only while it is kept"]
pub struct Job {
    id: u64,
    level: Level,
}

impl Job {
    /// Open a job named `label` for the user, listed as waiting until its
    /// first turn.
    pub fn new(label: impl Into<String>, level: Level) -> Self {
        let mut board = lock(&BOARD.state);
        let id = board.next_id();
        board.rows.push(Row {
            id,
            label: label.into(),
            level,
            phase: Phase::Opened,
            progress: None,
        });
        Self { id, level }
    }

    /// Wait for this job's next turn; `None` once `cancel` is set, while
    /// waiting or before. A turn borrows its job, so a job is never
    /// dropped while it holds or waits for one.
    pub fn turn(&self, cancel: &AtomicBool) -> Option<Turn<'_>> {
        take(self.level, Some(self.id), cancel)
    }

    /// Say how much is done, 0–1 (clamped).
    pub fn progress(&self, fraction: f32) {
        let mut board = lock(&BOARD.state);
        if let Some(row) = board.row(self.id) {
            row.progress = Some(fraction.clamp(0.0, 1.0));
        }
    }
}

impl Drop for Job {
    /// Takes the row off the list. A turn of this job cannot outlive it,
    /// short of being leaked; even then the turn is given back here, so
    /// jobs waiting behind it carry on.
    fn drop(&mut self) {
        let mut board = lock(&BOARD.state);
        board.rows.retain(|row| row.id != self.id);
        if matches!(board.slot, Slot::Held { job: Some(job), .. } if job == self.id) {
            board.slot = Slot::Free;
        }
        drop(board);
        BOARD.changed.notify_all();
    }
}

/// Wait for a turn at `level` for work that is not listed: short-lived
/// work, such as frames rendered ahead of a resting playhead, that would
/// only flicker in the list. `None` once `cancel` is set.
pub fn turn(level: Level, cancel: &AtomicBool) -> Option<Turn<'static>> {
    take(level, None, cancel)
}

/// One unit of GPU work; the next waiter goes once it is dropped. Work
/// submitted during a turn and waited on in the next (pipelined frames)
/// is fine: one job's frames never stack up more than one deep. It stays
/// on the thread that took it, where a turn asked for meanwhile is this
/// same one.
#[must_use = "the turn ends as soon as it is dropped"]
pub struct Turn<'a> {
    /// `None` for a turn taken inside another of its thread.
    ticket: Option<u64>,
    job: Option<u64>,
    _job: PhantomData<&'a Job>,
    /// Not `Send`: its thread counts the turns it holds.
    _thread: PhantomData<*const ()>,
}

thread_local! {
    /// The turns this thread holds, one inside another.
    static HELD: Cell<u32> = const { Cell::new(0) };
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        HELD.with(|held| held.set(held.get().saturating_sub(1)));
        let mut board = lock(&BOARD.state);
        // Taken over after `LONGEST_TURN`, the turn is no longer this one's
        // to give back.
        if matches!(board.slot, Slot::Held { ticket, .. } if Some(ticket) == self.ticket) {
            board.slot = Slot::Free;
        }
        if let Some(row) = self.job.and_then(|id| board.row(id)) {
            row.phase = Phase::Between;
        }
        drop(board);
        BOARD.changed.notify_all();
    }
}

/// Mark the GPU as watched for as long as the guard is kept, and [`GRACE`]
/// after. Never waits. Keep one per watched frame, not per playback.
pub fn watching() -> Watching {
    let mut board = lock(&BOARD.state);
    let id = board.next_id();
    board.watchers.push((id, Instant::now()));
    Watching { id }
}

/// See [`watching`].
#[must_use = "the GPU is watched only while the guard is kept"]
pub struct Watching {
    id: u64,
}

impl Drop for Watching {
    fn drop(&mut self) {
        let mut board = lock(&BOARD.state);
        board.watchers.retain(|(id, _)| *id != self.id);
        board.last_watched = Some(Instant::now());
    }
}

/// The jobs open now: running first, then waiting, each in the order
/// opened. Cheap (a lock and a few rows); compare two lists to see a
/// change, as states move with time as well as with turns.
pub fn jobs() -> Vec<Listed> {
    let board = lock(&BOARD.state);
    let now = Instant::now();
    let mut listed: Vec<Listed> = board
        .rows
        .iter()
        .map(|row| Listed {
            id: row.id,
            label: row.label.clone(),
            level: row.level,
            state: row.phase.state(now),
            progress: row.progress,
        })
        .collect();
    // Stable: the order opened stays within each state.
    listed.sort_by_key(|job| job.state == State::Waiting);
    listed
}

/// The one board every turn is taken on.
static BOARD: Board = Board {
    state: Mutex::new(Turns {
        next: 0,
        rows: Vec::new(),
        queue: VecDeque::new(),
        slot: Slot::Free,
        asked_in_a_row: 0,
        watchers: Vec::new(),
        last_watched: None,
    }),
    changed: Condvar::new(),
};

struct Board {
    state: Mutex<Turns>,
    /// A turn was given back, or the waiters changed.
    changed: Condvar,
}

struct Turns {
    /// Ids for jobs, tickets and watchers alike.
    next: u64,
    rows: Vec<Row>,
    /// Waiting for a turn, in the order they asked.
    queue: VecDeque<Waiter>,
    slot: Slot,
    /// Asked turns given in a row while background work waited for one.
    asked_in_a_row: u32,
    /// Watched frames under way, and when each began.
    watchers: Vec<(u64, Instant)>,
    /// When the last watched frame ended.
    last_watched: Option<Instant>,
}

struct Row {
    id: u64,
    label: String,
    level: Level,
    phase: Phase,
    progress: Option<f32>,
}

/// Where a job is between opening and dropping.
#[derive(Clone, Copy)]
enum Phase {
    /// No turn asked for yet.
    Opened,
    /// Asked for a turn at `since`; `ran` when it had one before.
    Waiting {
        since: Instant,
        ran: bool,
    },
    Holding,
    /// Had a turn and has not asked for the next yet.
    Between,
}

impl Phase {
    fn state(self, now: Instant) -> State {
        match self {
            Phase::Holding | Phase::Between => State::Running,
            Phase::Waiting { since, ran: true } if now - since < STILL_RUNNING => State::Running,
            Phase::Opened | Phase::Waiting { .. } => State::Waiting,
        }
    }

    /// The phase a waiter goes back to when its wait is cancelled.
    fn before_waiting(self) -> Phase {
        match self {
            Phase::Waiting { ran: true, .. } => Phase::Between,
            Phase::Waiting { ran: false, .. } => Phase::Opened,
            phase => phase,
        }
    }
}

struct Waiter {
    ticket: u64,
    level: Level,
}

/// The one GPU turn.
#[derive(Clone, Copy)]
enum Slot {
    Free,
    Held {
        ticket: u64,
        job: Option<u64>,
        since: Instant,
    },
}

impl Turns {
    fn next_id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn row(&mut self, id: u64) -> Option<&mut Row> {
        self.rows.iter_mut().find(|row| row.id == id)
    }

    fn watched(&self, now: Instant) -> bool {
        let framing = self
            .watchers
            .iter()
            .any(|(_, since)| now - *since < LONGEST_WATCHED_FRAME);
        framing || self.last_watched.is_some_and(|at| now - at < GRACE)
    }

    /// The oldest waiter at `level` that may go now.
    fn first(&self, level: Level, now: Instant) -> Option<u64> {
        if level == Level::Background && self.watched(now) {
            return None;
        }
        self.queue
            .iter()
            .find(|w| w.level == level)
            .map(|w| w.ticket)
    }

    /// Who gets the turn once it is free, or held longer than
    /// [`LONGEST_TURN`]: asked work first, but background work after
    /// [`BACKGROUND_EVERY`] − 1 asked turns in a row.
    fn next_up(&self, now: Instant) -> Option<u64> {
        if let Slot::Held { since, .. } = self.slot
            && now - since < LONGEST_TURN
        {
            return None;
        }
        let asked = self.first(Level::Asked, now);
        let background = self.first(Level::Background, now);
        match (asked, background) {
            (Some(_), Some(background)) if self.asked_in_a_row + 1 >= BACKGROUND_EVERY => {
                Some(background)
            }
            (Some(asked), _) => Some(asked),
            (None, background) => background,
        }
    }

    /// Give the turn to `ticket`, out of the queue.
    fn give(&mut self, ticket: u64, job: Option<u64>, now: Instant) {
        let Some(at) = self.queue.iter().position(|w| w.ticket == ticket) else {
            return;
        };
        let level = self.queue[at].level;
        self.queue.remove(at);
        let background_waits = self.first(Level::Background, now).is_some();
        self.asked_in_a_row = match level {
            Level::Asked if background_waits => self.asked_in_a_row + 1,
            Level::Asked | Level::Background => 0,
        };
        self.slot = Slot::Held {
            ticket,
            job,
            since: now,
        };
        if let Some(row) = job.and_then(|id| self.row(id)) {
            row.phase = Phase::Holding;
        }
    }
}

/// Queue for a turn at `level` for `job` (or unlisted work) and wait for
/// it, giving up once `cancel` is set.
fn take(level: Level, job: Option<u64>, cancel: &AtomicBool) -> Option<Turn<'static>> {
    let turn = |ticket: Option<u64>| {
        HELD.with(|held| held.set(held.get() + 1));
        Turn {
            ticket,
            job,
            _job: PhantomData,
            _thread: PhantomData,
        }
    };
    let mut board = lock(&BOARD.state);
    if HELD.with(Cell::get) > 0 {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        if let Some(row) = job.and_then(|id| board.row(id)) {
            row.phase = Phase::Holding;
        }
        return Some(turn(None));
    }
    let ticket = board.next_id();
    board.queue.push_back(Waiter { ticket, level });
    let now = Instant::now();
    if let Some(row) = job.and_then(|id| board.row(id)) {
        let ran = !matches!(row.phase, Phase::Opened);
        row.phase = Phase::Waiting { since: now, ran };
    }
    loop {
        if cancel.load(Ordering::Relaxed) {
            board.queue.retain(|w| w.ticket != ticket);
            if let Some(row) = job.and_then(|id| board.row(id)) {
                row.phase = row.phase.before_waiting();
            }
            drop(board);
            // The waiter behind this one may be next now.
            BOARD.changed.notify_all();
            return None;
        }
        let now = Instant::now();
        if board.next_up(now) == Some(ticket) {
            board.give(ticket, job, now);
            return Some(turn(Some(ticket)));
        }
        // Timed: a cancel and a watched mark lapsing ring no bell.
        board = wait_timeout(&BOARD.changed, board, RECHECK);
    }
}
