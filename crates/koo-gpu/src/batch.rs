//! One composed frame on the GPU ([`FxBatch`]). Every step is recorded as
//! render passes; pictures stay in textures between steps, and
//! [`FxBatch::submit`] sends all passes in one command buffer with a copy of
//! one picture into a readback buffer. [`Pending::wait`] maps it without
//! holding the engine lock, so the next frame can be recorded and submitted
//! meanwhile (each frame in flight has its own readback buffer). Surfaces
//! are RGBA8 (as the per-call methods return them); scratch canvases inside
//! a step are 16-bit float and are reused by later steps of the same batch.

use crate::wrap::{self, Clock};
use crate::{GpuFx, MID_FORMAT, OUT_FORMAT, composite, expect_kind, input_bindings};
use koo_core::fx::FxKind;
use koo_core::services::{
    Frame, FxBatch, FxCall, PendingFrame, PixelOrder, Placement, ServiceError, ServiceResult,
    Surface,
};
use std::num::NonZeroU64;
use std::sync::mpsc;

/// Readback buffers kept for frames in flight.
const MAX_READBACKS: usize = 4;

/// Premultiplying, unpremultiplying, cross-fading and swapping red and blue,
/// one pixel at a time.
const CONVERT: &str = r#"
struct Op {
    mode: u32,
    t: f32,
};

@group(0) @binding(0) var<uniform> op: Op;
@group(0) @binding(1) var a: texture_2d<f32>;
@group(0) @binding(2) var b: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let x = textureLoad(a, p, 0);
    switch op.mode {
        case 0u: { return vec4<f32>(x.rgb * x.a, x.a); }
        case 1u: {
            if (x.a <= 0.0) {
                return vec4<f32>(0.0);
            }
            return vec4<f32>(min(x.rgb / x.a, vec3<f32>(1.0)), x.a);
        }
        case 2u: { return mix(x, textureLoad(b, p, 0), op.t); }
        default: { return x.bgra; }
    }
}
"#;

const PREMULTIPLY: u32 = 0;
const UNPREMULTIPLY: u32 = 1;
const MIX: u32 = 2;
const SWAP: u32 = 3;

/// The convert program, built with the engine.
pub(crate) struct Converter {
    bind_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
}

impl Converter {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("koo-gpu convert"),
            source: wgpu::ShaderSource::Wgsl(CONVERT.into()),
        });
        let (bind_layout, layout) = composite::layouts(device, "koo-gpu convert", 16);
        let pipeline = composite::pipeline(device, &layout, &module, OUT_FORMAT);
        Self {
            bind_layout,
            pipeline,
        }
    }
}

pub(crate) struct Batch<'a> {
    fx: &'a GpuFx,
    /// Pictures by [`Surface`] index.
    surfaces: Vec<wgpu::Texture>,
    /// Scratch textures free for later steps (passes that used them run
    /// first).
    spare: Vec<wgpu::Texture>,
    passes: Vec<Pass>,
    /// Every pass's uniform block, each at an aligned offset.
    uniforms: Vec<u8>,
}

struct Pass {
    target: wgpu::TextureView,
    clear: wgpu::Color,
    /// `None` only clears the target.
    draw: Option<Draw>,
}

struct Draw {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// Offset and length in [`Batch::uniforms`]; bound at binding 0.
    uniform: (u64, u64),
    bindings: Vec<(u32, Bind)>,
}

enum Bind {
    Sampler,
    View(wgpu::TextureView),
}

impl<'a> Batch<'a> {
    pub(crate) fn new(fx: &'a GpuFx) -> Self {
        Self {
            fx,
            surfaces: Vec::new(),
            spare: Vec::new(),
            passes: Vec::new(),
            uniforms: Vec::new(),
        }
    }

    fn push(&mut self, texture: wgpu::Texture) -> Surface {
        self.surfaces.push(texture);
        Surface(self.surfaces.len() - 1)
    }

