# Blockouts on the GPU

## Status

Shipped in pull request #61 (November 2026). Code:
[`crates/flikk-core/src/blockout/`](../../crates/flikk-core/src/blockout/)
(`renderer.rs`, `gpu.rs`, the `.wgsl` files) and
[`crates/flikk-gpu/src/compute.rs`](../../crates/flikk-gpu/src/compute.rs).

## Problem

Blockout scenes were drawn only by a CPU ray tracer, up to 16 threads a
frame. A full frame at clip size (1280 px) took 100 to 300 ms on four
cores, so the preview narrowed its picture to keep up, and a clip of a few
seconds took most of a minute to write.

## Decision

Port the existing ray tracer to **WGSL compute** on the wgpu engine the
app already has, and keep the CPU renderer as the **reference and the
fallback**. The picture stays the same; a caller never knows which drew a
frame.

## How it works

### Split between CPU and GPU

The CPU still works out, per frame, where everything stands: motion,
solids, the box tree, the camera (`World`, wrapped as a `Plan`). That is
written flat into buffers and the shaders trace it.

| Stays on the CPU | Why |
| --- | --- |
| Motion, solids, box tree per frame | Small, branchy, and shared with the reviewer |
| `Scene::thing_at`, `review.rs` | Tiny queries |
| Guide lines of the outside view | One `stroke` for both renderers, over the depth the GPU reads back |

### One rule, one place

Each module packs its own data beside its own shader, and the shader gets
its constants from the Rust code, so a shape's intersection has its CPU
code, its packing and its GPU code side by side:

| Module | Packs | Shader |
| --- | --- | --- |
| `solid.rs` | 96-byte solids, holes appended | `solid.wgsl`: the nine shapes, taper, holes |
| `land.rs` | Height field | `land.wgsl`: the ray march |
| `world.rs` | 32-byte box-tree nodes, camera, sun | `world.wgsl`: traversal for camera rays, sun rays and `crowded` |
| `render.rs` | Grey table, spiral, picture settings | `render.wgsl`: the stages of a picture |

The steps mirror the CPU's stages: `look_all`, then `draft_out`; or
`sun_all`, `open_all` (half size), `finish_all`, `full_out`; or
`pass_out` for the depth, lines and things passes.

### Engine

`flikk-core` cannot depend on wgpu. It asks for a generic
`ComputeService` (`services.rs`): WGSL source, uniforms, read-only
inputs, scratch buffers, ordered steps, bytes to read back. `flikk-gpu`
implements it on the effects engine's device, with its pipeline cache,
buffer pool, limit checks, error scopes and lost-device recovery. It
knows nothing about blockouts.

### Entry point

`Services.blockout` is a `blockout::Renderer`, always present; the
default draws on the CPU.

```rust
renderer.render(&scene, t, size, Look::Shot(Detail::Full));        // asked work
renderer.render_watched(&scene, t, size, Look::Outside(from, d));  // on screen now
scene.write_clip(&renderer, size, &path, &cancel);                 // listed job
```

- Any GPU error or panic draws that frame on the CPU and leaves the GPU
  alone for 30 s.
- A frame takes a GPU turn only when it actually goes to the GPU
  ([GPU turns](gpu-turns.md)).
- `Scene::render`, `render_outside` and `render_pass` remain the CPU
  reference.

## Turned down

- **Triangles and a rasteriser.** A different look (shadow maps, screen
  space effects), and holes cut out of solids would need real geometry
  work. The scenes are analytic solids; tracing them is the simpler port
  and keeps the picture.
- **Fragment-shader passes.** Compute with storage buffers fits a
  per-pixel tracer with several intermediate sheets better than render
  targets.
- **Dropping the CPU renderer.** Machines without an adapter, lost
  devices and oversized frames still need it, and it is what the GPU
  picture is checked against.
- **A blockout-specific GPU service.** The generic compute service keeps
  the shaders and their data layout in one crate.

## Verified

Against the CPU renderer at 1280 × 720 on software Vulkan (lavapipe), 15
cases: every shape, tapers, a wall with holes, hills with 120 trees and a
parented camera, a 5,580-solid crowd, the outside view with guides, and
the three passes.

- Flat areas differ by at most 1/255 (dither).
- At most 0.019% of pixels differ by more than 2/255, all on edges where
  solids touch (float rounding).
- A 144-frame clip went through the GPU as a listed job; an oversized
  frame fell back to the CPU and the back-off held.

Not verified:

- Vulkan, Metal or DX12 hardware. Software Vulkan is slower than the CPU
  renderer, so there are no real timings yet.
- A lost device during a blockout frame.

## Open

- **Windows on DX12.** If a machine ends up on DX12 with the FXC
  compiler, the first frame may take long to compile. Watch the first
  blockout opened on Windows.
- **Frame size.** A frame over about 2.8 megapixels exceeds the 128 MiB
  storage binding and counts as a GPU failure, with the 30 s back-off for
  every size. Nothing reaches it today (the clip is 0.92 megapixels).
- **No adapter at start-up** is not retried, as for effects.
- **Pacing.** The view keeps narrowing its picture after slow frames on
  either renderer; it could hold full width whenever the GPU draws.
