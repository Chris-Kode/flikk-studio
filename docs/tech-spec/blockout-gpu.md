# Blockouts on the GPU

## Status

Pull request #61 added this design in November 2026.

- Renderer and shaders:
  [`crates/flikk-core/src/blockout/`](../../crates/flikk-core/src/blockout/)
  (`renderer.rs`, `gpu.rs` and the `.wgsl` files)
- Engine: [`crates/flikk-gpu/src/compute.rs`](../../crates/flikk-gpu/src/compute.rs)

## Problem

Only a CPU ray tracer drew the blockout scenes. It used a maximum of 16
threads for each frame. A full frame at clip size (1280 px) took 100 ms
to 300 ms on four cores. Thus the preview decreased its picture width to
keep its speed. A clip of some seconds took almost a minute.

## Decision

The ray tracer now also runs as **WGSL compute** on the wgpu engine of
the app. The CPU renderer stays as the **reference and the fallback**.
The picture does not change. A caller does not know which renderer drew a
frame.

## How it works

### CPU work and GPU work

For each frame, the CPU calculates the position of each object: the
motion, the solids, the box tree and the camera. The result is a `Plan`.
The code writes the `Plan` into flat buffers. The shaders trace the rays
through these buffers.

| Work that stays on the CPU | Reason |
| --- | --- |
| Motion, solids and box tree for each frame | The work is small and has many branches. The review code also uses it. |
| `Scene::thing_at` and `review.rs` | The queries are very small. |
| Guide lines of the outside view | One `stroke` function serves the two renderers. It uses the depth that the GPU returns. |

### One rule in one place

Each module writes its data for the GPU adjacent to its shader. The
shader gets its constants from the Rust code. Thus the CPU code, the data
layout and the GPU code of a shape are together.

| Module | Data | Shader |
| --- | --- | --- |
| `solid.rs` | Solids of 96 bytes, followed by their holes | `solid.wgsl`: the nine shapes, taper and holes |
| `land.rs` | Height field | `land.wgsl`: the ray march |
| `world.rs` | Box tree nodes of 32 bytes, camera and sun | `world.wgsl`: the tree search for camera rays, sun rays and `crowded` |
| `render.rs` | Grey table, spiral and picture settings | `render.wgsl`: the stages of a picture |

The steps are the same as the stages of the CPU renderer:

- Draft: `look_all`, then `draft_out`.
- Full: `look_all`, `sun_all`, `open_all` (half size), `finish_all`, then
  `full_out`.
- Pass (depth, lines or things): `look_all`, then `pass_out`.

### Engine

`flikk-core` cannot depend on wgpu. Thus it uses a general
`ComputeService` trait (`services.rs`). A request contains the WGSL
source, the uniforms, the read-only inputs, the scratch buffers, the
steps in sequence and the number of bytes to return.

`flikk-gpu` implements the trait on the device of the effects engine. It
uses the pipeline cache, the buffer pool, the limit checks, the error
scopes and the lost-device recovery of that engine. It has no data about
blockouts.

### Entry point

`Services.blockout` is a `blockout::Renderer`. It is always available.
The default renderer draws on the CPU.

```rust
renderer.render(&scene, t, size, Look::Shot(Detail::Full));        // asked work
renderer.render_watched(&scene, t, size, Look::Outside(from, d));  // on screen now
scene.write_clip(&renderer, size, &path, &cancel);                 // job in the list
```

- If the GPU returns an error or panics, the CPU draws that frame. Then
  the renderer does not use the GPU for 30 s.
- A frame uses a GPU turn only when it goes to the GPU. Refer to
  [GPU turns and the Jobs list](gpu-turns.md).
- `Scene::render`, `Scene::render_outside` and `Scene::render_pass` stay
  as the CPU reference.

## Alternatives that we rejected

- **Triangles and a rasterizer.** The picture changes, because shadows
  and contact shade need different methods. Holes in solids need new
  geometry code. The scenes use analytic solids, thus a ray tracer is the
  simpler port.
- **Fragment shader passes.** A ray tracer for each pixel needs many
  intermediate buffers. Compute with storage buffers is a better fit than
  render targets.
- **Removal of the CPU renderer.** A computer without a GPU adapter, a
  lost device and a frame that is too large all need it. It is also the
  reference for the GPU picture.
- **A GPU service only for blockouts.** With the general compute service,
  the shaders and their data layout stay in one crate.

## Verification

We compared the GPU picture with the CPU picture at 1280 × 720 on
software Vulkan (lavapipe). The 15 cases included:

- All nine shapes, tapers and a wall with holes.
- Hills with 120 trees and a camera that an object carries.
- A crowd of 5,580 solids.
- The outside view with guide lines.
- The three passes.

The results were:

- Flat areas differ by a maximum of 1/255. The cause is dither.
- A maximum of 0.019% of the pixels differ by more than 2/255. All of
  them are on edges where solids touch. The cause is float precision.
- A clip of 144 frames went through the GPU as a job in the list.
- A frame that was too large went to the CPU. The 30 s delay operated
  correctly.

We did not do these checks:

- Vulkan, Metal or DX12 hardware. Software Vulkan is slower than the CPU
  renderer. Thus we have no real times.
- A lost device during a blockout frame.

## Open items

- **Windows on DX12.** If a computer uses DX12 with the FXC compiler, the
  first frame can take a long time to compile. Monitor the first blockout
  on Windows.
- **Frame size.** A frame of more than approximately 2.8 megapixels is
  larger than the 128 MiB storage limit. The renderer records it as a GPU
  failure, and the 30 s delay applies to all sizes. The largest frame
  today is the clip, at 0.92 megapixels.
- **No adapter at start.** The app does not try again. The effects engine
  has the same behaviour.
- **Speed control.** After slow frames, the view decreases its picture
  width with the two renderers. A possible change is to keep the full
  width when the GPU draws.