    fn texture(&self, surface: Surface) -> ServiceResult<&wgpu::Texture> {
        self.surfaces
            .get(surface.0)
            .ok_or_else(|| ServiceError("no such surface".into()))
    }

    fn view(&self, surface: Surface) -> ServiceResult<wgpu::TextureView> {
        Ok(self.texture(surface)?.create_view(&Default::default()))
    }

    /// A spare texture of this size and format, or one from the pool.
    fn scratch(&mut self, width: u32, height: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
        if let Some(i) = self
            .spare
            .iter()
            .position(|t| (t.width(), t.height(), t.format()) == (width, height, format))
        {
            return self.spare.swap_remove(i);
        }
        let mut state = self.fx.lock();
        self.fx.texture(&mut state, width, height, format)
    }

    fn uniform(&mut self, bytes: &[u8]) -> (u64, u64) {
        let at = (self.uniforms.len() as u64).next_multiple_of(self.fx.uniform_align);
        self.uniforms.resize(at as usize, 0);
        self.uniforms.extend_from_slice(bytes);
        (at, bytes.len() as u64)
    }

    /// Run `record`; when it fails, drop the passes it recorded so the
    /// batch stays as it was.
    fn step<T>(&mut self, record: impl FnOnce(&mut Self) -> ServiceResult<T>) -> ServiceResult<T> {
        let mark = (self.passes.len(), self.uniforms.len());
        let result = record(self);
        if result.is_err() {
            self.passes.truncate(mark.0);
            self.uniforms.truncate(mark.1);
        }
        result
    }

    /// Record `call`'s program (a `kind`) drawing `inputs` into `target`.
    fn program(
        &mut self,
        call: &FxCall<'_>,
        kind: FxKind,
        inputs: Vec<wgpu::TextureView>,
        target: &wgpu::Texture,
    ) -> ServiceResult<()> {
        expect_kind(call.asset, kind)?;
        let fx = self.fx;
        let mut state = fx.lock();
        let key = fx
            .program(&mut state, call.asset)
            .map_err(|e| ServiceError(format!("{}: {e}", call.asset.name)))?;
        let images = fx.images(&mut state, call);
        let Some(Ok(program)) = state.programs.get_mut(&key) else {
            return Err(ServiceError("program evicted mid-call".into()));
        };
        let pipeline = fx.pipeline(program, target.format()).clone();
        let layout = program.bind_layout.clone();
        let clock = Clock::new(call, target.width(), target.height());
        let bytes = program.wrapped.uniforms(call.asset, call.values, clock);
        drop(state);
        let uniform = self.uniform(&bytes);
        let mut bindings = vec![(wrap::BIND_SAMPLER, Bind::Sampler)];
        bindings.extend(
            input_bindings(kind)
                .iter()
                .copied()
                .zip(inputs.into_iter().map(Bind::View)),
        );
        bindings.extend(
            images
                .into_iter()
                .enumerate()
                .map(|(k, view)| (wrap::BIND_IMAGES + k as u32, Bind::View(view))),
        );
        self.passes.push(Pass {
            target: target.create_view(&Default::default()),
            clear: wgpu::Color::TRANSPARENT,
            draw: Some(Draw {
                pipeline,
                layout,
                uniform,
                bindings,
            }),
        });
        Ok(())
    }

    /// A new surface from one convert pass over `a` and `b`.
    fn convert(&mut self, mode: u32, a: Surface, b: Surface, t: f32) -> ServiceResult<Surface> {
        let (a_view, b_view) = (self.view(a)?, self.view(b)?);
        let (w, h) = self.size(b);
        let output = self.scratch(w, h, OUT_FORMAT);
        let mut bytes = [0u8; 16];
        bytes[0..4].copy_from_slice(&mode.to_le_bytes());
        bytes[4..8].copy_from_slice(&t.to_le_bytes());
        let uniform = self.uniform(&bytes);
        let fx = self.fx;
        let converter = &fx.converter;
        self.passes.push(Pass {
            target: output.create_view(&Default::default()),
            clear: wgpu::Color::TRANSPARENT,
            draw: Some(Draw {
                pipeline: converter.pipeline.clone(),
                layout: converter.bind_layout.clone(),
                uniform,
                bindings: vec![(1, Bind::View(a_view)), (2, Bind::View(b_view))],
            }),
        });
        Ok(self.push(output))
    }
}

