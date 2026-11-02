# koo-editor — technical design

Engineering companion to the PRD ("AI-native video editor with agent chat",
Oct 26 2026). The PRD says *what*; this file says *how* the code does it
today. When the two disagree, the PRD wins on behaviour and this file wins on
structure. Keep it in step with the code: when a change alters something
described here, update the section in the same commit.

## 1. Shape of the system

```
                 ┌──────────────────────────── koo-editor (GPUI app) ────────────────────────────┐
  user ──input──▶│ workspaces (ui/*) ──▶ EditorState ──▶ CommandBus (koo-core) ◀── AgentLink     │
                 │        ▲                    │  Services: media, analysis, fx, html, lettering │
                 │        └────── notify ──────┘                                    │            │
                 └──────────────────────────────────────────────────────────────────┼────────────┘
     koo-media (FFmpeg)   koo-analysis + koo-models (Whisper, ONNX)                  │ TCP 127.0.0.1
     koo-gpu (wgpu FX)    koo-html (headless Chromium)                               │ + token
     koo-store (.koo files, SQLite)   koo-deliver (render, export)                   ▼
     koo-cloud (online providers: ElevenLabs, fal, Pika)
                                 claude -p / codex exec ──stdio──▶ koo-mcp (or `koo-editor mcp`)
                                 external MCP clients  ──stdio──▶ koo-editor mcp ── bridge.json
```

* **One command bus.** Every change, from a button, a shortcut, the agent or a
  recipe, is a `koo_core::command::Command` submitted to
  `koo_core::bus::CommandBus`. Permissions, undo and change sets live there
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
  `Author::Recipe` commands go into a change set for review (§4).
* **Engines behind traits.** The UI, the compositor and the agent host call
  the traits in `koo_core::services`: `MediaService` (`koo-media`),
  `AnalysisService` (`koo-analysis`), `FxService` (`koo-gpu`), `HtmlService`
  (`koo-html`), `LetteringService` (`koo-deliver`) and `GenerationService`
  (`koo-cloud`). `EditorState.services` holds each as an
  `Option<Arc<dyn …>>`; an engine that fails to start (no ffmpeg, no GPU
  adapter, no browser) is `None` and callers degrade quietly.

### Crates

| Crate | Role | Depends on |
| --- | --- | --- |
| `koo-core` | Model, time, ids, commands, ops, bus, change sets, history, queries, captions, cuts, grade, scopes, masks and subject mattes, audio FX DSP, FX format and built-ins, compositor, job queue, service traits, online capabilities, user library, paths | serde, serde_json, thiserror, uuid |
| `koo-store` | `.koo` project files, autosave, backups, recent files; SQLite for settings, style profiles and rules, source facts, fingerprints, recipes | core, rusqlite, chrono |
| `koo-media` | ffprobe, fingerprint, proxies, frame and PCM decode, playback streams, collages, waveforms, spectrogram pictures | core, blake3, png |
| `koo-analysis` | `AnalysisService`: the analysis pipeline over `koo-models`' task interfaces, plus shot changes, silences, loudness, the energy VAD and the analysis cache; `SubjectService` (`LocalSubjects`): subject masks found and tracked, and their matte cache | core, models |
| `koo-models` | A folder per task (its interface and the models that can do it) and per model (files pinned to a commit, settings, runner); downloads; shared ONNX Runtime and BPE tokenizer | core, ureq, whisper-rs, ort, mlx-rs (macOS) |
| `koo-cloud` | Online providers (ElevenLabs, fal, Pika): the provider catalog, `SpeechService` and `GenerationService`, the shared HTTP client, API keys in the system's credential store | core, ureq, keyring |
| `koo-agent` | Backend adapters (Claude, Codex), MCP server (`koo-mcp` bin) and bridge, tool catalog and groups, prompt builder, recipes, backend sessions | core, base64, png |
| `koo-deliver` | Render graph → FFmpeg, render queue, presets, SRT/ASS, OTIO, FCPXML, frame export, cosmic-text `Letterer` | core, cosmic-text |
| `koo-gpu` | `FxService` on wgpu: GLSL effects, transitions, generators, masks and layer compositing | core, wgpu, naga |
| `koo-html` | `HtmlService`: HTML generators in headless Chromium over CDP with a virtual clock, GSAP preloaded | core, tungstenite, GSAP (vendored) |
| `koo-editor` | GPUI app (gpui-kit): `EditorState`, workspaces and panels, agent link, persistence, playback, wiring of all of the above | all, cpal, image |

Crates other than `koo-editor` depend only on `koo-core`, never on each other,
except `koo-analysis`, which runs its models through `koo-models`. Analysis
reaches an online transcriber only through `koo_core::cloud::SpeechService`,
which the editor hands it (`AnalysisService::configure`).

## 2. Key decisions

