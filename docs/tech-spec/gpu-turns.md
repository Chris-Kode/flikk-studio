# GPU turns and the Jobs list

## Status

Pull request #61 added this design in November 2026.

- Scheduler: [`crates/flikk-os/src/gpu.rs`](../../crates/flikk-os/src/gpu.rs)
- List: [`crates/flikk/src/ui/jobs.rs`](../../crates/flikk/src/ui/jobs.rs)

## Problem

No module controlled the work on the GPU. Playback, export, the render
cache and the models used the GPU at the same time. There was only one
rule: the render cache stopped during playback and export. The user did
not see the jobs.

The solution had two requirements:

- Each job gets time on the GPU. No job uses all of the GPU, and no job
  waits without end.
- The design accepts new types of GPU work. A new type does not change
  the scheduler or the list.

## Decision

GPU work uses **turns**. Before each unit of work, the code asks the
module `flikk_os::gpu` for a turn. Each crate can use this module. The
scheduler and the list of jobs are the same module. Thus a job that uses
turns is in the list automatically.

The GPU users have different APIs: wgpu, ONNX Runtime, whisper.cpp and
MLX. Each API has its own device or context. No queue on the GPU side
connects them. Thus the control is on the CPU side, before a unit starts.

Turns are **advisory**. They divide the GPU time between the jobs. If two
units run at the same time, the result is still correct. Thus an error in
the use of turns causes an overlap. It does not stop the work.

## How it works

### Levels

| Level | Work | Behaviour |
| --- | --- | --- |
| Watched | The picture that the user sees now: playback, a scrubbed frame, the blockout view, a subject pick | It does not wait. `gpu::watching()` puts a mark on the GPU. The mark stops 250 ms after the last watched frame. |
| Asked | Work that the user started and waits for: an export, a blockout clip, subject tracking, the agent frames | One unit at a time, in the sequence of arrival. Two asked jobs alternate. |
| Background | Work that no person waits for: the render cache, render-ahead, analysis before use | One unit at a time. It waits while the GPU has the watched mark. It gets one turn in four when asked work runs. |

### Units, jobs and the list

- A **turn** is one unit: one frame or one inference. Decode, disk and
  network operations stay out of the turn when that is possible.
- A **job** (`gpu::Job`) is long work that uses many turns. It has a
  label for the user ("Export: Interview.mp4") and optional progress.
- The list shows a job from its first turn request until the code drops
  it. Work that runs on the CPU does not ask for a turn. The list does
  not show it.
- Short work uses `gpu::turn` and has no row. Examples are render-ahead
  and one agent frame.
- Some engines know if their work runs on the GPU. The caller gives such
  an engine a `gpu::Unit`. The engine uses it only for GPU work.

### How to add GPU work

```rust
let job = gpu::Job::new("Export: My film.mp4", gpu::Level::Asked);
for frame in frames {
    let Some(_turn) = job.turn(&cancel) else { break }; // None after a cancel
    job.progress(fraction);
    render(frame);
}

let _watching = gpu::watching(); // watched work: it does not wait
```

1. Open a job with a label and a level.
2. Ask for a turn before each unit.
3. Report the progress, if you know it.

No other step is necessary. The Jobs list reads `gpu::jobs()` each
250 ms. It draws again when the list changes.

### Protection

| Condition | Result |
| --- | --- |
| A panic, a cancel or an early return occurs during a turn | The drop of the turn releases the GPU. The drop of the job removes its row. |
| The code waits for a turn | The code examines the cancel flag each 50 ms. A cancelled job is not a failed job. |
| A thread asks for a turn while it has a turn | The thread gets the same turn. |
| A thread with a turn waits for a different thread that needs the GPU | `gpu::aside` gives the turn back during the wait. The thread gets the turn again first. |
| A unit continues for more than 5 s | The subsequent job takes the turn. The two units overlap. |
| The code keeps a watched mark in error | The mark stops after 2 s. |

### Work that uses turns

| Work | Level | Unit | Row |
| --- | --- | --- | --- |
| Program playback and scrub | Watched | Frame | None |
| Export | Asked | Composed frame | "Export: file" |
| Render cache | Background | Composed frame | "Render cache" |
| Viewer render-ahead | Background | Frame | None |
| Blockout view | Watched | Frame | None |
| Blockout clip | Asked | Frame | "Blockout clip" |
| Depth for a render | Asked or background, as the caller specifies | Inference | "Depth: file" |
| Depth and slow motion for the viewer | Watched | Inference | None |
| Slow motion for a render | The turn of the frame | Inference | None |
| Transcription (Whisper with GPU support) | The lane of the analysis job | One speech segment | "Transcription: file" |
| Subject tracking (MLX, macOS) | Asked | Frame | "Track subject: file" |
| Subject pick and hover | Watched | Frame | None |
| Agent frames and previews | Asked. A stop of the agent turn stops them. | Frame | None |

The agent jobs run on a `JobQueue`, a maximum of four at a time. Before
this change, each agent job had its own thread.

## Alternatives that we rejected

- **A queue in each engine.** The units are smaller. But an engine does
  not know who waits for its work. Pipelined frames need two turns at the
  same time. Each new engine needs its own integration.
- **A lock.** With a lock, an error in the use of turns stops the work.
  With advisory turns, the worst result is an overlap.
- **Watched work in the queue.** A background inference can continue for
  seconds. Playback must not wait for it.
- **One list for all job types.** The analysis jobs (`JobStatus`), the
  render queue and the turns stay as three systems. Refer to Open items.
- **FFmpeg hardware decode and encode.** They run in other processes.
  They use the video blocks of the GPU. A software fallback is available.

## Verification

We did these checks:

- Format, lint and build are clean on Linux. The project has no tests.
- Temporary programs called `flikk_os::gpu` from outside the repository.
  They showed these results:
  - Asked jobs alternate.
  - Background work gets one turn in four.
  - Background work waits during watched work.
  - A cancelled wait stops cleanly.
  - A holder that panics releases the turn.
  - A second request on the same thread gets the same turn.
  - A different job takes the turn after 5 s.
  - With `gpu::aside`, a wait takes 0.1 s. Without it, the wait takes 5 s.
- The app showed the Jobs list with no jobs.
- On software Vulkan and WebGPU, the list showed a blockout clip and a
  depth job with progress.

We did not do these checks:

- The MLX subject mask changes. They are for macOS only, and no compiler
  built them.
- Slow motion, Whisper on a GPU and the agent jobs in operation.
- Rows with real jobs in the app on real hardware.

## Open items

- The name of the list is Jobs, but the list shows only GPU work. It does
  not show CPU work, such as person masks and stabilization.
- One speech segment is one Whisper unit. A segment can continue for more
  than 5 s. Then other work overlaps it.
- Whisper with GPU support always uses turns. This also occurs when
  Whisper runs on the CPU.
- The viewer makes depth and slow motion frames around a stopped
  playhead. During this time, background work does not use the GPU.
- The limit of four agent jobs includes jobs that wait for the network.
- Two operations do not accept a cancel: a blockout clip after the
  approval of a shot, and a frame grab for a generation.