impl FxBatch for Batch<'_> {
    fn upload(&mut self, frame: &Frame) -> ServiceResult<Surface> {
        let mut state = self.fx.lock();
        let texture = self.fx.upload_frame(&mut state, frame)?;
        drop(state);
        Ok(self.push(texture))
    }

    fn size(&self, surface: Surface) -> (u32, u32) {
        self.surfaces
            .get(surface.0)
            .map_or((0, 0), |t| (t.width(), t.height()))
    }

    /// Ping-pongs between two float canvases; the last pass writes RGBA8.
    fn apply(&mut self, input: Surface, chain: &[FxCall<'_>]) -> ServiceResult<Surface> {
        if chain.is_empty() {
            return Ok(input);
        }
        let source = self.view(input)?;
        let (w, h) = self.size(input);
        let mids: Vec<wgpu::Texture> = (0..(chain.len() - 1).min(2))
            .map(|_| self.scratch(w, h, MID_FORMAT))
            .collect();
        let output = self.scratch(w, h, OUT_FORMAT);
        let result = self.step(|batch| {
            let mut source = source;
            for (i, call) in chain.iter().enumerate() {
                let target = if i + 1 == chain.len() {
                    &output
                } else {
                    &mids[i % 2]
                };
                batch.program(call, FxKind::Effect, vec![source], target)?;
                source = target.create_view(&Default::default());
            }
            Ok(())
        });
        self.spare.extend(mids);
        match result {
            Ok(()) => Ok(self.push(output)),
            Err(e) => {
                self.spare.push(output);
                Err(e)
            }
        }
    }

    /// The output takes `to`'s size; `from` is stretched to fit.
    fn transition(
        &mut self,
        from: Surface,
        to: Surface,
        call: &FxCall<'_>,
    ) -> ServiceResult<Surface> {
        let inputs = vec![self.view(from)?, self.view(to)?];
        let (w, h) = self.size(to);
        let output = self.scratch(w, h, OUT_FORMAT);
        match self.step(|batch| batch.program(call, FxKind::Transition, inputs, &output)) {
            Ok(()) => Ok(self.push(output)),
            Err(e) => {
                self.spare.push(output);
                Err(e)
            }
        }
    }

    fn generate(&mut self, call: &FxCall<'_>, width: u32, height: u32) -> ServiceResult<Surface> {
        self.fx.check_size(width, height)?;
        let output = self.scratch(width, height, OUT_FORMAT);
        match self.step(|batch| batch.program(call, FxKind::Generator, Vec::new(), &output)) {
            Ok(()) => Ok(self.push(output)),
            Err(e) => {
                self.spare.push(output);
                Err(e)
            }
        }
    }

    /// One pass per layer, ping-ponging the canvas between two float
    /// textures (blend modes read the canvas below); the last pass writes
    /// RGBA8.
    fn composite(
        &mut self,
        width: u32,
        height: u32,
        background: [u8; 4],
        layers: &[(Surface, Placement)],
    ) -> ServiceResult<Surface> {
        self.fx.check_size(width, height)?;
        let mut pictures = Vec::with_capacity(layers.len());
        for (surface, placement) in layers {
            let p = placement;
            if p.size[0] > 0.0 && p.size[1] > 0.0 && p.opacity > 0.0 && p.inverse().is_some() {
                pictures.push((self.view(*surface)?, self.size(*surface), p));
            }
        }
        let bg = background.map(|c| c as f64 / 255.0);
        let clear = wgpu::Color {
            r: bg[0],
            g: bg[1],
            b: bg[2],
            a: bg[3],
        };
        let output = self.scratch(width, height, OUT_FORMAT);
        let output_view = output.create_view(&Default::default());
        if pictures.is_empty() {
            self.passes.push(Pass {
                target: output_view,
                clear,
                draw: None,
            });
            return Ok(self.push(output));
        }
        let canvases = [
            self.scratch(width, height, MID_FORMAT),
            self.scratch(width, height, MID_FORMAT),
        ];
        let views = canvases
            .each_ref()
            .map(|t| t.create_view(&Default::default()));
        self.passes.push(Pass {
            target: views[0].clone(),
            clear,
            draw: None,
        });
        let fx = self.fx;
        let compositor = &fx.compositor;
        let count = pictures.len();
        for (i, (picture, size, placement)) in pictures.into_iter().enumerate() {
            let last = i + 1 == count;
            let (target, format) = if last {
                (output_view.clone(), OUT_FORMAT)
            } else {
                (views[(i + 1) % 2].clone(), MID_FORMAT)
            };
            let uniform = self.uniform(&composite::uniforms(placement, size));
            let pipeline = compositor
                .pipelines
                .get(&format)
                .ok_or_else(|| ServiceError("no composite pipeline".into()))?;
            self.passes.push(Pass {
                target,
                clear: wgpu::Color::TRANSPARENT,
                draw: Some(Draw {
                    pipeline: pipeline.clone(),
                    layout: compositor.bind_layout.clone(),
                    uniform,
                    bindings: vec![
                        (1, Bind::View(views[i % 2].clone())),
                        (2, Bind::View(picture)),
                    ],
                }),
            });
        }
        self.spare.extend(canvases);
        Ok(self.push(output))
    }

    fn premultiply(&mut self, surface: Surface) -> ServiceResult<Surface> {
        self.convert(PREMULTIPLY, surface, surface, 0.0)
    }

    fn unpremultiply(&mut self, surface: Surface) -> ServiceResult<Surface> {
        self.convert(UNPREMULTIPLY, surface, surface, 0.0)
    }

    fn mix(&mut self, from: Surface, to: Surface, progress: f32) -> ServiceResult<Surface> {
        if self.size(from) != self.size(to) {
            self.texture(to)?;
            return Ok(to);
        }
        self.convert(MIX, from, to, progress.clamp(0.0, 1.0))
    }

    fn submit<'p>(
        mut self: Box<Self>,
        surface: Surface,
        order: PixelOrder,
    ) -> ServiceResult<Box<dyn PendingFrame + 'p>>
    where
        Self: 'p,
    {
        let surface = match order {
            PixelOrder::Rgba => surface,
            PixelOrder::Bgra => self.convert(SWAP, surface, surface, 0.0)?,
        };
        let fx = self.fx;
        let texture = self.texture(surface)?.clone();
        let (width, height) = (texture.width(), texture.height());
        let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let size = row as u64 * height as u64;
        let mut state = fx.lock();
        let scope = fx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let need = (self.uniforms.len() as u64).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        self.uniforms.resize(need as usize, 0);
        if state.uniforms.as_ref().is_none_or(|b| b.size() < need) {
            state.uniforms = Some(fx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("koo-gpu uniforms"),
                size: need.next_power_of_two().max(4096),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let buffer = state.uniforms.clone().expect("just created");
        if need > 0 {
            fx.queue.write_buffer(&buffer, 0, &self.uniforms);
        }

        let mut encoder = fx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("koo-gpu batch"),
            });
        for pass in &self.passes {
            let bind_group =
                pass.draw.as_ref().map(|draw| {
                    let mut entries = vec![wgpu::BindGroupEntry {
                        binding: wrap::BIND_UNIFORMS,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &buffer,
                            offset: draw.uniform.0,
                            size: NonZeroU64::new(draw.uniform.1),
                        }),
                    }];
                    entries.extend(draw.bindings.iter().map(|(binding, bind)| {
                        wgpu::BindGroupEntry {
                            binding: *binding,
                            resource: match bind {
                                Bind::Sampler => wgpu::BindingResource::Sampler(&fx.sampler),
                                Bind::View(view) => wgpu::BindingResource::TextureView(view),
                            },
                        }
                    }));
                    fx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("koo-gpu pass"),
                        layout: &draw.layout,
                        entries: &entries,
                    })
                });
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("koo-gpu pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &pass.target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(pass.clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let (Some(draw), Some(bind_group)) = (&pass.draw, &bind_group) {
                rpass.set_pipeline(&draw.pipeline);
                rpass.set_bind_group(0, bind_group, &[]);
                rpass.draw(0..3, 0..1);
            }
        }
        let readback = match state.readbacks.iter().position(|b| b.size() >= size) {
            Some(i) => state.readbacks.swap_remove(i),
            None => fx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("koo-gpu readback"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        };
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(height),
                },
            },
            crate::extent(width, height),
        );
        let index = fx.queue.submit([encoder.finish()]);
        let error = pollster::block_on(scope.pop());
        drop(state);
        let pending = Pending {
            fx,
            readback: Some(readback),
            textures: self
                .surfaces
                .drain(..)
                .chain(self.spare.drain(..))
                .collect(),
            index,
            width,
            height,
            row,
        };
        match error {
            Some(e) => Err(ServiceError(format!("GPU error: {e}"))),
            None => Ok(Box::new(pending)),
        }
    }
}

