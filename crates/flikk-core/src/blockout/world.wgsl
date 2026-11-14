// A scene at one moment as rays meet it: `world.rs` on the GPU, and kept
// to it. The box tree is the CPU's own (`Solids`), packed by
// `World::pack`; on the GPU it serves the camera's rays and the light's
// alike, where the CPU also lists solids by tile of the picture and by
// cell of the light's view. Each finds what the other does.

// The camera, the light and the ground, as `World::pack` writes them.
struct World {
    // Where the camera is, and how large a pixel is a metre ahead.
    eye: vec4<f32>,
    // The way it looks, and the middle of the picture across.
    ahead: vec4<f32>,
    // A pixel across the picture a metre ahead, and its middle down.
    right: vec4<f32>,
    // A pixel down the picture a metre ahead.
    down: vec4<f32>,
    // The way to the light.
    light: vec4<f32>,
    // Across and up the light's view: where on the sun's disc a ray goes.
    across: vec4<f32>,
    up: vec4<f32>,
    // What the ground's darker and lighter tiles give back of the light.
    floor: vec4<f32>,
    // See `land.wgsl`.
    land: vec4<f32>,
    // The tree's nodes, whether there is a floor, and whether it has hills.
    counts: vec4<u32>,
}

// A box along the scene's axes: a leaf lists its solids (the first, in
// `low.w`, and how many), an inner node has two halves (the next node,
// and the one in `low.w`), split along the axis in `high.w`'s high half.
struct Node {
    low: vec4<f32>,
    high: vec4<f32>,
}

@group(0) @binding(2) var<storage, read> nodes: array<Node>;

// What a thing or a solid is when it is none: the sky, or the ground.
const SKY: u32 = 0u;
const GROUND: u32 = 1u;
const SOLIDS: u32 = 2u;

// What a ray from the camera met: the thing and the solid as `SKY`,
// `GROUND` or counted on from `SOLIDS`.
struct Hit {
    ok: bool,
    distance: f32,
    normal: vec3<f32>,
    tone: f32,
    thing: u32,
    solid: u32,
}

// One over each part of `dir`, without the infinities a part of nothing
// would give.
fn inverse_of(dir: vec3<f32>) -> vec3<f32> {
    let tiny = abs(dir) < vec3<f32>(1e-30);
    return select(1.0 / dir, vec3<f32>(BIG), tiny);
}

// Whether a ray from `origin`, `inverse` one over its direction, passes
// through `node`'s box within `reach`.
fn crosses(node: Node, origin: vec3<f32>, inverse: vec3<f32>, reach: f32) -> bool {
    var enter = 0.0;
    var leave = reach;
    for (var axis = 0u; axis < 3u; axis++) {
        let a = (node.low[axis] - origin[axis]) * inverse[axis];
        let b = (node.high[axis] - origin[axis]) * inverse[axis];
        enter = select(enter, select(b, a, a < b), select(b, a, a < b) > enter);
        leave = select(leave, select(b, a, a > b), select(b, a, a > b) < leave);
    }
    return enter <= leave;
}

fn link_of(node: Node) -> u32 {
    return bitcast<u32>(node.low.w);
}

fn count_of(node: Node) -> u32 {
    return bitcast<u32>(node.high.w) & 0xffffu;
}

fn axis_of(node: Node) -> u32 {
    return bitcast<u32>(node.high.w) >> 16u;
}

// The nearest solid the ray from `origin` along `dir` meets: of two met
// as far off, the first in the tree's order, as the CPU's tiles have it.
// Its solid's place in `solids` is in `n`'s stead in `solid`.
struct Nearest {
    met: Met,
    solid: u32,
}

fn nearest_solid(origin: vec3<f32>, dir: vec3<f32>) -> Nearest {
    var best = Nearest(Met(false, BIG, vec3<f32>(0.0)), 0u);
    if params.world.counts.x == 0u {
        return best;
    }
    let inverse = inverse_of(dir);
    var stack: array<u32, STACK>;
    stack[0] = 0u;
    var depth = 1u;
    while depth > 0u {
        depth -= 1u;
        let index = stack[depth];
        let node = nodes[index];
        if !crosses(node, origin, inverse, best.met.t) {
            continue;
        }
        let count = count_of(node);
        if count == 0u {
            // A tree this deep has more solids than a scene can hold.
            if depth + 2u <= STACK {
                // The half the ray comes from is tried first, so a hit
                // there shortens the reach for the other.
                var near = index + 1u;
                var far = link_of(node);
                if !(dir[axis_of(node)] >= 0.0) {
                    near = far;
                    far = index + 1u;
                }
                stack[depth] = far;
                stack[depth + 1u] = near;
                depth += 2u;
            }
            continue;
        }
        for (var k = 0u; k < count; k++) {
            let solid = link_of(node) + k;
            let m = hit(solid, origin, dir);
            let nearer_one = m.t < best.met.t || (best.met.ok && m.t == best.met.t && solid < best.solid);
            if m.ok && nearer_one {
                best = Nearest(m, solid);
            }
        }
    }
    return best;
}

