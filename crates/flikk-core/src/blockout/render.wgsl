// Drawing a scene: `render.rs` on the GPU, and kept to it. Each entry
// point is one of the CPU's stages over every pixel, written to the sheets
// below for the next: `look_all` the rays from the camera; for a full
// picture `sun_all` the sun's edge, `open_all` how open each surface is
// (at half the size) and `finish_all` the pixels in full; then one of
// `draft_out`, `full_out` or `pass_out` the greys a screen shows. The
// constants come from `render.rs`.

// How the picture is drawn, as `Picture::pack` writes it.
struct Look {
    // The way the fill comes from, and how much of their darkness lines
    // are drawn with.
    fill: vec4<f32>,
    // The picture's right and its top as directions of the scene, and the
    // sky's light at the horizon and overhead.
    right: vec4<f32>,
    up: vec4<f32>,
    // The distance at which a pass's depth is half way from light to dark.
    near: vec4<f32>,
    // Its width and height, the width of the sheet of how open each
    // surface is, and what is drawn (`DRAFT`, `FULL`, `DEPTH`, `LINES`,
    // `THINGS`).
    size: vec4<u32>,
}

struct Params {
    world: World,
    look: Look,
}

// What one ray from the camera found: as `Seen` in `render.rs`.
struct Seen {
    // The light it gives whatever the sun does, what the sun adds, the
    // light of the haze before it, and how far ahead it is (BIG for the
    // sky).
    light: vec4<f32>,
    // The way its surface faces, in 32767ths.
    faces: vec4<i32>,
    // Whether the sun reaches it, the thing it is of, the solid it is.
    ids: vec4<u32>,
}

// One pixel as it is drawn: as `Drawn` in `render.rs`.
struct Drawn {
    // Its light, the light of what is nearest the camera in it alone, and
    // how far ahead that is.
    light: vec4<f32>,
    // The solid nearest the camera in it, its thing, and how many quarters
    // of the pixel it fills.
    front: vec4<u32>,
}

@group(0) @binding(0) var<uniform> params: Params;
// The screen's greys from GREYS_AT, the spiral from SPIRAL_AT, and the
// ground's heights (see `land.wgsl`).
@group(0) @binding(3) var<storage, read> tables: array<f32>;
@group(0) @binding(4) var<storage, read_write> seens: array<Seen>;
// How much of the sun reaches each pixel, then how open each surface is
// at every second pixel either way.
@group(0) @binding(5) var<storage, read_write> shades: array<f32>;
@group(0) @binding(6) var<storage, read_write> drawn: array<Drawn>;
// The picture's pixels, RGBA, then the depth at each, as bits.
@group(0) @binding(7) var<storage, read_write> picture: array<u32>;

const TAU: f32 = 6.28318530717958647692;
const BESIDE = array<vec2<i32>, 4>(vec2<i32>(-1, 0), vec2<i32>(1, 0), vec2<i32>(0, -1), vec2<i32>(0, 1));
const AROUND = array<vec2<i32>, 8>(
    vec2<i32>(-2, -2), vec2<i32>(0, -2), vec2<i32>(2, -2), vec2<i32>(-2, 0),
    vec2<i32>(2, 0), vec2<i32>(-2, 2), vec2<i32>(0, 2), vec2<i32>(2, 2),
);

fn width() -> u32 {
    return params.look.size.x;
}

fn height() -> u32 {
    return params.look.size.y;
}

fn pixels() -> u32 {
    return width() * height();
}

// Pixel (`x`, `y`)'s place on a sheet; -1 off the picture.
fn pixel_index(x: i32, y: i32) -> i32 {
    if x >= 0 && y >= 0 && x < i32(width()) && y < i32(height()) {
        return y * i32(width()) + x;
    }
    return -1;
}

fn faint() -> f32 {
    return params.look.fill.w;
}

fn horizon() -> f32 {
    return params.look.right.w;
}

// A number from 0 to 1 for a pixel, unlike its neighbours' and evenly
// spread among them (Jimenez's interleaved gradient noise).
fn scatter(x: u32, y: u32) -> f32 {
    return fract(SCATTER.x * fract(SCATTER.y * f32(x) + SCATTER.z * f32(y)));
}

// Spot `spot` of SUNS on the sun's disc, spread over it evenly and turned
// its own way for pixel (`x`, `y`).
fn sun_spot(spot: u32, x: u32, y: u32) -> vec2<f32> {
    let out = sqrt((f32(spot) + 0.5) / f32(SUNS));
    let angle = scatter(x, y) * TAU + f32(spot) * 2.4;
    return vec2<f32>(out * cos(angle), out * sin(angle));
}

