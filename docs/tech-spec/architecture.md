# Shape of the system

```
                 ┌────────────────────────────── flikk (GPUI app) ───────────────────────────────┐
  user ──input──▶│ workspaces (ui/*) ──▶ EditorState ──▶ CommandBus (flikk-core) ◀── AgentLink   │
                 │        ▲                    │  Services: media, analysis, fx, html, lettering │
                 │        └────── notify ──────┘                                    │            │
                 └──────────────────────────────────────────────────────────────────┼────────────┘
     flikk-media (FFmpeg)   flikk-analysis + flikk-models (Whisper, ONNX)            │ TCP 127.0.0.1
     flikk-gpu (wgpu FX)    flikk-html (headless Chromium)                           │ + token
     flikk-store (.flikk files, SQLite)   flikk-deliver (render, export)             ▼
     flikk-cloud (online providers: ElevenLabs, fal, Pika)
                                 claude -p / codex exec ──stdio──▶ flikk-mcp (or `flikk mcp`)
                                 external MCP clients  ──stdio──▶ flikk mcp ── bridge.json
```

* **One command bus.** Every change, from a button, a shortcut, the agent or an
  external MCP client, is a `flikk_core::command::Command` submitted to
  `flikk_core::bus::CommandBus`. Permissions, undo and change sets live there
  and nowhere else (PRD principles 1–3). Any UI action that changes the
  project goes through a `Command`, so the agent can do it too.
* **Commands plan into ops.** `Command::plan(&Project) -> Result<Plan>` is
  pure; a `Plan` is invertible `ops::Op`s plus warnings
  (`TouchesProtected`, `NeedsConfirmation`). Each op carries the value it
  replaces, so undo is the inverse ops in reverse, applying stale ops fails
  with `CoreError::Conflict` instead of overwriting a hand edit (AG6), and the
  timeline draws a pending change set straight from its ops.
* **Agents propose, users apply.** `Author::User` commands apply at once as
  one undo step (`execute_batch` groups several). `Author::Agent` and
  `Author::External` commands go into a change set for review ([Editing model](editing-model.md)).
* **Engines behind traits.** The UI, the compositor and the agent host call
  the traits in `flikk_core::services`: `MediaService` (`flikk-media`),
  `AnalysisService` (`flikk-analysis`), `FxService` (`flikk-gpu`), `HtmlService`
  (`flikk-html`), `LetteringService` (`flikk-deliver`) and `GenerationService`
  (`flikk-cloud`). `EditorState.services` holds each as an
  `Option<Arc<dyn …>>`; an engine that fails to start (no ffmpeg, no GPU
  adapter, no browser) is `None` and callers degrade quietly.

## Crates

| Crate | Role | Depends on |
| --- | --- | --- |
| `flikk-core` | Model, time, ids, commands, ops, bus, change sets, history, queries, captions, cuts, grade, scopes, masks and subject mattes, speed ramps, depth maps, audio FX DSP, FX format and built-ins, compositor, blockouts (rough 3D scenes built from plain solids, their models and cycles, renderer, review and clip), job queue, service traits, online capabilities, user library, paths | os, serde, serde_json, thiserror, uuid |
| `flikk-os` | What every crate needs from the system, std only: files written whole (temp file and rename, durable or private) and outputs that never replace a file, the sweep of crash-left temp files, child processes killed on drop and the ffmpeg/ffprobe paths, poison-tolerant locks and panic messages, the 30 s retry back-off, GPU work taking turns and the list of its jobs | — |
| `flikk-store` | `.flikk` project files, autosave, backups, recent files; SQLite for settings, style profiles and rules, source facts, fingerprints | core, rusqlite, chrono |
| `flikk-media` | ffprobe, fingerprint, proxies, frame and PCM decode, playback streams, collages, waveforms, spectrogram pictures | core, blake3, png |
| `flikk-analysis` | `AnalysisService`: the analysis pipeline over `flikk-models`' task interfaces, plus shot changes, silences, loudness, the energy VAD and the analysis cache; `SubjectService` (`LocalSubjects`): subject masks found and tracked, person masks matted, and their matte caches; `InterpolationService` (`LocalInterpolation`): optical flow frames for slow motion, and their cache; `DepthService` (`LocalDepth`): depth maps per frame, smoothed, and their cache | core, models |
| `flikk-models` | A folder per task (its interface and the models that can do it) and per model (files pinned to a commit, settings, runner); downloads; shared ONNX Runtime and BPE tokenizer | core, ureq, whisper-rs, ort, mlx-rs (macOS) |
| `flikk-cloud` | Online providers (ElevenLabs, fal, Pika): the provider catalog, `SpeechService` and `GenerationService`, the shared HTTP client, API keys in the system's credential store | core, ureq, keyring |
| `flikk-agent` | Backend adapters (Claude, Codex), MCP server (`flikk-mcp` bin) and bridge, tool catalog and groups, prompt builder, skill and style tools, backend sessions, blockout changes by asking (`reblock`) | core, base64, png |
| `flikk-deliver` | Render graph → FFmpeg, render queue, presets, SRT/ASS, OTIO, FCPXML, frame export, cosmic-text `Letterer` | core, cosmic-text |
| `flikk-gpu` | `FxService` on wgpu: GLSL effects, transitions, generators, masks and layer compositing; `ComputeService`: WGSL compute programs, which draw blockouts | core, wgpu, naga |
| `flikk-html` | `HtmlService`: HTML generators in headless Chromium over CDP with a virtual clock, GSAP preloaded | core, tungstenite, GSAP (vendored) |
| `flikk` | GPUI app (gpui-kit): `EditorState`, workspaces and panels, agent link, persistence, playback, wiring of all of the above | all, cpal, image |

Crates other than `flikk` depend only on `flikk-core` and `flikk-os`, never
on each other, except `flikk-analysis`, which runs its models through
`flikk-models`. Analysis
reaches an online transcriber only through `flikk_core::cloud::SpeechService`,
which the editor hands it (`AnalysisService::configure`).
