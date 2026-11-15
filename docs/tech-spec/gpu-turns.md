# GPU turns and the Jobs list

## Status

Shipped in pull request #61 (November 2026). Code:
[`crates/flikk-os/src/gpu.rs`](../../crates/flikk-os/src/gpu.rs), list in
[`crates/flikk/src/ui/jobs.rs`](../../crates/flikk/src/ui/jobs.rs).

## Problem

Nothing coordinated GPU work. Playback, export, the background render
cache, depth maps, slow motion, transcription, subject masks and the
agent's previews could all reach the GPU at once. The only rule was that
the render cache stopped while the user played or exported. The user also
had no way to see what was running or waiting.

Two requirements came with the fix:

- every job gets its moment: nothing saturates the GPU and nothing is
  starved;
- it scales: a new kind of GPU work joins without changes to the
  scheduler or to the list.

## Decision

GPU work takes **turns**, coordinated on the CPU side before each unit of
work, in one small module that every crate can reach (`flikk_os::gpu`).
The scheduler and the list of jobs are the same module: a job is listed
because it takes turns.

The GPU users sit on separate APIs (wgpu, ONNX Runtime with Core ML,
DirectML or WebGPU, whisper.cpp, MLX), each with its own device or
context. There is no shared GPU-side queue to use, so the only place they
can be coordinated is before each one starts a unit.

Turns are **advisory**. They share the GPU out; nothing is incorrect when
two units overlap, which is what happened all the time before. So every
failure of turn-taking degrades to overlap, never to a stall.

## How it works

### Levels

| Level | For | Behaviour |
| --- | --- | --- |
| Watched | What the user looks at now: playback, a scrubbed frame, the blockout view, the viewer's depth and slow-motion frames, picking a subject | Never waits. `gpu::watching()` only marks the GPU as watched; the mark lapses 250 ms after the last watched frame |
| Asked | Work the user asked for and waits on: export, a blockout clip, depth for a render, subject tracking, the agent's frames | One unit at a time, in arrival order, so two asked jobs alternate |
| Background | Work nobody waits on: render cache, render-ahead, pre-analysis | One unit at a time. None while the GPU is watched. One turn in four beside asked work, so it is never starved |

### Units, jobs and the list

- A **turn** is one unit: one frame, one inference. Decode, disk and
  network stay outside it where the unit can be that narrow.
- A **job** (`gpu::Job`) is the long-lived work that takes many turns. It
  carries a label the user reads ("Export: Interview.mp4") and optional
  progress.
- A job is listed from its first turn asked for until it is dropped. Work
  that ends up on the CPU takes no turn and is never listed.
- Short-lived work that would only flicker in the list (render-ahead, a
  single agent frame) takes turns with `gpu::turn` and has no row.
- An engine that alone knows whether its work runs on the GPU (a model
  that may have moved to the CPU) is handed a `gpu::Unit` by its caller
  and takes it only there.

### Joining

```rust
let job = gpu::Job::new("Export: My film.mp4", gpu::Level::Asked);
for frame in frames {
    let Some(_turn) = job.turn(&cancel) else { break }; // None once cancelled
    job.progress(fraction);
    render(frame);
}

let _watching = gpu::watching(); // watched work: never blocks
```

That is all a new kind of GPU work has to do. The title bar's Jobs list
reads `gpu::jobs()` every 250 ms and repaints when it changes.

### Guarantees

| Hazard | What happens |
| --- | --- |
| Panic, cancel or early return while holding a turn | The turn and the job's row are released on drop |
| Waiting for a turn | Takes a cancel and checks it every 50 ms; a cancelled job is not a failed job |
| A turn asked for inside another on the same thread (a frame whose composing runs a model) | It is the same turn |
| A holder waits on another thread's work that needs the GPU | The holder gives its turn back meanwhile (`gpu::aside`) and takes it again first afterwards |
| A unit longer than 5 s, or a holder stuck | The next waiter takes the turn over; the two overlap |
| A watched guard kept by mistake | It stops counting after 2 s |

### Who takes turns

| Work | Level | Unit | Row |
| --- | --- | --- | --- |
| Program playback, scrub | Watched | frame | none |
| Export | Asked | composed frame | "Export: file" |
| Render cache | Background | composed frame | "Render cache" |
| Viewer render-ahead | Background | frame | none |
| Blockout view | Watched | frame | none |
| Blockout clip | Asked | frame | "Blockout clip" |
| Depth for a render | Asked or background, as the caller says | inference | "Depth: file" |
| Depth and slow motion for the viewer | Watched | inference | none |
| Slow motion for a render | Inside the frame's own turn | inference | none |
| Transcription (Whisper built for a GPU) | As the analysis job's lane | one stretch of speech | "Transcription: file" |
| Subject tracking (MLX, macOS) | Asked | frame | "Track subject: file" |
| Subject picking and hover | Watched | frame | none |
| Agent frames and previews | Asked, stopped with the agent's turn | frame | none |

The agent's jobs also run at most four at a time on a `JobQueue`, where
each used to get a thread of its own.

## Turned down

- **A queue inside each engine** (wgpu batch, ONNX session, Whisper).
  Narrower units, but the engines do not know who waits on their work,
  pipelined frames would hold two turns at once, and each new engine
  would need its own integration.
- **Strict exclusion (a lock).** Any mistake in taking turns would then
  stop work. Advisory turns with a takeover keep the worst case at
  overlap.
- **Watched work in the queue.** A background inference can run for
  seconds; playback must never wait behind one.
- **One list for every kind of job.** Analysis jobs (`JobStatus`), the
  render queue and the turn registry stay separate systems for now. See
  Open.
- **FFmpeg hardware decode and encode.** They run in other processes on
  the GPU's video blocks and already fall back to software.

## Verified

- Format, lint and build clean on Linux; the project has no tests.
- Throwaway programs outside the repository against `flikk_os::gpu`:
  asked jobs alternate, background gets one turn in four and waits while
  watched, a cancelled waiter leaves cleanly, a panicking holder
  releases, nested turns, the 5 s takeover, `aside` (0.1 s against a 5 s
  stall without it) and a panic inside it.
- The Jobs list's empty state in the running app; a blockout clip and a
  depth pass listed with progress on software Vulkan and WebGPU.

Not verified:

- the MLX subject-mask changes, which are macOS only and were never
  compiled;
- slow motion, Whisper on a GPU and the agent's jobs at runtime;
- rows with real jobs in the app on real hardware.

## Open

- The list is called Jobs but holds only work that takes GPU turns.
  CPU-only work (person masks, stabilizing, speaker and sound analysis)
  is not in it.
- A stretch of speech is one Whisper unit and can run past 5 s, after
  which other work overlaps it. Whisper built for a GPU takes turns even
  on a machine where it fell back to the CPU.
- While the viewer's depth or slow-motion queue fills around a paused
  playhead, background work stays off the GPU.
- The agent's cap of four includes network-bound jobs.
- A blockout clip written after approving a shot, and frame grabs for a
  generation, cannot be cancelled: their callers have no cancel to pass.
