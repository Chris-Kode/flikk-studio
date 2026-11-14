//! Compute programs ([`ComputeService`]) on the engine's device: a WGSL
//! program is compiled once per device (on the compiler's thread, see
//! [`COMPILE_STACK`]), its pipelines kept by entry point; buffers are
//! pooled by use and size, so a program run every frame reuses them. A run
//! writes its inputs, records one compute pass per step, copies what it
//! asks for into a readback buffer and waits for it. Anything the device
//! refuses (a limit, a program, a lost device) fails the run, which the
//! caller answers by doing the work another way.

use crate::{COMPILE_STACK, Gpu, GpuFx, State};
use flikk_core::services::{ComputeJob, ComputeService, ServiceError, ServiceResult};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::mpsc;

/// Pooled compute buffers kept before the pool is cleared.
const MAX_BUFFERS: usize = 32;
/// The smallest buffer made: small ones are rounded up to it.
const LEAST_BUFFER: u64 = 256;

/// A compiled program: its bindings and its pipelines by entry point.
pub(crate) struct Program {
    module: wgpu::ShaderModule,
    bind_layout: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    pipelines: HashMap<&'static str, wgpu::ComputePipeline>,
}

/// Compute programs and buffers kept on one device.
#[derive(Default)]
pub(crate) struct Kept {
    programs: HashMap<u64, Program>,
    buffers: Vec<wgpu::Buffer>,
}

impl ComputeService for GpuFx {
    fn run(&self, job: &ComputeJob<'_>) -> ServiceResult<Vec<u8>> {
        self.gpu()?.run(job)
    }
}

impl Gpu {
    fn run(&self, job: &ComputeJob<'_>) -> ServiceResult<Vec<u8>> {
        self.fits(job)?;
        let pipelines = self.pipelines(job)?;
        let usage = wgpu::BufferUsages::COPY_DST;
        let storage = wgpu::BufferUsages::STORAGE;
        let uniforms = self.buffer(
            wgpu::BufferUsages::UNIFORM | usage,
            job.uniforms.len() as u64,
        );
        let inputs: Vec<_> = (job.inputs.iter())
            .map(|input| self.buffer(storage | usage, input.len() as u64))
            .collect();
        let scratch: Vec<_> = (job.scratch.iter())
            .map(|size| self.buffer(storage | wgpu::BufferUsages::COPY_SRC, *size))
            .collect();
        let read = job.read.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        let readback = self.buffer(wgpu::BufferUsages::MAP_READ | usage, read);
        let ran = self.dispatch(job, &pipelines, (&uniforms, &inputs, &scratch, &readback))?;
        // Kept only after a run that went through: a failed one may leave
        // its readback mapped.
        let mut state = self.lock();
        for buffer in [uniforms, readback]
            .into_iter()
            .chain(inputs)
            .chain(scratch)
        {
            kept_buffers(&mut state).push(buffer);
        }
        Ok(ran)
    }

    /// Whether `job` is within what the device takes.
    fn fits(&self, job: &ComputeJob<'_>) -> ServiceResult<()> {
        let limits = self.device.limits();
        let refuse = |what: String| Err(ServiceError(format!("{}: {what}", job.label)));
        let storage =
            (job.inputs.iter().map(|input| input.len() as u64)).chain(job.scratch.iter().copied());
        if job.inputs.len() + job.scratch.len()
            > limits.max_storage_buffers_per_shader_stage as usize
        {
            return refuse("more storage buffers than the GPU binds".into());
        }
        let most = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        if let Some(size) = storage.into_iter().find(|size| *size > most) {
            return refuse(format!("a buffer of {size} bytes, past the GPU's {most}"));
        }
        if job.uniforms.len() as u64 > limits.max_uniform_buffer_binding_size {
            return refuse("uniforms past the GPU's limit".into());
        }
        let groups = limits.max_compute_workgroups_per_dimension;
        if (job.steps.iter()).any(|step| step.groups.iter().any(|count| *count > groups)) {
            return refuse(format!("more than {groups} workgroups across"));
        }
        if job.read > job.scratch.last().copied().unwrap_or(0) {
            return refuse("reads past its last buffer".into());
        }
        Ok(())
    }