| Topic | Decision | Why |
| --- | --- | --- |
| Time | `Time(i64)` in flicks (1/705 600 000 s); `TimeRange` half-open | Exact for every common frame and sample rate; no drift on long timelines |
| IDs | UUID v4 newtypes; `Display` prints 8 hex chars | Stable across save/load and safe to show in chat (`@clip 3f2a91c0`) |
| Model | One timeline per project; nested sequences / compound clips are not built | Keeps the bus simple |
| Format | `SequenceSettings` sizes the frame by aspect (`Format::ALL`: 16:9, 9:16, 1:1, 4:5, 21:9, 4:3) and short side (720p–4K), changed by the undoable `SetFormat` | One project can be landscape, vertical or square; presets and lettering follow the short side |
| Captions | Clips on `TrackKind::Caption` tracks, with per-word times | Ripple, move and undo retime captions for free (C3); highlights follow the speech |
| Protected ranges | `Marker { kind: Protected }`; cut planners add `Warning::TouchesProtected` | F4 enforced in the bus for hand and agent edits alike |
| FFmpeg | Spawn `ffmpeg`/`ffprobe` binaries (`KOOEDITOR_FFMPEG` / `KOOEDITOR_FFPROBE`, else `PATH`) | No libav linking on Windows/macOS |
| Playback | A 720p proxy (`cache/proxies/<fp>.mp4`) is made on import and used once it exists; streams pipe RGBA from a long-lived ffmpeg (`-hwaccel auto`, software fallback); frames are composed on the GPU and painted as images | Smooth playback on a laptop without a GPU video surface |
| Fingerprint | BLAKE3 over file size + head, tail and 8 evenly spaced 1 MiB chunks (whole file under 10 MiB) | Fast on multi-GB files; good enough to detect replacement (F2) |
| Compositor | One `koo_core::compose::program` draws the frame for the viewer, the agent's views and composited render ranges | The preview matches the export |
| Whisper | `whisper-rs` (whisper.cpp), ggml model in `paths::models()`; on the GPU (Metal on macOS, Vulkan on Windows and Linux), else the CPU | Local, word timestamps, any language; no per-use cost |
| Word times | Forced alignment of Whisper's words with Qwen3-ForcedAligner-0.6B (Apache-2.0; our ONNX export, 4-bit weights, 589 MB) on ONNX Runtime (`ort`, linked statically at build time) | Whisper's own word times drift by up to a second in conversation; the aligner puts 94 % of word starts within 100 ms on hand-labelled English (Whisper: 48 %). One permissively licensed model for 11 languages |
| Hearing | The model never hears audio: analysis turns it into the sound script (words, `[sounds]`, `♪` music with tempo and changes, silences), beats to cut on, vectors to search by description, and spectrogram pictures | Neither backend takes audio input, and models place sounds in time far worse than in text; the transcript and collages already work this way |
| Sounds | EfficientAT mn10_as (MIT; 527 AudioSet classes) on 2 s windows every second, each followed by 4 s of silence, grouped into about 180 names for the script | Small (20 MB) and fast on the CPU; trained on 10 s clips, it answers nonsense for 2 s alone, and 6 s gets nine in ten of its 10 s answers at half the cost |
| Beats | Beat This! `final0` (MIT) on each stretch of music, 30 s chunks, its own minimal peak picking | State of the art for beats and downbeats without a DBN; runs only where the tagger heard music |
| Sound search | LAION's larger_clap_general (Apache-2.0): a vector per 4 s window every 2 s, int8, in `<fp>.vectors`; the text tower encodes a description at query time | Finds sounds outside the tagger's vocabulary ("sizzling bacon") and sounds like an example; 2 s steps scored by the lower of their two windows place a sound to about 2 s |
| Caches | Derived media data is keyed by fingerprint under `paths::cache()` | Re-opening a project never recomputes (principle 4, V6) |
| Agent ↔ editor | The backend CLI launches an MCP server over stdio; it connects to the editor over `127.0.0.1:<port>` with a token, and the editor serves calls in `AgentLink::pump` on the UI thread | One code path for Windows and macOS; the bus stays single-threaded |
| Tool schema | Times in tool arguments are seconds, `"M:SS.mmm"`, `"H:MM:SS.s"`, timecode `"HH:MM:SS:FF"`, frames `"f:1234"` or `@time`/`@range`; results are seconds to the millisecond | What models read and write reliably |
| Persistence | Projects are `.koo` files (versioned JSON, atomic writes, media paths absolute and relative to the file) saved where the user picks; unsaved changes autosave to `paths::autosave()`, saved files get timed copies in `paths::backups()`. Undo history and pending change sets last the session. User data is SQLite (WAL) at `paths::database()` | Projects move between folders and machines like any pro editor's; one database per user |
| Library | FX programs (`.koofx`), presets (`.koopreset`: effects, audio effects or a grade with their values) sounds (`.koosound`, their audio in `paths::assets()`, a folder per content) and voices (`.koovoice`: the provider's voice id, how it was made, the stability and similarity it reads best at, a sample under `paths::assets()/voices/`) under `paths::library()`, shared by every project (`koo-core::library`). A voice lives on the provider's account; its file lists it, plays it and makes it again without a request. Using a program copies it into the project with its `origin`; presets carry the project programs they use; using a sound adds a media item on its file to the pool's Library bin. Removing a sound leaves the audio. Sounds without results are analyzed when they are added and on launch, for sounds, music, beats and vectors only (`AnalysisService::analyze_sound`), so nothing reaches a provider | A `.koo` file stays complete anywhere; library edits reach a project only when its copy is updated; audio is too heavy to copy per project, and a project using it must not go offline |
| User data | Only under `paths::home()` (`$KOOEDITOR_HOME` or `~/.koo-editor`); project files and exports go where the user saves them | AGENTS.md hard rule 2 |
| Online providers | `koo-cloud`: a catalog of providers, each a folder with its client behind `Api` and the capabilities it offers (`Capability`: transcription, music, sound effects, voiceover, video) on which models; the user picks one per capability in Settings → Integrations (`CloudSettings`, per user in the store). Nothing runs online unless picked, and transcription stays local by default | A new provider or capability is a catalog entry, and settings, analysis and the agent find it there |
| API keys | In the system's credential store (Keychain, Credential Manager, Secret Service; `keyring`) under the service "koo-editor"; where there is none, an owner-only `credentials.json` in `paths::home()`. A provider's environment variable (`ELEVENLABS_API_KEY`, `FAL_KEY`, `PIKA_API_KEY`) wins | Never in the database, `.koo` files or logs |

| Subject masks | SAM 2.1 tiny (Apache-2.0): on macOS written over MLX (`mlx-rs`) and run on the GPU; elsewhere as five ONNX graphs (square-zero-labs' video export, pinned), the memory bank assembled in Rust as `transformers`' `Sam2VideoModel` does; prompts on source frames, mattes (the model's 256² logits) kept by fingerprint, prompts and model | Clicks and a box say *which* object, the model finds its edge, for the user and the agent alike; tracking follows it through occlusion; the same weights on either runtime, one checked against PyTorch and the other against it |

Confirm CLI flags against the installed `claude --help` and the current Codex
docs before relying on them (PRD note).

## 3. Threading

GPUI owns the main thread. `EditorState` (the bus, selection, chat) lives
there. Anything slow (probing, decoding, proxies, collages, analysis,
rendering, the agent subprocess) runs on background threads or
`cx.background_executor()` and reports back through `Entity::update`. The
bus is not `Sync`, by design: agent tool calls arrive over a channel and are
served in `AgentLink::pump`, which `EditorState` calls every 33 ms. Slow
tools (views, previews, FFmpeg work) return a job that runs off-thread on a
`Project` snapshot; its proposals are applied back on the UI thread.

Heavy background work that must not pile up goes through
`koo_core::jobs::JobQueue`: FIFO, at most `limit` jobs at once on threads of
its own, a key already queued or running returns that job, and a job that
panics fails without stopping the queue.

## 4. Editing model

* **Project** (`koo-core/src/model.rs`): media, bins, one `Timeline` (tracks
  of kind Video, Audio, Title, Caption, Overlay; markers; ducking; a timeline
  grade; `SequenceSettings`) and the project's FX programs.
* **Clips** carry speed, enabled, name, color, a transform, a grade, an
  effect stack, audio effects, volume automation, `transition_in` /
  `transition_out` (resolved by `fx::transitions`) and masks.
* **Transform:** position, scale, rotation, opacity, per-axis stretch, skew,
  anchor, crop, flip, blend mode, `fit` (`Contain` with bars, or `Cover`,
  filling the frame and panned by position) and a corner pin; all keyframable.
  A pinned picture is placed through a homography (`Placement::homography`),
  others through one affine matrix shared by the GPU and CPU compositors.
  `compose::GroupMove` moves, scales and turns several clips about a common
  pivot and turns that back into each clip's transform.
* **Masks** (`koo-core/src/mask.rs`): rectangles, ellipses and closed paths
  (straight or smooth) in the picture's own uv, with feather, expansion,
  opacity and invert, combined in order by add, subtract or intersect, each
  keyframable as a whole. Captions and titles take no masks or corner pins:
  renders burn them in with ASS, which cannot draw them.
* **Subject masks** (`MaskShape::Subject`, `koo-core/src/subject.rs`): the
  mask keeps the object its `Subject`'s prompts pick: on some source
  frames, clicks on the object (or off it) and a box, in picture uv, so
  trims, splits and speed changes keep them on their frames. With `track`
  the object is followed over every frame the clip shows; otherwise each
  frame shows the nearest prompted frame's matte. `refine` eases the edge
  onto the picture's (hair). Feather, expansion, opacity, invert and mode
  work as for shapes; the prompts are not keyframed.
* **Cuts in speech** (`koo-core/src/cuts.rs`) go in the pause between two
  words, clear of both (40 ms after the earlier, 30 ms before the later), on
  a frame boundary when the pause holds one, with a 10 ms audio fade at the
  join. The transcript panel's word delete and the agent's `cut_words` use
  it; the agent's `blade` and `ripple_delete` move a cut that falls on a word
  into the nearest pause and say so (`snap: false` turns it off).
* **Queries** (`koo-core/src/query.rs`): topmost clip at a time, sample
  points, transcript↔timeline word mapping, typed timestamp and chat
  reference resolution, effective style rules.

### Change sets

`Author::Agent { role }` and `Author::Recipe` commands go into the author's
open `ChangeSet` (`koo-core/src/changeset.rs`, `bus.rs`), planned against that
set's *draft* (committed project + its own pending items), so the agent sees
the effect of its earlier calls.

* **One growing set per chat.** Agent sets are stamped with the bus's current
  chat; the timeline overlay shows only the open chat's sets. At the start of
  each turn the host calls `reopen_change_set`: the chat's pending set opens
  again and the turn adds to it. If the user edited since, the draft is
  rebuilt on the current project by replaying each item's ops (re-planning
  those that fail, dropping those that still fail).
* **Review waits for the turn.** At the end of a turn the set becomes
  `Pending`, or `Resolved` if empty; the card is posted on the newest message
  only if the set grew. While the set is `Open`, the card's Accept and Reject
  are disabled; Preview and Review still work.
* **Accept** takes all pending items or the ticked subset. An item becomes
  `Conflicted` if a clip its ops touch was changed by the user after the
  version it was planned against; otherwise its ops replay, or its command
  is re-planned if they no longer apply. The whole accept is one history
  entry (AG5).
* Change sets live in the bus only; saved chats keep a message's change-set
  id, not the set.

## 5. Compositor and FX

* **One FX format** (`koo-core/src/fx/mod.rs`; its module docs are the
  authoritative GLSL and HTML conventions). An `FxAsset` is a GLSL program
  (effect, transition or generator) or an HTML/CSS/JS document (generator:
  titles, lower thirds, motion graphics, image layouts) plus typed
  parameters (number, bool, color, point, text, choice, image). Built-ins:
  51 GLSL programs in `fx/shaders.rs` (27 effects, 17 transitions,
  7 generators) and 15 HTML templates in `fx/templates.rs`. Programs the
  user or the agent writes live in `Project::fx` and are edited in the FX
  code editor. An `FxInstance` uses an asset with parameter values and
  keyframes: on a clip's effect stack, as a transition, or as a generator
  clip's content.
* **GLSL** follows ShaderToy/gl-transitions: authors write `effect(uv)`,
  `transition(uv)` or `generate(uv)`; `koo-gpu` wraps it as `#version 450`,
  compiles it with naga on a thread with a 16 MiB stack, caches the result
  and reports errors against the author's lines. `iResolution` is in
  sequence pixels whatever size the effect runs at.
* **HTML** renders in headless Chromium (Chrome, Chromium, Edge or Brave, or
  `KOOEDITOR_CHROME`) over CDP with an engine-owned clock (`clock.js`: time,
  timers, rAF, CSS animations, Web Animations and GSAP all follow clip time,
  `Math.random` is seeded), so frames are deterministic and exportable, with
  alpha. GSAP 3.15 and its plugins are vendored in `koo-html/src/gsap` and
  preloaded; templates build their animation in `kooTimeline`, which the
  engine reruns when parameters or the clip length change. Pages lay out on
  the sequence's shape with a 1080 CSS px short side, so templates keep
  their proportions at any resolution; in portrait, bottom elements rise
  above the controls vertical apps draw.
* **Program frame** (`koo_core::compose::program`): for each layer, the
  picture (a media frame at the speed-aware source time, an overlay, or a
  generator) → clip grade, then timeline grade → effect stack → masks (a
  coverage raster multiplied into alpha; a subject's matte at the source
  time drawn into it, see "Subject masks") → transform and blend mode;
  transitions render each side onto its own canvas and mix them. Titles and
  captions are lettered on top by `LetteringService` (cosmic-text
  `koo_deliver::Letterer`). The whole frame is one `FxBatch` on the GPU: one
  upload per source picture, every pass in one submit, one readback (already
  BGRA); submit and wait are separate so playback pipelines frames.
  Without a GPU, `CpuBatch` and `compose/cpu.rs` composite in memory
  (transitions become a dissolve; effects and GLSL generators are skipped).
* **Grade** (`koo-core/src/grade.rs`): one pipeline (exposure, contrast, saturation,
  temperature/tint, lift/gamma/gain, offset, shadows/highlights, hue,
  curves, a LUT file), baked into a 65³ LUT for export.
  `scopes.rs` reads waveform, parade, vectorscope and histogram and matches
  shots.
* **Audio effects** (`koo-core/src/audiofx.rs`, `audiofx/dsp.rs`) run in the
  preview mixer and map to FFmpeg filters in `koo-deliver/src/audio_fx.rs`,
  table by table, so preview and export match.

## 6. Media and playback

* `koo-media`: ffprobe (variable rates snapped to standard ones),
  fingerprints, single frames (96 MiB in-memory cache), playback streams
  (RGBA frames as PAM over a pipe) and f32 PCM streams, proxies (short side
  ≤ 720, x264 veryfast, no B-frames, half-second GOP), 6 × 5 collages stamped
  with timeline and source timecodes, waveform peaks (200/s) and spectrogram
  pictures (`showspectrumpic`, one lane per track, words, sounds and beats
  marked). Derived files are written atomically.
* Cache layout under `paths::cache()`: `proxies/<fp>.mp4`,
  `waveforms/<fp>-<stream>.peaks`, `collages/`, `analysis/<fp>.json` and
  `<fp>.vectors`, `mattes/<fp>/` (subjects: `p-<key>.matte` per prompted
  frame, `t-<key>.mattes` per tracked subject), `html/` (pages and browser
  profiles), `render/<job>/` (scratch), `agent/frames/` (images for the
  activity log).
* **Viewer** (`ui/viewer*`): each playback stream reads 4 frames ahead on its
  own thread (`compose/pictures.rs`); the player submits frame N+1 before
  reading N back and drops to 1/2 or 1/4 resolution when frames cost more
  than the frame time, probing back up when they cost under half of it.
  Paused and full-resolution frames are cached (768 MiB, LRU, keyed by
  project version and previewed change set), and the 4 s after a resting
  playhead render ahead. Audio plays through cpal. The viewer zooms and pans
  with a safe-area overlay; gizmos move, scale, stretch, rotate, skew and
  corner-pin one or several items with snapping, and a mask mode draws and
  edits masks, the selected picture shown whole with what its masks cut
  away tinted (`compose::MaskView::Tinted`, a toggle). Its subject tool
  previews what a click would keep as the mouse moves (off the UI thread,
  the newest hover winning), keeps it on a click or boxes it on a drag; on
  the picked subject a click adds a point to the frame's prompt (Alt: off
  the object, Shift: a new subject), a click on a point takes it away, and
  points drag. Cached frames are dropped when the subjects' version moves,
  so mattes show as they are made.

## 7. Analysis and models

Analysis starts only when the user asks, from an item's "⋯" menu in the
media pool (or its right-click menu): "Analyze", or "Analyze again" once it
has results, which runs every step anew on the models picked now. The
exception is the library's sounds, analyzed on their own without speech or
shots, on local models only, so that it costs nothing. "Analyze" is
disabled while a model it needs is missing (or the transcription provider
has no key), or while the item is queued or analyzing; images are not
analyzed. Settings → Analysis turns steps off (`AnalysisSteps`:
transcript, speakers, sounds and music, beats, sound search, shot
changes); a step off stays empty in the results, and its model is not
needed. Speakers need the transcript; beats and sound search, the sound
pass. Items are analyzed one at a
time, in the order asked, on a `JobQueue`. A result is written to
`cache/analysis/<fp>.json` only when every step succeeded; otherwise it is
kept for the session and the job fails, naming the steps.
`ANALYSIS_VERSION` (1) marks the results: those of another version, and the
sound vectors beside them, are not read, so the item is not analyzed until
the user analyzes it again.

### Models (`koo-models`)

One folder per task (voice activity, transcription, word timing, speakers,
sound tagging, beats, sound search, segmentation). Its `mod.rs` holds the
task's interface, the trait analysis calls (`VoiceActivity`, `Transcriber`,
`Aligner`, `Diarizer`, `Tagger`, `BeatTracker`, `SoundSearch`, `Segmenter`); `MODELS`, the
models that can do it, the default first; and `runner`, which builds the
runner of one of them. In it, one folder per model: `mod.rs` is the model as
the user sees it, `config.rs` its files (pinned to a commit, with their
sizes) and the settings it runs with, `runner.rs` the code that loads and
runs it (behind its feature) and finds its own files. What models share
sits at the crate root: the catalog types, the audio rates, the downloads,
ONNX Runtime (`onnx.rs`; one environment per process, shared by the
aligner, the diarizer, the sound models and segmentation) and the BPE tokenizer
(`bpe.rs`).

