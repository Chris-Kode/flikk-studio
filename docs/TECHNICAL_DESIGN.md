# koo-editor — technical design

Engineering companion to the PRD ("AI-native video editor with agent chat",
Oct 26 2026). The PRD says *what*; this file says *how*, and how the work is
split so it can be built in parallel. When the two disagree, the PRD wins on
behaviour and this file wins on structure.

## 1. Shape of the system

```
                 ┌──────────────────────── koo-editor (GPUI app) ───────────────────────┐
  user ──input──▶│ panels (ui/*) ──▶ EditorState ──▶ CommandBus (koo-core) ◀── AgentLink │
                 │        ▲               │  services: MediaService, AnalysisService   │
                 │        └──── notify ───┘                                   │        │
                 └────────────────────────────────────────────────────────────┼────────┘
                        koo-media (FFmpeg)   koo-analysis (Whisper, VAD)       │ local IPC
                        koo-store (SQLite)   koo-deliver (render/export)       ▼
                                                     claude -p / codex exec ──▶ koo-mcp (stdio MCP)
```

* **One command bus.** Every change, from a button, a shortcut, the agent or a
  recipe, is a `koo_core::command::Command` submitted to
  `koo_core::bus::CommandBus`. Permissions, undo and change sets live there
  and nowhere else (PRD principles 1–3).
* **Commands plan into ops.** `Command::plan(&Project) -> Plan` is pure and
  returns invertible `ops::Op`s. Each op carries the value it replaces, so:
  undo = inverse ops in reverse; applying stale ops fails with
  `CoreError::Conflict` instead of overwriting a hand edit (AG6); the timeline
  draws a pending change set straight from its ops.
* **Agents propose, users apply.** `Author::User` commands apply immediately as
  one undo step. `Author::Agent`/`Recipe` commands go into the author's open
  `ChangeSet`, planned against that set's *draft* (committed project + its own
  pending items), so the agent sees the effect of its earlier calls. On accept,
  items whose clips the user touched after the set opened become `Conflicted`.
  Other items replay their ops, or are re-planned from their command if an
  earlier item was rejected. The whole accept is one history entry (AG5).
* **Engines behind traits.** The UI and the agent host call
  `services::MediaService` and `services::AnalysisService`, implemented by
  `koo-media` and `koo-analysis`. `EditorState.services` holds them; until a
  stream lands, they are `None` and panels degrade quietly.

### Crates

| Crate | Role | Depends on |
| --- | --- | --- |
| `koo-core` | Model, time, commands, ops, bus, change sets, history, shared types, service traits, paths | serde, uuid |
| `koo-store` | SQLite persistence, autosave, crash recovery, settings, memory stores | core, rusqlite |
| `koo-media` | ffprobe, fingerprint, proxies, frame decode, collages, waveforms | core, image |
| `koo-analysis` | Background jobs: Whisper transcript, forced word alignment, VAD, speakers, shot changes, silences, loudness | core, whisper-rs, ort |
| `koo-agent` | Backend adapters, MCP server + bridge, tool catalog, prompt/context builder | core |
| `koo-deliver` | Render graph → FFmpeg, render queue, SRT/ASS, OTIO, FCPXML, frame export | core |
| `koo-gpu` | `FxService` on wgpu: GLSL effects, transitions, generators, layer compositing | core, wgpu |
| `koo-html` | `HtmlService`: HTML generators in headless Chromium with a virtual clock | core, tungstenite |
| `koo-editor` | GPUI app: layout, `EditorState`, panels, wiring of all of the above | all |

Crates other than `koo-editor` depend only on `koo-core`, never on each other.

## 2. Key decisions

