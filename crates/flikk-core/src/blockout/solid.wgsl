// Where a ray meets a solid: `solid.rs` on the GPU, and kept to it. A
// solid is as `Solid::pack` writes it. NEAR, EDGE and the shapes' numbers
// come from `solid.rs`.
//
// Nothing here may make an infinity or a NaN: Metal compiles with fast
// math, where they compare as they please. BIG stands for "nothing met".

const BIG: f32 = 1e30;
const UP: vec3<f32> = vec3<f32>(0.0, 1.0, 0.0);

struct Solid {
    // Its centre, and the radius of a ball about it that holds it.
    a: vec4<f32>,
    // Its own axes, each with its half size along it.
    x: vec4<f32>,
    y: vec4<f32>,
    z: vec4<f32>,
    // How wide its base and its top are against its half size, and its
    // tone.
    e: vec4<f32>,
    // Its shape, its owner, where its holes are among the solids, and how
    // many.
    f: vec4<u32>,
}

@group(0) @binding(1) var<storage, read> solids: array<Solid>;

// Where a ray met something: how far, and the way the surface faces.
struct Met {
    ok: bool,
    t: f32,
    n: vec3<f32>,
}

fn missed() -> Met {
    return Met(false, 0.0, vec3<f32>(0.0));
}

fn nearer(a: Met, b: Met) -> Met {
    if a.ok && b.ok {
        if a.t <= b.t {
            return a;
        }
        return b;
    }
    if a.ok {
        return a;
    }
    return b;
}

fn unit_or(v: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let size = sqrt(dot(v, v));
    if size > 1e-6 {
        return v * (1.0 / size);
    }
    return fallback;
}

fn inward(s: Solid, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(s.x.xyz, v), dot(s.y.xyz, v), dot(s.z.xyz, v));
}

fn outward(s: Solid, v: vec3<f32>) -> vec3<f32> {
    return s.x.xyz * v.x + s.y.xyz * v.y + s.z.xyz * v.z;
}

fn half_size(s: Solid) -> vec3<f32> {
    return vec3<f32>(s.x.w, s.y.w, s.z.w);
}

// A face sloping from the top's middle down to the side `way`.
fn pitch(h: vec3<f32>, way: f32) -> vec4<f32> {
    return vec4<f32>(way * 2.0 * h.y, h.x, 0.0, h.x * h.y);
}

// How many faces the hull of `shape` has: a box's is narrowed.
fn face_count(shape: u32) -> u32 {
    return select(5u, 6u, shape == BOX);
}

// Face `i` of the hull of `shape` of half size `h`: the way out of it,
// and how far along that way the face is. A box's is narrowed to `ends`.
fn face(shape: u32, h: vec3<f32>, ends: vec2<f32>, i: u32) -> vec4<f32> {
    let base = vec4<f32>(0.0, -1.0, 0.0, h.y);
    let back = vec4<f32>(0.0, 0.0, -1.0, h.z);
    if i == 0u {
        return base;
    }
    if shape == BOX {
        let lean = ends.x - ends.y;
        let x = h.x * h.y * (ends.x + ends.y);
        let z = h.z * h.y * (ends.x + ends.y);
        switch i {
            case 1u: { return vec4<f32>(0.0, 1.0, 0.0, h.y); }
            case 2u: { return vec4<f32>(2.0 * h.y, lean * h.x, 0.0, x); }
            case 3u: { return vec4<f32>(-2.0 * h.y, lean * h.x, 0.0, x); }
            case 4u: { return vec4<f32>(0.0, lean * h.z, 2.0 * h.y, z); }
            default: { return vec4<f32>(0.0, lean * h.z, -2.0 * h.y, z); }
        }
    }
    if shape == ROOF {
        switch i {
            case 1u: { return back; }
            case 2u: { return vec4<f32>(0.0, 0.0, 1.0, h.z); }
            case 3u: { return pitch(h, 1.0); }
            default: { return pitch(h, -1.0); }
        }
    }
    if shape == PYRAMID {
        switch i {
            case 1u: { return pitch(h, 1.0); }
            case 2u: { return pitch(h, -1.0); }
            case 3u: { return vec4<f32>(0.0, h.z, 2.0 * h.y, h.z * h.y); }
            default: { return vec4<f32>(0.0, h.z, -2.0 * h.y, h.z * h.y); }
        }
    }
    // A wedge.
    switch i {
        case 1u: { return back; }
        case 2u: { return vec4<f32>(1.0, 0.0, 0.0, h.x); }
        case 3u: { return vec4<f32>(-1.0, 0.0, 0.0, h.x); }
        default: { return vec4<f32>(0.0, h.z, h.y, 0.0); }
    }
}