Analysis only orchestrates: it asks each task's runner for the selected
model (`Task::model`: the one the user picked, else the default) and never
names a model; a new pick of transcription model applies from the next
transcript, and results made already stay. A new model is a folder under
its task, its `MODEL` in `MODELS` and an arm in `runner`; a model in several
sizes (Whisper) has a `MODEL` for each in one folder.

Nothing downloads on its own. Settings → Models (the gear in the top bar, or
cmd-,) lists each task with its model, picked there when the task has more
than one (saved per user in the store), and downloads one or all of them,
with progress and cancel, through `<file>.part`; a failed download stays
there with Retry. `LocalAnalysis::model_tasks` says which tasks a build
runs; "Analyze" stays disabled until each has its picked model downloaded.
A runner whose files are missing fails ("… is not downloaded") and its step
is left out. While a model is missing, the top bar's notices button lists
them; its notices (warnings and errors, such as a failed download or
offline media) can be dismissed for the session.

Cargo features of `koo-analysis` (all default) gate the runners: `whisper`,
`aligner`, `diarizer`, `sounds`, `subjects`. Without one, its models stay
listed but its step falls back as described below (without `subjects`,
subject masks keep nothing). `koo-models`' `mlx` (macOS only: mlx-sys links
Apple's frameworks) adds the MLX subject mask model and makes it the
default; `koo-editor` turns it on with `metal` on macOS. mlx-sys builds
MLX from source, so a macOS build needs CMake and Xcode's Metal compiler. Segmentation is listed under Settings → Models
as "Subject masks" but is not part of analysis: `required_tasks` never asks
for it.

### Transcription online

With a provider picked for transcription in Settings → Integrations
(ElevenLabs Scribe v2, or v2 Medical), the speech steps are its and no
local speech model runs or is needed: voice activity, Whisper, the
aligner, the holes heard again and the speaker models are skipped, and the
Models page shows the provider on their rows. The stream is encoded once
more as lossless mono FLAC at its own rate, up to 48 kHz
(`cache/upload/<fp>.flac`, deleted after), and sent in one request with
word times, speakers (`diarize`, when the speakers step is on) and audio
events; filler words are kept, since cutting dead time needs them. Words
keep the times the provider aligned (the edges measured on the audio are
tuned to Whisper's late starts and early ends, and pulled good times off
the words); confidence is `exp(logprob)`; its voices are numbered by
`speakers::apply`, so ids stay when an item is analyzed again; a
three-letter language is turned into ISO 639-1. Speech is where the words
are: words under 250 ms apart make one range, which starts 60 ms early, as
Silero's do. The transcript records what made it (`engine`:
"elevenlabs/scribe_v2", or the Whisper model's id).

The other models become optional: a sound step whose model is missing is
left out rather than failing, and when the sound pass doesn't run its place
is taken by the provider's audio events ("laughter" as a sound, anything
with "music" as music without tags or beats). A provider error (no key,
out of credits, network) fails the transcript step like any other, and is
not retried with Whisper.

### Speech

* **Voice activity:** Silero VAD v6.2 (`voice_activity/silero/`;
  `ggml-silero-v6.2.0.bin` from ggml-org/whisper-vad at a pinned commit, or
  `KOOEDITOR_VAD_MODEL`), run by whisper.cpp: speech from 0.5 until under
  0.35, pauses under 250 ms kept, 60 ms before each range. Music is not
  speech to it. The energy VAD (`koo-analysis/src/vad.rs`), which took a
  music bed for speech, stands in without the `whisper` feature, or for the
  session when the model can't be had. Silero hears the audio in one go and
  in 4 s blocks, each after the second before it, and a window is speech
  when either finds it: music holds it down for seconds after, and a block
  that starts during a sentence doubts the rest of it. A silence never takes
  a transcribed word, so the quiet last word of a phrase Silero let go of
  stays.
* **Transcription:** Whisper small (264 MB, the default) or large
  (large-v3-turbo, 874 MB), both with 8-bit weights, picked in Settings →
  Models, or `KOOEDITOR_WHISPER_MODEL` (a path or a ggml file name in
  `paths::models()`) for whichever size is picked. whisper.cpp is built for
  the GPU (Metal on macOS, Vulkan on Windows and Linux, with flash
  attention), so building needs Xcode or the Vulkan SDK (`glslc` and the
  Vulkan headers on Linux); without a GPU it runs on the CPU. On Windows
  the editor delay-loads `vulkan-1.dll` (`koo-editor/build.rs`), and where
  the DLL is missing `koo-editor/src/vulkan_fallback.rs` makes Vulkan fail
  to start, so Whisper runs on the CPU there too.

  Whisper hears only speech (`transcription/whisper/`): given music, a long
  silence, or a 30 s window that starts with either, it made words up,
  often repeating what was said before, and its times came loose. It hears
  the stretches Silero finds as faster-whisper's VAD filter does (pauses
  under 2 s kept, 400 ms either side), each on its own, without the text of
  the window before, in the language of the longest; its notes such as
  "(upbeat music)" are dropped. A stretch that fits one window starts with
  0.5 s of silence: Whisper took speech from the very start of what it
  heard for a phrase cut off and dropped it (the first words of an item
  that starts talking), and Whisper that started on speech more often left
  out the last phrase of the stretch too. A longer stretch goes on in
  windows whisper.cpp starts where Whisper finished a segment; silence
  before it moved those windows, and in conversation one then ended at its
  first sentence and skipped the rest. Decoding is a beam search of 5, as
  OpenAI's transcribe does. Now and then Whisper gives one sentence a
  segment that runs on over seconds of speech it skips, or takes the short
  last phrase of a stretch for silence; so once words are aligned, Silero's
  speech that no word comes within 250 ms of, for 600 ms or more, is heard
  again on its own, and the words found there are aligned against that
  audio alone and added (on the measurements below every such hole held
  words that had been lost, and there were none otherwise).
