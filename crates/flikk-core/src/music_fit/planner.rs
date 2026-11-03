//! A path through a song's bars that lasts a given time: the intro and the
//! real ending kept, few jumps, each between bars that sound alike and at
//! the same place in the phrase.

use super::features::{BarFeatures, MFCC, PARTS};
use crate::time::{Time, TimeRange};

/// A song cut into bars, in source time.
#[derive(Clone, Debug)]
pub struct Song {
    /// What can play: from the clip's in point to the end of the media.
    pub source: TimeRange,
    /// Back to back, from the first downbeat at or after `source.start` to
    /// the last one; the head before and the tail after always play.
    pub bars: Vec<TimeRange>,
    /// Bars of the usual length; only those take jumps.
    pub regular: Vec<bool>,
    pub features: Vec<BarFeatures>,
    /// Typical beat.
    pub beat: Time,
}

impl Song {
    pub fn head(&self) -> Time {
        self.bars
            .first()
            .map_or(Time::ZERO, |b| b.start - self.source.start)
    }

    pub fn tail(&self) -> Time {
        self.bars
            .last()
            .map_or(self.source.duration(), |b| self.source.end - b.end)
    }

    fn bar_secs(&self, i: usize) -> f64 {
        self.bars[i].duration().as_secs_f64()
    }

