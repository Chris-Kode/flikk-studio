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

- **One command bus.** Each change is a `flikk_core::command::Command`.
  The code sends each `Command` to `flikk_core::bus::CommandBus`. A change
  can come from a button, a shortcut, the agent or an external MCP client.
  Permissions, undo and change sets are in the bus and in no other
  location (PRD principles 1–3). Each UI action that changes the project
  uses a `Command`. Thus the agent can also do that action.
- **Commands make a plan of ops.** `Command::plan(&Project) -> Result<Plan>`
  is pure. A `Plan` contains invertible `ops::Op`s and warnings
  (`TouchesProtected`, `NeedsConfirmation`). Each op holds the value that
  it replaces. This gives three results:
  - Undo applies the inverse ops in the reverse sequence.
  - A stale op fails with `CoreError::Conflict`. It does not overwrite a
    hand edit (AG6).
  - The timeline draws a pending change set directly from its ops.
- **Agents propose, users apply.** `Author::User` commands apply
  immediately as one undo step. `execute_batch` puts several of these
  commands in one group. `Author::Agent` and `Author::External` commands
  go into a change set for review ([Editing model](editing-model.md)).
- **Engines behind traits.** The UI, the compositor and the agent host
  call the traits in `flikk_core::services`:
  - `MediaService` (`flikk-media`)
  - `AnalysisService` (`flikk-analysis`)
  - `FxService` (`flikk-gpu`)
  - `HtmlService` (`flikk-html`)
  - `LetteringService` (`flikk-deliver`)
  - `GenerationService` (`flikk-cloud`)

  `EditorState.services` holds each engine as an `Option<Arc<dyn …>>`. If
  an engine does not start, its value is `None`. Examples are no ffmpeg,
  no GPU adapter and no browser. The callers then continue quietly with
  less function.

## Crates

| Crate | Role | Depends on |
| --- | --- | --- |
| `flikk-core` | Model, time, ids, commands, ops, bus, change sets and history. Queries, captions, cuts, grade and scopes. Masks and subject mattes, speed ramps, depth maps and audio FX DSP. FX format and built-ins, and the compositor. Blockouts: rough 3D scenes that the code builds from plain solids, with their models and cycles, renderer, review and clip. Job queue, service traits, online capabilities, user library and paths. | os, serde, serde_json, thiserror, uuid |
| `flikk-os` | The functions from the system that every crate needs. It uses std only. Files written whole (temp file and rename, durable or private). Outputs that never replace a file. The sweep of temp files that a crash left. Child processes killed on drop, and the ffmpeg/ffprobe paths. Poison-tolerant locks and panic messages. The 30 s retry back-off. GPU work that takes turns, and the list of its jobs. | — |
| `flikk-store` | `.flikk` project files, autosave, backups and recent files. SQLite for settings, style profiles and rules, source facts and fingerprints. | core, rusqlite, chrono |
| `flikk-media` | ffprobe, fingerprint, proxies, frame and PCM decode and playback streams. Collages, waveforms and spectrogram pictures. | core, blake3, png |
| `flikk-analysis` | `AnalysisService`: the analysis pipeline over the task interfaces of `flikk-models`. It also finds shot changes, silences and loudness, and has the energy VAD and the analysis cache. `SubjectService` (`LocalSubjects`): it finds and tracks subject masks, mattes person masks, and keeps their matte caches. `InterpolationService` (`LocalInterpolation`): optical flow frames for slow motion, and their cache. `DepthService` (`LocalDepth`): depth maps for each frame, smoothed, and their cache. | core, models |
| `flikk-models` | A folder for each task (its interface and the models that can do it). A folder for each model (files pinned to a commit, settings, runner). Downloads. Shared ONNX Runtime and BPE tokenizer. | core, ureq, whisper-rs, ort, mlx-rs (macOS) |
| `flikk-cloud` | Online providers (ElevenLabs, fal, Pika). The provider catalog, `SpeechService` and `GenerationService`. The shared HTTP client. API keys in the credential store of the system. | core, ureq, keyring |
| `flikk-agent` | Backend adapters (Claude, Codex), MCP server (`flikk-mcp` bin) and bridge. Tool catalog and groups, prompt builder, and skill and style tools. Backend sessions. Blockout changes on request (`reblock`). | core, base64, png |
| `flikk-deliver` | Render graph → FFmpeg, render queue and presets. SRT/ASS, OTIO, FCPXML and frame export. The cosmic-text `Letterer`. | core, cosmic-text |
| `flikk-gpu` | `FxService` on wgpu: GLSL effects, transitions, generators, masks and layer compositing. `ComputeService`: WGSL compute programs, which draw blockouts. | core, wgpu, naga |
| `flikk-html` | `HtmlService`: HTML generators in headless Chromium over CDP. It has a virtual clock, and GSAP loads before use. | core, tungstenite, GSAP (vendored) |
| `flikk` | GPUI app (gpui-kit): `EditorState`, workspaces and panels, agent link and persistence. Playback, and the connections between all of the above. | all, cpal, image |

Crates other than `flikk` depend only on `flikk-core` and `flikk-os`.
They do not depend on each other. The one exception is `flikk-analysis`,
which runs its models through `flikk-models`. Analysis gets access to an
online transcriber only through `flikk_core::cloud::SpeechService`. The
editor gives this service to analysis (`AnalysisService::configure`).