* **Word times** (`word_timing/qwen3_aligner/`, `aligner` feature):
  Qwen3-ForcedAligner-0.6B hears up to 30 s of audio with Whisper's words
  and answers with each word's start and end, in 80 ms steps; starts then
  move 20 ms earlier, never into the word before, and the edges of words
  next to a pause of 100 ms or more 20 ms into it, never past its middle
  (its starts after a pause came late and its ends before one early,
  against forced-aligned LibriSpeech). It knows English, Spanish, French,
  German, Italian, Portuguese, Russian, Chinese, Cantonese, Japanese and
  Korean. Its tokenizer comes from Qwen's repository and the model from
  ours (below), or `KOOEDITOR_ALIGNER_MODEL`. Other languages keep word
  edges measured on the audio levels (`koo-analysis/src/align.rs`), and so
  does the result for the session when the model can't be loaded; words
  with neither letters nor digits stay between the aligned words around
  them. That measurement runs only where the aligner can't: run before it,
  it moved words over the pause between two speakers and the aligner
  followed (at 12 of 55 turns in four conversations, 4 without; the word
  error rate there went from 5.9% to 7.1%), and run after it, it made cuts
  worse. The aligner's times never pass the audio it was given, so words
  stay in order.
* **Voiceover words:** a voiceover is analyzed as soon as it is made, when
  the transcript step is on, and its words are its script
  (`cloud::spoken_words`: no audio tags, punctuation on the word before),
  so neither Whisper nor a provider hears it and it costs no credits. The
  rest of the script is shared out over the speech left by written length
  (`align::spread`), the aligner hears 30 s from where the last word placed
  ended with the words expected in the first 25 s, and words it puts in the
  last 3 s go again with the next stretch (`follow_script`), so an error in
  how long the script takes doesn't add up over a long read. A stretch
  can't hear past a long pause: silences over 10 s that neither the voice
  activity nor the levels hear anything in are shortened to 3 s either side
  first (`Shortened`), and a word placed over one keeps the side with more
  speech. A passage of the script that isn't said, longer than the model
  can squeeze where it is, leaves the words after it late, and a stretch
  never hears before where the last one ended. Such a stretch squeezes many
  of its words into one 80 ms step in scattered runs (a tenth at most when
  well heard, one run for a passage squeezed in place); then short
  stretches from words further on find the fewest words to leave behind
  (`catch_up`), and the stretch before is heard again up to there, kept if
  it follows the audio but for one squeezed run. Measured on espeak-ng
  reads against its word events: 30–40 ms mean start error through 7
  minutes; a 48-word unsaid passage left 0.5 s mean error (8.7 s before),
  confined to around it. The language
  isn't known ("und"), so the aligner runs on any script. Its models are
  optional: without the aligner the words keep edges measured on the audio
  levels, and "Analyze again" aligns them once it is there. Analyzing a
  voiceover always takes this path.