    fn typical_bar(&self) -> f64 {
        let mut d: Vec<f64> = (0..self.bars.len()).map(|i| self.bar_secs(i)).collect();
        d.sort_by(f64::total_cmp);
        d.get(d.len() / 2).copied().unwrap_or(2.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitOptions {
    /// Equal-power crossfade at each jump, ending on the cut.
    pub crossfade: Time,
    /// Repeat sections to make the song longer.
    pub loops: bool,
    /// Bars played through after a jump before the next one.
    pub min_run: usize,
}

impl Default for FitOptions {
    fn default() -> Self {
        Self {
            crossfade: Time::from_millis(30),
            loops: true,
            min_run: 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JumpKind {
    /// Leaves bars out.
    Skip,
    /// Plays bars again.
    Loop,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Jump {
    /// From the fitted song's start.
    pub at: Time,
    /// Source time it leaves and lands at.
    pub from: Time,
    pub to: Time,
    pub kind: JumpKind,
    /// Source span left out (skip) or played again (loop).
    pub span: TimeRange,
    /// 0..1: how alike the two sides sound, and whether they sit at the
    /// same place in the phrase.
    pub quality: f32,
}

/// How a song fits a length.
#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    /// Source ranges played back to back.
    pub segments: Vec<TimeRange>,
    pub jumps: Vec<Jump>,
    /// Fade at the end, when the ending's tail had to be shortened.
    pub fade_out: Time,
    /// Length of the fitted song.
    pub length: Time,
    /// `length` minus the target: negative when it ends early.
    pub error: Time,
    /// How much of the song's ending was trimmed off.
    pub trimmed: Time,
    /// 0..1, the weakest jump times how close the ending lands.
    pub quality: f32,
}

/// Cost of one jump, before similarity.
const JUMP: f64 = 1.0;
/// Jump targets kept per bar.
const CANDIDATES: usize = 64;

/// Bar-to-bar similarity, 0..1.
pub fn similarity(features: &[BarFeatures]) -> Vec<Vec<f32>> {
    let n = features.len();
    let mfcc_dist = |a: &BarFeatures, b: &BarFeatures| -> f32 {
        let mut d = 0f32;
        for p in 0..PARTS {
            for k in 0..MFCC {
                let x = a.mfcc[p][k] - b.mfcc[p][k];
                d += x * x;
            }
        }
        d.sqrt()
    };
    // Timbre distances are scaled by their median, so songs of any texture
    // score alike.
    let mut dists = Vec::with_capacity(n * n / 2);
    for i in 0..n {
        for j in i + 1..n {
            dists.push(mfcc_dist(&features[i], &features[j]));
        }
    }
    dists.sort_by(f32::total_cmp);
    let scale = dists.get(dists.len() / 2).copied().unwrap_or(1.0).max(1e-3);
    let mut s = vec![vec![0f32; n]; n];
    for i in 0..n {
        for j in i..n {
            let (a, b) = (&features[i], &features[j]);
            let chroma = (0..PARTS)
                .map(|p| {
                    a.chroma[p]
                        .iter()
                        .zip(&b.chroma[p])
                        .map(|(x, y)| x * y)
                        .sum::<f32>()
                })
                .sum::<f32>()
                / PARTS as f32;
            let timbre = (-mfcc_dist(a, b) / scale).exp();
            let loud = (-(a.loudness_db - b.loudness_db).abs() / 6.0).exp();
            let v = 0.5 * chroma.clamp(0.0, 1.0).powi(3) + 0.35 * timbre + 0.15 * loud;
            s[i][j] = v;
            s[j][i] = v;
        }
    }
    s
}

/// The best path through `song` lasting `target`.
pub fn plan(song: &Song, target: Time, options: &FitOptions) -> Result<Fit, String> {
    let n = song.bars.len();
    if n < 8 {
        return Err("The song needs at least 8 bars found by analysis to be fitted.".into());
    }
    let sim = similarity(&song.features);
    let beat = song.beat.as_secs_f64().max(0.1);
    let bar = song.typical_bar();
    let head = song.head().as_secs_f64();
    let tail = song.tail().as_secs_f64();
    // The ending's hit and half a bar of its ring stay; the rest can be
    // trimmed under a fade.
    let min_tail = tail.min((bar / 2.0).max(1.0));
    let target_s = target.as_secs_f64();
    let keep_intro = (n / 6).clamp(1, 4);
    let keep_outro = (n / 6).clamp(1, 4);
    let min_run = options.min_run.max(1);
    let natural = head + (0..n).map(|i| song.bar_secs(i)).sum::<f64>() + tail;
    // The first run holds the intro and lasts at least `min_run` bars.
    let shortest = head + (keep_intro.max(min_run) + keep_outro) as f64 * bar + min_tail;
    if target_s < shortest - beat / 2.0 {
        return Err(format!(
            "{:.1} s is too short to keep the intro and the ending; the shortest fit is about {:.1} s.",
            target_s, shortest
        ));
    }
    if !options.loops && target_s > natural + beat * 0.5 {
        return Err(format!(
            "The song lasts {natural:.1} s; it can only grow longer by looping sections."
        ));
    }

    // Jumps: i → j plays bar j after bar i (j ≠ i + 1).
    let can_leave = |i: usize| {
        i + 1 >= keep_intro
            && i + keep_outro < n
            && song.regular[i]
            && song.regular.get(i + 1).copied().unwrap_or(false)
    };
    let can_land = |j: usize| {
        j >= keep_intro
            && j + keep_outro <= n
            && song.regular[j]
            && song
                .regular
                .get(j.wrapping_sub(1))
                .copied()
                .unwrap_or(false)
    };
    let mut jumps: Vec<Vec<(usize, f64, f32)>> = vec![Vec::new(); n];
    for (i, list) in jumps.iter_mut().enumerate() {
        if !can_leave(i) {
            continue;
        }
        let mut all: Vec<(usize, f64, f32)> = (0..n)
            .filter(|&j| j != i + 1 && can_land(j) && (options.loops || j > i + 1))
            .map(|j| {
                let (cost, quality) = jump_cost(&sim, &song.features, i, j);
                (j, cost, quality)
            })
            .collect();
        all.sort_by(|a, b| a.1.total_cmp(&b.1));
        all.truncate(CANDIDATES);
        *list = all;
    }

    // Lengths of path (in bars) worth trying: the bars must fill between
    // `target - head - tail` and `target - head - min_tail`.
    let lo = target_s - head - tail;
    let hi = target_s - head - min_tail;
    let l_min = ((lo / bar).floor() as i64 - 1).max(1) as usize;
    let l_max = ((hi / bar).ceil() as i64 + 1).max(l_min as i64) as usize;
    if l_max > 4000 {
        return Err("That is too long to fit by repeating sections.".into());
    }

    // dp[l][j][r]: cheapest path of l bars from bar 0 ending on bar j, r
    // bars into its run since the last jump (capped at min_run).
    let r_n = min_run + 1;
    let idx = |j: usize, r: usize| j * r_n + r;
    let width = n * r_n;
    let mut cost = vec![vec![f64::INFINITY; width]; l_max + 1];
    let mut back = vec![vec![u32::MAX; width]; l_max + 1];
    cost[1][idx(0, 1)] = 0.0;
    for l in 1..l_max {
        for i in 0..n {
            for r in 0..r_n {
                let c = cost[l][idx(i, r)];
                if !c.is_finite() {
                    continue;
                }
                let from = idx(i, r) as u32;
                if i + 1 < n {
                    let to = idx(i + 1, (r + 1).min(min_run));
                    if c < cost[l + 1][to] {
                        cost[l + 1][to] = c;
                        back[l + 1][to] = from;
                    }
                }
                // The first run holds the intro too (`can_leave`).
                if r >= min_run {
                    for &(j, jc, _) in &jumps[i] {
                        let to = idx(j, 1);
                        if c + jc < cost[l + 1][to] {
                            cost[l + 1][to] = c + jc;
                            back[l + 1][to] = from;
                        }
                    }
                }
            }
        }
    }

    let mut best: Option<(f64, Vec<usize>)> = None;
    for (l, row) in cost.iter().enumerate().take(l_max + 1).skip(l_min) {
        for r in 0..r_n {
            let c = row[idx(n - 1, r)];
            if !c.is_finite() {
                continue;
            }
            let path = walk(&back, l, idx(n - 1, r), r_n);
            let bars_s: f64 = path.iter().map(|&b| song.bar_secs(b)).sum();
            let total = c + timing_cost(head + bars_s, tail, min_tail, target_s, beat);
            if best.as_ref().is_none_or(|b| total < b.0) {
                best = Some((total, path));
            }
        }
    }
    let Some((_, path)) = best else {
        return Err("No way through the song lasts that long without jumping between bars that sound different.".into());
    };
    Ok(build(song, &path, &sim, target, beat, tail, min_tail))
}

/// Path of bar indices ending at state `end` of length `l`.
fn walk(back: &[Vec<u32>], l: usize, end: usize, r_n: usize) -> Vec<usize> {
    let mut path = Vec::with_capacity(l);
    let mut state = end;
    let mut l = l;
    loop {
        path.push(state / r_n);
        let prev = back[l][state];
        if prev == u32::MAX || l == 1 {
            break;
        }
        state = prev as usize;
        l -= 1;
    }
    path.reverse();
    path
}

/// How a jump from the end of bar `i` into bar `j` sounds: bar `j` should
/// sound like bar `i + 1`, which it stands in for, and bar `j − 1` like bar
/// `i`, whose ring carries over the cut; both at the same place in a
/// 4- or 8-bar phrase, and the cut on a phrase boundary.
fn jump_cost(sim: &[Vec<f32>], f: &[BarFeatures], i: usize, j: usize) -> (f64, f32) {
    let n = sim.len();
    let next = (i + 1 < n).then(|| sim[i + 1][j]);
    let prev = (j >= 1).then(|| sim[i][j - 1]);
    let s = match (next, prev) {
        (Some(a), Some(b)) => 0.6 * a + 0.4 * b,
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => 0.0,
    };
    let d = j as i64 - (i as i64 + 1);
    let (phase, phrase) = if d.rem_euclid(8) == 0 {
        (0.0, 1.0)
    } else if d.rem_euclid(4) == 0 {
        (0.4, 0.9)
    } else if d.rem_euclid(2) == 0 {
        (1.5, 0.6)
    } else {
        (3.0, 0.4)
    };
    let boundary = if (i + 1).is_multiple_of(4) { 0.0 } else { 0.5 };
    let loud = ((f[i].loudness_db - f[j].loudness_db).abs() / 6.0) as f64;
    let cost = JUMP + 5.0 * (1.0 - s as f64) + phase + boundary + loud;
    let quality = s * phrase * if boundary == 0.0 { 1.0 } else { 0.9 };
    (cost, quality.clamp(0.0, 1.0))
}

/// Cost of where a path of `played` seconds (head and bars) leaves the end.
fn timing_cost(played: f64, tail: f64, min_tail: f64, target: f64, beat: f64) -> f64 {
    let keep = target - played;
    if keep > tail {
        // The music stops before the target.
        let gap = (keep - tail) / beat;
        4.0 * gap * gap
    } else if keep < min_tail {
        // Into the ending's hit, or past the target altogether.
        let over = (min_tail - keep.max(0.0)) / beat;
        1.5 * over + if keep <= 0.0 { 10.0 } else { 0.0 }
    } else {
        // Trimming the ring of the ending is cheap, but keeping it is nicer.
        0.05 * (tail - keep) / beat
    }
}

fn build(
    song: &Song,
    path: &[usize],
    sim: &[Vec<f32>],
    target: Time,
    beat: f64,
    tail: f64,
    min_tail: f64,
) -> Fit {
    let mut segments: Vec<TimeRange> = Vec::new();
    let mut jumps = Vec::new();
    let mut start = song.source.start;
    let mut at = Time::ZERO;
    for w in 0..path.len() {
        let i = path[w];
        let last = w + 1 == path.len();
        let jumps_next = !last && path[w + 1] != i + 1;
        if !(last || jumps_next) {
            continue;
        }
        let end = song.bars[i].end;
        let seg = TimeRange::new(start, end);
        at = at + seg.duration();
        segments.push(seg);
        if jumps_next {
            let j = path[w + 1];
            let (_, quality) = jump_cost(sim, &song.features, i, j);
            let to = song.bars[j].start;
            let (kind, span) = if j > i {
                (JumpKind::Skip, TimeRange::new(end, to))
            } else {
                (JumpKind::Loop, TimeRange::new(to, end))
            };
            jumps.push(Jump {
                at,
                from: end,
                to,
                kind,
                span,
                quality,
            });
            start = to;
        }
    }
    // The tail: as much of the ending as reaches the target.
    let played = at;
    let keep = (target - played).as_secs_f64().clamp(0.0, tail);
    let keep = if keep < 0.01 {
        tail.min(min_tail)
    } else {
        keep
    };
    let trimmed = Time::from_secs_f64((tail - keep).max(0.0));
    let trimmed = if trimmed < Time::from_millis(10) {
        Time::ZERO
    } else {
        trimmed
    };
    let last = segments.last_mut().expect("a path has bars");
    last.end = song.source.end - trimmed;
    let length = played + (song.source.end - trimmed - song.bars[*path.last().unwrap()].end);
    let fade_out = if trimmed > Time::ZERO {
        // It may reach back into the bars before the ending.
        Time::from_secs_f64((keep * 0.6).max(beat).clamp(0.25, 1.5))
    } else {
        Time::ZERO
    };
    let error = length - target;
    let timing = (1.0 - (error.as_secs_f64().abs() / beat) as f32).clamp(0.0, 1.0);
    let ending = if keep + 1e-3 >= min_tail { 1.0 } else { 0.8 };
    let weakest = jumps.iter().map(|j| j.quality).fold(1f32, f32::min);
    Fit {
        segments,
        jumps,
        fade_out,
        length,
        error,
        trimmed,
        quality: weakest * timing * ending,
    }
}
