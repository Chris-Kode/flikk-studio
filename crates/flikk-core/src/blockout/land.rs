//! Ground that rises and falls: hills, dunes, a rolling field. It is
//! worked out once as heights on a grid, which rays walk over and things
//! are stood on, and which repeats too far off to be seen doing it.

use super::gpu::Words;
use super::motion::mixed;
use super::space::Vec3;
use serde::{Deserialize, Serialize};

/// Heights along a side of the grid, and how many of them span the
/// ground from one hilltop to the next.
const SIDE: usize = 256;
const FINE: f64 = 12.0;
/// The most steps a ray takes over the ground before it is let go, how
/// much further each goes than is sure to be clear (what it passes is
/// then looked for behind it), and how far off the ground is no longer
/// looked for: the haze has it by then.
const STEPS: usize = 64;
const HASTE: f32 = 4.0;
const FAR: f32 = 700.0;

/// Ground that rises and falls. Every height in the scene is then above
/// the ground under that thing: `y = 0` still stands it on the ground.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hills {
    /// Metres from its lowest to its highest.
    pub height: f64,
    /// About how many metres it is from one top to the next: 8 for
    /// dunes, 40 for rolling hills.
    #[serde(default = "rolling", skip_serializing_if = "is_rolling")]
    pub across: f64,
    /// Another number shapes it another way.
    #[serde(default, skip_serializing_if = "is_first")]
    pub seed: u32,
}

fn rolling() -> f64 {
    30.0
}

fn is_rolling(across: &f64) -> bool {
    *across == rolling()
}

fn is_first(seed: &u32) -> bool {
    *seed == 0
}

impl Hills {
    /// Bring its numbers into range.
    pub(super) fn settle(&mut self) -> Result<(), String> {
        super::finite("`hills`", &[self.height, self.across])?;
        self.height = self.height.clamp(0.0, 200.0);
        self.across = self.across.clamp(1.0, 1000.0);
        Ok(())
    }
}

/// The heights of a scene's ground, on a grid that repeats.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Land {
    /// Metres between heights.
    cell: f32,
    heights: Vec<f32>,
    /// Its highest point, and the steepest it rises over a metre.
    highest: f32,
    steepest: f32,
}

impl Land {
    pub fn of(hills: &Hills) -> Self {
        let cell = (hills.across / FINE).clamp(0.25, 8.0);
        // A whole number of hills fits the grid, so its far side meets
        // its near one.
        let hills_along = ((SIDE as f64 * cell / hills.across).round() as u32).max(2);
        let per_hill = SIDE as f64 / hills_along as f64;
        let mut heights = Vec::with_capacity(SIDE * SIDE);
        for row in 0..SIDE {
            for col in 0..SIDE {
                let (x, z) = (col as f64 / per_hill, row as f64 / per_hill);
                heights.push((rolling_noise(x, z, hills_along, hills.seed) * hills.height) as f32);
            }
        }
        let at = |col: usize, row: usize| heights[(row % SIDE) * SIDE + col % SIDE];
        let (mut along, mut across) = (0f32, 0f32);
        for row in 0..SIDE {
            for col in 0..SIDE {
                along = along.max((at(col + 1, row) - at(col, row)).abs());
                across = across.max((at(col, row + 1) - at(col, row)).abs());
            }
        }
        Self {
            cell: cell as f32,
            highest: heights.iter().copied().fold(0.0, f32::max),
            steepest: along.hypot(across) / cell as f32,
            heights,
        }
    }

    /// Write its measures for the GPU as `land.wgsl` reads a `Land`.
    pub fn pack(&self, words: &mut Words) {
        words.floats([self.cell, self.highest, self.steepest, 0.0]);
    }

    /// Write its heights for the GPU, as `land.wgsl` reads them.
    pub fn pack_heights(&self, words: &mut Words) {
        self.heights.iter().for_each(|height| words.float(*height));
    }

    /// The height at grid place (`col`, `row`), counted round its edges.
    fn at(&self, col: i64, row: i64) -> f32 {
        // The side is a power of two.
        let wrap = |n: i64| (n & (SIDE as i64 - 1)) as usize;
        self.heights[wrap(row) * SIDE + wrap(col)]
    }

    /// How high the ground is at (`x`, `z`).
    pub fn height(&self, x: f32, z: f32) -> f32 {
        let (u, v) = (x / self.cell, z / self.cell);
        let (col, row) = (u.floor(), v.floor());
        let (across, down) = (u - col, v - row);
        let (col, row) = (col as i64, row as i64);
        let near = self.at(col, row) + (self.at(col + 1, row) - self.at(col, row)) * across;
        let far =
            self.at(col, row + 1) + (self.at(col + 1, row + 1) - self.at(col, row + 1)) * across;
        near + (far - near) * down
    }

