//! GPU engine for GLSL effects, transitions and generators
//! ([`koo_core::services::FxService`]).
//!
//! Each program is wrapped (see [`wrap`]) into a `#version 450` fragment
//! shader, compiled with naga's GLSL front end and drawn as a fullscreen
//! triangle into an offscreen texture. Frames go up as RGBA8 textures and
//! come back as RGBA8; a whole effect chain runs on the GPU between one
//! upload and one readback, with 16-bit float intermediates.
//!
//! The engine owns its own wgpu device (the same wgpu GPUI uses) and is
//! synchronous: every call takes a lock, records, submits and waits, so it
//! can be used from any thread. Compiled programs are cached by a hash of
//! their code, kind and parameter layout (compile errors too, so a broken
//! shader is not recompiled every frame); Image parameters are cached by
//! path and modification time; textures and buffers are pooled by size.
//! Layer compositing for the program frame lives in [`composite`].

mod composite;
mod wrap;

use anyhow::Context as _;
use koo_core::fx::{FxAsset, FxKind, FxLanguage, ParamKind};
use koo_core::services::{CompositeLayer, Frame, FxCall, FxService, ServiceError, ServiceResult};
use std::borrow::Cow;
use std::collections::HashMap;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::SystemTime;
use wrap::{Clock, Wrapped};

/// Pixel format of uploads, of the last pass and of readbacks.
const OUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Pixel format between the passes of a chain.
const MID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Compiled programs kept before the cache is cleared.
const MAX_PROGRAMS: usize = 256;
/// Pooled textures kept before the pool is cleared.
const MAX_POOLED: usize = 24;

const VERTEX_WGSL: &str = r#"
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

/// The wgpu engine. Cheap to share behind an `Arc`.
pub struct GpuFx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    vertex: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    /// 1 × 1 transparent texture for missing images.
    blank: wgpu::TextureView,
    uniform_align: u64,
    max_size: u32,
    compositor: composite::Compositor,
    state: Mutex<State>,
}

struct Program {
    wrapped: Wrapped,
    module: wgpu::ShaderModule,
    bind_layout: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

#[derive(Default)]
struct State {
    programs: HashMap<u64, Result<Program, String>>,
    images: HashMap<PathBuf, (Option<SystemTime>, wgpu::TextureView)>,
    textures: HashMap<(u32, u32, wgpu::TextureFormat), Vec<wgpu::Texture>>,
    readback: Option<wgpu::Buffer>,
    uniforms: Option<wgpu::Buffer>,
}

/// One pass of a frame: which program, its uniforms, what it reads.
struct Pass<'a> {
    key: u64,
    uniforms: Vec<u8>,
    inputs: Vec<&'a wgpu::TextureView>,
    images: Vec<wgpu::TextureView>,
}