fn seen_light(seen: Seen, open: f32, sun: f32) -> f32 {
    return seen.light.z + seen.light.x * open + seen.light.y * sun * (0.5 + 0.5 * open);
}

fn seen_normal(seen: Seen) -> vec3<f32> {
    return vec3<f32>(seen.faces.xyz) / 32767.0;
}

fn sun_of(seen: Seen) -> f32 {
    return f32(seen.ids.x);
}

// Whether `b` is of the same solid as `a`, facing much the same way.
fn same_surface(a: Seen, b: Seen) -> bool {
    return a.ids.z == b.ids.z && (a.ids.z == SKY || dot(a.faces.xyz, b.faces.xyz) > ALIKE);
}

// `seen` drawn as `light` where no edge runs through its pixel.
fn plain(seen: Seen, light: f32) -> Drawn {
    return Drawn(vec4<f32>(light, light, seen.light.w, 0.0), vec4<u32>(seen.ids.z, seen.ids.y, 4u, 0u));
}

// What the ray through (`x`, `y`) of the picture finds. The sun is looked
// for at `spot` on its disc when `on_spot`, or at its middle.
fn look(x: f32, y: f32, on_spot: bool, spot: vec2<f32>) -> Seen {
    let dir = ray(x, y);
    let hit = seen_along(dir);
    if !hit.ok {
        // Lighter overhead, and darker under the horizon where there is
        // no ground.
        let over = (params.look.up.w - horizon()) * sqrt(max(dir.y, 0.0));
        let under = 0.3 * horizon() * sqrt(max(-dir.y, 0.0));
        return Seen(vec4<f32>(0.0, 0.0, horizon() + over - under, BIG), vec4<i32>(0), vec4<u32>(0u, SKY, SKY, 0u));
    }
    let at_point = params.world.eye.xyz + dir * hit.distance;
    // Seen from behind, a surface is lit as its other side.
    var normal = hit.normal;
    if dot(normal, dir) > 0.0 {
        normal = -normal;
    }
    let off = at_point + normal * (NEAR * 4.0);
    let facing = max(dot(normal, params.world.light.xyz), 0.0);
    var sun = false;
    if facing > 0.0 {
        if on_spot {
            sun = sunlit_from(off, spot);
        } else {
            sun = sunlit(off);
        }
    }
    // The fill reaches round a surface that turns away from it.
    let filled = clamp((dot(normal, params.look.fill.xyz) + 0.5) / 1.5, 0.0, 1.0);
    let around = SKY_LIGHT * (0.5 + 0.5 * normal.y) + GROUND_LIGHT * (0.5 - 0.5 * normal.y) + FILL_LIGHT * filled * filled;
    // A little darker where it turns away from the camera; then the haze.
    let rim = 0.86 + 0.14 * abs(dot(normal, dir));
    let clear = exp(-hit.distance / HAZE);
    let tone = hit.tone * rim * clear;
    return Seen(
        vec4<f32>(tone * around, tone * SUN_LIGHT * facing, horizon() * (1.0 - clear), hit.distance * dot(dir, params.world.ahead.xyz)),
        vec4<i32>(vec3<i32>(normal * 32767.0), 0),
        vec4<u32>(select(0u, 1u, sun), hit.thing, hit.solid, 0u),
    );
}

// Whether the sun says otherwise around pixel (`x`, `y`) on its surface,
// each second pixel out: its one ray toward it, or with `by_shades` how
// much of it reaches.
fn differs(by_shades: bool, x: u32, y: u32) -> bool {
    let here = pixel_index(i32(x), i32(y));
    let seen = seens[here];
    let mine = select(sun_of(seen), shades[here], by_shades);
    var around = AROUND;
    for (var k = 0u; k < 8u; k++) {
        let there = pixel_index(i32(x) + around[k].x, i32(y) + around[k].y);
        if there < 0 {
            continue;
        }
        let other = seens[there];
        let theirs = select(sun_of(other), shades[there], by_shades);
        if abs(theirs - mine) > 0.01 && same_surface(other, seen) {
            return true;
        }
    }
    return false;
}

// How much of the sun reaches pixel (`x`, `y`). Where the one ray toward
// it says otherwise than the pixels around, its edge shows from there,
// and more rays say how much of it.
fn sun_at(x: u32, y: u32) -> f32 {
    let seen = seens[pixel_index(i32(x), i32(y))];
    let first = sun_of(seen);
    if seen.light.y <= 0.0 || !differs(false, x, y) {
        return first;
    }
    let from_point = point_at(f32(x) + 0.5, f32(y) + 0.5, seen.light.w) + seen_normal(seen) * (NEAR * 4.0);
    let taken = u32(scatter(y, x) * f32(SUNS));
    var more = 0.0;
    for (var spot = 0u; spot < SUNS; spot++) {
        if spot != taken && sunlit_from(from_point, sun_spot(spot, x, y)) {
            more += 1.0;
        }
    }
    return (first + more) / f32(SUNS);
}