// Where a ray enters the solid inside all the faces of `shape`'s hull.
fn hit_hull(shape: u32, h: vec3<f32>, ends: vec2<f32>, o: vec3<f32>, d: vec3<f32>) -> Met {
    var near = -BIG;
    var far = BIG;
    var normal = vec3<f32>(0.0);
    for (var i = 0u; i < face_count(shape); i++) {
        let f = face(shape, h, ends, i);
        let toward = dot(f.xyz, d);
        let inside = f.w - dot(f.xyz, o);
        if abs(toward) < 1e-9 {
            if inside < 0.0 {
                return missed();
            }
            continue;
        }
        let t = inside / toward;
        if toward < 0.0 {
            if t > near {
                near = t;
                normal = f.xyz;
            }
        } else {
            far = min(far, t);
        }
    }
    if near <= far && near > NEAR {
        return Met(true, near, normal);
    }
    return missed();
}

// The roots of `a·t² + 2·b·t + c = 0`, nearer first.
struct Roots {
    ok: bool,
    near: f32,
    far: f32,
}

fn roots(a: f32, b: f32, c: f32) -> Roots {
    if abs(a) < 1e-9 {
        return Roots(false, 0.0, 0.0);
    }
    let disc = b * b - a * c;
    if disc < 0.0 {
        return Roots(false, 0.0, 0.0);
    }
    let root = sqrt(disc);
    let t0 = (-b - root) / a;
    let t1 = (-b + root) / a;
    if t0 <= t1 {
        return Roots(true, t0, t1);
    }
    return Roots(true, t1, t0);
}

fn hit_box(o: vec3<f32>, d: vec3<f32>, half: vec3<f32>) -> Met {
    var near = -BIG;
    var far = BIG;
    var normal = vec3<f32>(0.0);
    for (var axis = 0u; axis < 3u; axis++) {
        let oa = o[axis];
        let da = d[axis];
        let ha = half[axis];
        if abs(da) < 1e-9 {
            if abs(oa) > ha {
                return missed();
            }
            continue;
        }
        let a = (-ha - oa) / da;
        let b = (ha - oa) / da;
        let enter = select(b, a, a < b);
        let leave = select(a, b, a < b);
        if enter > near {
            near = enter;
            var n = vec3<f32>(0.0);
            n[axis] = select(1.0, -1.0, da > 0.0);
            normal = n;
        }
        far = min(far, leave);
    }
    if near <= far && near > NEAR {
        return Met(true, near, normal);
    }
    return missed();
}

fn hit_ellipsoid(o: vec3<f32>, d: vec3<f32>, half: vec3<f32>) -> Met {
    let os = o / half;
    let ds = d / half;
    let r = roots(dot(ds, ds), dot(os, ds), dot(os, os) - 1.0);
    if !r.ok || !(r.near > NEAR) {
        return missed();
    }
    return Met(true, r.near, (os + ds * r.near) / half);
}