* **Alignment model file:** Qwen publishes PyTorch weights only, so
  `qwen3-forced-aligner-0.6b-q4.onnx` is our export, hosted at
  [Chris-Kode/qwen3-forced-aligner-0.6b-onnx](https://huggingface.co/Chris-Kode/qwen3-forced-aligner-0.6b-onnx)
  with its build scripts (its `config.rs` pins the commit;
  589,447,679 bytes, SHA-256 `228f127b…9ab49df`). The graph takes 16 kHz
  samples (a multiple of 160) and the text's token ids, and returns the
  argmax of the 5,000 timestamp classes for each token. Inside it: the
  Whisper log-mel features of `WhisperFeatureExtractor` (400-point frames,
  hop 160, 128 mels), the audio encoder's convolutions per second of
  features and its layers with attention over the whole chunk (as Qwen's
  CPU path does), the audio between `<|audio_start|>` and `<|audio_end|>`
  ahead of the text, and the causal text model. Exported with
  `torch.onnx.export(dynamo=True)` at opset 18; constant-folded with ONNX
  Runtime's basic optimizer; `Gemm` nodes rewritten as `MatMul` + `Add`;
  weights quantized with `MatMulNBitsQuantizer` (4 bits, block 32,
  symmetric; the embedding table as `GatherBlockQuantized`). It matches
  Qwen's own pipeline exactly before quantization and to within 0.5 points
  of clean cuts after; dynamic int8 quantization breaks it. The repository
  carries the Apache-2.0 licence text and states that it is a modified
  (exported, quantized) Qwen3-ForcedAligner-0.6B.
* **Speakers** (`speakers/pyannote/`, `diarizer` feature) follow pyannote's
  speaker-diarization-3.1 on ONNX Runtime. pyannote segmentation-3.0 (MIT)
  hears 10 s windows 2.5 s apart and says, every 17 ms, which of up to three
  voices of the window speak. WeSpeaker ResNet34-LM (CC-BY-4.0, trained on
  VoxCeleb) turns each window's voice, where it speaks alone, into a
  256-number voiceprint from Kaldi filterbank features (`fbank.rs`, equal to
  `kaldi-native-fbank` to 1e-5). The voiceprints are clustered with centroid
  linkage at pyannote's threshold (0.7046), clusters of fewer than 12 join
  the nearest larger one, and the windows are averaged back into who speaks
  in each frame. A word goes to the voice that speaks through most of it,
  overlaps weighed by how many windows heard each; below 0.6 of the
  speaking it keeps no speaker. Speakers are numbered in order of
  appearance, and attributed after the aligner. Both models come from their
  own Hugging Face repositories (`onnx-community/pyannote-segmentation-3.0`,
  6 MB, and `Wespeaker/wespeaker-voxceleb-resnet34-LM`, 26 MB; its
  `config.rs` pins the commits). When they can't be loaded, every word is
  "Speaker 1" for the session.

### Sounds, music and beats

* After the transcript (`sounds` feature), the audio is decoded once more at
  32 kHz and streamed through the sound tagger (`sound_tagging/efficientat/`)
  and CLAP's audio tower (`sound_search/clap/`) together; then the beat
  tracker (`beats/beat_this/`) hears each stretch of music the tagger found,
  with 1 s either side. Runs of tagged windows become `Analysis::sounds` and
  `music`: a run starts up to 2 s before its sound, so it is shrunk by that
  much, and a sound under a second long moves onto the loudest stretch of
  the 10 ms levels around it. Silence alone scores 0.25 as music, so music
  needs 0.5 to start; bells, claps and other musical sounds inside music are
  dropped. Music carries tags (genre, mood, vocals, lead instruments), beats
  and downbeats; its tempo is the mean beat interval near the median. The
  vectors go to `cache/analysis/<fp>.vectors`. A model that cannot be had
  leaves the sounds out, and what the others found is kept for the session.
  On 4 CPU threads the pass takes about 2 minutes per hour of audio for the
  tagger and 6 for CLAP, plus the beat tracker on music.
* `koo-core/src/heard.rs` maps sounds, music, beats, level changes and
  silences onto the timeline, and finds beats, downbeats and onsets (a 6 dB
  rise over the 50 ms before) near a cut.
* **Sound model files:** our exports, built by `scripts/models/` and hosted
  (with those scripts and the licence texts) at
  [Chris-Kode/efficientat-mn10-as-onnx](https://huggingface.co/Chris-Kode/efficientat-mn10-as-onnx),
  [Chris-Kode/beat-this-final0-onnx](https://huggingface.co/Chris-Kode/beat-this-final0-onnx)
  and [Chris-Kode/larger-clap-general-onnx](https://huggingface.co/Chris-Kode/larger-clap-general-onnx);
  each model's `config.rs` pins their URLs to a commit. JKU's server was
  unreachable, so the Beat This! export was made from the
  `safe-models/beat-this-final0` mirror, whose weights have not been
  compared with JKU's. Each graph takes 32 kHz mono samples and holds its
  model's own front end, so analysis decodes once for all three:
  `efficientat-mn10-as.onnx` (20 MB; windows × samples in, 527
  probabilities out; matches EfficientAT's pipeline to 2e-6),
  `beat-this-final0.onnx` (85 MB; up to 30 s in, beat and downbeat logits
  per 20 ms frame out; a windowed-sinc resampler to 22.05 kHz stands in for
  soxr), `larger-clap-general-audio.onnx` (78 MB; 10 s windows in, unit
  vectors out; resampled to 48 kHz inside; dynamic int8, cosine 0.997 to the
  float model) and `larger-clap-general-text.onnx` (141 MB; RoBERTa token
  ids in; 8-bit block weights and an int8 embedding table, cosine 0.998).
  The text tower's tokenizer comes from LAION's repository at a pinned
  commit (`bpe.rs` encodes it). `KOOEDITOR_TAGGER_MODEL`,
  `KOOEDITOR_BEATS_MODEL`, `KOOEDITOR_CLAP_AUDIO_MODEL`,
  `KOOEDITOR_CLAP_TEXT_MODEL`, `KOOEDITOR_CLAP_VOCAB` and
  `KOOEDITOR_CLAP_MERGES` point at local copies instead.

### Subject masks

* **Model** (`koo-models/src/segmentation/sam2/`): SAM 2.1 tiny (Meta,
  Apache-2.0) as square-zero-labs' ONNX export of `transformers`'
  `Sam2VideoModel` with the memory modules (190 MB; its authors checked it
  against `propagate_in_video_iterator`, worst per-frame IoU 0.9967), pinned
  to a commit: a vision encoder (1024² picture → features at three scales),
  a decoder (features, labelled points → 256² logits, a score, the object's
  pointer), a memory encoder, the memory attention and the pointers'
  temporal positions, plus `constants.json`. Pictures are stretched to the
  model's square with a triangle filter (PIL's bilinear) and normalized; a
  box is two corner points labelled 2 and 3 before the clicks. Checked
  here against a Python reference of the same loop on `transformers`'
  bedroom clip: per-frame IoU ≥ 0.9988.
* **On MLX** (`koo-models/src/segmentation/sam2_mlx/`, feature `mlx`, the
  default model on macOS): the same network written over `mlx-rs` from
  SAM 2's modules (Hiera with windowed and pooled attention, the FPN neck,
  the prompt encoder and two-way decoder, the memory encoder and the memory
  attention with axial rotary positions), on the GPU in unified memory, with
  the MLX conversion of Meta's checkpoint in half precision (90 MB, from
  avbiswas' `sam2-mlx`, pinned; computed in f32). Pictures go through the
  same `plan::square`. Its decoder chooses as SAM 2 does: best of the three
  multimask answers for one click or none, else the single answer unless it
  is unstable; the ONNX decoder always takes the best of three, so a box or
  several clicks can differ slightly (IoU 0.95 on the bedroom clip's box;
  forcing the same choice gives 1.0000). One click: IoU 1.0000 and the same
  scores. Tracking attends to the memory there is, as SAM 2 does: every
  prompted frame (`plan::bank` without a cap) and the tracked ones, without
  the padding the ONNX graphs' fixed bank needs (per-frame IoU 0.97–1.0
  against the ONNX runner over 8 frames; padded the same way, 1.0000 on 7
  of them and 0.9977 on the other). Checked on Linux, MLX on the CPU (a
  local build of mlx-sys linking OpenBLAS); not yet run on a Mac.
* **Tracking** (`runner.rs`, `Segmenter::track`): each prompted frame is
  segmented by itself (its memory binarized); the frames after the first
  prompted one are tracked forward from it and those before it backward,
  each attending to a fixed bank of 7 frames of memory (the nearest
  prompted frames before and after, then the latest tracked, padded by
  repeating the latest) and 16 object pointers (prompted frames passed and
  the latest tracked, padded the same way), with temporal positions as
  `transformers` gives them. Occlusion is the model's: a hidden object
  gives an empty matte and is found again when it comes back.
* **Service** (`koo-analysis/src/subjects.rs`, `LocalSubjects`): frames come
  from the media engine (the proxy when there is one), numbered on the
  media's rate as it decodes them; tracking reads forward with one stream
  and backward in chunks of 24. Each prompted frame is kept by
  fingerprint, frame and prompt (a frame previewed in the viewer is not
  segmented again once the click lands in a subject); a tracked subject by
  fingerprint and `Subject::key` (all its prompts), over every frame its
  clip shows and its prompted frames. Both keys hold the model's id, so
  picking the other model in Settings → Models makes them anew, and the
  runners are made again for it (an `Encoded` frame is only read by its
  own model's runner). Mattes are cleaned (islands and holes
  under 16 cells), kept to 1/16 of a logit up to ±8 and run-length coded
  (about 6 KB a frame), in memory and under `cache/mattes/` (`store.rs`).
  Previews run on a model instance of their own and keep the last 3
  encoded frames, so picking stays quick (the first click on a frame
  encodes it, about 2–4 s on a laptop CPU; later ones 50–80 ms) while a
  subject tracks on the other. A tracking job waits 1.2 s before it starts,
  so a few quick clicks run one; a newer subject of the same mask cancels
  its older job. Tracking costs about 4 s a frame on 4 CPU cores. A failed
  frame or job is not tried again until the model is downloaded anew.
* **Drawing** (`Matte::field`): the logits are upsampled as the model does
  (bilinear between cell centres) to the mask raster, with an edge one
  pixel soft from the logits over their slope; `refine` eases it onto the
  decoded frame's luma with a guided filter (radius a 135th of the height,
  ε 10⁻³) within three radii of the edge. A feathered or expanded subject
  is drawn from signed distances instead (the logits near the edge, an
  exact distance transform further out), like shapes. A subject without
  its matte yet is left out of the raster; layout bounds a picture by its
  subjects' mattes.

## 8. Agent

* **Backends** (`koo-agent/src/backend/`): Claude runs as
  `claude -p --input-format stream-json --output-format stream-json` with
  only the koo MCP server (`--strict-mcp-config`, `--tools ""`,
  `--permission-mode dontAsk`, no setting sources or slash commands), the
  system prompt, `--model`/`--effort`, and `--resume` or `--session-id`.
  Codex runs as `codex exec --json --sandbox read-only` with the user's
  config and rules ignored, shell and patch tools off, and the MCP server,
  developer instructions, model and reasoning effort as `-c` overrides,
  resuming its thread. Each turn runs in an empty workdir under
  `agent/work`, so the user's own CLAUDE.md/AGENTS.md are not picked up. A
  backend that lost its session is retried fresh. `KOOEDITOR_CLAUDE` and
  `KOOEDITOR_CODEX` point at the CLIs.
* **Models and thinking:** two pickers next to Send list what the installed
  CLI offers (Claude's stream-JSON `initialize` request, Codex's
  `model/list` on `codex app-server`), cached in `agent/models.json` and
  refreshed in the background; each model brings its thinking levels.
  Choices are kept per backend in the user settings.
* **MCP server:** `koo_agent::mcp::run()`, launched by the backend as
  `$KOOEDITOR_MCP`, else the `koo-mcp` binary next to the editor, else
  `koo-editor mcp`. It connects to the editor over TCP on 127.0.0.1 with the
  turn's token (`KOO_BRIDGE`/`KOO_TOKEN`).
* **Roles and permissions** (`koo-core/src/bus.rs`): `AgentRole::Editor`
  (default) or `Builder` (read tools, memory and FX authoring only), each
  with its own prompt and backend session. Permissions are per
  `ToolCategory`: read and view are always allowed; edit categories are Off
  or Propose (never auto-applied); project memory (rules, recipes, facts,
  saving to the library) and deliver (renders, exports) always need the
  user's approval, asked with Approve/Deny cards above the composer and
  carried out by `agent_link/approve.rs`. The call waits: the host holds
  its MCP reply until the user decides and answers it with the outcome
  (a generated voiceover's media id, for one), so the turn carries on; an
  outcome whose call is gone (the turn ended or was cancelled) reaches the
  agent in the next turn's `<approvals>` block. Backends allow a tool call
  an hour for this. Generate media (music, sound
  effects and voiceovers, which spend the user's credits) is Off, Ask each time (the
  default) or Allow. A "Web" switch (off by default,
  overridable per project) gives Claude WebSearch/WebFetch and Codex web
  search.
* **Tools** (`koo-agent/src/tools/`): 82 in the catalog (80 plus
  `load_tools` and `run_tool`). Remove, move and detail operations are flags
  of their `set_*`/`view_*` sibling (for example `view_range` with `detail`).
  Each turn lists 20 core tools plus `run_tool`, and the groups (`edit`,
  `transform`, `captions`, `color`, `audio`, `effects`, `overlays`,
  `generate`, `memory`, `deliver`) the turn is likely to need, picked from the
  workspace, the selection and words in the message. A chat keeps the list
  it started with (`agent/tool_groups.json`) so the backend's prompt cache
  holds; later groups come through `load_tools` and are called with
  `run_tool`. External MCP clients see every tool. Each turn's use, with
  result sizes, is logged to `agent/tool_usage.jsonl`. Results are compact
  JSON that leave out neutral defaults.
* **Seeing and hearing:** `view_range` (collages, opening with the sound
  script), `view_frame` (annotated, with a `zoom` region under a pixel
  grid, or with `masks` showing what masks cut away tinted, naming subjects
  still being found), `get_layout`, `get_scopes`, `preview_fx`, `get_audio` (the sound
  script for any range), `get_music` (tempo, downbeats, level changes; every
  beat with `beats`), `search_sounds` (2 s steps ranked by a description or
  an example range) and `view_audio` (up to 30 s of spectrogram lanes).
  `blade` and `ripple_delete` take `align: beat | downbeat | onset`; a
  target clear of words wins over a nearer one on a word.
* **Subject masks:** `set_mask` with `shape.type: subject` (`tools/masks.rs`)
  takes `include` and `exclude` points and a `box`, in picture fractions
  or frame pixels, on the frame at `at`; each list given replaces that
  frame's own. Before proposing, it runs the model on that frame and says
  what it kept (its share of the picture, its box in frame pixels, the
  model's confidence), with advice when it found nothing, took nearly the
  whole picture or is unsure; with `track` it says how many frames are
  left and how long that takes here. `get_clip` lists a subject's prompts
  on the timeline and how far it is tracked. The prompt steers the agent to
  subjects for people and objects, and to check with `view_frame` `masks`.
* **Generating audio:** `generate_audio` (`tools/generate.rs`) makes music
  (a prompt and a length, or `sections` with lengths of their own, so it
  changes on cue), a sound effect (up to 30 s, optionally looping) or a
  voiceover (a script of up to 10,000 characters, one voice from
  `find_voices`, its stability and similarity) with the provider picked
  for it. The file goes to `paths::generated()` with a `<file>.json` of
  what was asked, and into the pool's "Music Generated", "Sound Effects
  Generated" or "Voiceover Generated" bin as the user's edit (`CommandBus::execute_shared`, which also adds it to the
  drafts waiting for review), so a rejected placement keeps what was paid
  for; `MediaItem::generated` records the provider, model and prompt, and
  for a voiceover who read it, so the pool can regenerate it. With
  `at` or `range` it is proposed on an audio track free over its length,
  else a new one. Asked each time, the approval runs it in the background,
  proposes the placement into the chat's set and answers the waiting call
  with the media id; allowed, it runs as a tool job and answers the same. A
  refused prompt (an artist's name) comes back with the provider's rewrite.
* **Generating video:** `generate_video` (`tools/video.rs`) makes a shot
  with the provider and model picked for video (on fal: Seedance 2.0, 2.0
  Fast and 2.5, Kling 4.0 and O3 Pro, MiniMax H3 Max, Wan 3.0, Happy Horse
  1.1, Gemini Omni Flash 1.1, FLUX 3 Max, LTX 2.5 Pro; on Pika: Pika H3
  Flex and Pika 2.5, and Seedance, Kling, MiniMax H3, Hailuo, Wan, Happy
  Horse, Gemini Omni Flash, FLUX and Grok Imagine as Pika hosts them): from the prompt alone, from a first frame (a picture in the pool,
  or the program at a time, grabbed with `compose::program` into
  `paths::cache()/frames` just before) to an optional last one, or after
  references (pictures, clips and sounds in the pool, which the prompt names
  `@Image1`, `@Video1`, `@Audio1`). Each model's `cloud::VideoModel` (its
  lengths, shapes, resolutions and reference limits) refuses what it can't
  make before anything is spent. Models name their endpoints and fields
  differently (`duration` as text or a number, `image_url` or
  `start_image_url`, a word for "the model picks the shape"), so each is
  an entry in `fal/models.rs` saying how it takes a request; references
  named `@Image1` are renamed to what the model reads (`Image 1`,
  `character1`, `<IMAGE_REF_0>`). fal runs it on its queue
  (`fal/queue.rs`), uploading the files it follows to fal's storage first;
  Pika runs it as a media job (`pika/jobs.rs`, `docs/pika.md`) after
  uploading to Pika, each model an entry in `pika/models.rs` as on fal, and
  sends a last frame on Pika 2.5 to Pikaframes.
  `cloud::GenerationTask` carries where it is (queued, running, coming
  down) to the media pool's row, which can stop it until it is made where
  the provider can stop it (fal can; Pika can't, so its job is marked
  `unstoppable` once taken). It
  lands in the "Video Generated" bin and, with `at` or `range`, is proposed
  on a video track free over its length (else a new one on top), its sound
  on an audio track beside it. The media pool's Video form makes the same
  request, starting on a pool picture or the frame at the playhead. However
  a generated item reaches the pool (the media pool, an approval, or an
  agent job the editor sees on its next pump), `analyze_generated` queues it
  once: a video like footage when the analysis models are ready, a
  voiceover when transcripts are on. Items already in a project when it
  opens are left as they were. Settings → Integrations has a "Video sound"
  switch under Video (`CloudSettings::silent_video`; sound costs more on
  some models): the media pool's form starts its Sound switch from it, and
  with it off the agent's videos are made silent whatever it asks. Each
  model's `cloud::VideoSound` says whether it makes sound at all, always
  (no switch: Pika H3 Flex, Kling 4.0, Kling 3.0 Turbo on Pika), or on a
  switch; only the last are made silent.
* **Voiceovers:** Eleven v4 (`elevenlabs/speech.rs`) takes its delivery from
  the script: audio tags, ellipses and capitals, with no SSML, so
  `cloud::check_script` refuses a `<break>` before any credits are spent and
  offers the script with pauses in its place. Only stability and similarity
  are sent; speed is left to retiming the clip. The media pool's form picks
  the voice (`/v2/voices`, samples cached under `paths::cache()`) and puts
  tags in at the cursor, and the system prompt teaches the agent the same
  rules. A script runs to one request, about 10 minutes; longer ones are
  several clips, one voice each. The form lists the user's own voices
  first, starts a library voice on the stability and similarity it last read
  with (`EditorState::keep_reading` keeps them), and its + button opens the
  Voices dialog over it.
* **Voices** (`ui/voices.rs`, `state/voices.rs`, `elevenlabs/voices.rs`,
  `elevenlabs/design.rs`): one dialog, from the pool's Generate menu, the
  voiceover form and the transcript. *Find* lists the account's voices
  (`/v2/voices`, read once per session into `EditorState::account_voices`)
  or searches the shared Voice Library (`/v1/shared-voices`, by words,
  gender, age, language and use, 30 a page), each with its sample; a shared
  one is added to the account (`/v1/voices/add/{owner}/{voice}`, a voice
  slot), and one with a custom rate shows it. *Describe* designs voices from
  a description (`/v1/text-to-voice/design` on `eleven_ttv_v3`, 20–1,000
  characters, checked before credits are spent), plays each and keeps the
  one picked (`/v1/text-to-voice`). *From audio* clones a voice
  (`/v1/voices/add`, multipart) from files or pool items, sent as their
  sound when they are video, with background-noise removal and a consent
  box it needs. In the transcript, a speaker's words open the dialog as one
  WAV of where they speak (`state::voices::speaker_spans`, padded and
  joined stretches of the source they say most in, up to 3 minutes,
  `koo_analysis::cut_speech`): to clone it, or to find shared voices that
  sound like the first 30 s (`/v1/similar-voices`). Whatever is picked,
  added or made is kept in the library (`library::save_voice`) and the next
  voiceover starts on it. The library's Voices section reads a voiceover on
  a voice, plays its sample, renames it (on the account too, for voices the
  user made), removes it, or deletes it from the account after asking;
  one the account no longer has is dimmed, and a designed one can be
  designed again from its description. A voiceover in the pool can save
  the voice that read it. The agent's `find_voices` lists the user's
  library first, with how each reads, or searches the Voice Library with
  `library` and filters; `add_voice` and `design_voice` (Generate media,
  so approved unless allowed) add or design one and keep it in the library.
  Their approval cards are heard first: `find_voices` remembers where each
  voice listed can be heard and the request carries it, so the card of an
  `add_voice` or a voiceover plays the voice's sample (the library's copy
  when it has one). Approving a design opens the Voices dialog on the
  agent's description, designing; the user hears the takes and keeps one
  (or any other voice), which settles the request, and closing the dialog
  leaves it waiting. Cloning is not a tool: it needs the speaker's consent, so the prompt
  sends the user to the dialog.
* **Finding sounds:** `find_sounds` (`tools/sounds.rs`) searches the user's
  library and the project's audio by words (name, category, tags, what a
  provider was asked) and, where analysis left vectors, by how they sound;
  it says how many had none. `use_library_sound` adds a sound to the pool's
  "Library" bin as the user's edit, like generated audio, and with `at`
  proposes it the same way. `save_sound_to_library` waits for approval.
  The media pool's ♫ menu makes the same from a form (Music runs as long as
  In/Out).
