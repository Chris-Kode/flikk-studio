//! Layer compositing on the GPU ([`koo_core::services::FxService::composite`]):
//! the same arithmetic as `koo_core::compose::composite_cpu`, one
//! fullscreen pass per layer (recorded by [`crate::batch`]).

use crate::{MID_FORMAT, OUT_FORMAT};
use koo_core::model::BlendMode;
use koo_core::services::Placement;
use std::collections::HashMap;
use std::num::NonZeroU64;

const SHADER: &str = r#"
struct Layer {
    center: vec2<f32>,
    frame: vec2<f32>,
    inv: vec4<f32>,
    crop: vec4<f32>,
    flip: vec2<f32>,
    opacity: f32,
    mode: u32,
};

@group(0) @binding(0) var<uniform> layer: Layer;
@group(0) @binding(1) var canvas: texture_2d<f32>;
@group(0) @binding(2) var picture: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

fn texel(p: vec2<i32>) -> vec4<f32> {
    let last = vec2<i32>(layer.frame) - vec2<i32>(1, 1);
    let c = textureLoad(picture, clamp(p, vec2<i32>(0, 0), last), 0);
    return vec4<f32>(c.rgb * c.a, c.a);
}

fn hard_light(b: f32, s: f32) -> f32 {
    if (s <= 0.5) {
        return 2.0 * b * s;
    }
    return 1.0 - 2.0 * (1.0 - b) * (1.0 - s);
}

fn blend(b: f32, s: f32) -> f32 {
    switch layer.mode {
        case 1u: { return min(b + s, 1.0); }
        case 2u: { return b + s - b * s; }
        case 3u: { return b * s; }
        case 4u: { return hard_light(s, b); }
        case 5u: {
            if (s <= 0.5) {
                return b - (1.0 - 2.0 * s) * b * (1.0 - b);
            }
            var d = sqrt(b);
            if (b <= 0.25) {
                d = ((16.0 * b - 12.0) * b + 4.0) * b;
            }
            return b + (2.0 * s - 1.0) * (d - b);
        }
        case 6u: { return hard_light(b, s); }
        case 7u: { return max(b, s); }
        case 8u: { return min(b, s); }
        case 9u: { return abs(b - s); }
        default: { return s; }
    }
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let dst = textureLoad(canvas, vec2<i32>(pos.xy), 0);
    let d = pos.xy - layer.center;
    let m = layer.inv;
    var uv = vec2<f32>(m.x * d.x + m.y * d.y, m.z * d.x + m.w * d.y) + 0.5;
    if (layer.flip.x > 0.5) {
        uv.x = 1.0 - uv.x;
    }
    if (layer.flip.y > 0.5) {
        uv.y = 1.0 - uv.y;
    }
    if (uv.x < layer.crop.x || uv.x > 1.0 - layer.crop.z
        || uv.y < layer.crop.y || uv.y > 1.0 - layer.crop.w) {
        return dst;
    }
    let t = uv * layer.frame - 0.5;
    let base = floor(t);
    let a = t - base;
    let b = vec2<i32>(base);
    let top = mix(texel(b), texel(b + vec2<i32>(1, 0)), a.x);
    let bottom = mix(texel(b + vec2<i32>(0, 1)), texel(b + vec2<i32>(1, 1)), a.x);
    let src = mix(top, bottom, a.y) * layer.opacity;
    let a_s = src.a;
    if (a_s <= 0.0) {
        return dst;
    }
    let a_b = dst.a;
    let a_o = a_s + a_b * (1.0 - a_s);
    let cs = clamp(src.rgb / a_s, vec3<f32>(0.0), vec3<f32>(1.0));
    let cb = dst.rgb;
    let mixed = vec3<f32>(blend(cb.r, cs.r), blend(cb.g, cs.g), blend(cb.b, cs.b));
    let co = a_s * (1.0 - a_b) * cs + a_s * a_b * mixed + (1.0 - a_s) * a_b * cb;
    return vec4<f32>(co / a_o, a_o);
}
"#;

/// Bytes of one layer's uniform block.
const UNIFORM_SIZE: u64 = 64;

/// The composite program, built with the engine.
pub(crate) struct Compositor {
    pub(crate) bind_layout: wgpu::BindGroupLayout,
    pub(crate) pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl Compositor {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("koo-gpu composite"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let (bind_layout, layout) = layouts(device, "koo-gpu composite", UNIFORM_SIZE);
        let pipelines = [MID_FORMAT, OUT_FORMAT]
            .into_iter()
            .map(|format| (format, pipeline(device, &layout, &module, format)))
            .collect();
        Self {
            bind_layout,
            pipelines,
        }
    }
}

/// Bind group and pipeline layouts of a WGSL pass with a `uniform_size`
/// uniform block at binding 0 and two unfiltered textures at 1 and 2.
pub(crate) fn layouts(
    device: &wgpu::Device,
    label: &str,
    uniform_size: u64,
) -> (wgpu::BindGroupLayout, wgpu::PipelineLayout) {
    let texture = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(uniform_size),
                },
                count: None,
            },
            texture(1),
            texture(2),
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(&bind_layout)],
        immediate_size: 0,
    });
    (bind_layout, layout)
}

/// A fullscreen-triangle pipeline of `module` (`vs` and `fs`) writing
/// `format`.
pub(crate) fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("koo-gpu builtin"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn mode_index(mode: BlendMode) -> u32 {
    BlendMode::ALL
        .iter()
        .position(|m| *m == mode)
        .unwrap_or_default() as u32
}

/// The uniform block of a `frame`-sized picture at placement `p`, as laid
/// out in [`SHADER`]. The batch leaves out flat pictures ([`Placement::inverse`] is `None`).
pub(crate) fn uniforms(p: &Placement, frame: (u32, u32)) -> [u8; UNIFORM_SIZE as usize] {
    let inv = p.inverse().unwrap_or_default();
    let floats = [
        p.center[0],
        p.center[1],
        frame.0 as f32,
        frame.1 as f32,
        inv[0],
        inv[1],
        inv[2],
        inv[3],
        p.crop[0].clamp(0.0, 1.0),
        p.crop[1].clamp(0.0, 1.0),
        p.crop[2].clamp(0.0, 1.0),
        p.crop[3].clamp(0.0, 1.0),
        if p.flip[0] { 1.0 } else { 0.0 },
        if p.flip[1] { 1.0 } else { 0.0 },
        p.opacity.clamp(0.0, 1.0),
    ];
    let mut out = [0u8; UNIFORM_SIZE as usize];
    for (i, f) in floats.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&f.to_le_bytes());
    }
    out[60..64].copy_from_slice(&mode_index(p.blend).to_le_bytes());
    out
}
