//! GPU engine for GLSL effects, transitions and generators
//! ([`flikk_core::services::FxService`]).
//!
//! Each program is wrapped (see [`wrap`]) into a `#version 450` fragment
//! shader, compiled with naga's GLSL front end and drawn as a fullscreen
//! triangle into an offscreen texture. Frames go up as RGBA8 textures and
//! come back as RGBA8; a whole composed frame runs on the GPU between one
//! upload per source picture and one readback ([`batch`]), with 16-bit
//! float intermediates.
//!
//! The engine owns its own wgpu device (the same wgpu GPUI uses) and is
//! synchronous: a batch records its steps (locking briefly for programs and
//! textures), then submits under the lock and waits, so it can be used from
//! any thread. Compiled programs are cached by a hash of
//! their code, kind and parameter layout (compile errors too, so a broken
//! shader is not recompiled every frame); Image parameters are cached by
//! path and modification time; textures and buffers are pooled by size.
//! All of these belong to one device ([`Gpu`]): when wgpu reports it lost,
//! the next batch opens a new one with empty caches (see [`GpuFx::gpu`]).
//! Layer compositing for the program frame lives in [`composite`].

mod batch;
mod composite;
mod wrap;

use anyhow::Context as _;
use batch::Batch;
use flikk_core::fx::{FxAsset, FxKind, FxLanguage, ParamKind};
use flikk_core::services::{
    CompositeLayer, Frame, FxBatch, FxCall, FxService, ServiceError, ServiceResult,
};
use std::borrow::Cow;
use std::collections::HashMap;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use wrap::Wrapped;

/// Pixel format of uploads, of the last pass and of readbacks.
const OUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Pixel format between the passes of a chain.
const MID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Compiled programs kept before the cache is cleared.
const MAX_PROGRAMS: usize = 256;
/// Pooled textures kept before the pool is cleared.
const MAX_POOLED: usize = 32;
/// Stack of the thread programs compile on. naga's GLSL front end recurses
/// deeply (past the 512 KiB of GPUI's background threads in debug builds).
const COMPILE_STACK: usize = 16 << 20;
/// Least time between two tries at reopening a lost device, so a GPU gone
/// for good does not cost a full init every frame.
const REOPEN_EVERY: Duration = Duration::from_secs(3);

const VERTEX_WGSL: &str = r#"
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

/// The wgpu engine. Cheap to share behind an `Arc`.
pub struct GpuFx {
    slot: Mutex<Slot>,
}

struct Slot {
    /// `None` once the device was lost and could not be reopened yet.
    gpu: Option<Arc<Gpu>>,
    /// The last time a device was opened (or tried).
    tried: Instant,
}

/// One wgpu device and everything made on it. A batch keeps the `Gpu` it
/// started on, so its textures never reach a newer device's pool.
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    vertex: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    /// 1 × 1 transparent texture for missing images.
    blank: wgpu::TextureView,
    uniform_align: u64,
    max_size: u32,
    compositor: composite::Compositor,
    converter: batch::Converter,
    /// Set by wgpu's device-lost callback (and when a wait finds it lost).
    lost: Arc<AtomicBool>,
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
    readbacks: Vec<wgpu::Buffer>,
    uniforms: Option<wgpu::Buffer>,
}

impl GpuFx {
    /// Open the GPU. Fails when there is no adapter (no driver, headless
    /// without software Vulkan).
    pub fn new() -> anyhow::Result<Self> {
        let gpu = Gpu::open()?;
        Ok(Self {
            slot: Mutex::new(Slot {
                gpu: Some(Arc::new(gpu)),
                tried: Instant::now(),
            }),
        })
    }

    /// The current device. A lost one is dropped (with its programs, pools
    /// and buffers; batches in flight keep theirs until they end) and a new
    /// one opened, at most every [`REOPEN_EVERY`]; until then this fails and
    /// callers take the CPU path.
    fn gpu(&self) -> ServiceResult<Arc<Gpu>> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(gpu) = &slot.gpu {
            if !gpu.is_lost() {
                return Ok(gpu.clone());
            }
            slot.gpu = None;
        }
        if slot.tried.elapsed() < REOPEN_EVERY {
            return Err(lost());
        }
        slot.tried = Instant::now();
        match Gpu::open() {
            Ok(gpu) => {
                eprintln!("flikk-gpu: reopened the GPU device");
                let gpu = Arc::new(gpu);
                slot.gpu = Some(gpu.clone());
                Ok(gpu)
            }
            Err(e) => {
                eprintln!("flikk-gpu: could not reopen the GPU device: {e:#}");
                Err(lost())
            }
        }
    }
}

fn lost() -> ServiceError {
    ServiceError("the GPU device was lost".into())
}

