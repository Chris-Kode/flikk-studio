//! The GLSL the engine actually compiles: a `#version 450` prelude (uniform
//! block, samplers, helpers), the author's code, then `main()`. Also packs
//! parameter values into the uniform block and maps compile errors back to
//! the author's lines.

use koo_core::fx::{FxAsset, FxKind, ParamKind, ParamValue};
use koo_core::services::FxCall;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};

/// Bindings of the one bind group every program uses.
pub const BIND_UNIFORMS: u32 = 0;
pub const BIND_SAMPLER: u32 = 1;
/// Effect input, or a transition's outgoing picture.
pub const BIND_TEX0: u32 = 2;
/// A transition's incoming picture.
pub const BIND_TEX1: u32 = 3;
/// First Image parameter; the rest follow in declaration order, then the
/// depth map of an effect that reads it (`getDepth`).
pub const BIND_IMAGES: u32 = 4;

/// Names the prelude defines; parameters may not reuse them.
const RESERVED: &[&str] = &[
    "iResolution",
    "iTime",
    "iDuration",
    "iProgress",
    "progress",
    "ratio",
    "getColor",
    "getFromColor",
    "getToColor",
    "getDepth",
    "effect",
    "transition",
    "generate",
    "main",
    "uv",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Float,
    Int,
    Vec2,
    Vec4,
}

impl Slot {
    fn glsl(self) -> &'static str {
        match self {
            Slot::Float => "float",
            Slot::Int => "int",
            Slot::Vec2 => "vec2",
            Slot::Vec4 => "vec4",
        }
    }

    /// std140 alignment and size of the scalar and vector types used here.
    fn align_size(self) -> (usize, usize) {
        match self {
            Slot::Float | Slot::Int => (4, 4),
            Slot::Vec2 => (8, 8),
            Slot::Vec4 => (16, 16),
        }
    }
}

/// One parameter's place in the uniform block.
#[derive(Clone, Debug)]
struct Field {
    param: usize,
    slot: Slot,
    offset: usize,
}

/// A program's source and the facts the engine needs to feed it.
#[derive(Clone, Debug)]
pub struct Wrapped {
    pub source: String,
    pub kind: FxKind,
    /// Lines before the author's first line.
    prelude_lines: usize,
    /// Lines of the author's code.
    author_lines: usize,
    /// Prelude line (1-based) → the parameter it declares.
    param_lines: BTreeMap<usize, String>,
    fields: Vec<Field>,
    /// Offset of `_koo_target`, the render target's size in pixels.
    target_offset: usize,
    /// Size of the uniform block, a multiple of 16.
    pub uniform_size: usize,
    /// Image parameters, in binding order.
    pub images: Vec<String>,
    /// The binding of the depth map, when the program reads it.
    pub depth: Option<u32>,
}

/// Hash of everything the generated source depends on: the code, the kind
/// and the parameter layout (names and GLSL types).
pub fn program_key(asset: &FxAsset) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    asset.code.hash(&mut h);
    asset.kind.hash(&mut h);
    for p in &asset.params {
        p.name.hash(&mut h);
        std::mem::discriminant(&p.kind).hash(&mut h);
    }
    h.finish()
}

fn slot(kind: &ParamKind) -> Option<Slot> {
    match kind {
        ParamKind::Float { .. } => Some(Slot::Float),
        ParamKind::Int { .. } | ParamKind::Bool | ParamKind::Choice { .. } => Some(Slot::Int),
        ParamKind::Color => Some(Slot::Vec4),
        ParamKind::Point => Some(Slot::Vec2),
        ParamKind::Text | ParamKind::Image => None,
    }
}

fn align(offset: usize, to: usize) -> usize {
    offset.div_ceil(to) * to
}