impl GpuFx {
    /// Open the GPU. Fails when there is no adapter (no driver, headless
    /// without software Vulkan).
    pub fn new() -> anyhow::Result<Self> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .context("no GPU adapter")?;
        let limits = wgpu::Limits::default().using_resolution(adapter.limits());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("koo-gpu"),
            required_limits: limits.clone(),
            ..Default::default()
        }))
        .context("could not open the GPU device")?;
        device.on_uncaptured_error(Arc::new(|e: wgpu::Error| {
            eprintln!("koo-gpu: {e}");
        }));
        let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("koo-gpu fullscreen"),
            source: wgpu::ShaderSource::Wgsl(VERTEX_WGSL.into()),
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("koo-gpu linear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blank = upload(&device, &queue, 1, 1, &[0, 0, 0, 0])
            .create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self {
            compositor: composite::Compositor::new(&device),
            uniform_align: limits.min_uniform_buffer_offset_alignment as u64,
            max_size: limits.max_texture_dimension_2d,
            device,
            queue,
            vertex,
            sampler,
            blank,
            state: Mutex::new(State::default()),
        })
    }

    /// The engine state, with the program cache trimmed (between calls, so
    /// a chain's programs stay put while it runs).
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.programs.len() >= MAX_PROGRAMS {
            state.programs.clear();
        }
        state
    }

    /// Compile `asset` into the cache (once per key); its error otherwise.
    fn program(&self, state: &mut State, asset: &FxAsset) -> Result<u64, String> {
        let key = wrap::program_key(asset);
        match state
            .programs
            .entry(key)
            .or_insert_with(|| self.compile(asset))
        {
            Ok(_) => Ok(key),
            Err(e) => Err(e.clone()),
        }
    }

    fn compile(&self, asset: &FxAsset) -> Result<Program, String> {
        if asset.language != FxLanguage::Glsl {
            return Err("only GLSL assets run on the GPU".into());
        }
        let wrapped = wrap::wrap(asset)?;
        let options = naga::front::glsl::Options::from(naga::ShaderStage::Fragment);
        let module = naga::front::glsl::Frontend::default()
            .parse(&options, &wrapped.source)
            .map_err(|e| {
                let errors: Vec<_> = e
                    .errors
                    .iter()
                    .map(|e| (Some(e.meta), e.kind.to_string()))
                    .collect();
                wrapped.describe(&asset.code, &errors)
            })?;
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .map_err(|e| {
            let mut message = e.as_inner().to_string();
            let mut source = std::error::Error::source(e.as_inner());
            while let Some(s) = source {
                message.push_str(": ");
                message.push_str(&s.to_string());
                source = s.source();
            }
            // Name IR handles (`[20]`) by the code they came from.
            for (span, label) in e.spans() {
                if let Some(handle) = label.rsplit_once(' ').map(|(_, h)| h)
                    && handle.starts_with('[')
                    && span.is_defined()
                    && let Some(text) = wrapped.source.get(span.to_range().unwrap_or(0..0))
                {
                    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    message = message.replace(handle, &format!("`{text}`"));
                }
            }
            // The innermost span is the most specific.
            let span = e.spans().last().map(|(s, _)| *s);
            wrapped.describe(&asset.code, &[(span, message)])
        })?;

        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("koo-gpu program"),
                source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
            });
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: wrap::BIND_UNIFORMS,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(wrapped.uniform_size as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: wrap::BIND_SAMPLER,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ];
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        for b in input_bindings(asset.kind) {
            entries.push(texture(*b));
        }
        for i in 0..wrapped.images.len() {
            entries.push(texture(wrap::BIND_IMAGES + i as u32));
        }
        let bind_layout = self
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("koo-gpu program"),
                entries: &entries,
            });
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("koo-gpu program"),
                bind_group_layouts: &[Some(&bind_layout)],
                immediate_size: 0,
            });
        let mut program = Program {
            wrapped,
            module,
            bind_layout,
            layout,
            pipelines: HashMap::new(),
        };
        // Build the common target now so backend errors surface in `check`.
        let format = if asset.kind == FxKind::Effect {
            MID_FORMAT
        } else {
            OUT_FORMAT
        };
        self.pipeline(&mut program, format);
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(format!("the GPU rejected the shader: {e}"));
        }
        Ok(program)
    }

    fn pipeline<'p>(
        &self,
        program: &'p mut Program,
        format: wgpu::TextureFormat,
    ) -> &'p wgpu::RenderPipeline {
        let Program {
            module,
            layout,
            pipelines,
            ..
        } = program;
        pipelines.entry(format).or_insert_with(|| {
            self.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("koo-gpu program"),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module: &self.vertex,
                        entry_point: Some("vs"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some("main"),
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
        })
    }

    /// A pooled texture usable as a render target and a sampled input.
    fn texture(
        &self,
        state: &mut State,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> wgpu::Texture {
        if let Some(t) = state
            .textures
            .get_mut(&(width, height, format))
            .and_then(Vec::pop)
        {
            return t;
        }
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("koo-gpu frame"),
            size: extent(width, height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn give_back(state: &mut State, textures: impl IntoIterator<Item = wgpu::Texture>) {
        for t in textures {
            if state.textures.values().map(Vec::len).sum::<usize>() >= MAX_POOLED {
                state.textures.clear();
            }
            let key = (t.width(), t.height(), t.format());
            state.textures.entry(key).or_default().push(t);
        }
    }

    fn upload_frame(&self, state: &mut State, frame: &Frame) -> ServiceResult<wgpu::Texture> {
        self.check_size(frame.width, frame.height)?;
        let len = frame.width as usize * frame.height as usize * 4;
        if frame.rgba.len() != len {
            return Err(ServiceError(format!(
                "frame data is {} bytes; {}×{} RGBA needs {len}",
                frame.rgba.len(),
                frame.width,
                frame.height
            )));
        }
        let texture = self.texture(state, frame.width, frame.height, OUT_FORMAT);
        write_rgba(&self.queue, &texture, &frame.rgba);
        Ok(texture)
    }

    fn check_size(&self, width: u32, height: u32) -> ServiceResult<()> {
        if width == 0 || height == 0 || width > self.max_size || height > self.max_size {
            return Err(ServiceError(format!(
                "frame size {width}×{height} is outside 1..{} pixels",
                self.max_size
            )));
        }
        Ok(())
    }

    /// The textures of `call`'s Image parameters, in binding order.
    fn images(&self, state: &mut State, call: &FxCall<'_>) -> Vec<wgpu::TextureView> {
        call.asset
            .params
            .iter()
            .filter(|p| matches!(p.kind, ParamKind::Image))
            .map(|p| {
                let path = call
                    .values
                    .get(&p.name)
                    .unwrap_or(&p.default)
                    .as_text()
                    .filter(|t| !t.trim().is_empty());
                match path {
                    Some(path) => self.image(state, PathBuf::from(path)),
                    None => self.blank.clone(),
                }
            })
            .collect()
    }

    /// An image file as a texture, reloaded when its mtime changes;
    /// transparent when it cannot be read.
    fn image(&self, state: &mut State, path: PathBuf) -> wgpu::TextureView {
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if let Some((m, view)) = state.images.get(&path)
            && *m == mtime
        {
            return view.clone();
        }
        let view = match image::open(&path) {
            Ok(img) => {
                let img = img.to_rgba8();
                let (w, h) = img.dimensions();
                if self.check_size(w, h).is_ok() {
                    upload(&self.device, &self.queue, w, h, &img)
                        .create_view(&wgpu::TextureViewDescriptor::default())
                } else {
                    self.blank.clone()
                }
            }
            Err(e) => {
                eprintln!("koo-gpu: image {}: {e}", path.display());
                self.blank.clone()
            }
        };
        if state.images.len() >= 64 {
            state.images.clear();
        }
        state.images.insert(path, (mtime, view.clone()));
        view
    }

    /// Record `passes` in order, each drawing into its target, submit, and
    /// read the last target back.
    fn run(
        &self,
        state: &mut State,
        passes: &[Pass<'_>],
        targets: &[&wgpu::Texture],
    ) -> ServiceResult<Frame> {
        let last = targets.last().expect("at least one pass");
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);

        // All uniform blocks in one buffer, one aligned slot per pass.
        let slot = passes
            .iter()
            .map(|p| (p.uniforms.len() as u64).next_multiple_of(self.uniform_align))
            .max()
            .unwrap_or(self.uniform_align);
        let need = slot * passes.len() as u64;
        if state.uniforms.as_ref().is_none_or(|b| b.size() < need) {
            state.uniforms = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("koo-gpu uniforms"),
                size: need.next_power_of_two().max(4096),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let uniforms = state.uniforms.clone().expect("just created");
        for (i, p) in passes.iter().enumerate() {
            self.queue
                .write_buffer(&uniforms, slot * i as u64, &p.uniforms);
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("koo-gpu"),
            });
        for (i, (pass, target)) in passes.iter().zip(targets).enumerate() {
            let program = match state.programs.get_mut(&pass.key) {
                Some(Ok(p)) => p,
                _ => return Err(ServiceError("program evicted mid-call".into())),
            };
            let mut entries = vec![
                wgpu::BindGroupEntry {
                    binding: wrap::BIND_UNIFORMS,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &uniforms,
                        offset: slot * i as u64,
                        size: NonZeroU64::new(pass.uniforms.len() as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: wrap::BIND_SAMPLER,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ];
            for (b, view) in input_bindings(program.wrapped.kind)
                .iter()
                .zip(&pass.inputs)
            {
                entries.push(wgpu::BindGroupEntry {
                    binding: *b,
                    resource: wgpu::BindingResource::TextureView(view),
                });
            }
            for (k, view) in pass.images.iter().enumerate() {
                entries.push(wgpu::BindGroupEntry {
                    binding: wrap::BIND_IMAGES + k as u32,
                    resource: wgpu::BindingResource::TextureView(view),
                });
            }
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("koo-gpu pass"),
                layout: &program.bind_layout,
                entries: &entries,
            });
            let pipeline = self.pipeline(program, target.format());
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("koo-gpu pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            rpass.set_pipeline(pipeline);
            rpass.set_bind_group(0, &bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }
        self.read_back(state, encoder, last, scope)
    }

    /// Copy `texture` out after the commands in `encoder`, submit, check
    /// `scope` for errors and wait for the pixels.
    fn read_back(
        &self,
        state: &mut State,
        mut encoder: wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        scope: wgpu::ErrorScopeGuard,
    ) -> ServiceResult<Frame> {
        let (width, height) = (texture.width(), texture.height());
        let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let size = row as u64 * height as u64;
        if state.readback.as_ref().is_none_or(|b| b.size() < size) {
            state.readback = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("koo-gpu readback"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let readback = state.readback.clone().expect("just created");

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
            extent(width, height),
        );
        let index = self.queue.submit([encoder.finish()]);
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(ServiceError(format!("GPU error: {e}")));
        }

        let slice = readback.slice(..size);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(index),
                timeout: None,
            })
            .map_err(ServiceError::new)?;
        rx.recv()
            .map_err(ServiceError::new)?
            .map_err(ServiceError::new)?;
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        {
            let data = slice.get_mapped_range();
            for y in 0..height as usize {
                let start = y * row as usize;
                rgba.extend_from_slice(&data[start..start + width as usize * 4]);
            }
        }
        readback.unmap();
        Ok(Frame {
            width,
            height,
            rgba: rgba.into(),
        })
    }

    fn uniforms(state: &State, key: u64, call: &FxCall<'_>, width: u32, height: u32) -> Vec<u8> {
        let Some(Ok(program)) = state.programs.get(&key) else {
            return Vec::new();
        };
        program.wrapped.uniforms(
            call.asset,
            call.values,
            Clock {
                width,
                height,
                time: call.time,
                duration: call.duration,
            },
        )
    }
}

fn expect_kind(asset: &FxAsset, kind: FxKind) -> ServiceResult<()> {
    if asset.kind == kind {
        Ok(())
    } else {
        Err(ServiceError(format!(
            "`{}` is a {}, not a {}",
            asset.name,
            asset.kind.label().to_lowercase(),
            kind.label().to_lowercase()
        )))
    }
}

fn input_bindings(kind: FxKind) -> &'static [u32] {
    match kind {
        FxKind::Effect => &[wrap::BIND_TEX0],
        FxKind::Transition => &[wrap::BIND_TEX0, wrap::BIND_TEX1],
        FxKind::Generator => &[],
    }
}

fn extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}