// The upper half of an ellipsoid twice as high as `half`, standing on the
// underside of its box.
fn hit_dome(start: vec3<f32>, d: vec3<f32>, half: vec3<f32>) -> Met {
    let o = vec3<f32>(start.x, start.y + half.y, start.z);
    let radii = vec3<f32>(half.x, half.y * 2.0, half.z);
    let os = o / radii;
    let ds = d / radii;
    let r = roots(dot(ds, ds), dot(os, ds), dot(os, os) - 1.0);
    if !r.ok {
        return missed();
    }
    if r.near > NEAR && o.y + d.y * r.near >= 0.0 {
        return Met(true, r.near, (os + ds * r.near) / radii);
    }
    // Under the round part: in through the flat underside, if at all.
    if d.y > 1e-9 {
        let under = -o.y / d.y;
        if under > NEAR && under >= r.near && under <= r.far {
            return Met(true, under, vec3<f32>(0.0, -1.0, 0.0));
        }
    }
    return missed();
}

// Where a ray comes in through the side of a round solid: the nearer root
// of `a·t² + 2·b·t + c = 0` where it is going inward.
fn into_side(a: f32, b: f32, c: f32) -> Met {
    if abs(a) < 1e-9 {
        if b < -1e-9 {
            return Met(true, -c / (2.0 * b), vec3<f32>(0.0));
        }
        return missed();
    }
    let disc = b * b - a * c;
    if disc >= 0.0 {
        return Met(true, (-b - sqrt(disc)) / a, vec3<f32>(0.0));
    }
    return missed();
}

// A cylinder of half size `half` whose base and top are `ends` as wide as
// it: a cone's top is nothing wide.
fn hit_frustum(o: vec3<f32>, d: vec3<f32>, half: vec3<f32>, ends: vec2<f32>) -> Met {
    let ox = o.x / half.x;
    let oz = o.z / half.z;
    let dx = d.x / half.x;
    let dz = d.z / half.z;
    let low = ends.x;
    let high = ends.y;
    let slope = (high - low) / (2.0 * half.y);
    let wide = low + slope * (o.y + half.y);
    var side = into_side(
        dx * dx + dz * dz - slope * slope * d.y * d.y,
        ox * dx + oz * dz - wide * slope * d.y,
        ox * ox + oz * oz - wide * wide,
    );
    if side.ok && side.t > NEAR && abs(o.y + d.y * side.t) <= half.y {
        let t = side.t;
        let radius = wide + slope * d.y * t;
        side.n = vec3<f32>((ox + dx * t) / half.x, -radius * slope, (oz + dz * t) / half.z);
    } else {
        side = missed();
    }
    // The end it comes in through, if it comes in through one.
    var end = missed();
    if abs(d.y) > 1e-9 {
        let up = select(1.0, -1.0, d.y > 0.0);
        let y = half.y * up;
        let t = (y - o.y) / d.y;
        let x = ox + dx * t;
        let z = oz + dz * t;
        let radius = select(low, high, up > 0.0);
        if t > NEAR && x * x + z * z <= radius * radius {
            end = Met(true, t, vec3<f32>(0.0, up, 0.0));
        }
    }
    return nearer(side, end);
}

// A capsule as the balls at its ends (how high each middle is, and its
// radius) and the side between them, from where it leaves the lower to
// where it meets the upper: `solid.rs`'s `Balls`.
struct Balls {
    low: vec2<f32>,
    high: vec2<f32>,
    side_low: vec2<f32>,
    side_high: vec2<f32>,
}

fn balls_of(half: vec3<f32>, ends: vec2<f32>) -> Balls {
    let radius = min(min(half.x, half.z), half.y);
    let low = radius * ends.x;
    let high = radius * ends.y;
    let start = low - half.y;
    let to = half.y - high;
    let lean = clamp((low - high) / max(to - start, 1e-6), -0.99, 0.99);
    let level = sqrt(1.0 - lean * lean);
    return Balls(
        vec2<f32>(start, low),
        vec2<f32>(to, high),
        vec2<f32>(start + low * lean, low * level),
        vec2<f32>(to + high * lean, high * level),
    );
}