| Topic | Decision | Why |
| --- | --- | --- |
| Time | `Time(i64)` in flicks (1/705 600 000 s); `TimeRange` half-open | Exact for every common frame and sample rate; no drift on long timelines |
| IDs | UUID v4 newtypes; `Display` prints 8 hex chars | Stable across save/load and safe to show in chat (`@clip 3f2a91c0`) |
| Model | One timeline per project (nested sequences, E7, come later as compound clips) | Keeps the v1 bus simple |
| Captions | Clips on `TrackKind::Caption` tracks | Ripple, move and undo retime captions for free (C3) |
| Protected ranges | `Marker { kind: Protected }`; cut planners add `Warning::TouchesProtected` | F4 enforced in the bus for hand and agent edits alike |
| FFmpeg | Spawn `ffmpeg`/`ffprobe` binaries (`KOOEDITOR_FFMPEG` / `KOOEDITOR_FFPROBE`, else `PATH`, later bundled) | No libav linking on Windows/macOS; `-hwaccel auto` for GPU decode |
| Playback | Proxies by default (720p, short GOP H.264, `cache/proxies/<fp>.mp4`); a streaming decoder pipes RGBA frames from a long-lived ffmpeg process | Smooth 1080p60 on a laptop (PRD NFR); revisit after the GPUI video spike |
| Fingerprint | BLAKE3 over file size + head, tail and 8 evenly spaced 1 MiB chunks | Fast on multi-GB files; good enough to detect replacement (F2) |
| Whisper | `whisper-rs` (whisper.cpp), ggml model in `paths::models()`, GPU when built with it | Local, word timestamps, any language; no per-use cost |
| Word times | Forced alignment of Whisper's words with MMS (`mms-300m-1130-forced-aligner`, int8 ONNX, 317 MB in `paths::models()`) on ONNX Runtime (`ort`, prebuilt and linked statically at build time) | Whisper's own word times drift by up to a second in conversation; the aligner puts 94–99 % of word starts within 100 ms on hand-labelled speech. One model for 1,100+ languages, written in Latin letters with `any_ascii` |
| Caches | Everything derived is keyed by fingerprint under `paths::cache()` | Re-opening a project never recomputes (principle 4, V6) |
| Agent ↔ editor | The backend CLI launches `koo-mcp` (stdio MCP server). It connects to the editor over `127.0.0.1:<port>` with a per-session token; the editor serves tool calls on the UI thread in `AgentLink::pump` | One code path for Windows and macOS; the bus stays single-threaded |
| Tool schema | Times in tool arguments and results are seconds (`f64`) or `"M:SS.mmm"` strings, converted to `Time` at the boundary | What models read and write reliably |
| Persistence | SQLite (WAL) at `paths::database()`; project JSON + op-log for crash recovery | Already in the scaffold; one file per user |
| User data | Only under `paths::home()` (`$KOOEDITOR_HOME` or `~/.koo-editor`) | AGENTS.md hard rule 2 |

Confirm CLI flags against the installed `claude --help` and the current Codex
docs before relying on them (PRD note).

## 3. Threading

GPUI owns the main thread. `EditorState` (the bus, selection, chat) lives
there. Anything slow (probing, decoding, collages, analysis, rendering, the
agent subprocess) runs on background threads or `cx.background_executor()`
and reports back through `Entity::update`. The bus is not `Sync`, by design:
agent tool calls arrive over a channel and are executed in
`AgentLink::pump`, called every ~33 ms from `EditorState`. View tools that need
FFmpeg take a `Project` snapshot and finish off-thread.

## 4. UI guidelines

The user asked for a clean, minimal UI, and CLAUDE.md says "let the layout do
the explaining":

* No secondary labels, helper text or descriptions under controls. Use icons
  with tooltips, placeholders in inputs, and grouping by spacing.
* Colors come from `cx.theme()` only. Change sets use `theme.green` (added),
  `theme.red` (removed) and `theme.yellow` (modified), drawn as tints and
  outlines, never as banners.
* Dark theme by default, compact density (`text_sm`/`text_xs`, 28 px rows).
* Empty states are a single muted line or an icon, not a paragraph.
* Prefer gpui-kit components (`gpui_kit::component::*`: button, input, slider,
  tab, list, popover, dialog, tooltip, resizable) over hand-rolled widgets.
