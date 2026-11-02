//! Built-in effects that read the picture's depth map (`getDepth`, see
//! [`crate::depth`]): a lens blur that keeps one distance sharp, fog that
//! thickens with distance, and a 2.5D camera move for stills and short
//! clips. Distance is 0 nearest to 1 farthest.

use super::{FxAsset, FxKind, FxLanguage, FxRef, ParamKind, ParamSpec, ParamValue, color, float};

/// Keys of the built-ins here.
pub const LENS_BLUR: &str = "depth_blur";
pub const FOG: &str = "depth_fog";
pub const PARALLAX: &str = "depth_parallax";

/// The parameter of [`LENS_BLUR`] a click in the viewer sets.
pub const FOCUS: &str = "focus";

/// The camera moves of [`PARALLAX`], in the order its `motion` lists them.
pub const MOTIONS: [&str; 4] = ["Dolly", "Zoom", "Orbit", "Pan"];

fn effect(key: &str, name: &str, description: &str, code: &str, params: Vec<ParamSpec>) -> FxAsset {
    FxAsset {
        id: FxRef::builtin(key),
        name: name.into(),
        kind: FxKind::Effect,
        language: FxLanguage::Glsl,
        category: "Depth".into(),
        description: description.into(),
        code: code.into(),
        params,
        origin: None,
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

const LENS_BLUR_GLSL: &str = r#"float hash12(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}
vec4 premul(vec4 c) { return vec4(c.rgb * c.a, c.a); }
vec4 unpremul(vec4 c) { return c.a > 0.00001 ? vec4(c.rgb / c.a, c.a) : vec4(0.0); }
// Blur radius in pixels at distance d: none within the focus range, full
// blur 0.3 past it.
float coc(float d) {
    float off = max(abs(d - focus) - range * 0.5, 0.0);
    return blur * clamp(off / 0.3, 0.0, 1.0);
}
vec4 effect(vec2 uv) {
    if (blur < 0.5) return getColor(uv);
    vec2 px = 1.0 / iResolution;
    float dc = getDepth(uv);
    float cc = coc(dc);
    float spin = hash12(uv * iResolution) * 6.2831853;
    vec4 sum = premul(getColor(uv));
    float total = 1.0;
    // Two spirals of taps: one over this pixel's own blur, one over the
    // widest blur, for a blurred foreground spilling over it.
    for (int ring = 0; ring < 2; ring++) {
        float reach = ring == 0 ? cc : blur;
        if (reach < 0.5) continue;
        for (int i = 1; i <= 48; i++) {
            float r = reach * sqrt(float(i) / 48.0);
            float a = float(i) * 2.39996323 + spin;
            vec2 suv = uv + vec2(cos(a), sin(a)) * r * px;
            float ds = getDepth(suv);
            float cs = coc(ds);
            // What lies behind this pixel blurs no further over it than
            // the pixel itself does.
            if (ds > dc) cs = min(cs, cc);
            float w = clamp(cs - r + 1.0, 0.0, 1.0);
            sum += premul(getColor(suv)) * w;
            total += w;
        }
    }
    return unpremul(sum / total);
}"#;

const FOG_GLSL: &str = r#"vec4 effect(vec2 uv) {
    vec4 c = getColor(uv);
    float x = max(getDepth(uv) - start, 0.0) / max(1.0 - start, 0.001);
    float f = (1.0 - exp(-density * 4.0 * x)) * tint.a;
    return vec4(mix(c.rgb, tint.rgb, f), c.a);
}"#;

/// Layers of distance searched from the front for the surface a pixel
/// shows, then refined; the first one found is the nearest, so what is in
/// front covers what is behind and the gaps it uncovers are filled by the
/// edge beside them.
const PARALLAX_GLSL: &str = r#"float ease(float t) { return t * t * (3.0 - 2.0 * t); }
float aspect() { return iResolution.x / iResolution.y; }
vec2 heading() { float a = radians(angle); return vec2(cos(a), sin(a)); }
// How the picture at distance z moves at eased time e: magnification about
// the centre, and a shift in picture heights.
vec3 camera(float z, float e) {
    if (motion == 0) return vec3(max(1.0 + 0.35 * amount * e * (1.0 - z), 0.2), 0.0, 0.0);
    if (motion == 1) return vec3(max(1.0 + 0.5 * amount * e * (focus - z), 0.2), 0.0, 0.0);
    if (motion == 2) {
        float th = radians(angle) + iProgress * 6.2831853;
        return vec3(1.0, 0.04 * amount * (focus - z) * vec2(cos(th), sin(th)));
    }
    return vec3(1.0, 0.16 * amount * (e - 0.5) * (focus - z) * heading());
}
// The zoom that keeps the frame's edges covered all through the move.
float overscan() {
    float m = 1.0;
    float shift = 0.0;
    if (motion == 0) m = 1.0 + 0.35 * min(amount, 0.0);
    else if (motion == 1) m = 1.0 - 0.5 * abs(amount) * max(focus, 1.0 - focus);
    else if (motion == 2) shift = 0.04 * abs(amount) * max(focus, 1.0 - focus);
    else shift = 0.08 * abs(amount) * max(focus, 1.0 - focus);
    return max(1.0 / max(m, 0.2), 1.0) * (1.0 + 2.0 * shift);
}
// Where the picture at distance z lands on uv comes from.
vec2 source(vec2 uv, float z, float e, float base) {
    vec3 c = camera(z, e);
    vec2 shift = vec2(c.y / aspect(), c.z);
    return 0.5 + (uv - 0.5 - shift) / (c.x * base);
}
vec4 effect(vec2 uv) {
    float e = ease(clamp(iProgress, 0.0, 1.0));
    float base = overscan();
    float lo = 0.0;
    float hi = 1.0;
    for (int i = 0; i <= 48; i++) {
        float z = float(i) / 48.0;
        if (getDepth(source(uv, z, e, base)) <= z) {
            hi = z;
            break;
        }
        lo = z;
    }
    for (int k = 0; k < 6; k++) {
        float mid = 0.5 * (lo + hi);
        if (getDepth(source(uv, mid, e, base)) <= mid) hi = mid; else lo = mid;
    }
    return getColor(source(uv, hi, e, base));
}"#;

pub(super) fn all() -> Vec<FxAsset> {
    vec![
        effect(
            LENS_BLUR,
            "Lens Blur",
            "Blurs by depth, keeping `focus` (0 near, 1 far) sharp; key `focus` for a rack focus.",
            LENS_BLUR_GLSL,
            vec![
                float(FOCUS, "Focus", 0.0, 1.0, 0.25),
                float("range", "Range", 0.0, 1.0, 0.1),
                float("blur", "Blur", 0.0, 60.0, 16.0),
            ],
        ),
        effect(
            FOG,
            "Depth Fog",
            "Haze thickening with distance past `start` (0 near, 1 far).",
            FOG_GLSL,
            vec![
                color("tint", "Color", [0.78, 0.82, 0.88, 1.0]),
                float("density", "Density", 0.0, 1.0, 0.6),
                float("start", "Start", 0.0, 1.0, 0.3),
            ],
        ),
        effect(
            PARALLAX,
            "3D Camera",
            "A camera move over the clip with depth parallax (dolly, dolly zoom, orbit, pan): the 3D Ken Burns look for stills and short clips.",
            PARALLAX_GLSL,
            vec![
                choice("motion", "Motion", &MOTIONS, 0),
                float("amount", "Amount", -1.0, 1.0, 0.5),
                float(FOCUS, "Focus", 0.0, 1.0, 0.5),
                float("angle", "Angle", -180.0, 180.0, 0.0),
            ],
        ),
    ]
}
