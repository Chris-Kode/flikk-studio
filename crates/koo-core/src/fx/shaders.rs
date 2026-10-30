//! Built-in GLSL assets: effects, transitions and generators. See the module
//! docs of [`super`] for the conventions every shader follows.
//!
//! Shaders that need noise or soft blurs paste the snippets below; each
//! program compiles on its own, so the helpers are per shader.

use super::{FxAsset, FxKind, FxLanguage, FxRef, ParamKind, ParamSpec, ParamValue, color, float};

fn glsl(
    key: &str,
    name: &str,
    kind: FxKind,
    category: &str,
    description: &str,
    code: &str,
    params: Vec<ParamSpec>,
) -> FxAsset {
    FxAsset {
        id: FxRef::builtin(key),
        name: name.into(),
        kind,
        language: FxLanguage::Glsl,
        category: category.into(),
        description: description.into(),
        code: code.into(),
        params,
        origin: None,
    }
}

fn int(name: &str, label: &str, min: i64, max: i64, default: i64) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        label: label.into(),
        kind: ParamKind::Int { min, max },
        default: ParamValue::Number(default as f64),
    }
}

fn toggle(name: &str, label: &str, default: bool) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        label: label.into(),
        kind: ParamKind::Bool,
        default: ParamValue::Bool(default),
    }
}

fn point(name: &str, label: &str, xy: [f64; 2]) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        label: label.into(),
        kind: ParamKind::Point,
        default: ParamValue::List(xy.to_vec()),
    }
}

fn choice(name: &str, label: &str, options: &[&str], default: usize) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        label: label.into(),
        kind: ParamKind::Choice {
            options: options.iter().map(|o| (*o).into()).collect(),
        },
        default: ParamValue::Text(options[default].into()),
    }
}

const DIRECTIONS: &[&str] = &["Left", "Right", "Up", "Down"];

/// Hash without `sin` (Dave Hoskins), stable across GPUs.
macro_rules! hash {
    () => {
        r#"float hash12(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}
"#
    };
}

/// `hash12`, smooth value noise and 5-octave fbm.
macro_rules! noise {
    () => {
        concat!(
            hash!(),
            r#"float vnoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    float a = hash12(i);
    float b = hash12(i + vec2(1.0, 0.0));
    float c = hash12(i + vec2(0.0, 1.0));
    float d = hash12(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}
float fbm(vec2 p) {
    float s = 0.0;
    float a = 0.5;
    for (int i = 0; i < 5; i++) {
        s += a * vnoise(p);
        p = p * 2.03 + vec2(17.1, 9.2);
        a *= 0.5;
    }
    return s / 0.96875;
}
"#
        )
    };
}

/// Straight-alpha averaging: blur in premultiplied space, then undo it.
macro_rules! premul {
    () => {
        r#"vec4 premul(vec4 c) { return vec4(c.rgb * c.a, c.a); }
vec4 unpremul(vec4 c) { return c.a > 0.00001 ? vec4(c.rgb / c.a, c.a) : vec4(0.0); }
"#
    };
}

pub(super) fn all() -> Vec<FxAsset> {
    let mut all = effects();
    all.extend(transitions());
    all.extend(generators());
    all
}