fn write_rgba(queue: &wgpu::Queue, texture: &wgpu::Texture, rgba: &[u8]) {
    queue.write_texture(
        texture.as_image_copy(),
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(texture.width() * 4),
            rows_per_image: Some(texture.height()),
        },
        texture.size(),
    );
}

/// A sampled-only RGBA8 texture holding `rgba`.
fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("koo-gpu image"),
        size: extent(width, height),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OUT_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    write_rgba(queue, &texture, rgba);
    texture
}

impl FxService for GpuFx {
    fn check(&self, asset: &FxAsset) -> ServiceResult<()> {
        asset.validate().map_err(ServiceError)?;
        let mut state = self.lock();
        self.program(&mut state, asset)
            .map(|_| ())
            .map_err(ServiceError)
    }

    /// Runs on the GPU from one upload to one readback; an empty chain
    /// returns the frame unchanged.
    fn apply(&self, frame: &Frame, chain: &[FxCall<'_>]) -> ServiceResult<Frame> {
        if chain.is_empty() {
            return Ok(frame.clone());
        }
        let mut state = self.lock();
        let (w, h) = (frame.width, frame.height);
        let mut keys = Vec::with_capacity(chain.len());
        for call in chain {
            expect_kind(call.asset, FxKind::Effect)?;
            keys.push(
                self.program(&mut state, call.asset)
                    .map_err(|e| ServiceError(format!("{}: {e}", call.asset.name)))?,
            );
        }
        let input = self.upload_frame(&mut state, frame)?;
        // Ping-pong between two float textures; the last pass writes RGBA8.
        let mids: Vec<wgpu::Texture> = (0..chain.len().saturating_sub(1).min(2))
            .map(|_| self.texture(&mut state, w, h, MID_FORMAT))
            .collect();
        let output = self.texture(&mut state, w, h, OUT_FORMAT);
        let input_view = input.create_view(&Default::default());
        let mid_views: Vec<_> = mids
            .iter()
            .map(|t| t.create_view(&Default::default()))
            .collect();
        let mut passes = Vec::with_capacity(chain.len());
        let mut targets = Vec::with_capacity(chain.len());
        for (i, (call, key)) in chain.iter().zip(&keys).enumerate() {
            let source = if i == 0 {
                &input_view
            } else {
                &mid_views[(i - 1) % 2]
            };
            targets.push(if i + 1 == chain.len() {
                &output
            } else {
                &mids[i % 2]
            });
            passes.push(Pass {
                key: *key,
                uniforms: Self::uniforms(&state, *key, call, w, h),
                inputs: vec![source],
                images: self.images(&mut state, call),
            });
        }
        let result = self.run(&mut state, &passes, &targets);
        drop(passes);
        Self::give_back(&mut state, mids.into_iter().chain([input, output]));
        result
    }

    /// Frames are expected to be the same size. If they differ, the output
    /// takes `to`'s size and `from` is stretched to fit.
    fn transition(&self, from: &Frame, to: &Frame, call: &FxCall<'_>) -> ServiceResult<Frame> {
        expect_kind(call.asset, FxKind::Transition)?;
        let mut state = self.lock();
        let key = self
            .program(&mut state, call.asset)
            .map_err(|e| ServiceError(format!("{}: {e}", call.asset.name)))?;
        let a = self.upload_frame(&mut state, from)?;
        let b = self.upload_frame(&mut state, to)?;
        let output = self.texture(&mut state, to.width, to.height, OUT_FORMAT);
        let (va, vb) = (
            a.create_view(&Default::default()),
            b.create_view(&Default::default()),
        );
        let pass = Pass {
            key,
            uniforms: Self::uniforms(&state, key, call, to.width, to.height),
            inputs: vec![&va, &vb],
            images: self.images(&mut state, call),
        };
        let result = self.run(&mut state, std::slice::from_ref(&pass), &[&output]);
        drop(pass);
        Self::give_back(&mut state, [a, b, output]);
        result
    }

    fn generate(&self, call: &FxCall<'_>, width: u32, height: u32) -> ServiceResult<Frame> {
        expect_kind(call.asset, FxKind::Generator)?;
        self.check_size(width, height)?;
        let mut state = self.lock();
        let key = self
            .program(&mut state, call.asset)
            .map_err(|e| ServiceError(format!("{}: {e}", call.asset.name)))?;
        let output = self.texture(&mut state, width, height, OUT_FORMAT);
        let pass = Pass {
            key,
            uniforms: Self::uniforms(&state, key, call, width, height),
            inputs: Vec::new(),
            images: self.images(&mut state, call),
        };
        let result = self.run(&mut state, std::slice::from_ref(&pass), &[&output]);
        Self::give_back(&mut state, [output]);
        result
    }

    /// One pass per layer on the GPU; matches the CPU reference.
    fn composite(
        &self,
        width: u32,
        height: u32,
        background: [u8; 4],
        layers: &[CompositeLayer<'_>],
    ) -> ServiceResult<Frame> {
        self.composite_layers(width, height, background, layers)
    }
}
