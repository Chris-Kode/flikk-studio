//! Built-in GLSL assets: effects, transitions and generators. See the module
//! docs of [`super`] for the conventions every shader follows.

use super::{FxAsset, FxKind, FxLanguage, FxRef, ParamSpec, color, float};

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
    }
}

pub(super) fn all() -> Vec<FxAsset> {
    use FxKind::*;
    vec![
        // ---- effects
        glsl(
            "gaussian_blur",
            "Gaussian Blur",
            Effect,
            "Blur",
            "Soft blur; radius in pixels at the output size.",
            r#"vec4 effect(vec2 uv) {
    vec2 px = 1.0 / iResolution;
    float sigma = max(radius / 2.0, 0.001);
    vec4 sum = vec4(0.0);
    float total = 0.0;
    for (int x = -6; x <= 6; x++) {
        for (int y = -6; y <= 6; y++) {
            vec2 o = vec2(float(x), float(y)) * radius / 6.0;
            float w = exp(-dot(o, o) / (2.0 * sigma * sigma));
            sum += getColor(uv + o * px) * w;
            total += w;
        }
    }
    return sum / total;
}"#,
            vec![float("radius", "Radius", 0.0, 100.0, 8.0)],
        ),
        glsl(
            "vignette",
            "Vignette",
            Effect,
            "Stylize",
            "Darkens the corners.",
            r#"vec4 effect(vec2 uv) {
    vec4 c = getColor(uv);
    vec2 d = (uv - 0.5) * vec2(iResolution.x / iResolution.y, 1.0);
    float v = smoothstep(size, size - softness, length(d));
    return vec4(mix(c.rgb * (1.0 - amount), c.rgb, v), c.a);
}"#,
            vec![
                float("amount", "Amount", 0.0, 1.0, 0.6),
                float("size", "Size", 0.1, 1.5, 0.8),
                float("softness", "Softness", 0.01, 1.0, 0.45),
            ],
        ),
        // ---- transitions
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
        // ---- generators
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
            r#"vec4 generate(vec2 uv) {
    float a = radians(angle);
    vec2 dir = vec2(cos(a), sin(a));
    float t = clamp(dot(uv - 0.5, dir) + 0.5, 0.0, 1.0);
    return mix(from_color, to_color, t);
}"#,
            vec![
                color("from_color", "From", [0.11, 0.13, 0.35, 1.0]),
                color("to_color", "To", [0.55, 0.18, 0.45, 1.0]),
                float("angle", "Angle", -180.0, 180.0, 90.0),
            ],
        ),
    ]
}