impl Gpu {
    fn open() -> anyhow::Result<Self> {
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
            label: Some("flikk-gpu"),
            required_limits: limits.clone(),
            ..Default::default()
        }))
        .context("could not open the GPU device")?;
        device.on_uncaptured_error(Arc::new(|e: wgpu::Error| {
            eprintln!("flikk-gpu: {e}");
        }));
        let lost = Arc::new(AtomicBool::new(false));
        let flag = lost.clone();
        device.set_device_lost_callback(move |reason, message| {
            eprintln!("flikk-gpu: device lost ({reason:?}): {message}");
            flag.store(true, Ordering::Release);
        });
        let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flikk-gpu fullscreen"),
            source: wgpu::ShaderSource::Wgsl(VERTEX_WGSL.into()),
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("flikk-gpu linear clamp"),
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
            converter: batch::Converter::new(&device),
            uniform_align: limits.min_uniform_buffer_offset_alignment as u64,
            max_size: limits.max_texture_dimension_2d,
            device,
            queue,
            vertex,
            sampler,
            blank,
            lost,
            state: Mutex::new(State::default()),
        })
    }

    fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Acquire)
    }

    /// Wait for `index` to finish. wgpu panics when the wait finds the
    /// device lost; that is caught here and the device marked lost.
    fn wait(&self, index: wgpu::SubmissionIndex) -> ServiceResult<()> {
        if self.is_lost() {
            return Err(lost());
        }
        let poll = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.device.poll(wgpu::PollType::Wait {
                submission_index: Some(index),
                timeout: None,
            })
        }));
        match poll {
            Ok(Ok(_)) if !self.is_lost() => Ok(()),
            Ok(Err(e)) => Err(ServiceError::new(e)),
            Ok(Ok(_)) | Err(_) => {
                self.lost.store(true, Ordering::Release);
                Err(lost())
            }
        }
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

    /// Compile `asset` into the cache (once per key, on a thread with room
    /// for the compiler, see [`COMPILE_STACK`]); its error otherwise.
    fn program(&self, state: &mut State, asset: &FxAsset) -> Result<u64, String> {
        let key = wrap::program_key(asset);
        if let Some(cached) = state.programs.get(&key) {
            return cached.as_ref().map(|_| key).map_err(Clone::clone);
        }
        // A compiler thread that could not start is not cached, nor is a
        // failure on a lost device: neither is the shader's fault.
        let compiled = std::thread::scope(|scope| {
            let compiler = std::thread::Builder::new()
                .name("flikk-gpu compile".into())
                .stack_size(COMPILE_STACK)
                .spawn_scoped(scope, || self.compile(asset))
                .map_err(|e| format!("could not start the shader compiler: {e}"))?;
            Ok::<_, String>(
                compiler
                    .join()
                    .unwrap_or_else(|_| Err("the shader compiler crashed".into())),
            )
        })?;
        if let Err(e) = &compiled
            && self.is_lost()
        {
            return Err(e.clone());
        }
        match state.programs.entry(key).or_insert(compiled) {
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
                label: Some("flikk-gpu program"),
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
        if let Some(binding) = wrapped.depth {
            entries.push(texture(binding));
        }
        let bind_layout = self
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("flikk-gpu program"),
                entries: &entries,
            });
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("flikk-gpu program"),
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
                    label: Some("flikk-gpu program"),
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
            label: Some("flikk-gpu frame"),
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
                eprintln!("flikk-gpu: image {}: {e}", path.display());
                self.blank.clone()
            }
        };
        if state.images.len() >= 64 {
            state.images.clear();
        }
        state.images.insert(path, (mtime, view.clone()));
        view
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
        label: Some("flikk-gpu image"),
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
        let gpu = self.gpu()?;
        let mut state = gpu.lock();
        gpu.program(&mut state, asset)
            .map(|_| ())
            .map_err(ServiceError)
    }

    fn batch(&self) -> Box<dyn FxBatch + '_> {
        match self.gpu() {
            Ok(gpu) => Box::new(Batch::new(gpu)),
            Err(e) => Box::new(batch::Lost(e.0)),
        }
    }

    /// Runs on the GPU from one upload to one readback; an empty chain
    /// returns the frame unchanged.
    fn apply(&self, frame: &Frame, chain: &[FxCall<'_>]) -> ServiceResult<Frame> {
        if chain.is_empty() {
            return Ok(frame.clone());
        }
        let mut batch = self.batch();
        let input = batch.upload(frame)?;
        let output = batch.apply(input, chain)?;
        batch.finish(output)
    }

    /// Frames are expected to be the same size. If they differ, the output
    /// takes `to`'s size and `from` is stretched to fit.
    fn transition(&self, from: &Frame, to: &Frame, call: &FxCall<'_>) -> ServiceResult<Frame> {
        let mut batch = self.batch();
        let (a, b) = (batch.upload(from)?, batch.upload(to)?);
        let output = batch.transition(a, b, call)?;
        batch.finish(output)
    }

    fn generate(&self, call: &FxCall<'_>, width: u32, height: u32) -> ServiceResult<Frame> {
        let mut batch = self.batch();
        let output = batch.generate(call, width, height)?;
        batch.finish(output)
    }

    /// One pass per layer on the GPU; matches the CPU reference.
    fn composite(
        &self,
        width: u32,
        height: u32,
        background: [u8; 4],
        layers: &[CompositeLayer<'_>],
    ) -> ServiceResult<Frame> {
        let mut batch = self.batch();
        let mut placed = Vec::with_capacity(layers.len());
        for l in layers {
            placed.push((batch.upload(l.frame)?, l.placement));
        }
        let output = batch.composite(width, height, background, &placed)?;
        batch.finish(output)
    }
}