fn hit_ball(o: vec3<f32>, d: vec3<f32>, ball: vec2<f32>) -> Met {
    let start = vec3<f32>(o.x, o.y - ball.x, o.z);
    let r = roots(1.0, dot(start, d), dot(start, start) - ball.y * ball.y);
    if !r.ok || !(r.near > NEAR) {
        return missed();
    }
    return Met(true, r.near, start + d * r.near);
}

fn hit_capsule(o: vec3<f32>, d: vec3<f32>, balls: Balls) -> Met {
    let start = balls.side_low.x;
    let low = balls.side_low.y;
    let to = balls.side_high.x;
    let high = balls.side_high.y;
    let slope = (high - low) / max(to - start, 1e-6);
    let wide = low + slope * (o.y - start);
    var side = into_side(
        d.x * d.x + d.z * d.z - slope * slope * d.y * d.y,
        o.x * d.x + o.z * d.z - wide * slope * d.y,
        o.x * o.x + o.z * o.z - wide * wide,
    );
    let along = o.y + d.y * side.t;
    if side.ok && side.t > NEAR && start <= along && along <= to {
        let t = side.t;
        let radius = wide + slope * d.y * t;
        side.n = vec3<f32>(o.x + d.x * t, -radius * slope, o.z + d.z * t);
    } else {
        side = missed();
    }
    // Each ball shows past its end of the side.
    var below = hit_ball(o, d, balls.low);
    if below.ok && !(o.y + d.y * below.t <= start) {
        below = missed();
    }
    var above = hit_ball(o, d, balls.high);
    if above.ok && !(o.y + d.y * above.t >= to) {
        above = missed();
    }
    return nearer(side, nearer(below, above));
}

// Where the ray from `origin` along `dir` first meets `s`, taken whole.
fn hit_whole(s: Solid, origin: vec3<f32>, dir: vec3<f32>) -> Met {
    let to = s.a.xyz - origin;
    let along = dot(to, dir);
    let reach = s.a.w;
    if along < -reach || dot(to, to) - along * along > reach * reach {
        return missed();
    }
    let o = inward(s, origin - s.a.xyz);
    let d = inward(s, dir);
    let h = half_size(s);
    let ends = s.e.xy;
    var m: Met;
    switch s.f.x {
        case BOX: {
            if ends.x == 1.0 && ends.y == 1.0 {
                m = hit_box(o, d, h);
            } else {
                m = hit_hull(BOX, h, ends, o, d);
            }
        }
        case SPHERE: { m = hit_ellipsoid(o, d, h); }
        case CYLINDER: { m = hit_frustum(o, d, h, ends); }
        case CONE: { m = hit_frustum(o, d, h, vec2<f32>(1.0, 0.0)); }
        case CAPSULE: { m = hit_capsule(o, d, balls_of(h, ends)); }
        case DOME: { m = hit_dome(o, d, h); }
        default: { m = hit_hull(s.f.x, h, ends, o, d); }
    }
    if !m.ok {
        return m;
    }
    return Met(true, m.t, unit_or(outward(s, m.n), UP));
}