/// Generate the full source for `asset`, or explain why its parameters
/// cannot be declared.
pub fn wrap(asset: &FxAsset) -> Result<Wrapped, String> {
    for p in &asset.params {
        if RESERVED.contains(&p.name.as_str()) || p.name.starts_with("gl_") || p.name.contains("__")
        {
            return Err(format!(
                "parameter `{}` clashes with a name the engine defines; rename it",
                p.name
            ));
        }
        if p.name.starts_with("_koo") {
            return Err(format!(
                "parameter `{}`: names starting with `_koo` are reserved",
                p.name
            ));
        }
    }

    let mut src = String::new();
    let mut param_lines = BTreeMap::new();
    let line = |src: &String| src.matches('\n').count() + 1;

    src.push_str("#version 450\nprecision highp float;\n");
    src.push_str(&format!(
        "layout(set = 0, binding = {BIND_UNIFORMS}, std140) uniform _KooUniforms {{\n"
    ));
    let mut offset = 0usize;
    let push = |src: &mut String, offset: &mut usize, ty: Slot, name: &str| {
        let (a, s) = ty.align_size();
        *offset = align(*offset, a);
        let at = *offset;
        *offset += s;
        let _ = writeln!(src, "    {} {};", ty.glsl(), name);
        at
    };
    push(&mut src, &mut offset, Slot::Vec2, "iResolution");
    push(&mut src, &mut offset, Slot::Float, "iTime");
    push(&mut src, &mut offset, Slot::Float, "iDuration");
    push(&mut src, &mut offset, Slot::Float, "iProgress");
    if asset.kind == FxKind::Transition {
        push(&mut src, &mut offset, Slot::Float, "progress");
        push(&mut src, &mut offset, Slot::Float, "ratio");
    }
    let mut fields = Vec::new();
    let mut bools = Vec::new();
    for (i, p) in asset.params.iter().enumerate() {
        let Some(ty) = slot(&p.kind) else { continue };
        let name = if matches!(p.kind, ParamKind::Bool) {
            bools.push(p.name.clone());
            format!("_koo_bool_{}", p.name)
        } else {
            p.name.clone()
        };
        param_lines.insert(line(&src), p.name.clone());
        let at = push(&mut src, &mut offset, ty, &name);
        fields.push(Field {
            param: i,
            slot: ty,
            offset: at,
        });
    }
    let target_offset = push(&mut src, &mut offset, Slot::Vec2, "_koo_target");
    src.push_str("};\n");
    let uniform_size = align(offset.max(16), 16);
    for b in &bools {
        param_lines.insert(line(&src), b.clone());
        let _ = writeln!(src, "#define {b} (_koo_bool_{b} != 0)");
    }

    let _ = writeln!(
        src,
        "layout(set = 0, binding = {BIND_SAMPLER}) uniform sampler _koo_sampler;"
    );
    // Textures are stored top row first; uv has its origin at the bottom-left.
    let sample = |src: &mut String, name: &str, tex: &str| {
        let _ = writeln!(
            src,
            "vec4 {name}(vec2 uv) {{ return texture(sampler2D({tex}, _koo_sampler), vec2(uv.x, 1.0 - uv.y)); }}"
        );
    };
    match asset.kind {
        FxKind::Effect => {
            let _ = writeln!(
                src,
                "layout(set = 0, binding = {BIND_TEX0}) uniform texture2D _koo_tex0;"
            );
            sample(&mut src, "getColor", "_koo_tex0");
        }
        FxKind::Transition => {
            let _ = writeln!(
                src,
                "layout(set = 0, binding = {BIND_TEX0}) uniform texture2D _koo_tex0;"
            );
            let _ = writeln!(
                src,
                "layout(set = 0, binding = {BIND_TEX1}) uniform texture2D _koo_tex1;"
            );
            sample(&mut src, "getFromColor", "_koo_tex0");
            sample(&mut src, "getToColor", "_koo_tex1");
        }
        FxKind::Generator => {}
    }
    let mut images = Vec::new();
    for p in &asset.params {
        if !matches!(p.kind, ParamKind::Image) {
            continue;
        }
        let binding = BIND_IMAGES as usize + images.len();
        param_lines.insert(line(&src), p.name.clone());
        let _ = writeln!(
            src,
            "layout(set = 0, binding = {binding}) uniform texture2D _koo_img_{0};\n\
             vec4 image_{0}(vec2 uv) {{ if (uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0) return vec4(0.0); \
             return texture(sampler2D(_koo_img_{0}, _koo_sampler), vec2(uv.x, 1.0 - uv.y)); }}",
            p.name
        );
        images.push(p.name.clone());
    }
    let depth = koo_core::depth::needs_depth(asset).then(|| {
        let binding = BIND_IMAGES + images.len() as u32;
        // Distance from the camera, 0 nearest to 1 farthest (koo_core::depth).
        let _ = writeln!(
            src,
            "layout(set = 0, binding = {binding}) uniform texture2D _koo_depth;\n\
             float getDepth(vec2 uv) {{ return texture(sampler2D(_koo_depth, _koo_sampler), vec2(uv.x, 1.0 - uv.y)).r; }}"
        );
        binding
    });
    src.push_str("layout(location = 0) out vec4 _koo_out;\n");

    let prelude_lines = line(&src) - 1;
    src.push_str(&asset.code);
    if !asset.code.ends_with('\n') {
        src.push('\n');
    }
    let author_lines = asset.code.lines().count().max(1);
    let entry = match asset.kind {
        FxKind::Effect => "effect",
        FxKind::Transition => "transition",
        FxKind::Generator => "generate",
    };
    let _ = write!(
        src,
        "void main() {{\n    \
         vec2 _koo_uv = vec2(gl_FragCoord.x, _koo_target.y - gl_FragCoord.y) / _koo_target;\n    \
         _koo_out = clamp({entry}(_koo_uv), vec4(0.0), vec4(1.0));\n}}\n"
    );

    Ok(Wrapped {
        source: src,
        kind: asset.kind,
        prelude_lines,
        author_lines,
        param_lines,
        fields,
        target_offset,
        uniform_size,
        images,
        depth,
    })
}

/// The clock and sizes of one call.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    /// `iResolution`: the frame's size in canvas (sequence) pixels.
    pub resolution: [f64; 2],
    /// The render target's size in pixels.
    pub target: (u32, u32),
    pub time: f64,
    pub duration: f64,
}