impl Drop for Batch<'_> {
    /// Textures of a batch that was never finished go back to the pool.
    fn drop(&mut self) {
        if self.surfaces.is_empty() && self.spare.is_empty() {
            return;
        }
        let mut state = self.fx.lock();
        GpuFx::give_back(
            &mut state,
            self.surfaces.drain(..).chain(self.spare.drain(..)),
        );
    }
}

/// A submitted batch: its readback buffer, and its textures until the GPU
/// is done with them.
struct Pending<'a> {
    fx: &'a GpuFx,
    /// Back to the pool on drop, unless a failed wait left it mapped.
    readback: Option<wgpu::Buffer>,
    textures: Vec<wgpu::Texture>,
    index: wgpu::SubmissionIndex,
    width: u32,
    height: u32,
    /// Bytes per row in `readback`.
    row: u32,
}

impl PendingFrame for Pending<'_> {
    fn wait(mut self: Box<Self>) -> ServiceResult<Frame> {
        let buffer = self
            .readback
            .take()
            .ok_or_else(|| ServiceError("no readback buffer".into()))?;
        let slice = buffer.slice(..self.row as u64 * self.height as u64);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.fx
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(self.index.clone()),
                timeout: None,
            })
            .map_err(ServiceError::new)?;
        rx.recv()
            .map_err(ServiceError::new)?
            .map_err(ServiceError::new)?;
        let (width, height, row) = (self.width as usize, self.height as usize, self.row as usize);
        let mut rgba = Vec::with_capacity(width * height * 4);
        {
            let data = slice.get_mapped_range();
            for y in 0..height {
                rgba.extend_from_slice(&data[y * row..y * row + width * 4]);
            }
        }
        buffer.unmap();
        self.readback = Some(buffer);
        Ok(Frame {
            width: self.width,
            height: self.height,
            rgba: rgba.into(),
        })
    }
}

impl Drop for Pending<'_> {
    /// Textures and the readback buffer go back to the pool; later
    /// submissions that reuse them run after this one.
    fn drop(&mut self) {
        let mut state = self.fx.lock();
        GpuFx::give_back(&mut state, self.textures.drain(..));
        if let Some(buffer) = self.readback.take()
            && state.readbacks.len() < MAX_READBACKS
        {
            state.readbacks.push(buffer);
        }
    }
}