fn effects() -> Vec<FxAsset> {
    use FxKind::Effect;
    vec![
        // ---- Blur
        glsl(
            "gaussian_blur",
            "Gaussian Blur",
            Effect,
            "Blur",
            "Soft blur; radius in pixels at the output size.",
            concat!(
                hash!(),
                premul!(),
                r#"vec4 effect(vec2 uv) {
    if (radius < 0.5) return getColor(uv);
    vec2 px = 1.0 / iResolution;
    float sigma = radius / 2.0;
    // Taps cover 3 sigma; wide blurs spread them and jitter within cells.
    int n = int(clamp(ceil(radius * 1.5 / 2.5), 2.0, 10.0));
    float spacing = radius * 1.5 / float(n);
    vec2 frag = uv * iResolution;
    vec2 jitter = (vec2(hash12(frag), hash12(frag + 17.0)) - 0.5) * max(spacing - 1.5, 0.0);
    vec4 sum = vec4(0.0);
    float total = 0.0;
    for (int x = -10; x <= 10; x++) {
        if (x < -n || x > n) continue;
        for (int y = -10; y <= 10; y++) {
            if (y < -n || y > n) continue;
            vec2 o = vec2(float(x), float(y)) * spacing + jitter;
            float w = exp(-dot(o, o) / (2.0 * sigma * sigma));
            sum += premul(getColor(uv + o * px)) * w;
            total += w;
        }
    }
    return unpremul(sum / total);
}"#
            ),
            vec![float("radius", "Radius", 0.0, 100.0, 8.0)],
        ),
        glsl(
            "directional_blur",
            "Directional Blur",
            Effect,
            "Blur",
            "Motion blur along one direction.",
            concat!(
                premul!(),
                r#"vec4 effect(vec2 uv) {
    float a = radians(angle);
    vec2 dir = vec2(cos(a), sin(a)) * size / iResolution;
    vec4 sum = vec4(0.0);
    for (int i = 0; i < 48; i++) {
        float t = (float(i) + 0.5) / 48.0 - 0.5;
        sum += premul(getColor(uv + dir * t));
    }
    return unpremul(sum / 48.0);
}"#
            ),
            vec![
                float("size", "Length", 0.0, 400.0, 40.0),
                float("angle", "Angle", -180.0, 180.0, 0.0),
            ],
        ),
        glsl(
            "zoom_blur",
            "Zoom Blur",
            Effect,
            "Blur",
            "Radial blur streaking out from a center point.",
            concat!(
                premul!(),
                r#"vec4 effect(vec2 uv) {
    vec4 sum = vec4(0.0);
    for (int i = 0; i < 48; i++) {
        float s = 1.0 - amount * float(i) / 47.0;
        sum += premul(getColor(center + (uv - center) * s));
    }
    return unpremul(sum / 48.0);
}"#
            ),
            vec![
                float("amount", "Amount", 0.0, 1.0, 0.2),
                point("center", "Center", [0.5, 0.5]),
            ],
        ),
        glsl(
            "sharpen",
            "Sharpen",
            Effect,
            "Sharpen",
            "Unsharp mask on luminance, so edges sharpen without color fringes.",
            r#"const vec3 LUMA = vec3(0.2126, 0.7152, 0.0722);
vec4 effect(vec2 uv) {
    vec2 px = radius / iResolution;
    vec4 c = getColor(uv);
    vec3 cross4 = getColor(uv + vec2(px.x, 0.0)).rgb + getColor(uv - vec2(px.x, 0.0)).rgb
        + getColor(uv + vec2(0.0, px.y)).rgb + getColor(uv - vec2(0.0, px.y)).rgb;
    vec3 diag4 = getColor(uv + px).rgb + getColor(uv - px).rgb
        + getColor(uv + vec2(px.x, -px.y)).rgb + getColor(uv + vec2(-px.x, px.y)).rgb;
    vec3 blur = cross4 * 0.15 + diag4 * 0.1;
    float detail = dot(c.rgb - blur, LUMA);
    return vec4(clamp(c.rgb + detail * amount, 0.0, 1.0), c.a);
}"#,
            vec![
                float("amount", "Amount", 0.0, 4.0, 1.0),
                float("radius", "Radius", 0.5, 5.0, 1.0),
            ],
        ),
        // ---- Stylize
        glsl(
            "vignette",
            "Vignette",
            Effect,
            "Stylize",
            "Darkens the corners.",
            r#"vec4 effect(vec2 uv) {
    vec4 c = getColor(uv);
    vec2 d = (uv - 0.5) * vec2(iResolution.x / iResolution.y, 1.0);
    float v = 1.0 - smoothstep(size - softness, size, length(d));
    return vec4(c.rgb * mix(1.0 - amount, 1.0, v), c.a);
}"#,
            vec![
                float("amount", "Amount", 0.0, 1.0, 0.6),
                float("size", "Size", 0.1, 1.5, 0.8),
                float("softness", "Softness", 0.01, 1.0, 0.45),
            ],
        ),
        glsl(
            "film_grain",
            "Film Grain",
            Effect,
            "Stylize",
            "Animated photographic grain, strongest in the midtones.",
            concat!(
                noise!(),
                r#"vec4 effect(vec2 uv) {
    vec4 c = getColor(uv);
    float frame = mod(floor(iTime * 24.0), 97.0);
    vec2 p = uv * iResolution / size;
    vec2 seed = vec2(frame * 13.17, frame * 7.31);
    vec3 n = vec3(vnoise(p + seed));
    if (colored) {
        n.g = vnoise(p + seed + 41.7);
        n.b = vnoise(p + seed + 83.1);
    }
    n = (n - 0.5) * 2.0;
    float l = dot(c.rgb, vec3(0.2126, 0.7152, 0.0722));
    float w = 1.0 - 0.7 * pow(abs(l - 0.5) * 2.0, 2.0);
    return vec4(clamp(c.rgb + n * amount * 0.35 * w, 0.0, 1.0), c.a);
}"#
            ),
            vec![
                float("amount", "Amount", 0.0, 1.0, 0.3),
                float("size", "Size", 1.0, 4.0, 1.5),
                toggle("colored", "Color", false),
            ],
        ),
        glsl(
            "glow",
            "Glow",
            Effect,
            "Stylize",
            "Blooms the highlights above a threshold.",
            concat!(
                hash!(),
                r#"vec4 effect(vec2 uv) {
    vec4 c = getColor(uv);
    vec2 px = 1.0 / iResolution;
    float spin = hash12(uv * iResolution) * 6.2831853;
    float sigma = radius * 0.5;
    vec3 bloom = vec3(0.0);
    float total = 0.0;
    for (int i = 0; i < 64; i++) {
        float f = (float(i) + 0.5) / 64.0;
        float r = radius * sqrt(f);
        float th = float(i) * 2.39996323 + spin;
        vec4 s = getColor(uv + vec2(cos(th), sin(th)) * r * px);
        float w = exp(-r * r / (2.0 * sigma * sigma));
        vec3 bright = max(s.rgb - threshold, 0.0) / max(1.0 - threshold, 0.001);
        bloom += bright * s.a * w;
        total += w;
    }
    bloom = clamp(bloom / total * intensity, 0.0, 1.0);
    vec3 rgb = 1.0 - (1.0 - c.rgb) * (1.0 - bloom);
    return vec4(rgb, max(c.a, max(bloom.r, max(bloom.g, bloom.b))));
}"#
            ),
            vec![
                float("threshold", "Threshold", 0.0, 1.0, 0.6),
                float("intensity", "Intensity", 0.0, 4.0, 1.5),
                float("radius", "Radius", 2.0, 150.0, 30.0),
            ],
        ),
        glsl(
            "chromatic_aberration",
            "Chromatic Aberration",
            Effect,
            "Stylize",
            "Splits red and blue toward the edges like a cheap lens.",
            r#"vec4 effect(vec2 uv) {
    vec2 d = uv - 0.5;
    vec2 off = d * 2.0 * amount / iResolution;
    vec4 c = getColor(uv);
    vec4 r = getColor(uv - off);
    vec4 b = getColor(uv + off);
    return vec4(r.r, c.g, b.b, max(c.a, max(r.a, b.a)));
}"#,
            vec![float("amount", "Amount", 0.0, 40.0, 6.0)],
        ),
        glsl(
            "pixelate",
            "Pixelate",
            Effect,
            "Stylize",
            "Mosaic of square cells.",
            r#"vec4 effect(vec2 uv) {
    vec2 cell = vec2(max(size, 1.0)) / iResolution;
    vec2 p = (floor((uv - 0.5) / cell) + 0.5) * cell + 0.5;
    vec2 q = cell * 0.25;
    return (getColor(p + vec2(-q.x, -q.y)) + getColor(p + vec2(q.x, -q.y))
        + getColor(p + vec2(-q.x, q.y)) + getColor(p + vec2(q.x, q.y))) * 0.25;
}"#,
            vec![float("size", "Cell Size", 2.0, 200.0, 24.0)],
        ),
        glsl(
            "posterize",
            "Posterize",
            Effect,
            "Stylize",
            "Reduces each channel to a few levels.",
            r#"vec4 effect(vec2 uv) {
    vec4 c = getColor(uv);
    float n = max(float(levels), 2.0) - 1.0;
    return vec4(floor(c.rgb * n + 0.5) / n, c.a);
}"#,
            vec![int("levels", "Levels", 2, 32, 6)],
        ),
        glsl(
            "halftone",
            "Halftone",
            Effect,
            "Stylize",
            "Print-style dot screen, in color (CMY) or black ink.",
            r#"vec2 rot(vec2 p, float a) {
    float c = cos(a);
    float s = sin(a);
    return vec2(c * p.x - s * p.y, s * p.x + c * p.y);
}
// Ink coverage (0..1) at `frag` for one screen rotated by `a`.
float screen(vec2 frag, float a, int channel) {
    vec2 p = rot(frag, -a);
    vec2 cell = (floor(p / size) + 0.5) * size;
    vec4 c = getColor(rot(cell, a) / iResolution);
    float v = dot(c.rgb, vec3(0.2126, 0.7152, 0.0722));
    if (channel == 0) v = c.r;
    if (channel == 1) v = c.g;
    if (channel == 2) v = c.b;
    float r = size * 0.8 * sqrt(clamp(1.0 - v, 0.0, 1.0));
    return (1.0 - smoothstep(r - 0.75, r + 0.75, length(p - cell))) * min(r, 1.0);
}
vec4 effect(vec2 uv) {
    vec2 frag = uv * iResolution;
    float a = radians(angle);
    float alpha = getColor(uv).a;
    if (mode == 1) {
        return vec4(vec3(1.0 - screen(frag, a, 3)), alpha);
    }
    vec3 ink = vec3(
        screen(frag, a + radians(15.0), 0),
        screen(frag, a + radians(75.0), 1),
        screen(frag, a, 2));
    return vec4(1.0 - ink, alpha);
}"#,
            vec![
                float("size", "Dot Size", 4.0, 80.0, 12.0),
                float("angle", "Angle", -90.0, 90.0, 45.0),
                choice("mode", "Ink", &["Color", "Black"], 0),
            ],
        ),
        glsl(
            "edge_detect",
            "Find Edges",
            Effect,
            "Stylize",
            "Sobel edge outline, as glowing lines or ink on paper.",
            r#"float lum(vec2 uv) {
    return dot(getColor(uv).rgb, vec3(0.2126, 0.7152, 0.0722));
}
vec4 effect(vec2 uv) {
    vec2 px = 1.0 / iResolution;
    float tl = lum(uv + vec2(-px.x, px.y));
    float t = lum(uv + vec2(0.0, px.y));
    float tr = lum(uv + vec2(px.x, px.y));
    float l = lum(uv + vec2(-px.x, 0.0));
    float r = lum(uv + vec2(px.x, 0.0));
    float bl = lum(uv + vec2(-px.x, -px.y));
    float b = lum(uv + vec2(0.0, -px.y));
    float br = lum(uv + vec2(px.x, -px.y));
    float gx = tr + 2.0 * r + br - tl - 2.0 * l - bl;
    float gy = tl + 2.0 * t + tr - bl - 2.0 * b - br;
    float e = clamp(length(vec2(gx, gy)) * strength, 0.0, 1.0);
    vec3 edges = invert ? vec3(1.0 - e) : vec3(e);
    vec4 c = getColor(uv);
    return vec4(mix(edges, c.rgb, blend), c.a);
}"#,
            vec![
                float("strength", "Strength", 0.0, 5.0, 1.5),
                toggle("invert", "Ink on Paper", false),
                float("blend", "Blend", 0.0, 1.0, 0.0),
            ],
        ),
        glsl(
            "scanlines",
            "Scanlines",
            Effect,
            "Stylize",
            "CRT scanlines, optionally rolling.",
            r#"vec4 effect(vec2 uv) {
    vec4 c = getColor(uv);
    float n = min(lines, iResolution.y * 0.5);
    float y = uv.y * n + iTime * speed;
    float s = 0.5 + 0.5 * cos(y * 6.2831853);
    float m = 1.0 - intensity * (1.0 - s);
    return vec4(clamp(c.rgb * m * (1.0 + intensity * 0.3), 0.0, 1.0), c.a);
}"#,
            vec![
                float("lines", "Lines", 50.0, 1500.0, 480.0),
                float("intensity", "Intensity", 0.0, 1.0, 0.35),
                float("speed", "Roll", -20.0, 20.0, 0.0),
            ],
        ),
        glsl(
            "vhs",
            "VHS",
            Effect,
            "Stylize",
            "Tape look: line wobble, chroma bleed, tracking band and noise.",
            concat!(
                noise!(),
                r#"vec4 effect(vec2 uv) {
    float t = iTime;
    float frame = mod(floor(t * 30.0), 101.0);
    float row = floor(uv.y * iResolution.y / 2.0);
    float wobble = (vnoise(vec2(row * 0.05, t * 8.0)) - 0.5) * 0.004 * amount;
    float band = fract(-t * 0.15) * 1.2 - 0.1;
    float x = uv.y - band;
    float inBand = smoothstep(0.0, 0.02, x) * (1.0 - smoothstep(0.03, 0.08, x));
    wobble += inBand * (hash12(vec2(row, frame)) - 0.5) * 0.03 * amount;
    vec2 p = vec2(uv.x + wobble, uv.y);
    vec4 c = getColor(p);
    // Luma stays sharp; chroma is smeared to the right.
    float off = 4.0 * amount / iResolution.x;
    vec3 smear = (getColor(p - vec2(off, 0.0)).rgb + getColor(p - vec2(2.0 * off, 0.0)).rgb
        + getColor(p - vec2(3.0 * off, 0.0)).rgb + getColor(p - vec2(4.0 * off, 0.0)).rgb) * 0.25;
    float y = dot(c.rgb, vec3(0.299, 0.587, 0.114));
    float i = dot(smear, vec3(0.596, -0.274, -0.322)) * (1.0 + 0.2 * amount);
    float q = dot(smear, vec3(0.211, -0.523, 0.312)) * (1.0 + 0.2 * amount);
    vec3 rgb = vec3(y + 0.956 * i + 0.621 * q, y - 0.272 * i - 0.647 * q, y - 1.106 * i + 1.703 * q);
    rgb += (hash12(uv * iResolution + frame * 3.7) - 0.5) * 0.12 * amount;
    rgb += inBand * step(0.93, hash12(vec2(uv.x * 300.0, row + frame))) * 0.5 * amount;
    rgb = mix(rgb, rgb * vec3(1.03, 0.98, 1.04) + 0.03, amount * 0.5);
    return vec4(clamp(rgb, 0.0, 1.0), c.a);
}"#
            ),
            vec![float("amount", "Amount", 0.0, 1.0, 0.6)],
        ),
        glsl(
            "glitch",
            "Digital Glitch",
            Effect,
            "Stylize",
            "Torn scanline blocks, RGB split and corrupted tiles.",
            concat!(
                hash!(),
                r#"vec4 effect(vec2 uv) {
    float t = floor(iTime * speed);
    float rows = 6.0 + 30.0 * hash12(vec2(t, 3.0));
    float rowId = floor(uv.y * rows);
    float on = step(1.0 - amount * 0.6, hash12(vec2(rowId, t)));
    float shift = (hash12(vec2(rowId, t + 9.0)) - 0.5) * 0.25 * amount * on;
    vec2 p = vec2(fract(uv.x + shift), uv.y);
    float split = (0.004 + 0.03 * on) * amount;
    vec4 c = getColor(p);
    vec3 col = vec3(getColor(p + vec2(split, 0.0)).r, c.g, getColor(p - vec2(split, 0.0)).b);
    vec2 tile = floor(uv * vec2(24.0, 14.0));
    float h = hash12(tile + t * 1.31);
    if (h > 1.0 - amount * 0.06) {
        col = 1.0 - col;
    } else if (h > 1.0 - amount * 0.14) {
        col = floor(col * 3.0) / 3.0;
    }
    return vec4(col, c.a);
}"#
            ),
            vec![
                float("amount", "Amount", 0.0, 1.0, 0.5),
                float("speed", "Rate", 1.0, 30.0, 8.0),
            ],
        ),
        glsl(
            "old_film",
            "Old Film",
            Effect,
            "Stylize",
            "Sepia, flicker, gate weave, scratches and dust.",
            concat!(
                hash!(),
                r#"vec4 effect(vec2 uv) {
    float fr = mod(floor(iTime * 18.0), 113.0);
    vec2 weave = (vec2(hash12(vec2(fr, 1.0)), hash12(vec2(fr, 2.0))) - 0.5) * vec2(0.002, 0.004) * amount;
    vec4 c = getColor(uv + weave);
    float l = dot(c.rgb, vec3(0.2126, 0.7152, 0.0722));
    vec3 col = mix(c.rgb, mix(vec3(l), vec3(l) * vec3(1.08, 0.92, 0.72), sepia), amount);
    col *= 1.0 + (hash12(vec2(fr, 5.0)) - 0.5) * 0.18 * amount;
    col += (hash12(uv * iResolution + fr * 7.0) - 0.5) * 0.12 * amount;
    for (int i = 0; i < 3; i++) {
        float h = hash12(vec2(fr, float(i) + 10.0));
        if (h < 0.45 * amount) {
            float x = hash12(vec2(fr, float(i) + 20.0));
            float w = (0.6 + 1.5 * h) / iResolution.x;
            float s = 1.0 - smoothstep(0.0, w, abs(uv.x - x));
            col = mix(col, vec3(h < 0.2 ? 0.1 : 0.85), s * 0.6);
        }
    }
    vec2 grid = vec2(40.0, 22.5);
    vec2 cell = floor(uv * grid);
    float dh = hash12(cell + fr * 3.1);
    if (dh > 1.0 - 0.025 * amount) {
        vec2 center = (cell + vec2(hash12(cell + fr), hash12(cell + fr + 5.0))) / grid;
        float d = length((uv - center) * iResolution);
        float r = 1.5 + 3.5 * hash12(cell + fr * 1.7);
        col = mix(col, vec3(0.05), (1.0 - smoothstep(r * 0.5, r, d)) * 0.85);
    }
    vec2 v = uv - 0.5;
    col *= 1.0 - dot(v, v) * 1.4 * amount;
    return vec4(clamp(col, 0.0, 1.0), c.a);
}"#
            ),
            vec![
                float("amount", "Amount", 0.0, 1.0, 0.8),
                float("sepia", "Sepia", 0.0, 1.0, 0.7),
            ],
        ),
        // ---- Color
        glsl(
            "duotone",
            "Duotone",
            Effect,
            "Color",
            "Maps shadows and highlights to two colors.",
            r#"vec4 effect(vec2 uv) {
    vec4 c = getColor(uv);
    float l = dot(c.rgb, vec3(0.2126, 0.7152, 0.0722));
    vec3 d = mix(shadows.rgb, highlights.rgb, smoothstep(0.0, 1.0, l));
    return vec4(mix(c.rgb, d, amount), c.a);
}"#,
            vec![
                color("shadows", "Shadows", [0.1, 0.08, 0.35, 1.0]),
                color("highlights", "Highlights", [1.0, 0.72, 0.35, 1.0]),
                float("amount", "Amount", 0.0, 1.0, 1.0),
            ],
        ),
        glsl(
            "tint_bw",
            "Black & White",
            Effect,
            "Color",
            "Monochrome with contrast and an optional tint.",
            r#"vec4 effect(vec2 uv) {
    vec4 c = getColor(uv);
    vec3 w = vec3(0.2126, 0.7152, 0.0722);
    float l = clamp((dot(c.rgb, w) - 0.5) * contrast + 0.5, 0.0, 1.0);
    vec3 bw = clamp(l * tint.rgb / max(dot(tint.rgb, w), 0.01), 0.0, 1.0);
    return vec4(mix(c.rgb, bw, amount), c.a);
}"#,
            vec![
                float("amount", "Amount", 0.0, 1.0, 1.0),
                float("contrast", "Contrast", 0.5, 2.0, 1.0),
                color("tint", "Tint", [1.0, 1.0, 1.0, 1.0]),
            ],
        ),
        glsl(
            "invert",
            "Invert",
            Effect,
            "Color",
            "Negative image.",
            r#"vec4 effect(vec2 uv) {
    vec4 c = getColor(uv);
    return vec4(mix(c.rgb, 1.0 - c.rgb, amount), c.a);
}"#,
            vec![float("amount", "Amount", 0.0, 1.0, 1.0)],
        ),
        // ---- Distort
        glsl(
            "lens_distortion",
            "Lens Distortion",
            Effect,
            "Distort",
            "Barrel (positive) or pincushion (negative) lens curvature.",
            r#"vec4 effect(vec2 uv) {
    float aspect = iResolution.x / iResolution.y;
    vec2 p = (uv - 0.5) * vec2(aspect, 1.0);
    float r2 = dot(p, p) / (0.25 * (aspect * aspect + 1.0));
    vec2 q = p * (1.0 + amount * r2) / (1.0 + max(amount, 0.0)) / zoom;
    vec2 s = q / vec2(aspect, 1.0) + 0.5;
    if (s.x < 0.0 || s.y < 0.0 || s.x > 1.0 || s.y > 1.0) return vec4(0.0);
    return getColor(s);
}"#,
            vec![
                float("amount", "Amount", -1.0, 1.0, 0.3),
                float("zoom", "Zoom", 0.5, 2.0, 1.0),
            ],
        ),
        glsl(
            "wave_warp",
            "Wave Warp",
            Effect,
            "Distort",
            "Sine-wave displacement that can travel over time.",
            r#"vec4 effect(vec2 uv) {
    vec2 frag = uv * iResolution;
    float phase = iTime * speed * 6.2831853;
    if (direction == 0) {
        frag.x += sin(frag.y / wavelength * 6.2831853 + phase) * amplitude;
    } else {
        frag.y += sin(frag.x / wavelength * 6.2831853 + phase) * amplitude;
    }
    return getColor(frag / iResolution);
}"#,
            vec![
                float("amplitude", "Amplitude", 0.0, 200.0, 15.0),
                float("wavelength", "Wavelength", 10.0, 2000.0, 200.0),
                float("speed", "Speed", -10.0, 10.0, 1.0),
                choice("direction", "Direction", &["Horizontal", "Vertical"], 0),
            ],
        ),
        glsl(
            "mirror",
            "Mirror",
            Effect,
            "Distort",
            "Reflects one half (or quarter) of the picture onto the other.",
            r#"vec4 effect(vec2 uv) {
    vec2 p = uv;
    if (mode == 0 || mode == 4) p.x = p.x > 0.5 ? 1.0 - p.x : p.x;
    if (mode == 1) p.x = p.x < 0.5 ? 1.0 - p.x : p.x;
    if (mode == 2 || mode == 4) p.y = p.y < 0.5 ? 1.0 - p.y : p.y;
    if (mode == 3) p.y = p.y > 0.5 ? 1.0 - p.y : p.y;
    return getColor(p);
}"#,
            vec![choice(
                "mode",
                "Mode",
                &[
                    "Left to Right",
                    "Right to Left",
                    "Top to Bottom",
                    "Bottom to Top",
                    "Quad",
                ],
                0,
            )],
        ),
        glsl(
            "kaleidoscope",
            "Kaleidoscope",
            Effect,
            "Distort",
            "Mirrored wedges around a center.",
            r#"vec2 mirrored(vec2 p) { return 1.0 - abs(mod(p, 2.0) - 1.0); }
vec4 effect(vec2 uv) {
    float aspect = iResolution.x / iResolution.y;
    vec2 p = (uv - center) * vec2(aspect, 1.0) / zoom;
    float r = length(p);
    float seg = 6.2831853 / float(max(segments, 2));
    float a = mod(atan(p.y, p.x) - radians(angle), seg);
    a = abs(a - seg * 0.5) + radians(angle);
    vec2 q = vec2(cos(a), sin(a)) * r;
    return getColor(mirrored(center + q / vec2(aspect, 1.0)));
}"#,
            vec![
                int("segments", "Segments", 2, 24, 6),
                float("angle", "Angle", -180.0, 180.0, 0.0),
                point("center", "Center", [0.5, 0.5]),
                float("zoom", "Zoom", 0.25, 4.0, 1.0),
            ],
        ),
        glsl(
            "bulge",
            "Bulge",
            Effect,
            "Distort",
            "Magnifies (positive) or pinches (negative) a round area.",
            r#"vec4 effect(vec2 uv) {
    float aspect = iResolution.x / iResolution.y;
    vec2 d = (uv - center) * vec2(aspect, 1.0);
    float r = length(d);
    if (r < radius && r > 0.0) {
        d *= pow(r / radius, amount);
    }
    return getColor(center + d / vec2(aspect, 1.0));
}"#,
            vec![
                point("center", "Center", [0.5, 0.5]),
                float("radius", "Radius", 0.05, 1.0, 0.35),
                float("amount", "Amount", -1.0, 1.0, 0.5),
            ],
        ),
        // ---- Keying
        glsl(
            "chroma_key",
            "Chroma Key",
            Effect,
            "Keying",
            "Removes a green or blue screen, with spill suppression.",
            r#"vec2 chroma(vec3 c) {
    return vec2(-0.1146 * c.r - 0.3854 * c.g + 0.5 * c.b, 0.5 * c.r - 0.4542 * c.g - 0.0458 * c.b);
}
vec4 effect(vec2 uv) {
    vec4 c = getColor(uv);
    vec2 k = chroma(key.rgb);
    vec2 p = chroma(c.rgb);
    float kl = max(length(k), 0.05);
    float d = length(p - k) / kl;
    float alpha = smoothstep(tolerance, tolerance + max(softness, 0.001), d);
    // Pull the key's hue out of what remains, keeping luminance.
    vec2 kd = k / kl;
    float along = dot(p, kd);
    vec3 rgb = c.rgb;
    if (along > 0.0) {
        vec2 q = p - kd * along * spill;
        float y = dot(c.rgb, vec3(0.2126, 0.7152, 0.0722));
        rgb = vec3(y + 1.5748 * q.y, y - 0.1873 * q.x - 0.4681 * q.y, y + 1.8556 * q.x);
    }
    return vec4(clamp(rgb, 0.0, 1.0), c.a * alpha);
}"#,
            vec![
                color("key", "Key Color", [0.0, 0.69, 0.25, 1.0]),
                float("tolerance", "Tolerance", 0.0, 1.0, 0.3),
                float("softness", "Softness", 0.0, 1.0, 0.15),
                float("spill", "Spill", 0.0, 1.0, 0.6),
            ],
        ),
        glsl(
            "luma_key",
            "Luma Key",
            Effect,
            "Keying",
            "Makes dark (or, inverted, bright) areas transparent.",
            r#"vec4 effect(vec2 uv) {
    vec4 c = getColor(uv);
    float l = dot(c.rgb, vec3(0.2126, 0.7152, 0.0722));
    float a = smoothstep(threshold, threshold + max(softness, 0.001), l);
    if (invert) a = 1.0 - a;
    return vec4(c.rgb, c.a * a);
}"#,
            vec![
                float("threshold", "Threshold", 0.0, 1.0, 0.15),
                float("softness", "Softness", 0.0, 1.0, 0.1),
                toggle("invert", "Invert", false),
            ],
        ),
        // ---- Framing
        glsl(
            "letterbox",
            "Letterbox",
            Effect,
            "Framing",
            "Bars to a target aspect ratio (2.39 = scope).",
            r#"vec4 effect(vec2 uv) {
    vec4 c = getColor(uv);
    vec2 frag = uv * iResolution;
    float inside;
    if (aspect > iResolution.x / iResolution.y) {
        float h = iResolution.x / aspect;
        inside = clamp(h * 0.5 - abs(frag.y - iResolution.y * 0.5) + 0.5, 0.0, 1.0);
    } else {
        float w = iResolution.y * aspect;
        inside = clamp(w * 0.5 - abs(frag.x - iResolution.x * 0.5) + 0.5, 0.0, 1.0);
    }
    return mix(fill, c, inside);
}"#,
            vec![
                float("aspect", "Aspect", 0.5, 3.0, 2.39),
                color("fill", "Color", [0.0, 0.0, 0.0, 1.0]),
            ],
        ),
    ]
}

