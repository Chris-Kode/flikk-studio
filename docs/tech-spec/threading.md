# Threading

GPUI owns the main thread. `EditorState` (the bus, selection, chat) is on
that thread. Slow work runs on background threads or on
`cx.background_executor()`. Examples are probes, decodes, proxies,
collages, analysis, renders and the agent subprocess. This work sends its
results back through `Entity::update`.

The bus is not `Sync`, by design. Agent tool calls come through a channel.
`AgentLink::pump` answers them, and `EditorState` calls it every 33 ms.
Slow tools (views, previews, FFmpeg work) return a job. The job runs on a
different thread, on a `Project` snapshot. The UI thread then applies its
proposals.

Heavy background work must not accumulate. This work goes through
`flikk_core::jobs::JobQueue`:

- The queue is FIFO.
- A maximum of `limit` jobs run at the same time, on threads of the queue.
- If a key is already in the queue or runs, the queue returns that job.
- If a job panics, the job fails, but the queue does not stop.

Work on the GPU also takes turns, one frame or one inference each. This is
true on all threads. Refer to [GPU turns and the Jobs list](gpu-turns.md).