* Check visually: `scripts/screenshot.sh out.png` renders the app under Xvfb.

## 5. PRD → code map (MVP / P0)

| PRD | Where |
| --- | --- |
| M1 import, bins, search | `ui/media_pool.rs`, `Command::ImportMedia` |
| M2 inspector, M4 proxies | `koo-media`, `ui/media_pool.rs` |
| M3 analysis | `koo-analysis` |
| E1, E2 timeline and tools | `ui/timeline*`, `koo-core/src/edit.rs` |
| E3 transcript editing | `ui/transcript.rs` + transcript↔timeline mapping in core |
| E4 silences | `AnalysisService::silences` + timeline toolbar |
| E5 transform, keyframes | `Command::AddZoom/SetKeyframe`, `ui/inspector.rs`, viewer preview |
| E8 timecode readout | `Clip::to_source`, viewer |
| C1–C3, C6 | `koo-core/src/captions.rs`, caption tracks, `koo-deliver` SRT/ASS |
| K1 primary grade | `model::Grade`, `ui/inspector.rs`, render graph |
| A1–A3 mixer, loudness, ducking | `ui/inspector.rs`, `AnalysisService::loudness`, render graph |
| D1 render | `koo-deliver`, `ui/deliver.rs` |
| AG1–AG10 | `koo-agent`, `agent_link.rs`, `ui/chat.rs`, `ui/settings.rs` |
| V1–V7 collages | `MediaService::collage`, `koo-agent` `view_*` tools |
| S1, S3, S4, F1, F2, F4 | `koo-core::memory`, `koo-store`, cut planners |

## 6. Work streams

Each stream runs in its own cloud session on its own branch
(`claude/koo-<stream>`) cut from the foundation commit, and owns the files
listed. The coordinator session merges the branches. To keep merges clean:

* **Edit only files you own.** Cross-stream needs go in your summary, not in
  someone else's file.