* **Prompt** (`koo-agent/src/prompt.rs`): the system prompt is the tool
  guidance (with or without web), the shared rules (seeing and hearing,
  time formats, references, cutting in pauses, protected ranges), the
  role's part, the FX conventions, the language and the user's standing
  instructions. Each message carries an `<editor_context>` (project
  summary, format, workspace, playhead, selection, in/out, selected words),
  the approved style rules, a recipe line, `<approvals>`, and a
  `<references>` block when the user referenced objects.
* **Chat** (`ui/chat*`): sessions per project in
  `paths::projects()/<project>.chats.json`, each resuming its own backend
  conversation (`agent/sessions.json`). Tool calls stack into one card per
  run. The change-set card sits above the composer, once per set however
  many turns grew it, and stays with its outcome until the next message;
  it has Preview, Review (a checklist with ticks), Reject and Accept. `ask_user` shows up to six choices. Recipes run as
  `/name key=value …` (seven built-ins: rough-cut, dead-time, teaser,
  captions, zooms, music, feedback); cmd/ctrl-K opens the quick command bar.
* **References:** the composer's `@` picker and drops (Alt-drag clips,
  transitions and markers from the timeline; media, effects, audio effects,
  transitions and transcript words; "Add to chat" menu items) insert chips
  that are sent as tokens the agent reads: `@clip <id8>`, `@media <id8>`,
  `@marker <id8 or name>`, `@track <id8 or name>`, `@bin <id8>`,
  `@effect <clip id8>/<effect id8>`, `@transition <clip id8>/in|out`,
  `@fx <builtin key or project id8>`, `@audiofx <kind>`,
  `@range <start>-<end>` (timeline seconds, three decimals) and
  `@time <secs>`. `<id8>` is the id's `Display`. Clicking a chip reveals
  the object in the editor.

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
available to them.