// How open to the sky the surface at pixel (`x`, `y`) is: 1 in the open,
// less where the picture shows another surface close before it.
fn open_at(x: u32, y: u32) -> f32 {
    let seen = seens[pixel_index(i32(x), i32(y))];
    if seen.ids.y == SKY {
        return 1.0;
    }
    let depth = seen.light.w;
    let reach = min(CLOSE / (depth * params.world.eye.w), CLOSE_PIXELS);
    let px = f32(x) + 0.5;
    let py = f32(y) + 0.5;
    let from_point = point_at(px, py, depth);
    if reach < 1.5 || (seen.ids.y == GROUND && !crowded(from_point, CLOSE)) {
        return 1.0;
    }
    let normal = seen_normal(seen);
    // Out along a spiral, turned its own way at each pixel.
    let angle = scatter(x, y) * TAU;
    let sine = sin(angle);
    let cosine = cos(angle);
    var closed = 0.0;
    for (var k = 0u; k < LOOKS; k++) {
        let out = vec2<f32>(tables[SPIRAL_AT + 2u * k], tables[SPIRAL_AT + 2u * k + 1u]);
        let across = out.x * cosine - out.y * sine;
        let down = out.x * sine + out.y * cosine;
        let qx = floor(px + reach * across);
        let qy = floor(py + reach * down);
        if qx < 0.0 || qy < 0.0 || qx >= f32(width()) || qy >= f32(height()) {
            continue;
        }
        let other = seens[pixel_index(i32(qx), i32(qy))];
        if other.ids.y == SKY {
            continue;
        }
        let to = point_at(qx + 0.5, qy + 0.5, other.light.w) - from_point;
        let apart = dot(to, to);
        let over = dot(to, normal) - 0.01 - 0.002 * depth;
        if over > 0.0 && apart < CLOSE * CLOSE {
            closed += over / sqrt(apart) * (1.0 - apart / (CLOSE * CLOSE));
        }
    }
    return 1.0 - CLOSE_DARK * min(closed / f32(LOOKS) * 3.0, 1.0);
}

// The light of the line beside a solid of light `front`, drawn over what
// has light `under`: darker than both. `round` a thing it is darker than
// between the parts of one.
fn line_light(under: f32, front: f32, round_thing: bool) -> f32 {
    let dark = select(INLINE, OUTLINE, round_thing);
    return min(under, front) * (1.0 - dark * faint());
}