    /// `job`'s pipelines in the order of its steps, compiled where missing.
    fn pipelines(
        &self,
        job: &ComputeJob<'_>,
    ) -> ServiceResult<(wgpu::BindGroupLayout, Vec<wgpu::ComputePipeline>)> {
        let key = program_key(job);
        let found = |kept: &mut Kept| {
            let program = kept.programs.get(&key)?;
            let pipelines = (job.steps.iter())
                .map(|step| program.pipelines.get(step.entry).cloned())
                .collect::<Option<Vec<_>>>()?;
            Some((program.bind_layout.clone(), pipelines))
        };
        let mut state = self.lock();
        if let Some(found) = found(kept(&mut state)) {
            return Ok(found);
        }
        let had = kept(&mut state).programs.remove(&key);
        // Compiled without the lock, so other work goes on meanwhile.
        drop(state);
        let compiled = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("flikk-gpu compile".into())
                .stack_size(COMPILE_STACK)
                .spawn_scoped(scope, || self.compile_program(job, had))
                .map_err(|e| format!("could not start the shader compiler: {e}"))?
                .join()
                .unwrap_or_else(|_| Err("the shader compiler crashed".into()))
        });
        let program = compiled.map_err(|e| ServiceError(format!("{}: {e}", job.label)))?;
        let mut state = self.lock();
        let kept = kept(&mut state);
        kept.programs.insert(key, program);
        found(kept).ok_or_else(|| ServiceError(format!("{}: a pipeline went missing", job.label)))
    }

    /// `had`, or a new program of `job`'s source, with a pipeline for each
    /// of its steps. Errors are not kept: the caller tries again later.
    fn compile_program(
        &self,
        job: &ComputeJob<'_>,
        had: Option<Program>,
    ) -> Result<Program, String> {
        let device = &self.device;
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut program = match had {
            Some(program) => program,
            None => {
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(job.label),
                    source: wgpu::ShaderSource::Wgsl(job.source.into()),
                });
                let buffer = |binding, ty| wgpu::BindGroupLayoutEntry {
                    binding,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                };
                let read_only = |read_only| wgpu::BufferBindingType::Storage { read_only };
                let mut entries = vec![buffer(0, wgpu::BufferBindingType::Uniform)];
                let storage =
                    (job.inputs.iter().map(|_| true)).chain(job.scratch.iter().map(|_| false));
                for (i, only_read) in storage.enumerate() {
                    entries.push(buffer(1 + i as u32, read_only(only_read)));
                }
                let bind_layout =
                    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some(job.label),
                        entries: &entries,
                    });
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some(job.label),
                    bind_group_layouts: &[Some(&bind_layout)],
                    immediate_size: 0,
                });
                Program {
                    module,
                    bind_layout,
                    layout,
                    pipelines: HashMap::new(),
                }
            }
        };
        for step in job.steps {
            if program.pipelines.contains_key(step.entry) {
                continue;
            }
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(step.entry),
                layout: Some(&program.layout),
                module: &program.module,
                entry_point: Some(step.entry),
                compilation_options: Default::default(),
                cache: None,
            });
            program.pipelines.insert(step.entry, pipeline);
        }
        match pollster::block_on(scope.pop()) {
            Some(e) => Err(format!("the GPU rejected the program: {e}")),
            None => Ok(program),
        }
    }

    /// A buffer for `usage` of at least `size` bytes, from the pool.
    fn buffer(&self, usage: wgpu::BufferUsages, size: u64) -> wgpu::Buffer {
        let most = (self.device.limits().max_buffer_size).max(LEAST_BUFFER);
        let size = size.max(LEAST_BUFFER).next_power_of_two().min(most);
        let mut state = self.lock();
        let buffers = kept_buffers(&mut state);
        if let Some(at) = (buffers.iter()).position(|b| b.usage() == usage && b.size() == size) {
            return buffers.swap_remove(at);
        }
        drop(state);
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flikk-gpu compute"),
            size,
            usage,
            mapped_at_creation: false,
        })
    }

    /// Write `job`'s inputs, run its steps and read back what it asks for.
    fn dispatch(
        &self,
        job: &ComputeJob<'_>,
        (bind_layout, pipelines): &(wgpu::BindGroupLayout, Vec<wgpu::ComputePipeline>),
        (uniforms, inputs, scratch, readback): (
            &wgpu::Buffer,
            &[wgpu::Buffer],
            &[wgpu::Buffer],
            &wgpu::Buffer,
        ),
    ) -> ServiceResult<Vec<u8>> {
        let device = &self.device;
        let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        write(&self.queue, uniforms, job.uniforms);
        for (buffer, bytes) in inputs.iter().zip(job.inputs) {
            write(&self.queue, buffer, bytes);
        }
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: uniforms.as_entire_binding(),
        }];
        for (i, buffer) in inputs.iter().chain(scratch).enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 1 + i as u32,
                resource: buffer.as_entire_binding(),
            });
        }
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(job.label),
            layout: bind_layout,
            entries: &entries,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some(job.label),
        });
        // A pass a step, so each sees what the one before wrote.
        for (step, pipeline) in job.steps.iter().zip(pipelines) {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(step.entry),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(step.groups[0], step.groups[1], 1);
        }
        let read = job.read.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        if let Some(last) = scratch.last()
            && read > 0
        {
            encoder.copy_buffer_to_buffer(last, 0, readback, 0, read);
        }
        let index = self.queue.submit([encoder.finish()]);
        let refused = pollster::block_on(scope.pop()).or(pollster::block_on(oom.pop()));
        if let Some(e) = refused {
            return Err(ServiceError(format!(
                "{}: the GPU refused it: {e}",
                job.label
            )));
        }
        if read == 0 {
            return Ok(Vec::new());
        }
        let slice = readback.slice(..read);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |mapped| {
            let _ = tx.send(mapped);
        });
        self.wait(index)?;
        rx.recv()
            .map_err(ServiceError::new)?
            .map_err(ServiceError::new)?;
        let mut bytes = slice.get_mapped_range().to_vec();
        readback.unmap();
        bytes.truncate(job.read as usize);
        Ok(bytes)
    }
}

/// What a program is kept by: its source and its bindings.
fn program_key(job: &ComputeJob<'_>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (job.source, job.inputs.len(), job.scratch.len()).hash(&mut hasher);
    hasher.finish()
}

fn kept(state: &mut State) -> &mut Kept {
    &mut state.compute
}

/// The pool, emptied first when it is full.
fn kept_buffers(state: &mut State) -> &mut Vec<wgpu::Buffer> {
    let buffers = &mut state.compute.buffers;
    if buffers.len() >= MAX_BUFFERS {
        buffers.clear();
    }
    buffers
}

/// `bytes` into the start of `buffer`, padded to the GPU's copy alignment.
fn write(queue: &wgpu::Queue, buffer: &wgpu::Buffer, bytes: &[u8]) {
    let padded = (bytes.len() as u64).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) as usize;
    match padded == bytes.len() {
        true => queue.write_buffer(buffer, 0, bytes),
        false => {
            let mut bytes = bytes.to_vec();
            bytes.resize(padded, 0);
            queue.write_buffer(buffer, 0, &bytes);
        }
    }
}