## 9. Persistence

* **Project files** (`koo-store/src/project_file.rs`): `.koo` JSON
  (`format: "koo-project"`, `version: 1`), written atomically; media keeps
  its absolute path and its path relative to the file, tried in that order;
  per-project agent setting overrides travel in the file. New, Open, Save,
  Save As and recent files (cmd/ctrl-N, O, S, shift-S) are in the project
  menu (`koo-editor/src/projects.rs`), with a dot while there are unsaved
  changes and a Save / Don't Save / Cancel question before New, Open, Quit
  or closing the window.
* **Autosave and backups:** unsaved changes autosave to
  `paths::autosave()/<id>.koo` (1 s after the last change, at most 5 s after
  the first) for crash recovery; saved files get a backup in
  `paths::backups()` at most every 5 minutes, 10 kept.
* **Offline media** is flagged with a notice and relinked from a folder
  (`koo-editor/src/relink.rs`): matched by file name, confirmed by
  fingerprint when it can be.
* **Database** (`paths::database()`, SQLite in WAL mode,
  `koo-store/src/schema.rs`): `meta` (agent settings, model picks, analysis
  steps, the provider per capability, recent projects), `style_profiles` (caption styles), `style_rules`,
  `source_facts`, `media_fingerprints` and `recipes`. Nothing per project.
  Imported Markdown notes become proposed style rules, one per bullet or
  paragraph (S5); facts tied to a path are suspended when its media is
  replaced (F2); the seven built-in recipes are re-added on open.
* **Caption styles** (`CaptionStyle`, `koo-core/src/memory.rs`): font,
  weight, spacing, fill, stroke, shadow, background box, position and
  nudge, a spoken-word highlight (color, box, karaoke, word-by-word reveal)
  and intro/outro motion (fade, pop, slide), with twelve ready-made looks;
  sizes are at 1080p, scaled by the frame's short side.
* **Other user files** under `paths::home()`: `projects/` (chats),
  `library/`, `models/`, `cache/`, `agent/` (sessions, models, tool groups,
  tool usage, work dirs), `assets/` (the library's audio and voice samples), `generated/` (music, sound effects and
  voiceovers with their `.json`), `logs/`, `bridge.json`, and `credentials.json`
  (owner-only, only where the system has no credential store).

## 10. Deliver

* **Render graph** (`koo-deliver/src/graph.rs`): one FFmpeg `filter_complex`
  per job, sources opened inside it. Cuts, transforms, keyframes as FFmpeg
  expressions matching the viewer's curves, the grade as a `lut3d`, captions
  and titles burned in with ASS (libass; the viewer letters them with the
  same timing code in `captions/motion.rs`), gain, fades, equal-power
  crossfades, `atempo` retiming, ducking with `sidechaincompress`, audio
  effects, and two-pass `loudnorm`.
* **Composited ranges** (`composed.rs`): ranges that need the compositor
  (effects, generators, transitions, blend modes, crop, flip, stretch/skew/
  anchor, corner pins, masks, retimed media; `compose::needs_compositor`)
  are drawn by `compose::program` from the original media and piped into
  the graph as raw RGBA, which keeps captions and audio. Subject masks in
  the range are found (and tracked) first, `SubjectService::complete`
  taking half the progress; a subject that cannot be found fails the
  render.
* **Queue** (`queue.rs`): jobs run one after another with progress from
  `-progress pipe:1`, can be cancelled and never overwrite a file.
* **Hardware encoding** (`hw.rs`): H.264 and HEVC go to VideoToolbox
  (macOS), NVENC, Quick Sync, VA-API (Linux) or AMF (Windows) when the
  installed FFmpeg lists the encoder (`ffmpeg -encoders`) and a five-frame
  trial encode with the render's own arguments succeeds. The probe runs
  once per session, the trials in parallel (about 0.1 s here), off the UI
  thread. The preset's CRF maps to each encoder's rate control: NVENC
  `-rc vbr -cq`, Quick Sync `-global_quality`, VA-API and AMF constant QP,
  one step below the CRF; VideoToolbox an average bitrate from the CRF,
  size and rate (its constant quality is Apple silicon only). Frames are
  converted to 4:2:0 by swscale as for libx264, so the matrix and colour
  tags match; VA-API then `hwupload`s at the end of the graph. Composited
  ranges pipe RGBA into the same graph, so they encode on hardware too.
  The Deliver workspace's Encoder choice (Auto, Hardware, Software) is
  saved per user (`Store::render_encoder`); Auto falls back to libx264 /
  libx265 when the hardware encode fails, Hardware fails instead. The
  agent's `queue_render` takes an optional `encoder`, else the user's
  choice.
* **Presets:** 1080p60, 1080p30 and 4K30 H.264, 4K30 HEVC (x265 CRF 22),
  ProRes 422 HQ, WAV and Custom, sized by the short side so they keep the sequence's shape
  ("1080p" is 1080 × 1920 for 9:16).
* **Exports:** SRT, ASS, OTIO, FCPXML and PNG frames.

## 11. UI

Pool items and library sounds play on their own (`state/source.rs`): the
item is the one clip of a scratch project, so it goes through the program's
compositor and audio mixer while the timeline and playhead stay put. Sounds
play from the pool and the library; pictures open in the viewer, with a bar
to seek on, until the user goes back to Program or moves the timeline.