/// `moveDir()` (where the incoming shot moves, from a `direction` choice
/// over [`DIRECTIONS`]) and `inside(p)`, for the slide/push family.
macro_rules! slide {
    () => {
        r#"vec2 moveDir() {
    if (direction == 1) return vec2(1.0, 0.0);
    if (direction == 2) return vec2(0.0, 1.0);
    if (direction == 3) return vec2(0.0, -1.0);
    return vec2(-1.0, 0.0);
}
bool inside(vec2 p) { return p.x >= 0.0 && p.y >= 0.0 && p.x <= 1.0 && p.y <= 1.0; }
"#
    };
}

fn transitions() -> Vec<FxAsset> {
    use FxKind::Transition;
    vec![
        // ---- Dissolve
        glsl(
            "cross_dissolve",
            "Cross Dissolve",
            Transition,
            "Dissolve",
            "Blends from one shot to the next.",
            r#"vec4 transition(vec2 uv) {
    return mix(getFromColor(uv), getToColor(uv), progress);
}"#,
            vec![],
        ),
        glsl(
            "dip_to_color",
            "Dip to Color",
            Transition,
            "Dissolve",
            "Fades out to a color, then in to the next shot.",
            r#"vec4 transition(vec2 uv) {
    vec4 a = mix(getFromColor(uv), dip, smoothstep(0.0, 0.5, progress));
    vec4 b = mix(dip, getToColor(uv), smoothstep(0.5, 1.0, progress));
    return progress < 0.5 ? a : b;
}"#,
            vec![color("dip", "Color", [0.0, 0.0, 0.0, 1.0])],
        ),
        glsl(
            "additive_dissolve",
            "Additive Dissolve",
            Transition,
            "Dissolve",
            "Adds the shots together, so the middle runs bright.",
            r#"vec4 transition(vec2 uv) {
    vec4 a = getFromColor(uv);
    vec4 b = getToColor(uv);
    vec3 rgb = a.rgb * a.a * min(1.0, 2.0 * (1.0 - progress)) + b.rgb * b.a * min(1.0, 2.0 * progress);
    float alpha = mix(a.a, b.a, progress);
    return vec4(alpha > 0.0 ? min(rgb / alpha, vec3(1.0)) : vec3(0.0), alpha);
}"#,
            vec![],
        ),
        glsl(
            "blur_dissolve",
            "Blur Dissolve",
            Transition,
            "Dissolve",
            "Defocuses out of one shot and into the next.",
            concat!(
                hash!(),
                premul!(),
                r#"vec4 blurred(vec2 uv, float r, bool to, float spin) {
    if (r < 0.5) return to ? getToColor(uv) : getFromColor(uv);
    vec2 px = 1.0 / iResolution;
    vec4 sum = vec4(0.0);
    float total = 0.0;
    for (int i = 0; i < 40; i++) {
        float f = (float(i) + 0.5) / 40.0;
        float d = r * sqrt(f);
        float th = float(i) * 2.39996323 + spin;
        vec2 q = uv + vec2(cos(th), sin(th)) * d * px;
        float w = exp(-2.0 * f);
        sum += premul(to ? getToColor(q) : getFromColor(q)) * w;
        total += w;
    }
    return unpremul(sum / total);
}
vec4 transition(vec2 uv) {
    float r = radius * sin(3.14159265 * progress);
    float spin = hash12(uv * iResolution) * 6.2831853;
    float m = smoothstep(0.25, 0.75, progress);
    vec4 a = m < 1.0 ? blurred(uv, r, false, spin) : vec4(0.0);
    vec4 b = m > 0.0 ? blurred(uv, r, true, spin) : vec4(0.0);
    return mix(a, b, m);
}"#
            ),
            vec![float("radius", "Blur", 0.0, 100.0, 40.0)],
        ),
        glsl(
            "luma_fade",
            "Luma Fade",
            Transition,
            "Dissolve",
            "Reveals the next shot through the darkest parts first.",
            r#"vec4 transition(vec2 uv) {
    vec4 a = getFromColor(uv);
    float l = dot(a.rgb, vec3(0.2126, 0.7152, 0.0722));
    if (invert) l = 1.0 - l;
    float s = max(softness, 0.0001);
    float edge = progress * (1.0 + 2.0 * s) - s;
    float m = 1.0 - smoothstep(edge - s, edge + s, l);
    return mix(a, getToColor(uv), m);
}"#,
            vec![
                float("softness", "Softness", 0.0, 0.5, 0.1),
                toggle("invert", "Brightest First", false),
            ],
        ),
        // ---- Wipe
        glsl(
            "wipe",
            "Wipe",
            Transition,
            "Wipe",
            "A straight edge sweeps across; 0° goes left to right.",
            r#"vec4 transition(vec2 uv) {
    float a = radians(angle);
    vec2 dir = vec2(cos(a), sin(a));
    vec2 p = (uv - 0.5) * vec2(ratio, 1.0);
    float extent = abs(dir.x) * ratio + abs(dir.y);
    float t = dot(p, dir) / extent + 0.5;
    float s = max(softness, 0.0001);
    float edge = progress * (1.0 + 2.0 * s) - s;
    float m = 1.0 - smoothstep(edge - s, edge + s, t);
    return mix(getFromColor(uv), getToColor(uv), m);
}"#,
            vec![
                float("angle", "Angle", -180.0, 180.0, 0.0),
                float("softness", "Softness", 0.0, 0.5, 0.05),
            ],
        ),
        glsl(
            "clock_wipe",
            "Clock Wipe",
            Transition,
            "Wipe",
            "A sweeping hand reveals the next shot.",
            r#"vec4 transition(vec2 uv) {
    vec2 p = (uv - center) * vec2(ratio, 1.0);
    float a = atan(p.x, p.y) / 6.2831853;
    if (!clockwise) a = -a;
    a = fract(a - start / 360.0);
    float s = max(softness, 0.0001);
    float edge = progress * (1.0 + 2.0 * s) - s;
    float m = 1.0 - smoothstep(edge - s, edge + s, a);
    return mix(getFromColor(uv), getToColor(uv), m);
}"#,
            vec![
                point("center", "Center", [0.5, 0.5]),
                float("start", "Start Angle", -180.0, 180.0, 0.0),
                float("softness", "Softness", 0.0, 0.2, 0.01),
                toggle("clockwise", "Clockwise", true),
            ],
        ),
        glsl(
            "iris",
            "Iris",
            Transition,
            "Wipe",
            "A growing circle, diamond or square opens on the next shot.",
            r#"float dist(vec2 p) {
    if (shape == 1) return abs(p.x) + abs(p.y);
    if (shape == 2) return max(abs(p.x), abs(p.y));
    return length(p);
}
vec4 transition(vec2 uv) {
    vec2 asp = vec2(ratio, 1.0);
    float far = max(max(dist((vec2(0.0, 0.0) - center) * asp), dist((vec2(1.0, 0.0) - center) * asp)),
                    max(dist((vec2(0.0, 1.0) - center) * asp), dist((vec2(1.0, 1.0) - center) * asp)));
    float s = max(softness, 0.0001);
    float r = progress * (far + s);
    float m = 1.0 - smoothstep(r - s, r, dist((uv - center) * asp));
    return mix(getFromColor(uv), getToColor(uv), m);
}"#,
            vec![
                point("center", "Center", [0.5, 0.5]),
                choice("shape", "Shape", &["Circle", "Diamond", "Square"], 0),
                float("softness", "Softness", 0.0, 0.5, 0.02),
            ],
        ),
        // ---- Motion
        glsl(
            "slide",
            "Slide",
            Transition,
            "Motion",
            "The next shot slides in over the current one.",
            concat!(
                r#"float ease(float t) { return t * t * (3.0 - 2.0 * t); }
"#,
                slide!(),
                r#"vec4 transition(vec2 uv) {
    vec2 q = uv + moveDir() * (1.0 - ease(progress));
    return inside(q) ? getToColor(q) : getFromColor(uv);
}"#
            ),
            vec![choice("direction", "Direction", DIRECTIONS, 0)],
        ),
        glsl(
            "push",
            "Push",
            Transition,
            "Motion",
            "The next shot pushes the current one out of frame.",
            concat!(
                r#"float ease(float t) { return t * t * (3.0 - 2.0 * t); }
"#,
                slide!(),
                r#"vec4 transition(vec2 uv) {
    vec2 d = moveDir();
    float s = ease(progress);
    vec2 q = uv + d * (1.0 - s);
    return inside(q) ? getToColor(q) : getFromColor(uv - d * s);
}"#
            ),
            vec![choice("direction", "Direction", DIRECTIONS, 0)],
        ),
        glsl(
            "whip_pan",
            "Whip Pan",
            Transition,
            "Motion",
            "A fast motion-blurred push, like a whipped camera.",
            concat!(
                premul!(),
                slide!(),
                r#"vec4 pushed(vec2 uv, vec2 d, float s) {
    vec2 q = uv + d * (1.0 - s);
    return inside(q) ? getToColor(q) : getFromColor(uv - d * s);
}
vec4 transition(vec2 uv) {
    float p = progress;
    // Cubic ease in-out and its speed.
    float s = p < 0.5 ? 4.0 * p * p * p : 1.0 - pow(2.0 - 2.0 * p, 3.0) * 0.5;
    float v = p < 0.5 ? 12.0 * p * p : 3.0 * pow(2.0 - 2.0 * p, 2.0);
    vec2 d = moveDir();
    float len = v * blur * 0.12;
    vec4 sum = vec4(0.0);
    for (int i = 0; i < 32; i++) {
        float k = ((float(i) + 0.5) / 32.0 - 0.5) * len;
        sum += premul(pushed(uv, d, clamp(s + k, 0.0, 1.0)));
    }
    return unpremul(sum / 32.0);
}"#
            ),
            vec![
                choice("direction", "Direction", DIRECTIONS, 0),
                float("blur", "Motion Blur", 0.0, 1.0, 0.6),
            ],
        ),
        glsl(
            "zoom_in",
            "Zoom In",
            Transition,
            "Motion",
            "Rushes into the current shot and lands on the next.",
            concat!(
                premul!(),
                r#"vec4 zoomed(vec2 uv, float scale, float streak, bool to) {
    vec4 sum = vec4(0.0);
    for (int i = 0; i < 16; i++) {
        float s = scale * (1.0 + streak * float(i) / 15.0);
        vec2 q = (uv - 0.5) / s + 0.5;
        sum += premul(to ? getToColor(q) : getFromColor(q));
    }
    return unpremul(sum / 16.0);
}
vec4 transition(vec2 uv) {
    float p = progress;
    float streak = strength * 0.12 * sin(3.14159265 * p);
    float m = smoothstep(0.4, 0.6, p);
    vec4 a = m < 1.0 ? zoomed(uv, 1.0 + strength * p * p * 2.0, streak, false) : vec4(0.0);
    vec4 b = m > 0.0 ? zoomed(uv, 1.0 + strength * (1.0 - p) * (1.0 - p) * 0.8, streak, true) : vec4(0.0);
    return mix(a, b, m);
}"#
            ),
            vec![float("strength", "Strength", 0.1, 3.0, 1.0)],
        ),
        // ---- Stylize
        glsl(
            "pixelize",
            "Pixelize",
            Transition,
            "Stylize",
            "Breaks into blocks and rebuilds as the next shot.",
            r#"vec4 transition(vec2 uv) {
    float d = 1.0 - abs(progress * 2.0 - 1.0);
    float cell = max(1.0, floor(size * d * d));
    vec2 p = uv;
    if (cell > 1.0) {
        vec2 c = vec2(cell) / iResolution;
        p = (floor((uv - 0.5) / c) + 0.5) * c + 0.5;
    }
    return mix(getFromColor(p), getToColor(p), smoothstep(0.4, 0.6, progress));
}"#,
            vec![float("size", "Max Block", 4.0, 200.0, 60.0)],
        ),
        glsl(
            "glitch_transition",
            "Glitch",
            Transition,
            "Stylize",
            "Digital tearing and RGB split while tiles flip to the next shot.",
            concat!(
                hash!(),
                r#"vec4 pick(vec2 p, bool to) { return to ? getToColor(p) : getFromColor(p); }
vec4 transition(vec2 uv) {
    float env = sin(3.14159265 * progress) * amount;
    float t = floor(iTime * 20.0);
    float rows = 8.0 + 24.0 * hash12(vec2(t, 3.0));
    float rowId = floor(uv.y * rows);
    float on = step(1.0 - env * 0.8, hash12(vec2(rowId, t)));
    float shift = (hash12(vec2(rowId, t + 9.0)) - 0.5) * 0.3 * env * on;
    vec2 p = vec2(fract(uv.x + shift), uv.y);
    vec2 tile = floor(uv * vec2(16.0, 9.0));
    float flip = 0.25 + 0.5 * hash12(tile + 0.37);
    flip += (hash12(tile + t) - 0.5) * 0.2 * env;
    bool to = progress > flip;
    float split = 0.04 * env;
    vec4 c = pick(p, to);
    vec3 col = vec3(pick(p + vec2(split, 0.0), to).r, c.g, pick(p - vec2(split, 0.0), to).b);
    return vec4(col, c.a);
}"#
            ),
            vec![float("amount", "Amount", 0.0, 1.0, 0.7)],
        ),
        glsl(
            "swirl",
            "Swirl",
            Transition,
            "Stylize",
            "Twists into a vortex and unwinds on the next shot.",
            r#"vec4 transition(vec2 uv) {
    vec2 p = (uv - 0.5) * vec2(ratio, 1.0);
    float f = max(0.0, 1.0 - length(p) / radius);
    float a = strength * sin(3.14159265 * progress) * f * f;
    float c = cos(a);
    float s = sin(a);
    vec2 q = vec2(c * p.x - s * p.y, s * p.x + c * p.y) / vec2(ratio, 1.0) + 0.5;
    return mix(getFromColor(q), getToColor(q), smoothstep(0.3, 0.7, progress));
}"#,
            vec![
                float("strength", "Strength", -20.0, 20.0, 6.0),
                float("radius", "Radius", 0.1, 2.0, 1.0),
            ],
        ),
        glsl(
            "ripple",
            "Ripple",
            Transition,
            "Stylize",
            "Water ripples spread out while the shots blend.",
            r#"vec4 transition(vec2 uv) {
    vec2 p = (uv - 0.5) * vec2(ratio, 1.0);
    float r = length(p);
    vec2 dir = r > 0.0 ? p / r : vec2(0.0);
    float env = sin(3.14159265 * progress);
    float off = sin(r * frequency - progress * speed) * amplitude * env;
    vec2 q = uv + dir * off / vec2(ratio, 1.0);
    return mix(getFromColor(q), getToColor(q), smoothstep(0.2, 0.8, progress));
}"#,
            vec![
                float("amplitude", "Amplitude", 0.0, 0.2, 0.03),
                float("frequency", "Frequency", 1.0, 80.0, 30.0),
                float("speed", "Speed", 0.0, 60.0, 20.0),
            ],
        ),
        glsl(
            "film_burn",
            "Film Burn",
            Transition,
            "Stylize",
            "A hot light leak flares across and burns through to the next shot.",
            concat!(
                noise!(),
                r#"vec4 transition(vec2 uv) {
    vec2 p = uv * vec2(ratio, 1.0);
    float env = sin(3.14159265 * progress);
    vec2 hot = vec2(mix(0.15, 0.85, progress) * ratio, 0.45 + 0.2 * sin(progress * 3.0 + seed));
    float n = fbm(p * 3.0 + vec2(progress * 2.0, seed));
    float burn = clamp(env * 1.3 - length(p - hot) * 1.3 + (n - 0.5) * 0.9 * env, 0.0, 1.0);
    vec4 base = mix(getFromColor(uv), getToColor(uv), smoothstep(0.35, 0.65, progress));
    vec3 heat = mix(tint.rgb, vec3(1.0, 0.97, 0.9), smoothstep(0.5, 1.0, burn));
    vec3 rgb = 1.0 - (1.0 - base.rgb) * (1.0 - heat * burn);
    rgb = mix(rgb, vec3(1.0), smoothstep(0.85, 1.0, burn) * env);
    return vec4(rgb, max(base.a, burn));
}"#
            ),
            vec![
                color("tint", "Color", [1.0, 0.45, 0.1, 1.0]),
                float("seed", "Seed", 0.0, 100.0, 0.0),
            ],
        ),
    ]
}