// Pixel (`x`, `y`) in full: as open as the pixels around say, under as
// much of the sun as they say, and drawn again from four rays where an
// edge runs through it.
fn finished(x: u32, y: u32) -> Drawn {
    let ix = i32(x);
    let iy = i32(y);
    let here = pixel_index(ix, iy);
    let seen = seens[here];
    var beside = BESIDE;
    var edge = false;
    for (var k = 0u; k < 4u; k++) {
        let there = pixel_index(ix + beside[k].x, iy + beside[k].y);
        if there >= 0 && !same_surface(seens[there], seen) {
            edge = true;
        }
    }
    if seen.ids.y == SKY && !edge {
        return plain(seen, seen.light.z);
    }
    // How open, from those around that were asked on this surface.
    let half = params.look.size.z;
    var sum = 0.0;
    var count = 0u;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let cx = ix / 2 + dx;
            let cy = iy / 2 + dy;
            let there = pixel_index(cx * 2, cy * 2);
            if there >= 0 && same_surface(seens[there], seen) {
                sum += shades[pixels() + u32(cy) * half + u32(cx)];
                count += 1u;
            }
        }
    }
    var open = sum / f32(max(count, 1u));
    if seen.ids.y == SKY {
        open = 1.0;
    } else if count == 0u {
        open = open_at(x, y);
    }
    // The sun's edge is evened out among those around.
    var sun = shades[here];
    if seen.light.y > 0.0 && differs(true, x, y) {
        var sum_sun = 0.0;
        var count_sun = 0u;
        for (var dy = -2; dy <= 2; dy++) {
            for (var dx = -2; dx <= 2; dx++) {
                let there = pixel_index(ix + dx, iy + dy);
                if there >= 0 && same_surface(seens[there], seen) {
                    sum_sun += shades[there];
                    count_sun += 1u;
                }
            }
        }
        sun = sum_sun / f32(max(count_sun, 1u));
    }
    if !edge {
        return plain(seen, seen_light(seen, open, sun));
    }
    var offsets = SPOTS;
    var spots: array<Seen, 4>;
    var lights: array<f32, 4>;
    for (var k = 0u; k < 4u; k++) {
        spots[k] = look(f32(x) + offsets[k].x, f32(y) + offsets[k].y, false, vec2<f32>(0.0));
        if spots[k].ids.z == seen.ids.z {
            lights[k] = seen_light(spots[k], open, sun);
        } else {
            lights[k] = seen_light(spots[k], open, sun_of(spots[k]));
        }
    }
    // The solid nearest the camera fills some of the pixel; the line round
    // it runs under the rest.
    var front = -1;
    for (var k = 0; k < 4; k++) {
        if spots[k].ids.y >= SOLIDS && (front < 0 || spots[k].light.w < spots[front].light.w) {
            front = k;
        }
    }
    if front < 0 {
        return plain(seen, (lights[0] + lights[1] + lights[2] + lights[3]) / 4.0);
    }
    let front_solid = spots[front].ids.z;
    let front_thing = spots[front].ids.y;
    var fills = 0u;
    var front_sum = 0.0;
    for (var k = 0u; k < 4u; k++) {
        if spots[k].ids.z == front_solid {
            fills += 1u;
            front_sum += lights[k];
        }
    }
    let front_light = front_sum / f32(fills);
    var light = 0.0;
    for (var k = 0u; k < 4u; k++) {
        if spots[k].ids.z == front_solid {
            light += lights[k];
        } else {
            light += line_light(lights[k], front_light, spots[k].ids.y != front_thing);
        }
    }
    return Drawn(
        vec4<f32>(light / 4.0, front_light, spots[front].light.w, 0.0),
        vec4<u32>(front_solid, front_thing, fills, 0u),
    );
}

// What is nearest the camera in pixel `there`, and its light: from the
// finished pixels, or from the one ray of each.
fn front_at(finished_pixels: bool, there: u32) -> Drawn {
    if finished_pixels {
        return drawn[there];
    }
    let seen = seens[there];
    return plain(seen, seen_light(seen, 1.0, sun_of(seen)));
}

// The light of pixel (`x`, `y`), drawn as `here`, with the lines that
// reach it: a solid nearer the camera in a pixel beside it has its line
// here, a pixel wide in all, darker round a thing than between its parts.
// And a solid's own edge is a little lighter where it sticks out, darker
// where it goes in.
fn lined(here: Drawn, finished_pixels: bool, x: u32, y: u32) -> f32 {
    let ix = i32(x);
    let iy = i32(y);
    let own_solid = here.front.x;
    var light = here.light.x;
    var beside = BESIDE;
    for (var k = 0u; k < 4u; k++) {
        let there = pixel_index(ix + beside[k].x, iy + beside[k].y);
        if there < 0 {
            continue;
        }
        let other = front_at(finished_pixels, u32(there));
        let before = other.front.x >= SOLIDS && other.front.x != own_solid
            && (own_solid < SOLIDS || other.light.z < here.light.z);
        // As much of the line as the solid fills of its own pixel: the
        // rest of the line is drawn there.
        if before {
            let line = line_light(here.light.x, other.light.y, other.front.y != here.front.y);
            light = min(light, here.light.x + (line - here.light.x) * f32(other.front.z) * 0.25);
        }
    }
    // How the surface bends, from the way it faces either side.
    let seen = seens[pixel_index(ix, iy)];
    var bend = 0.0;
    if seen.ids.z >= SOLIDS {
        var normals: array<vec3<f32>, 4>;
        var all = true;
        for (var k = 0u; k < 4u; k++) {
            let there = pixel_index(ix + beside[k].x, iy + beside[k].y);
            if there < 0 || seens[there].ids.z != seen.ids.z {
                all = false;
                break;
            }
            normals[k] = seen_normal(seens[there]);
        }
        if all {
            bend = dot(normals[1] - normals[0], params.look.right.xyz) + dot(normals[2] - normals[3], params.look.up.xyz);
        }
    }
    let bent = clamp(bend * 0.3, -VALLEY, RIDGE);
    return light * (1.0 + bent);
}

fn grey(value: u32) -> u32 {
    return value | (value << 8u) | (value << 16u) | (255u << 24u);
}

