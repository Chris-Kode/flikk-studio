// Ground that rises and falls: `land.rs` on the GPU, and kept to it. Its
// measures are `params.world.land` (metres between heights, the highest, the
// steepest rise over a metre), its heights in `tables` from HEIGHTS_AT.

fn land_cell() -> f32 {
    return params.world.land.x;
}

// The height at grid place (`col`, `row`), counted round its edges.
fn land_at(col: i32, row: i32) -> f32 {
    // The side is a power of two.
    let wrap = LAND_SIDE - 1;
    return tables[HEIGHTS_AT + u32((row & wrap) * LAND_SIDE + (col & wrap))];
}

// How high the ground is at (`x`, `z`).
fn land_height(x: f32, z: f32) -> f32 {
    let u = x / land_cell();
    let v = z / land_cell();
    let col = floor(u);
    let row = floor(v);
    let across = u - col;
    let down = v - row;
    let c = i32(col);
    let r = i32(row);
    let near = land_at(c, r) + (land_at(c + 1, r) - land_at(c, r)) * across;
    let far = land_at(c, r + 1) + (land_at(c + 1, r + 1) - land_at(c, r + 1)) * across;
    return near + (far - near) * down;
}

// The way the ground faces at (`x`, `z`).
fn land_normal(x: f32, z: f32) -> vec3<f32> {
    let by = land_cell();
    let rise_x = land_height(x + by, z) - land_height(x - by, z);
    let rise_z = land_height(x, z + by) - land_height(x, z - by);
    return unit_or(vec3<f32>(-rise_x, 2.0 * by, -rise_z), UP);
}

fn land_over(origin: vec3<f32>, dir: vec3<f32>, along: f32) -> f32 {
    let at = origin + dir * along;
    return at.y - land_height(at.x, at.z);
}

// How far along the ray from `origin` along `dir` the ground is first
// met, within `reach`.
fn land_met(origin: vec3<f32>, dir: vec3<f32>, reach_in: f32) -> Met {
    let highest = params.world.land.y;
    let steepest = params.world.land.z;
    // Down to its highest point nothing is in the way.
    var along = 0.0;
    if origin.y > highest {
        if dir.y >= 0.0 {
            return missed();
        }
        along = (highest - origin.y) / dir.y;
    }
    let reach = min(reach_in, FAR);
    // The gap to the ground closes no faster than this: its steepest
    // slope against the ray's own fall.
    let closing = steepest * sqrt(dir.x * dir.x + dir.z * dir.z) - dir.y;
    var gap = land_over(origin, dir, along);
    if gap < 0.0 {
        return missed();
    }
    for (var i = 0u; i < LAND_STEPS; i++) {
        if gap < 0.002 + 0.001 * along {
            return Met(true, along, UP);
        }
        if closing <= 1e-6 || along > reach {
            return missed();
        }
        let sure = max(gap / closing, 0.01);
        let next = along + sure * HASTE;
        let gap_next = land_over(origin, dir, next);
        if gap_next < 0.0 {
            // Past it: it is met between here and there, about where a
            // straight line between the two gaps says.
            var clear = vec2<f32>(along, gap);
            var under = vec2<f32>(next, gap_next);
            var met = next;
            for (var k = 0u; k < 4u; k++) {
                met = clear.x + (under.x - clear.x) * clear.y / (clear.y - under.y);
                let g = land_over(origin, dir, met);
                if g < 0.0 {
                    under = vec2<f32>(met, g);
                } else {
                    clear = vec2<f32>(met, g);
                }
            }
            if met <= reach {
                return Met(true, met, UP);
            }
            return missed();
        }
        along = next;
        gap = gap_next;
        if origin.y + dir.y * along > highest && dir.y >= 0.0 {
            return missed();
        }
    }
    // Still skimming it far off: as good as met.
    if along <= reach {
        return Met(true, along, UP);
    }
    return missed();
}