fn generators() -> Vec<FxAsset> {
    use FxKind::Generator;
    vec![
        glsl(
            "solid_color",
            "Solid Color",
            Generator,
            "Backgrounds",
            "A flat color.",
            r#"vec4 generate(vec2 uv) {
    return fill;
}"#,
            vec![color("fill", "Color", [0.08, 0.08, 0.1, 1.0])],
        ),
        glsl(
            "linear_gradient",
            "Linear Gradient",
            Generator,
            "Backgrounds",
            "Two-color gradient at an angle.",
            concat!(
                hash!(),
                r#"vec4 generate(vec2 uv) {
    float a = radians(angle);
    vec2 dir = vec2(cos(a), sin(a));
    float t = clamp(dot(uv - 0.5, dir) + 0.5, 0.0, 1.0);
    float dither = (hash12(uv * iResolution) - 0.5) / 255.0;
    return mix(from_color, to_color, t) + vec4(vec3(dither), 0.0);
}"#
            ),
            vec![
                color("from_color", "From", [0.11, 0.13, 0.35, 1.0]),
                color("to_color", "To", [0.55, 0.18, 0.45, 1.0]),
                float("angle", "Angle", -180.0, 180.0, 90.0),
            ],
        ),
        glsl(
            "radial_gradient",
            "Radial Gradient",
            Generator,
            "Backgrounds",
            "Two-color gradient out from a center.",
            concat!(
                hash!(),
                r#"vec4 generate(vec2 uv) {
    vec2 p = (uv - center) * vec2(iResolution.x / iResolution.y, 1.0);
    float t = clamp(length(p) / radius, 0.0, 1.0);
    float dither = (hash12(uv * iResolution) - 0.5) / 255.0;
    return mix(inner, outer, t) + vec4(vec3(dither), 0.0);
}"#
            ),
            vec![
                color("inner", "Inner", [0.95, 0.6, 0.35, 1.0]),
                color("outer", "Outer", [0.12, 0.06, 0.2, 1.0]),
                point("center", "Center", [0.5, 0.5]),
                float("radius", "Radius", 0.05, 2.0, 0.9),
            ],
        ),
        glsl(
            "animated_gradient",
            "Animated Gradient",
            Generator,
            "Backgrounds",
            "Four colors drifting into each other, like a mesh gradient.",
            concat!(
                hash!(),
                r#"vec4 generate(vec2 uv) {
    float aspect = iResolution.x / iResolution.y;
    float t = iTime * speed;
    vec2 p = uv * vec2(aspect, 1.0);
    p += 0.08 * vec2(sin(p.y * 3.0 + t * 1.3), cos(p.x * 3.0 - t * 1.1));
    vec2 a = vec2(0.25 + 0.2 * sin(t * 0.9), 0.3 + 0.2 * cos(t * 1.1)) * vec2(aspect, 1.0);
    vec2 b = vec2(0.75 + 0.2 * cos(t * 0.7), 0.3 + 0.2 * sin(t * 1.3)) * vec2(aspect, 1.0);
    vec2 c = vec2(0.3 + 0.2 * cos(t * 1.2 + 1.0), 0.75 + 0.2 * sin(t * 0.8)) * vec2(aspect, 1.0);
    vec2 d = vec2(0.7 + 0.2 * sin(t * 1.0 + 2.0), 0.7 + 0.2 * cos(t * 0.6 + 1.0)) * vec2(aspect, 1.0);
    float wa = 1.0 / (pow(length(p - a), 2.5) + 0.01);
    float wb = 1.0 / (pow(length(p - b), 2.5) + 0.01);
    float wc = 1.0 / (pow(length(p - c), 2.5) + 0.01);
    float wd = 1.0 / (pow(length(p - d), 2.5) + 0.01);
    vec4 col = (color_a * wa + color_b * wb + color_c * wc + color_d * wd) / (wa + wb + wc + wd);
    float dither = (hash12(uv * iResolution) - 0.5) / 255.0;
    return col + vec4(vec3(dither), 0.0);
}"#
            ),
            vec![
                color("color_a", "Color 1", [0.98, 0.42, 0.45, 1.0]),
                color("color_b", "Color 2", [0.35, 0.3, 0.95, 1.0]),
                color("color_c", "Color 3", [0.1, 0.75, 0.8, 1.0]),
                color("color_d", "Color 4", [0.99, 0.8, 0.35, 1.0]),
                float("speed", "Speed", 0.0, 3.0, 0.4),
            ],
        ),
        glsl(
            "noise",
            "Noise",
            Generator,
            "Patterns",
            "Animated fractal noise (clouds, smoke, textures).",
            concat!(
                hash!(),
                r#"float vnoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash12(i), hash12(i + vec2(1.0, 0.0)), u.x),
               mix(hash12(i + vec2(0.0, 1.0)), hash12(i + vec2(1.0, 1.0)), u.x), u.y);
}
float fbm(vec2 p) {
    float s = 0.0;
    float a = 0.5;
    float total = 0.0;
    for (int i = 0; i < 8; i++) {
        if (i >= detail) break;
        s += a * vnoise(p);
        total += a;
        p = p * 2.03 + vec2(17.1, 9.2);
        a *= 0.5;
    }
    return s / max(total, 0.001);
}
vec4 generate(vec2 uv) {
    vec2 p = uv * vec2(iResolution.x / iResolution.y, 1.0) * scale;
    float t = iTime * speed;
    vec2 q = vec2(fbm(p + vec2(0.0, t)), fbm(p + vec2(5.2, 1.3 - t)));
    float n = fbm(p + 1.5 * q + vec2(t * 0.5, 0.0));
    n = clamp((n - 0.5) * contrast + 0.5, 0.0, 1.0);
    return mix(color_a, color_b, n);
}"#
            ),
            vec![
                color("color_a", "Dark", [0.0, 0.0, 0.0, 1.0]),
                color("color_b", "Light", [1.0, 1.0, 1.0, 1.0]),
                float("scale", "Scale", 0.5, 50.0, 4.0),
                float("speed", "Speed", 0.0, 5.0, 0.3),
                int("detail", "Detail", 1, 8, 5),
                float("contrast", "Contrast", 0.0, 4.0, 1.5),
            ],
        ),
        glsl(
            "grid",
            "Grid",
            Generator,
            "Patterns",
            "Checkerboard, grid lines or dots, optionally scrolling.",
            r#"vec4 generate(vec2 uv) {
    vec2 frag = uv * iResolution - iResolution * 0.5 + vec2(iTime * speed);
    vec2 cell = frag / size;
    float m;
    if (pattern == 0) {
        vec2 f = floor(cell);
        m = mod(f.x + f.y, 2.0);
    } else if (pattern == 1) {
        vec2 g = abs(fract(cell + 0.5) - 0.5) * size;
        float d = min(g.x, g.y);
        m = 1.0 - smoothstep(width * 0.5 - 0.5, width * 0.5 + 0.5, d);
    } else {
        float d = length((fract(cell) - 0.5) * size);
        m = 1.0 - smoothstep(width - 0.5, width + 0.5, d);
    }
    return mix(color_a, color_b, m);
}"#,
            vec![
                choice("pattern", "Pattern", &["Checker", "Lines", "Dots"], 0),
                float("size", "Size", 4.0, 400.0, 80.0),
                float("width", "Line Width", 0.5, 60.0, 2.0),
                color("color_a", "Background", [0.1, 0.1, 0.12, 1.0]),
                color("color_b", "Foreground", [0.85, 0.85, 0.88, 1.0]),
                float("speed", "Scroll", 0.0, 400.0, 0.0),
            ],
        ),
        glsl(
            "light_leaks",
            "Light Leaks",
            Generator,
            "Light",
            "Warm drifting light leaks; overlay them with a screen or add blend.",
            concat!(
                noise!(),
                r#"vec4 generate(vec2 uv) {
    float aspect = iResolution.x / iResolution.y;
    vec2 p = uv * vec2(aspect, 1.0);
    float t = iTime * speed;
    vec3 col = vec3(0.0);
    for (int i = 0; i < 4; i++) {
        float fi = float(i);
        vec2 c = vec2(aspect * (0.5 + 0.6 * sin(t * (0.7 + 0.13 * fi) + fi * 2.1)),
                      0.5 + 0.6 * cos(t * (0.5 + 0.11 * fi) + fi * 1.3));
        float r = 0.35 + 0.15 * sin(t * 0.9 + fi);
        float d = length(p - c);
        col += mix(color_a.rgb, color_b.rgb, 0.5 + 0.5 * sin(fi * 1.7 + t)) * exp(-d * d / (r * r));
    }
    col *= (0.6 + 0.8 * fbm(p * 2.0 + t * 0.3)) * intensity;
    col = 1.0 - exp(-col * 1.5);
    if (overlay) {
        float a = max(col.r, max(col.g, col.b));
        return a > 0.0 ? vec4(col / a, a) : vec4(0.0);
    }
    return vec4(col, 1.0);
}"#
            ),
            vec![
                color("color_a", "Color 1", [1.0, 0.35, 0.08, 1.0]),
                color("color_b", "Color 2", [1.0, 0.8, 0.35, 1.0]),
                float("intensity", "Intensity", 0.0, 3.0, 1.0),
                float("speed", "Speed", 0.0, 3.0, 0.4),
                toggle("overlay", "Transparent", false),
            ],
        ),
    ]
}