// `light` as the grey a screen shows at pixel (`x`, `y`). Between two of
// the screen's greys, some pixels take one and some the other, so a slow
// gradient shows no steps.
fn shown(x: u32, y: u32, light: f32) -> u32 {
    let along = clamp(light, 0.0, 1.0) * f32(GREYS - 1u);
    let k = u32(along);
    let from_grey = tables[GREYS_AT + k];
    let to_grey = tables[GREYS_AT + min(k + 1u, GREYS - 1u)];
    let value = from_grey + (to_grey - from_grey) * fract(along);
    return grey(u32(clamp(value * 255.0 + scatter(x, y), 0.0, 255.0)));
}

// A colour for thing `thing`, unlike those of the things counted next to
// it.
fn own_colour(thing: u32) -> u32 {
    // A step of the golden ratio round the colours for each.
    let hue = fract(f32(thing) * 0.618034) * 6.0;
    let rising = f32(u32(fract(hue) * 255.0));
    let full = 240.0;
    let low = 70.0;
    let up = u32(low + (full - low) * rising / 255.0);
    let down = u32(full + (low - full) * rising / 255.0);
    var rgb: vec3<u32>;
    switch u32(hue) {
        case 0u: { rgb = vec3<u32>(240u, up, 70u); }
        case 1u: { rgb = vec3<u32>(down, 240u, 70u); }
        case 2u: { rgb = vec3<u32>(70u, 240u, up); }
        case 3u: { rgb = vec3<u32>(70u, down, 240u); }
        case 4u: { rgb = vec3<u32>(up, 70u, 240u); }
        default: { rgb = vec3<u32>(240u, 70u, down); }
    }
    return rgb.x | (rgb.y << 8u) | (rgb.z << 16u) | (255u << 24u);
}

fn depth_bits(here: u32) -> u32 {
    return bitcast<u32>(seens[here].light.w);
}

@compute @workgroup_size(8, 8)
fn look_all(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= width() || id.y >= height() {
        return;
    }
    let spot = sun_spot(u32(scatter(id.y, id.x) * f32(SUNS)), id.x, id.y);
    let on_spot = params.look.size.w == FULL;
    seens[id.y * width() + id.x] = look(f32(id.x) + 0.5, f32(id.y) + 0.5, on_spot, spot);
}

@compute @workgroup_size(8, 8)
fn draft_out(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= width() || id.y >= height() {
        return;
    }
    let here = id.y * width() + id.x;
    let light = lined(front_at(false, here), false, id.x, id.y);
    picture[here] = shown(id.x, id.y, light);
    picture[pixels() + here] = depth_bits(here);
}

@compute @workgroup_size(8, 8)
fn sun_all(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= width() || id.y >= height() {
        return;
    }
    shades[id.y * width() + id.x] = sun_at(id.x, id.y);
}

@compute @workgroup_size(8, 8)
fn open_all(@builtin(global_invocation_id) id: vec3<u32>) {
    let half = params.look.size.z;
    if id.x >= half || id.y * 2u >= height() {
        return;
    }
    shades[pixels() + id.y * half + id.x] = open_at(id.x * 2u, id.y * 2u);
}

@compute @workgroup_size(8, 8)
fn finish_all(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= width() || id.y >= height() {
        return;
    }
    drawn[id.y * width() + id.x] = finished(id.x, id.y);
}

@compute @workgroup_size(8, 8)
fn full_out(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= width() || id.y >= height() {
        return;
    }
    let here = id.y * width() + id.x;
    picture[here] = shown(id.x, id.y, lined(drawn[here], true, id.x, id.y));
    picture[pixels() + here] = depth_bits(here);
}

@compute @workgroup_size(8, 8)
fn pass_out(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= width() || id.y >= height() {
        return;
    }
    let here = id.y * width() + id.x;
    let seen = seens[here];
    var value: u32;
    switch params.look.size.w {
        case DEPTH: {
            let near = params.look.near.x;
            value = grey(u32(clamp(near / (near + seen.light.w), 0.0, 1.0) * 255.0));
        }
        case LINES: {
            var edge = false;
            for (var k = 0; k < 2; k++) {
                let there = pixel_index(i32(id.x) + 1 - k, i32(id.y) + k);
                if there >= 0 && !same_surface(seens[there], seen) {
                    edge = true;
                }
            }
            value = grey(select(0u, 255u, edge));
        }
        default: {
            if seen.ids.y == SKY {
                value = grey(0u);
            } else if seen.ids.y == GROUND {
                value = grey(u32(0.25 * 255.0));
            } else {
                value = own_colour(seen.ids.y);
            }
        }
    }
    picture[here] = value;
    picture[pixels() + here] = depth_bits(here);
}
