//! Layer compositing on the GPU ([`koo_core::services::FxService::composite`]):
//! the same arithmetic as `koo_core::compose::composite_cpu`, one
//! fullscreen pass per layer, ping-ponging the canvas between two float
//! textures (blend modes read the canvas below), the last pass writing
//! RGBA8.

use crate::{GpuFx, MID_FORMAT, OUT_FORMAT, State};
use koo_core::model::BlendMode;
use koo_core::services::{CompositeLayer, Frame, ServiceError, ServiceResult};
use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::Arc;

const SHADER: &str = r#"
struct Layer {
    center: vec2<f32>,
    inv_size: vec2<f32>,
    sin_cos: vec2<f32>,
    frame: vec2<f32>,
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
    let s = layer.sin_cos.x;
    let c = layer.sin_cos.y;
    var uv = vec2<f32>(d.x * c + d.y * s, -d.x * s + d.y * c) * layer.inv_size + 0.5;
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
    bind_layout: wgpu::BindGroupLayout,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl Compositor {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("koo-gpu composite"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
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
            label: Some("koo-gpu composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(UNIFORM_SIZE),
                    },
                    count: None,
                },
                texture(1),
                texture(2),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("koo-gpu composite"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipelines = [MID_FORMAT, OUT_FORMAT]
            .into_iter()
            .map(|format| {
                let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("koo-gpu composite"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
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
                });
                (format, pipeline)
            })
            .collect();
        Self {
            bind_layout,
            pipelines,
        }
    }
}

fn mode_index(mode: BlendMode) -> u32 {
    BlendMode::ALL
        .iter()
        .position(|m| *m == mode)
        .unwrap_or_default() as u32
}

/// The uniform block of `layer`, as laid out in [`SHADER`].
fn uniforms(layer: &CompositeLayer<'_>) -> [u8; UNIFORM_SIZE as usize] {
    let p = &layer.placement;
    let (sin, cos) = p.rotation_deg.to_radians().sin_cos();
    let floats = [
        p.center[0],
        p.center[1],
        1.0 / p.size[0],
        1.0 / p.size[1],
        sin,
        cos,
        layer.frame.width as f32,
        layer.frame.height as f32,
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

impl GpuFx {
    pub(crate) fn composite_layers(
        &self,
        width: u32,
        height: u32,
        background: [u8; 4],
        layers: &[CompositeLayer<'_>],
    ) -> ServiceResult<Frame> {
        self.check_size(width, height)?;
        let layers: Vec<&CompositeLayer> = layers
            .iter()
            .filter(|l| {
                let p = &l.placement;
                p.size[0] > 0.0 && p.size[1] > 0.0 && p.opacity > 0.0
            })
            .collect();
        if layers.is_empty() {
            let rgba: Vec<u8> = background
                .iter()
                .copied()
                .cycle()
                .take(width as usize * height as usize * 4)
                .collect();
            return Ok(Frame {
                width,
                height,
                rgba: Arc::from(rgba),
            });
        }
        let mut state = self.lock();
        let mut uploads = Vec::with_capacity(layers.len());
        for l in &layers {
            match self.upload_frame(&mut state, l.frame) {
                Ok(t) => uploads.push(t),
                Err(e) => {
                    Self::give_back(&mut state, uploads);
                    return Err(e);
                }
            }
        }
        let result = self.composite_run(&mut state, width, height, background, &layers, &uploads);
        Self::give_back(&mut state, uploads);
        result
    }

    fn composite_run(
        &self,
        state: &mut State,
        width: u32,
        height: u32,
        background: [u8; 4],
        layers: &[&CompositeLayer<'_>],
        uploads: &[wgpu::Texture],
    ) -> ServiceResult<Frame> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let slot = UNIFORM_SIZE.next_multiple_of(self.uniform_align);
        let need = slot * layers.len() as u64;
        if state.uniforms.as_ref().is_none_or(|b| b.size() < need) {
            state.uniforms = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("koo-gpu uniforms"),
                size: need.next_power_of_two().max(4096),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let buffer = state.uniforms.clone().expect("just created");
        for (i, l) in layers.iter().enumerate() {
            self.queue
                .write_buffer(&buffer, slot * i as u64, &uniforms(l));
        }
        let canvases = [
            self.texture(state, width, height, MID_FORMAT),
            self.texture(state, width, height, MID_FORMAT),
        ];
        let output = self.texture(state, width, height, OUT_FORMAT);
        let views = canvases
            .each_ref()
            .map(|t| t.create_view(&Default::default()));
        let output_view = output.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("koo-gpu composite"),
            });
        let bg = background.map(|c| c as f64 / 255.0);
        let clear = wgpu::Color {
            r: bg[0],
            g: bg[1],
            b: bg[2],
            a: bg[3],
        };
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("koo-gpu composite clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &views[0],
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        for (i, upload) in uploads.iter().enumerate() {
            let last = i + 1 == uploads.len();
            let (source, target) = (&views[i % 2], &views[(i + 1) % 2]);
            let target = if last { &output_view } else { target };
            let format = if last { OUT_FORMAT } else { MID_FORMAT };
            let picture = upload.create_view(&Default::default());
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("koo-gpu composite"),
                layout: &self.compositor.bind_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &buffer,
                            offset: slot * i as u64,
                            size: NonZeroU64::new(UNIFORM_SIZE),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&picture),
                    },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("koo-gpu composite layer"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let pipeline = self
                .compositor
                .pipelines
                .get(&format)
                .ok_or_else(|| ServiceError("no composite pipeline".into()))?;
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        let result = self.read_back(state, encoder, &output, scope);
        Self::give_back(state, canvases.into_iter().chain([output]));
        result
    }
}