impl Clock {
    /// The clock of `call` rendering at `width` × `height`: `iResolution`
    /// is that size fitted inside the call's canvas.
    pub fn new(call: &FxCall<'_>, width: u32, height: u32) -> Self {
        let (w, h) = (width.max(1) as f64, height.max(1) as f64);
        let (cw, ch) = call.canvas;
        let fit = if cw == 0 || ch == 0 {
            1.0
        } else {
            (cw as f64 / w).min(ch as f64 / h)
        };
        Self {
            resolution: [w * fit, h * fit],
            target: (width, height),
            time: call.time,
            duration: call.duration,
        }
    }
}

impl Wrapped {
    /// The uniform block's bytes for `values` (missing ones take the
    /// asset's defaults) at `clock`.
    pub fn uniforms(
        &self,
        asset: &FxAsset,
        values: &BTreeMap<String, ParamValue>,
        clock: Clock,
    ) -> Vec<u8> {
        let mut out = vec![0u8; self.uniform_size];
        let mut put = |at: usize, v: &[u8]| out[at..at + v.len()].copy_from_slice(v);
        let f = |v: f64| (v as f32).to_le_bytes();
        let progress = if clock.duration > 0.0 {
            (clock.time / clock.duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let [rw, rh] = clock.resolution;
        put(0, &f(rw));
        put(4, &f(rh));
        put(8, &f(clock.time));
        put(12, &f(clock.duration));
        put(16, &f(progress));
        if self.kind == FxKind::Transition {
            put(20, &f(progress));
            put(24, &f(rw / rh.max(1.0)));
        }
        put(self.target_offset, &f(clock.target.0 as f64));
        put(self.target_offset + 4, &f(clock.target.1 as f64));
        for field in &self.fields {
            let spec = &asset.params[field.param];
            let raw = values.get(&spec.name).unwrap_or(&spec.default);
            let value = spec.kind.coerce(raw, &spec.default);
            match field.slot {
                Slot::Float => put(field.offset, &f(value.as_f64().unwrap_or(0.0))),
                Slot::Int => {
                    let i = match (&spec.kind, &value) {
                        (ParamKind::Choice { options }, ParamValue::Text(t)) => {
                            options.iter().position(|o| o == t).unwrap_or(0) as i32
                        }
                        _ => value.as_f64().unwrap_or(0.0).round() as i32,
                    };
                    put(field.offset, &i.to_le_bytes());
                }
                Slot::Vec2 | Slot::Vec4 => {
                    let n = if field.slot == Slot::Vec2 { 2 } else { 4 };
                    let list = value.as_list().unwrap_or(&[]);
                    for k in 0..n {
                        let v = list.get(k).copied().unwrap_or(0.0);
                        put(field.offset + 4 * k, &f(v));
                    }
                }
            }
        }
        out
    }

    /// Where 1-based line `line` of the generated source is, in the author's
    /// terms.
    fn place(&self, line: usize) -> Place<'_> {
        if line <= self.prelude_lines {
            match self.param_lines.get(&line) {
                Some(p) => Place::Param(p),
                None => Place::Engine,
            }
        } else if line <= self.prelude_lines + self.author_lines {
            Place::Author(line - self.prelude_lines)
        } else {
            Place::Main
        }
    }

    /// One readable message per error: `line L, column C: message`, the
    /// offending author line and a caret.
    pub fn describe(&self, author_code: &str, errors: &[(Option<naga::Span>, String)]) -> String {
        let author: Vec<&str> = author_code.lines().collect();
        let entry = match self.kind {
            FxKind::Effect => "vec4 effect(vec2 uv)",
            FxKind::Transition => "vec4 transition(vec2 uv)",
            FxKind::Generator => "vec4 generate(vec2 uv)",
        };
        let mut out = Vec::new();
        for (span, message) in errors {
            let loc = span
                .filter(|s| s.is_defined())
                .map(|s| s.location(&self.source));
            let Some(loc) = loc else {
                out.push(message.clone());
                continue;
            };
            let msg = match self.place(loc.line_number as usize) {
                Place::Author(l) => {
                    let mut m = format!("line {l}, column {}: {message}", loc.line_position);
                    if let Some(text) = author.get(l - 1) {
                        let caret = " ".repeat(loc.line_position.saturating_sub(1) as usize);
                        let _ = write!(m, "\n    {text}\n    {caret}^");
                    }
                    m
                }
                Place::Param(p) => format!(
                    "parameter `{p}` cannot be declared ({message}); rename it, it may clash \
                     with a GLSL keyword or built-in"
                ),
                Place::Main => format!(
                    "{message} (in the engine's main(); the code must define `{entry}` \
                     returning vec4)"
                ),
                Place::Engine => format!("{message} (in the engine's prelude)"),
            };
            out.push(msg);
        }
        if out.is_empty() {
            out.push("the shader failed to compile".into());
        }
        out.join("\n")
    }
}

enum Place<'a> {
    Author(usize),
    Param(&'a str),
    Main,
    Engine,
}