* **Shared files, additive only:** `crates/koo-editor/src/state.rs` (add
  fields and methods, don't change existing signatures) and
  `crates/koo-core/src/lib.rs` (add a `pub mod` line).
* **Never change a public type in `koo-core`** except in stream 1 (core
  editing), and there only additively. If you need a change, add a new
  item and explain in your summary.
* Dependencies go in your crate's `Cargo.toml`, not in
  `[workspace.dependencies]`. `Cargo.lock` conflicts are resolved by the
  coordinator.
* No tests of any kind (AGENTS.md rule 1). Run `cargo fmt`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace`
  before every push.

| # | Stream | Owns | Delivers |
| --- | --- | --- | --- |
| 1 | core-editing | `koo-core/src/edit.rs`, new `koo-core/src/query.rs` | Every `NotImplemented` planner (insert/overwrite, trim, roll, slip, slide, zoom, keyframes, playlist); protected-range warnings on every cut; keyframe evaluation; timeline queries: topmost clip at t, `sample_points(range, n)` (V3), transcript↔timeline word mapping, typed timestamp/reference resolution (AG3), effective style rules (S3) |
| 2 | store | `crates/koo-store`, `koo-editor/src/persistence.rs` | Schema + migrations, project save/load with history, debounced autosave + crash recovery, settings, profiles/rules/facts/recipes CRUD, F2 suspension, 7 built-in recipes, Markdown → draft rules (S5) |
| 3 | media | `crates/koo-media` | `MediaService` impl: probe, fingerprint, frame, streaming playback decoder, proxies, collages (6×5, stamped timeline and source timecodes, cached), waveforms |
| 4 | analysis | `crates/koo-analysis` | `AnalysisService` impl: job queue, Whisper word-level transcript, VAD, silences, shot changes, loudness, basic speaker labels, fingerprint-keyed cache |
| 5 | agent | `crates/koo-agent`, `koo-editor/src/agent_link.rs` | Claude/Codex adapters, `koo-mcp` binary + IPC, tool catalog for every PRD tool, read/view tools incl. `view_range`, `ask_user`, context and prompt builder, activity entries, per-project resume, recipe slash commands |
| 6 | deliver | `crates/koo-deliver`, `koo-editor/src/ui/deliver.rs` | Timeline → FFmpeg render graph (cuts, transforms/zooms, grades, captions burn-in, gain/fades/ducking, loudnorm), render queue with progress, presets, SRT/ASS, OTIO, FCPXML, frame export, Deliver panel |
| 7 | ui-timeline | `koo-editor/src/ui/timeline.rs` (+ `ui/timeline/`), `ui/viewer.rs` (+ `ui/viewer/`) | Multi-track timeline, ruler, playhead, zoom/scroll, select/move/trim/blade, snapping, markers, in/out, JKL, change-set overlay, silence tool; program viewer with playback, transform preview, timecode readout |
| 8 | ui-agent | `koo-editor/src/ui/chat.rs` (+ `ui/chat/`), `ui/settings.rs` | Chat thread, input with @references, choice chips, change-set card (preview / accept all / review one by one / reject), activity log, quick command bar, Agent settings |
| 9 | ui-panels | `ui/media_pool.rs`, `ui/inspector.rs`, `ui/transcript.rs` (+ subdirs), new `koo-core/src/captions.rs` | Media pool (import, bins, search, tags, analysis progress, drag to timeline); inspector (transform/keyframes, primary grade, gain/fades, mixer, loudness, ducking); transcript editing and captions (C1 grouping, inline edit, styles) |

### Merge order

1 → 2 → 3 → 4 → 6 → 5 → 9 → 7 → 8. Core first, then engines, then the agent,
then the panels that use them. After each merge, the coordinator runs the
full check suite; wiring (`EditorState.services`, removing the temporary
`#![allow(dead_code)]` in `main.rs`) happens last.

## 7. Status

All nine streams are merged. What runs today:

* **Editing:** every PRD edit command plans through the bus, with undo,
  change sets, protected-range warnings and conflict-safe accept.
* **Media:** FFmpeg probe, fingerprint, frames, playback/audio streams,
  proxies, 6 × 5 collages (cached) and waveforms.
* **Analysis:** background jobs, VAD, silences, shot changes, loudness;
  Whisper transcription behind the default `whisper` feature. The model
  downloads to `paths::models()` on first use, or set
  `KOOEDITOR_WHISPER_MODEL`. Word times come from a forced aligner
  (`koo-analysis/src/forced.rs`, behind the default `aligner` feature): a
  CTC model hears the audio and Whisper's words are fitted to it letter by
  letter, in chunks of at most 30 s. Its model downloads on first use too
  (or set `KOOEDITOR_ALIGNER_MODEL` to a path). The model hears a letter
  slightly late, so starts move 40 ms earlier and ends 20 ms later, never
  over a neighbour. Where the model is unavailable, word edges are
  measured on the audio levels (`koo-analysis/src/align.rs`) and the
  result keeps version 1, to be aligned once the model is there; words
  the model has no letters for (digits) keep those edges too. Running
  that measurement after the aligner made cuts worse, so it is only the
  fallback. Results of an older `ANALYSIS_VERSION` are upgraded when a
  project opens, without running Whisper again.
* **Cuts in speech** (`koo-core/src/cuts.rs`) go in the pause between two
  words, clear of both, on a frame boundary when the pause holds one, with
  a 10 ms audio fade at the join. The transcript panel's word delete and
  the agent's `cut_words` use it; `blade` and `ripple_delete` move a cut
  that falls on a word into the nearest pause and say so.
* **Agent:** `claude -p` verified end to end (collage → proposals → one
  change set → accept); Codex adapter written from its docs, not yet run.
  The editor binary doubles as the MCP server (`koo-editor mcp`).
* **Store:** projects with undo history, autosave and crash recovery,
  settings, style profiles, source facts, built-in recipes, chat
  sessions per project (each resumes its own backend conversation).
* **Deliver:** FFmpeg render graph, render queue, SRT/ASS, OTIO, FCPXML,
  frame export.
* **UI:** all six workspaces, timeline with change-set overlay, track
  add/delete and cut stepping (Up/Down), viewer with playback, media pool
  with removal, an inspector that shows the selected clip's properties by
  kind (video, audio, caption, title), transcript/captions, chat sessions
  with change-set review, agent settings, project menu.

The pro-editing build-out (§8) is merged on top:

* **FX:** GLSL effects, transitions and generators on the GPU (51
  built-ins) and HTML/CSS/JS motion graphics (15 templates), with typed,
  keyframeable parameters; programs written by the user or the agent live
  in the project and are edited in the FX code editor.
* **Compositor:** one program frame for the viewer, the export and the
  agent's view tools; exported frames match the viewer at 38–39 dB PSNR
  (the yuv420p encode is the limit).
* **Editing:** transitions on cuts and edges, speed, enable/disable, clip
  names and colors, crop/flip/blend modes, multi-select, copy/paste/
  duplicate, nudge, a trim mode (slip, ripple, roll), resizable tracks.
* **Color:** one grade pipeline in core, baked into a 3D LUT for export;
  wheels, curves, scopes (waveform, parade, vectorscope, histogram), shot
  matching, LUT list.
* **Audio:** clip and track effect inserts matched between preview and
  export, volume automation, crossfades, retimed audio, meters and a
  mixer.
* **Agent:** a tool for every command (72), `create_fx`/`update_fx` with
  line-numbered compile errors, `preview_fx` to look at a program over
  time, graded scopes, timecode and frame addressing, and an external MCP
  bridge (below).
* **Chat:** drag clips (Alt-drag on the timeline), media, effects,
  transitions and transcript words into the composer, an `@` picker for
  every object kind, and a resolved `<references>` block sent with each
  message.

Try it without media: `KOOEDITOR_DEMO=1 KOOEDITOR_HOME=/tmp/koo-demo cargo run -p koo-editor`
(`KOOEDITOR_DEMO_WORKSPACE=Captions` opens another workspace). More demo
states: `KOOEDITOR_DEMO_FX=effects|generator|transition|library|code`,
`KOOEDITOR_DEMO_SELECTION=clips|transition`, `KOOEDITOR_DEMO_CHAT=1`,
`KOOEDITOR_DEMO_PLAY=1`, and `KOOEDITOR_DEMO_COMPOSE=<seconds>` (every
compositor feature on real FFmpeg-made media).

### Known gaps

* Pending change sets are in memory only; they don't survive a restart.
* Codex backend untested.
* Inside connected speech (no pause between words) about 4 in 10 cuts
  still clip the tail or the onset of a word by more than 20 ms (on
  hand-labelled English and Dutch; 9 in 10 before the aligner), and may
  be heard; the agent's results say so. whisper.cpp's DTW timestamps were tried and came
  out 0.2–0.9 s late on real speech, so they are off.
* The alignment model (MMS) is licensed CC-BY-NC 4.0, so commercial use
  needs another model. `facebook/wav2vec2-base-960h` (Apache-2.0) is
  English-only and leaves fewer cuts clean on conversation (50 % against
  60 % on Buckeye; Whisper alone: 19 %).
* Scripts other than Latin are aligned through `any_ascii`
  transliteration, which is rough for Japanese kanji and abjads (Hebrew,
  Arabic); only English and Dutch were measured. Whisper's digits have no
  letters to align and keep the measured edges.
* ONNX Runtime adds about 25 MB to the binary, and the first build
  downloads it (from pyke's CDN).
* No GPU video surface yet: the viewer paints decoded RGBA frames (the
  PRD's GPUI video spike is still open), and Windows/macOS builds haven't
  been run.
* P1/P2 PRD items not yet built: nested sequences, music playlist UI,
  voice cleanup, vertical reframing, voice input.
* Mouse and keyboard interactions (drags, drops, context menus,
  shortcuts) were checked by code review and screenshots of seeded states
  only: the cloud containers have no input automation.
* Preview vs export: noise reduction previews with an expander (export
  uses `afftdn`); the limiter and time-stretch differ slightly in shape.
* The FX code editor has no syntax highlighting (gpui-kit's `syntect`
  feature is not vendored). A shader that never returns can stall the GPU.
* A program an external agent proposes is usable by its later connections
  only once the user accepts it.
* If the editor crashes, its headless browser can keep running.
* CPU fallbacks (no GPU adapter) are slow in debug builds; use release
  builds on machines without a GPU.

## 8. Pro editing: FX, shaders and HTML motion graphics

The second build-out takes the editor to Resolve-level tools while keeping
it agent-first: everything below is a `Command`, so the agent reaches it
with the same precision as the UI.

### Design

* **One FX format** (`koo-core/src/fx/mod.rs`; its module docs are the
  authoritative GLSL and HTML conventions). An `FxAsset` is a GLSL program
  (effect, transition or generator) or an HTML/CSS/JS document (generator:
  titles, lower thirds, motion graphics, image layouts) plus typed
  parameters (number, bool, color, point, text, choice, image). Built-ins
  ship in `fx/shaders.rs` and `fx/templates.rs`; programs the user or the
  agent writes live in `Project::fx`. An `FxInstance` uses an asset with
  parameter values and keyframes: on a clip's effect stack, as a clip's
  transition, or as a generator clip's content.
* **GLSL** follows ShaderToy/gl-transitions: authors write
  `effect(uv)`, `transition(uv)` or `generate(uv)`; the engine adds
  uniforms and helpers and reports errors against the author's lines.
* **HTML** renders in headless Chromium with an engine-owned clock (CSS
  animations, Web Animations, rAF and timers all follow clip time), so
  frames are deterministic and exportable, with alpha.
* **Engines behind traits:** `services::FxService` (`koo-gpu`, wgpu — the
  same wgpu GPUI uses) and `services::HtmlService` (`koo-html`). Both may be
  `None` (no GPU adapter, no browser); callers degrade quietly.
* **Model additions:** clip effects, `transition_in`/`transition_out`
  (resolved by `fx::transitions`), speed, enabled, name, color; transform
  crop/flip/blend; grade offset/shadows/highlights/hue/curves; audio
  effects on clips and tracks (`audiofx.rs`); crop and volume keyframes;
  `PlaceClips` for paste/duplicate; scope readings and shot matching
  (`scopes.rs`).
* **Compositor:** `koo_core::compose::program` draws the program at a
  time (picture → clip and timeline grade → effect stack in sequence
  pixels → transform with crop/flip → blend mode; transitions blend each
  side's own canvas), with `FxService::composite` doing placement and
  blending (wgpu in `koo-gpu`, CPU reference in core). The viewer (paused
  and playing), frame export, renders that use FX (piped as raw RGBA into
  the FFmpeg graph, which keeps captions and audio) and the agent's view
  and scope tools all use it; ranges without FX still render through the
  graph. Source pictures come through `compose::Pictures` (single decodes
  or per-layer streams that honour speed).

### Driving the editor from an external agent

While the editor runs it publishes its MCP bridge in
`paths::home()/bridge.json` (address, token, pid; owner-only). Any MCP
client can then use the same tools as the built-in agent:

```sh
claude mcp add koo -- koo-editor mcp          # Claude Code
```

or for Codex, in `~/.codex/config.toml`:

```toml
[mcp_servers.koo]
command = "koo-editor"
args = ["mcp"]
```

External calls act with the agent's current role and permissions. Their
edits land in an "External agent" change set that goes to review when the
client disconnects or after 90 s without a call; `ask_user` is not
available to them. The HTML engine finds Chrome, Chromium, Edge or Brave
on its own; set `KOOEDITOR_CHROME` to use another build.

### Streams

Each stream runs in its own cloud session, branched from the foundation on
`claude/zen-fermat-fzspvz`, and pushes to its own branch. The coordinator
session merges them.

| Stream | Branch | Owns |
| --- | --- | --- |
| gpu | `claude/fx-gpu` | new `crates/koo-gpu`; `koo-core/src/fx/shaders.rs`; `fx:` in `state.rs` `services()` |
| html | `claude/fx-html` | new `crates/koo-html`; `koo-core/src/fx/templates.rs`; `html:` in `services()` |
| timeline | `claude/fx-timeline` | `ui/timeline.rs`, `ui/timeline/*` |
| library | `claude/fx-library` | `ui/inspector.rs`, `ui/inspector/{clip,edit,param}.rs` + new inspector files; new `ui/library*`, `ui/code_editor.rs`; the Edit arm of `app.rs` |
| chat | `claude/fx-chat` | `ui/chat.rs`, `ui/chat/*`; `resolve_references` in `koo-core/src/query.rs`; word drag/"Add to chat" in `ui/transcript*` |
| agent | `claude/fx-agent` | `crates/koo-agent/**`; `agent_link.rs`, `agent_link/` |
| audio | `claude/fx-audio` | `ui/viewer/audio.rs`, `ui/inspector/audio.rs`, new `ui/mixer*`; audio half of `koo-deliver/src/graph.rs` + new `audio_fx.rs`; `koo-core/src/audiofx.rs`; the Audio arm of `app.rs` |
| color | `claude/fx-color` | new `koo-core/src/grade.rs`; `koo-core/src/scopes.rs`; `ui/viewer/grade.rs`, `ui/inspector/color*`, new `ui/scopes.rs`; `grade_filters` in `graph.rs` (+ new `koo-deliver/src/grade.rs`); the Color arm of `app.rs` |

Rules for every stream (in addition to AGENTS.md):

* **Edit only what you own.** Shared files are additive only:
  `state.rs` (add fields and methods in one block marked with your stream;
  don't change existing signatures), `app.rs` (only your arm), `demo.rs`
  (additive demo seeding for your feature, in its own function). Never
  change a public type in `koo-core` outside your files; if you need one,
  work around it and say so in your report.
* **Shared seams** come from the foundation: the drag payloads in
  `ui/mod.rs` (`MediaDrag`, `FxDrag`, `AudioFxDrag`, `RefDrag`),
  `EditorState::select_transition` / `selected_transition`, and
  `mention` / `take_mentions` (references queued for the chat composer).
  Leave the foundation's `#[allow(dead_code)]` markers alone; the
  coordinator removes them when the seams are in use.
* **Chat reference tokens** (what the agent reads; every stream that
  produces or parses them uses exactly this): `@clip <id8>`,
  `@media <id8>`, `@marker <id8>`, `@track <id8>`, `@bin <id8>`,
  `@effect <clip id8>/<effect id8>`, `@transition <clip id8>/in|out`,
  `@fx <builtin key or project id8>`, `@range <start>-<end>` (timeline
  seconds, three decimals) and `@time <secs>`. `<id8>` is the id's
  `Display`.
* **Agent parity:** any UI action that changes the project goes through a
  `Command`.
* **UI copy:** let the layout explain (§4).
* **No tests** of any kind; throwaway verification programs live outside
  the repo. Before every push: `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo build --workspace`, `cargo test --workspace` (0 tests).
* **Looking at the app:** `scripts/screenshot.sh out.png [wait]` renders it
  under Xvfb; `KOOEDITOR_DEMO=1` seeds a demo project and
  `KOOEDITOR_DEMO_WORKSPACE=Color|Audio|Captions|Deliver` opens a
  workspace. There is no input automation in the containers, so seed the
  states you need to see. Chromium for the HTML engine is under
  `/opt/pw-browsers/` in the cloud containers.
* **Report:** end your session with a short report (what you built by
  file, how you verified it, what is left, integration notes, skipped test
  items) and make sure everything is pushed.
