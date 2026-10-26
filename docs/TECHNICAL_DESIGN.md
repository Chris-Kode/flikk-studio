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
| `koo-analysis` | Background jobs: Whisper transcript, VAD, speakers, shot changes, silences, loudness | core, whisper-rs |
| `koo-agent` | Backend adapters, MCP server + bridge, tool catalog, prompt/context builder | core |
| `koo-deliver` | Render graph → FFmpeg, render queue, SRT/ASS, OTIO, FCPXML, frame export | core |
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
