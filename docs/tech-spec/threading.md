# Threading

GPUI owns the main thread. `EditorState` (the bus, selection, chat) lives
there. Anything slow (probing, decoding, proxies, collages, analysis,
rendering, the agent subprocess) runs on background threads or
`cx.background_executor()` and reports back through `Entity::update`. The
bus is not `Sync`, by design: agent tool calls arrive over a channel and are
served in `AgentLink::pump`, which `EditorState` calls every 33 ms. Slow
tools (views, previews, FFmpeg work) return a job that runs off-thread on a
`Project` snapshot; its proposals are applied back on the UI thread.

Heavy background work that must not pile up goes through
`flikk_core::jobs::JobQueue`: FIFO, at most `limit` jobs at once on threads of
its own, a key already queued or running returns that job, and a job that
panics fails without stopping the queue.

Work on the GPU takes turns besides, a frame or an inference each, whatever
thread it runs on: see [GPU turns and the Jobs list](gpu-turns.md).