Workspaces (`Workspace::ALL`, rendered in `koo-editor/src/app.rs`): Edit
(media pool and effects library on the left, viewer, inspector, timeline),
Captions (transcript and caption style panel), Color (wheels, curves,
scopes), Audio (mixer and meters) and Deliver. The agent chat is docked on
the right in every workspace. Settings (the gear in the top bar, cmd-,, or
the chat's gear for its Agent page) has five pages: Agent, Integrations
(each provider's key and account, and who does each capability on which
model), Analysis (the steps), Models (download, pick and delete) and
Storage (what the user-data folder holds, with Clear for previews,
subject mattes among them, and analysis results). The FX code editor replaces
the inspector while a program is open. With nothing selected the inspector
shows the sequence: format, resolution, and Fit/Fill for all footage (whose
Reframe asks the agent to fill the frame and keep each subject in shot).

The user asked for a clean, minimal UI, and CLAUDE.md says "let the layout
do the explaining":

* No secondary labels, helper text or descriptions under controls. Use icons
  with tooltips, placeholders in inputs, and grouping by spacing.
* Colors come from `cx.theme()` only. Change sets use `theme.green` (added),
  `theme.red` (removed) and `theme.yellow` (modified), drawn as tints and
  outlines, never as banners.
* Dark theme by default, compact density (`text_sm`/`text_xs`, 28 px rows).
* Empty states are a single muted line or an icon, not a paragraph.
* Prefer gpui-kit components (`gpui_kit::component::*`: button, input,
  slider, switch, select, tab, menu, popover, tooltip, scroll) over
  hand-rolled widgets.

### Looking at the app

`scripts/screenshot.sh out.png [wait]` builds the editor and renders it
under Xvfb (with `KOOEDITOR_HOME=/tmp/koo-editor-dev` by default). There is
no input automation in the cloud containers, so seed the states you need to
see:

| Variable | Values | Seeds |
| --- | --- | --- |
| `KOOEDITOR_DEMO` | `1` | A demo project with stand-in engines |
| `KOOEDITOR_DEMO_WORKSPACE` | `Edit`, `Captions`, `Color`, `Audio`, `Deliver` | The workspace to open |
| `KOOEDITOR_DEMO_FX` | `effects`, `generator`, `transition`, `library`, `code` | An effects or library view |
| `KOOEDITOR_DEMO_SELECTION` | `clips`, `transition` | A selection |
| `KOOEDITOR_DEMO_CHAT` | any | Chat references |
| `KOOEDITOR_DEMO_CHAT_PICKER` | `@query` | The composer's `@` list, open |
| `KOOEDITOR_DEMO_VIEW` | `captions` | The transcript's captions view |
| `KOOEDITOR_DEMO_PLAY` | `1` | Playback |
| `KOOEDITOR_DEMO_COMPOSE` | seconds | Every compositor feature on real FFmpeg-made media |
| `KOOEDITOR_DEMO_SETTINGS` | `agent`, `integrations`, `analysis`, `models`, `storage` | Settings, open on that page |
| `KOOEDITOR_DEMO_GENERATE` | `music`, `sfx`, `voiceover`, `voices`, `voices-describe`, `voices-audio` | The media pool's Generate form, or the Voices dialog on that tab |

```sh
KOOEDITOR_DEMO=1 KOOEDITOR_HOME=/tmp/koo-demo cargo run -p koo-editor
```

`scripts/cloud-session-start.sh` prepares cloud sessions: `cargo fetch`,
`glslc` and the Vulkan headers for whisper.cpp, and lavapipe so the window
opens under Xvfb. Chromium for the HTML engine is under `/opt/pw-browsers/`
there.

## 12. PRD → code map

| PRD | Where |
| --- | --- |
| M1 import, bins, search | `ui/media_pool*`, `Command::ImportMedia` |
| M2 inspector, M4 proxies | `koo-media`, `ui/media_pool*` |
| M3 analysis | `koo-analysis`, `koo-models` |
| E1, E2 timeline and tools | `ui/timeline*`, `koo-core/src/edit.rs` |
| E3 transcript editing | `ui/transcript*`, `koo-core/src/query.rs`, `cuts.rs` |
| E4 silences | `AnalysisService::silences`, `ui/timeline/silence.rs` |
| E5 transform, keyframes | `model::Transform`, `mask.rs`, `ui/inspector*`, `ui/viewer/gizmo.rs`, `ui/viewer/masks.rs` |
| Subject masks | `koo-core/src/subject.rs`, `mask.rs`, `compose.rs`, `koo-models/src/segmentation/`, `koo-analysis/src/subjects*`, `ui/viewer/masks.rs`, `ui/inspector/masks.rs`, `tools/masks.rs` |
| E8 timecode readout | `Clip::to_source`, viewer |
| C1–C3, C6 | `koo-core/src/captions.rs`, caption tracks, `CaptionStyle`, `ui/caption_style*`, `koo-deliver` SRT/ASS, `ui/lettering.rs` |
| K1 grade | `koo-core/src/grade.rs`, `scopes.rs`, `ui/inspector/color*`, `ui/scopes.rs` |
| A1–A3 mixer, loudness, ducking | `ui/mixer*`, `ui/inspector/audio.rs`, `audiofx.rs`, `AnalysisService::loudness`, render graph |
| D1 render | `koo-deliver`, `ui/deliver.rs`; hardware encoders `koo-deliver/src/hw.rs` |
| AG1–AG10 | `koo-agent`, `agent_link*`, `ui/chat*`, `ui/settings.rs` |
| Online providers | `koo-cloud`, `koo-core/src/cloud.rs`, `state/cloud.rs`, `ui/editor_settings/integrations.rs`, `ui/media_pool/generate.rs`, `tools/generate.rs` |
| V1–V7 collages | `MediaService::collage`, `koo-agent` view tools |
| S1, S3, S4, F1, F2, F4 | `koo-core::memory`, `koo-store`, cut planners |

## 13. Known gaps

* Pending change sets and undo history are in memory only; they don't
  survive a restart.
* Codex backend written from its docs; not verified end to end the way
  `claude -p` is.
* No GPU video surface: the viewer paints read-back frames as images.
  Windows and macOS builds haven't been run here.
* P1/P2 PRD items not built: nested sequences, a music playlist UI (the
  `SetPlaylist` command and agent tool exist), voice cleanup, voice input.
* Mouse and keyboard interactions (drags, drops, context menus, shortcuts)
  were checked by code review and screenshots of seeded states only.
* Preview vs export: noise reduction previews with an expander (export uses
  `afftdn`); the limiter and time-stretch differ slightly in shape.
* The FX code editor has no syntax highlighting (gpui-kit's `syntect`
  feature is not vendored). A shader that never returns can stall the GPU.
* A program an external agent proposes is usable by its later connections
  only once the user accepts it.
* If the editor crashes, its headless browser can keep running.
* ElevenLabs was checked here against the live API with an invalid key
  only (account, transcription upload, music, sound effects, voiceovers,
  the voice list: each answered "invalid key"); no request has run with a working key, so Scribe's word
  times are unmeasured against the LibriSpeech set above, the music output
  formats each plan allows are untested (the client steps down from 320
  kbps MP3 when one is refused), the music composition plan's instrumental
  chunks are written from the docs, and whether Eleven v4 ever reads an
  audio tag aloud is unheard. The voice calls (shared library search,
  adding, similar voices, design, cloning, renaming, deleting) are written
  from the API reference and have run against no account; the shared
  library's query and reply fields are from memory where its reference
  pages would not load, and whether Eleven v4 reads designed and instantly
  cloned voices as well as the provider's own is unheard.
* CPU fallbacks (no GPU adapter) are slow in debug builds; use release
  builds on machines without a GPU.
* Off macOS, subject tracking runs on the CPU (ONNX Runtime's CPU
  provider, as every model here): about 4 s a frame on 4 cores, so a minute
  of 30 fps footage takes hours. GPU providers (DirectML, CUDA) were not
  tried. A changed prompt tracks the whole clip again rather than from that
  frame.
* The MLX runner has not run on a Mac: its numbers were checked on Linux
  with MLX on the CPU, so its speed on Apple GPUs is unmeasured. MLX loads
  its Metal kernels from `mlx.metallib`, which mlx-sys builds under
  `~/.mlx/lib/` (or `MLX_RS_METAL_PATH`) with Xcode's Metal compiler; a
  packaged app has to ship it next to the binary, where MLX looks first.
  Its weights come from a third party's repository too (pinned).
* A subject mask follows one object; several objects are several masks.
  The matte is the model's 256² logits stretched over the picture: fine
  detail (hair) relies on `refine`, which only eases the edge.
* The SAM 2.1 graphs come from a third party's Hugging Face repository
  (pinned to a commit); a mirror under the project's own account would
  keep them reachable.
* Hardware encoding has run on no real hardware encoder: this container
  has no GPU, so the probe found none (its FFmpeg 6.1 lists NVENC, Quick
  Sync and VA-API; the trials fail as they should). The fallback and the
  NVENC argument path (graph, piped composited frames, audio) were checked
  with a stand-in `KOOEDITOR_FFMPEG` that accepts `h264_nvenc`; VideoToolbox
  and AMF are written from FFmpeg's encoder options, VA-API's `hwupload`
  tail only parsed. The CRF-to-QP/bitrate mapping is not measured against
  libx264 (no VMAF runs), nor the speed-up.
* ONNX Runtime adds about 25 MB to the binary, and the first build
  downloads it (from pyke's CDN). Its build script unpacks the archive very
  slowly when unoptimized; `CARGO_PROFILE_DEV_BUILD_OVERRIDE_OPT_LEVEL=3`
  makes the first debug build take minutes instead of hours.

### Analysis accuracy

* Inside connected speech (no pause between words) more than half of the
  cuts still clip the tail or the onset of a word by more than 20 ms (on
  hand-labelled English; nearly 9 in 10 before the aligner), and may be
  heard; the agent's results say so. The aligner's 80 ms steps are the
  limit: `facebook/wav2vec2-base-960h` (Apache-2.0, English only) left 4
  points more cuts clean. whisper.cpp's DTW timestamps were tried and came
  out 0.2–0.9 s late on real speech, so they are off.
* Only English was measured against human labels. Spanish read speech was
  only compared with another aligner (they agree about as well as on
  English read speech). Japanese and Korean words are aligned as Whisper
  splits them, not as Qwen's own word segmenters would. Dutch and every
  other language outside the 11 keep Whisper's times as measured on the
  audio.
* The aligner needs about 600 MB of memory on top of Whisper (1.2 GB peak
  while it runs on 30 s), a 589 MB download and ~4 s to load; it runs at
  about 6 s per minute of audio on 4 CPU threads.
* Speakers were measured on four AMI test meetings (four people each,
  headset mix; pyannote's word-based reference): 4.1% of the speech of one
  person at a time went to the wrong voice. On two meetings' Whisper words,
  98% of the words said by one person alone got a speaker (98.5% of them
  right), and 59% of those said over someone else (96% right; weighing
  overlaps by the windows labelled 8 points more of them than leaving ties
  unattributed, no less right). Voices are told apart within one item
  only; across items "Speaker 1" is not the same person. Diarization runs
  at about 3% of the audio's length on 4 CPU threads (10 s for 5 minutes);
  the clustering fits on at most 2,000 voiceprints (about an hour of
  conversation) and matches the rest to its voices.
* The sound models were checked on a 64 s collage of real recordings
  (speech, ESC-50 effects, a drum loop), not measured against labels:
  laughter, glass, knocks, a dog, applause, the music and its tempo were
  found, rain under speech was not. Sounds the tagger is unsure of carry
  "?" in the script.
* Silero misses some quiet or short speech. On 5 minutes of an AMI meeting
  (headset mix), its ranges covered 89% of the labelled speech and left
  13 s of it in silences of 0.5 s or more; the energy VAD covered 92% and
  left 5.5 s, but called 33 s outside the labels speech (Silero 5 s). Where
  both found speech, Silero's ranges started 10 ms and ended 30 ms later
  (medians). Whisper may still invent words over sounds Silero takes for
  speech; the script shows them as speech, marked "?" when unsure.
* Transcription was measured on LibriSpeech test-other and dev-clean
  (forced-aligned word times from `gilkeyio/librispeech-alignments`) made
  into items that start talking at once, run on for minutes with short
  pauses, fade out at the end of every phrase, or pass between two to four
  speakers: 5,190 words through the whole pipeline, Whisper small on 4 CPU
  threads. The word edges measured only where the aligner can't, the
  silence before a stretch that fits one window, the beam search and the
  holes heard again took the word error rate from 7.7% to 6.7% and the
  words left out from 66 to 38: 28 to 12 at the end of a phrase, 12 to 7 at
  its start, and both items that lost their first word kept it. Words heard
  on the far side of the pause between two phrases fell from 26 to 8. It
  all takes about 15% more time on the CPU, the beam search most of it.
  Speakers were told apart as before (92% of the words in the
  conversations). Whisper still now and then skips a short phrase and puts
  the next one's words in its place, which leaves no hole. Moving the
  aligner's edges next to a pause took the ends there that came over 30 ms
  early from 40% to 25% and the starts that came over 30 ms late from 40%
  to 23%, with as many within 50 ms. Silences that keep the words clipped
  over 50 ms off 8 of 50 fading last words, 15 before.
* The Beat This! export was verified here from a mirrored copy of the
  `final0` weights; the hosted file must be built from the official
  checkpoint (`export_beat_this.py` downloads it from JKU's server).