    /// The way the ground faces at (`x`, `z`).
    pub fn normal(&self, x: f32, z: f32) -> Vec3 {
        let step = self.cell;
        let rise_x = self.height(x + step, z) - self.height(x - step, z);
        let rise_z = self.height(x, z + step) - self.height(x, z - step);
        Vec3::new(-rise_x, 2.0 * step, -rise_z).unit_or(Vec3::UP)
    }

    /// How far along the ray from `origin` along `dir` (one unit long)
    /// the ground is first met, within `reach`; `None` from under it.
    pub fn met(&self, origin: Vec3, dir: Vec3, reach: f32) -> Option<f32> {
        // Down to its highest point nothing is in the way.
        let mut along = match origin.y > self.highest {
            true if dir.y >= 0.0 => return None,
            true => (self.highest - origin.y) / dir.y,
            false => 0.0,
        };
        let reach = reach.min(FAR);
        let over = |along: f32| {
            let at = origin + dir * along;
            at.y - self.height(at.x, at.z)
        };
        // The gap to the ground closes no faster than this: its
        // steepest slope against the ray's own fall.
        let closing = self.steepest * dir.x.hypot(dir.z) - dir.y;
        let mut gap = over(along);
        if gap < 0.0 {
            return None;
        }
        for _ in 0..STEPS {
            if gap < 0.002 + 0.001 * along {
                return Some(along);
            }
            if closing <= 1e-6 || along > reach {
                return None;
            }
            // As far as is sure to be clear, and on by as much as the
            // gap has been closing, which over smooth ground is most of
            // the way there.
            let sure = (gap / closing).max(0.01);
            let next = along + sure * HASTE;
            let gap_next = over(next);
            if gap_next < 0.0 {
                // Past it: it is met between here and there, about
                // where a straight line between the two gaps says.
                let (mut clear, mut under) = ((along, gap), (next, gap_next));
                let mut met = next;
                for _ in 0..4 {
                    met = clear.0 + (under.0 - clear.0) * clear.1 / (clear.1 - under.1);
                    let gap = over(met);
                    match gap < 0.0 {
                        true => under = (met, gap),
                        false => clear = (met, gap),
                    }
                }
                return Some(met).filter(|met| *met <= reach);
            }
            (along, gap) = (next, gap_next);
            if origin.y + dir.y * along > self.highest && dir.y >= 0.0 {
                return None;
            }
        }
        // Still skimming it far off: as good as met.
        Some(along).filter(|along| *along <= reach)
    }
}

/// The GPU's side of this module: heights as [`Land::pack_heights`] writes
/// them from `heights_at` in the tables, walked over as [`Land::met`].
pub(super) fn shader(heights_at: usize) -> String {
    format!(
        "const LAND_SIDE: i32 = {SIDE};\nconst LAND_STEPS: u32 = {STEPS}u;\n\
         const HASTE: f32 = {HASTE:?};\nconst FAR: f32 = {FAR:?};\n\
         const HEIGHTS_AT: u32 = {heights_at}u;\n{}",
        include_str!("land.wgsl")
    )
}

/// A height from 0 to 1 at (`x`, `z`), counted in hills, rolling smoothly
/// and the same again every `period` hills either way.
fn rolling_noise(x: f64, z: f64, period: u32, seed: u32) -> f64 {
    let mut height = 0.0;
    let mut strength = 1.0;
    for octave in 0..3u32 {
        let scale = (1u32 << octave) as f64;
        let period = (period << octave) as i64;
        let seed = mixed(seed.wrapping_add(octave.wrapping_mul(0x9e37_79b9)));
        let corner = |col: i64, row: i64| {
            let (col, row) = (col.rem_euclid(period) as u32, row.rem_euclid(period) as u32);
            mixed(seed ^ mixed(col.wrapping_mul(0x85eb_ca6b) ^ mixed(row))) as f64 / u32::MAX as f64
        };
        let (u, v) = (x * scale, z * scale);
        let (col, row) = (u.floor(), v.floor());
        let eased =
            |within: f64| within * within * within * (within * (within * 6.0 - 15.0) + 10.0);
        let (across, down) = (eased(u - col), eased(v - row));
        let (col, row) = (col as i64, row as i64);
        let near = corner(col, row) + (corner(col + 1, row) - corner(col, row)) * across;
        let far = corner(col, row + 1) + (corner(col + 1, row + 1) - corner(col, row + 1)) * across;
        height += strength * (near + (far - near) * down);
        strength *= 0.5;
    }
    height / 1.75
}