// Whether any solid stands in the way of a ray from `origin` along `dir`.
fn blocked(origin: vec3<f32>, dir: vec3<f32>) -> bool {
    if params.world.counts.x == 0u {
        return false;
    }
    let inverse = inverse_of(dir);
    var stack: array<u32, STACK>;
    stack[0] = 0u;
    var depth = 1u;
    while depth > 0u {
        depth -= 1u;
        let index = stack[depth];
        let node = nodes[index];
        if !crosses(node, origin, inverse, BIG) {
            continue;
        }
        let count = count_of(node);
        if count == 0u {
            if depth + 2u <= STACK {
                stack[depth] = link_of(node);
                stack[depth + 1u] = index + 1u;
                depth += 2u;
            }
            continue;
        }
        for (var k = 0u; k < count; k++) {
            if hit(link_of(node) + k, origin, dir).ok {
                return true;
            }
        }
    }
    return false;
}

// Whether the middle of the sun shows from `at`.
fn sunlit(at: vec3<f32>) -> bool {
    return !blocked(at, params.world.light.xyz);
}

// Whether a spot on the sun shows from `at`: `spot` is across and up its
// disc from the middle, its edge one unit out.
fn sunlit_from(at: vec3<f32>, spot: vec2<f32>) -> bool {
    let light = params.world.light.xyz;
    let off = params.world.across.xyz * spot.x + params.world.up.xyz * spot.y;
    return !blocked(at, unit_or(light + off * SUN_WIDTH, light));
}

// Whether a leaf of the tree is within `reach` of `point`.
fn crowded(point: vec3<f32>, reach: f32) -> bool {
    if params.world.counts.x == 0u {
        return false;
    }
    var stack: array<u32, STACK>;
    stack[0] = 0u;
    var depth = 1u;
    while depth > 0u {
        depth -= 1u;
        let index = stack[depth];
        let node = nodes[index];
        let outside = max(max(node.low.xyz - point, point - node.high.xyz), vec3<f32>(0.0));
        if dot(outside, outside) > reach * reach {
            continue;
        }
        if count_of(node) > 0u {
            return true;
        }
        if depth + 2u <= STACK {
            stack[depth] = link_of(node);
            stack[depth + 1u] = index + 1u;
            depth += 2u;
        }
    }
    return false;
}

fn to_middle(x: f32, y: f32) -> vec3<f32> {
    let w = params.world;
    return w.ahead.xyz + w.right.xyz * (x - w.ahead.w) + w.down.xyz * (y - w.right.w);
}

// The way the ray through pixel (`x`, `y`) goes.
fn ray(x: f32, y: f32) -> vec3<f32> {
    return unit_or(to_middle(x, y), params.world.ahead.xyz);
}

// The point seen through pixel (`x`, `y`), `depth` ahead of the camera.
fn point_at(x: f32, y: f32, depth: f32) -> vec3<f32> {
    return params.world.eye.xyz + to_middle(x, y) * depth;
}

// What the camera sees along `dir`: a solid, else the ground.
fn seen_along(dir: vec3<f32>) -> Hit {
    let eye = params.world.eye.xyz;
    let first = nearest_solid(eye, dir);
    let nearest = select(BIG, first.met.t, first.met.ok);
    var floor_met = missed();
    if params.world.counts.z != 0u {
        floor_met = land_met(eye, dir, nearest);
    } else if params.world.counts.y != 0u && dir.y < -1e-6 && eye.y > 0.0 {
        floor_met = Met(true, -eye.y / dir.y, UP);
    }
    if floor_met.ok && floor_met.t < nearest {
        let distance = floor_met.t;
        let at = eye + dir * distance;
        var normal = UP;
        if params.world.counts.z != 0u {
            normal = land_normal(at.x, at.z);
        }
        // Metre tiles a touch lighter and darker, so a camera move reads
        // against the ground, fading out before they shimmer.
        let odd = (i32(floor(at.x)) + i32(floor(at.z))) & 1;
        let tones = params.world.floor;
        let even_tone = (tones.x + tones.y) * 0.5;
        let fade = exp(-distance / TILE_FADE);
        let tone = even_tone + (select(tones.x, tones.y, odd == 1) - even_tone) * fade;
        return Hit(true, distance, normal, tone, GROUND, GROUND);
    }
    if first.met.ok {
        let s = solids[first.solid];
        return Hit(true, first.met.t, first.met.n, s.e.z, SOLIDS + s.f.y, SOLIDS + first.solid);
    }
    return Hit(false, 0.0, vec3<f32>(0.0), 0.0, SKY, SKY);
}