// Whether `point` is inside `s`, taken whole.
fn holds_whole(s: Solid, point: vec3<f32>) -> bool {
    let off = point - s.a.xyz;
    if dot(off, off) > s.a.w * s.a.w {
        return false;
    }
    let p = inward(s, off);
    let h = half_size(s);
    let rounded = (p.x / h.x) * (p.x / h.x) + (p.z / h.z) * (p.z / h.z);
    let low = s.e.x;
    let high = s.e.y;
    let wide = low + (high - low) * clamp((p.y + h.y) / (2.0 * h.y), 0.0, 1.0);
    switch s.f.x {
        case BOX: {
            return abs(p.x) <= h.x * wide && abs(p.y) <= h.y && abs(p.z) <= h.z * wide;
        }
        case SPHERE: { return rounded + (p.y / h.y) * (p.y / h.y) <= 1.0; }
        case CYLINDER: { return abs(p.y) <= h.y && rounded <= wide * wide; }
        case CONE: {
            let narrowed = (h.y - p.y) / (2.0 * h.y);
            return abs(p.y) <= h.y && rounded <= narrowed * narrowed;
        }
        case CAPSULE: {
            let balls = balls_of(h, s.e.xy);
            let along = clamp(p.y, balls.low.x, balls.high.x);
            let up = (along - balls.low.x) / max(balls.high.x - balls.low.x, 1e-6);
            let radius = balls.low.y + (balls.high.y - balls.low.y) * up;
            let start = p - vec3<f32>(0.0, along, 0.0);
            return sqrt(dot(start, start)) <= radius;
        }
        case DOME: {
            let up = (p.y + h.y) / (2.0 * h.y);
            return up >= 0.0 && rounded + up * up <= 1.0;
        }
        default: {
            for (var i = 0u; i < face_count(s.f.x); i++) {
                let f = face(s.f.x, h, s.e.xy, i);
                if dot(f.xyz, p) > f.w {
                    return false;
                }
            }
            return true;
        }
    }
}

// How far along the ray it is inside `s` taken whole, from and to, and
// the way its surface faces where the ray leaves.
struct Span {
    ok: bool,
    enter: f32,
    leave: f32,
    out: vec3<f32>,
}

fn span(s: Solid, origin: vec3<f32>, dir: vec3<f32>) -> Span {
    // Looked at from outside it, either way along the ray: each is then
    // the first surface met.
    let middle = dot(s.a.xyz - origin, dir);
    let back = s.a.w * 2.0 + 0.01;
    let enter = hit_whole(s, origin + dir * (middle - back), dir);
    if !enter.ok {
        return Span(false, 0.0, 0.0, vec3<f32>(0.0));
    }
    let leave = hit_whole(s, origin + dir * (middle + back), -dir);
    if !leave.ok {
        return Span(false, 0.0, 0.0, vec3<f32>(0.0));
    }
    return Span(true, middle - back + enter.t, middle + back - leave.t, leave.n);
}

// Where the ray from `origin` along `dir` first meets solid `index`, its
// holes cut out.
fn hit(index: u32, origin: vec3<f32>, dir: vec3<f32>) -> Met {
    let s = solids[index];
    let first = hit_whole(s, origin, dir);
    let holes = s.f.w;
    if holes == 0u {
        return first;
    }
    // Most rays meet it where no hole is.
    if first.ok {
        let at = origin + dir * first.t;
        var in_hole = false;
        for (var k = 0u; k < holes; k++) {
            if holds_whole(solids[s.f.z + k], at) {
                in_hole = true;
                break;
            }
        }
        if !in_hole {
            return first;
        }
    }
    let whole = span(s, origin, dir);
    if !whole.ok {
        return missed();
    }
    // From where the ray comes into it, on through each hole that is there
    // to where the ray leaves that hole: one may run on into another.
    var at = max(whole.enter, 0.0);
    var through = false;
    var faces = vec3<f32>(0.0);
    for (var round_ = 0u; round_ <= holes; round_++) {
        var found = false;
        var to = 0.0;
        var out = vec3<f32>(0.0);
        for (var k = 0u; k < holes; k++) {
            let hole = span(solids[s.f.z + k], origin, dir);
            if hole.ok && hole.enter <= at + EDGE && hole.leave > at + EDGE {
                found = true;
                to = hole.leave;
                out = hole.out;
                break;
            }
        }
        if !found {
            break;
        }
        at = to;
        faces = -out;
        through = true;
    }
    if at >= whole.leave - EDGE || at <= NEAR || !through {
        return missed();
    }
    return Met(true, at, faces);
}
