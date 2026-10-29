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
| `koo-media` | ffprobe, fingerprint, proxies, frame decode, collages, waveforms, audio pictures | core, image |
| `koo-analysis` | Background jobs: the analysis pipeline (voice activity, transcript, word times, speakers, sounds, music and beats, sound vectors) over the task interfaces of `koo-models`; shot changes, silences, loudness, the energy VAD | core, models |
| `koo-agent` | Backend adapters, MCP server + bridge, tool catalog, prompt/context builder | core |
| `koo-deliver` | Render graph → FFmpeg, render queue, SRT/ASS, OTIO, FCPXML, frame export | core |
| `koo-gpu` | `FxService` on wgpu: GLSL effects, transitions, generators, layer compositing | core, wgpu |
| `koo-html` | `HtmlService`: HTML generators in headless Chromium with a virtual clock, GSAP preloaded | core, tungstenite, GSAP (vendored) |
| `koo-models` | Everything about the models: a folder per task (its interface, the models that can do it) and per model (its files pinned to a commit, its config, its runner); whether the files are on disk, and the downloads the user starts | core, ureq, whisper-rs, ort |
| `koo-editor` | GPUI app: layout, `EditorState`, panels, wiring of all of the above | all |

Crates other than `koo-editor` depend only on `koo-core`, never on each other,
except `koo-analysis`, which runs its models through `koo-models`.

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
| Whisper | `whisper-rs` (whisper.cpp), ggml model in `paths::models()`; on the GPU (Metal on macOS, Vulkan on Windows and Linux), else the CPU | Local, word timestamps, any language; no per-use cost |
| Word times | Forced alignment of Whisper's words with Qwen3-ForcedAligner-0.6B (Apache-2.0; our ONNX export, 4-bit weights, 589 MB in `paths::models()`) on ONNX Runtime (`ort`, prebuilt and linked statically at build time) | Whisper's own word times drift by up to a second in conversation; the aligner puts 94 % of word starts within 100 ms on hand-labelled English (Whisper: 48 %). One permissively licensed model for 11 languages |
| Hearing | The model never hears audio: analysis turns it into the sound script (words, `[sounds]`, `♪` music with tempo and changes, silences), beats to cut on, vectors to search by description, and spectrogram pictures | Neither backend takes audio input, and models place sounds in time far worse than in text; the transcript and collages already work this way |
| Sounds | EfficientAT mn10_as (MIT; 527 AudioSet classes) on 2 s windows every second, each followed by 4 s of silence, grouped into about 180 names for the script | Small (20 MB) and fast on the CPU; trained on 10 s clips, it answers nonsense for 2 s alone, and 6 s gets nine in ten of its 10 s answers at half the cost |
| Beats | Beat This! `final0` (MIT) on each stretch of music, 30 s chunks, its own minimal peak picking | State of the art for beats and downbeats without a DBN; runs only where the tagger heard music |
| Sound search | LAION's larger_clap_general (Apache-2.0): a vector per 4 s window every 2 s, int8, in `<fp>.vectors`; the text tower encodes a description at query time | Finds sounds outside the tagger's vocabulary ("sizzling bacon") and sounds like an example; 2 s steps scored by the lower of their two windows place a sound to about 2 s |
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
| C1–C3, C6 | `koo-core/src/captions.rs`, caption tracks, `CaptionStyle` (`koo-core/src/memory.rs`), `ui/caption_style.rs`, `koo-deliver` SRT/ASS; the viewer letters captions like the ASS burn-in (`ui/lettering.rs`, `koo-deliver/src/captions/motion.rs`) |
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
* **Analysis jobs:** analysis starts only when the user asks, from an
  item's "⋯" menu in the media pool (or its right-click menu): "Analyze",
  or "Analyze again" once it has results, which runs every step anew on
  the models picked now. It is disabled while any task's picked model is
  missing, or while the item is queued or analyzing; images are not
  analyzed. Items are analyzed one at a time, in the order asked, on a
  `JobQueue` (`koo-core/src/jobs.rs`): at most `limit` jobs at once on
  threads of its own, work under a key already queued or running is that
  job, and a job that panics fails without stopping the queue. Other
  heavy work can have a queue of its own. A result is written to
  `cache/analysis/<fp>.json` only when every step succeeded; otherwise it
  is kept for the session and the job fails, naming the steps.
  `ANALYSIS_VERSION` (1) marks the results: those of another version, and
  the sound vectors beside them, are not read, so the item is not
  analyzed until the user analyzes it again.
* **Analysis:** VAD, silences, shot changes, loudness, speakers (below);
  Whisper transcription behind the default `whisper` feature, in small
  (264 MB, the default) or large (large-v3-turbo, 874 MB), both with
  8-bit weights, picked in Settings → Models. whisper.cpp is built for
  the GPU (Metal on macOS, Vulkan on Windows and Linux, with flash
  attention), so building needs Xcode or the Vulkan SDK (`glslc` and the
  Vulkan headers on Linux). On a machine without a GPU it runs on the
  CPU. On Windows the editor loads `vulkan-1.dll` only when Whisper first
  runs (`/DELAYLOAD`), and where the DLL is missing
  `koo-editor/src/vulkan_fallback.rs` makes Vulkan fail to start, so
  Whisper runs on the CPU there too. The picked size is in
  `paths::models()` once the user downloads it (Models, below), or set
  `KOOEDITOR_WHISPER_MODEL` to a path or a ggml file name there, which
  stands in for whichever size is picked. Speech, and so silences, comes
  from Silero VAD v6.2 (`koo-models/src/voice_activity/silero/`;
  `ggml-silero-v6.2.0.bin` from ggml-org/whisper-vad at a pinned commit,
  or `KOOEDITOR_VAD_MODEL`), run by whisper.cpp: speech from 0.5 until
  under 0.35, pauses under 250 ms kept, 60 ms before each range. Music is
  not speech to it. The energy VAD (`vad.rs`), which took a music bed for
  speech, stands in without the `whisper` feature, or for the session
  when the model can't be had. Silero
  hears the audio in one go and in 4 s blocks, each after the second
  before it, and a window is speech when either finds it: music holds it
  down for seconds after, and a block that starts during a sentence
  doubts the rest of it. Whisper hears only speech
  (`koo-models/src/transcription/whisper/`): given music, a long
  silence, or a 30 s window that starts with either, it made words up,
  often repeating what was said before, and its times came loose. It hears
  the stretches Silero finds as faster-whisper's VAD filter does (pauses
  under 2 s kept, 400 ms either side), each on its own, without the text
  of the window before, in the language of the longest; its notes such as
  "(upbeat music)" are dropped. Word times come from a forced aligner
  (`koo-models/src/word_timing/qwen3_aligner/`, behind the default `aligner` feature):
  Qwen3-ForcedAligner-0.6B hears up to 30 s of audio with Whisper's words
  and answers with each word's start and end, in 80 ms steps; starts then
  move 20 ms earlier, never into the word before. It knows English,
  Spanish, French, German, Italian, Portuguese, Russian, Chinese,
  Cantonese, Japanese and Korean. Its tokenizer (`koo-models/src/bpe.rs`) comes from
  Qwen's repository and the model from ours (below), or set
  `KOOEDITOR_ALIGNER_MODEL` to a path. Other languages, and words
  with neither letters nor digits, keep word edges measured on the audio
  levels (`koo-analysis/src/align.rs`). When the model can't be loaded,
  the result keeps those edges for the session. Running that measurement
  after the aligner made cuts worse, so it is only the fallback. The
  aligner's times never pass the audio it was given, so words stay in
  order.
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
  Qwen's own pipeline exactly before quantization and to within 0.5
  points of clean cuts after; dynamic int8 quantization breaks it. The
  repository carries the Apache-2.0 licence text and states that it is a
  modified (exported, quantized) Qwen3-ForcedAligner-0.6B.
* **Speakers** (`koo-models/src/speakers/pyannote/`, behind the default
  `diarizer` feature) follow pyannote's speaker-diarization-3.1 on ONNX
  Runtime. pyannote segmentation-3.0 (MIT) hears 10 s windows 2.5 s
  apart and says, every 17 ms, which of up to three voices of the window
  speak. WeSpeaker ResNet34-LM (CC-BY-4.0, trained on VoxCeleb) turns
  each window's voice, where it speaks alone, into a 256-number
  voiceprint from Kaldi filterbank features (`fbank.rs`, equal to
  `kaldi-native-fbank` to 1e-5). The voiceprints are clustered with
  centroid linkage at pyannote's threshold (0.7046), clusters of fewer
  than 12 join the nearest larger one, and the windows are averaged back
  into who speaks in each frame. A word goes to the voice that speaks
  through most of it, overlaps weighed by how many windows heard each;
  below 0.6 of the speaking it keeps no speaker. Speakers are numbered
  in order of appearance, and attributed after the aligner. Both models
  come from their own Hugging Face repositories
  (`onnx-community/pyannote-segmentation-3.0`, 6 MB, and
  `Wespeaker/wespeaker-voxceleb-resnet34-LM`, 26 MB; its `config.rs` pins
  the commits). When they can't be loaded, every word is "Speaker 1"
  for the session.
  ONNX Runtime allows one environment per process, so the aligner, the
  diarizer and the sound models share it (`koo-models/src/onnx.rs`).
* **Cuts in speech** (`koo-core/src/cuts.rs`) go in the pause between two
  words, clear of both, on a frame boundary when the pause holds one, with
  a 10 ms audio fade at the join. The transcript panel's word delete and
  the agent's `cut_words` use it; `blade` and `ripple_delete` move a cut
  that falls on a word into the nearest pause and say so.
* **Sounds, music and beats** (behind the default `sounds` feature): after
  the transcript, the audio is decoded once more at 32 kHz and streamed
  through the sound tagger (`sound_tagging/efficientat/` in `koo-models`)
  and CLAP's audio tower (`sound_search/clap/`) together; then the beat
  tracker (`beats/beat_this/`) hears each
  stretch of music the tagger found, with 1 s either side. Runs of tagged
  windows become `Analysis::sounds` and `music`: a run starts
  up to 2 s before its sound, so it is shrunk by that much, and a sound
  under a second long moves onto the loudest stretch of the 10 ms levels
  around it. Silence alone scores 0.25 as music, so music needs 0.5 to
  start; bells, claps and other musical sounds inside music are dropped.
  Music carries tags (genre, mood, vocals, lead instruments), beats and
  downbeats; its tempo is the mean beat interval near the median. The
  vectors go to `cache/analysis/<fp>.vectors`. A model that cannot be
  had leaves the sounds out, and what the others found is kept for the
  session. On 4 CPU threads the pass takes about 2 minutes per hour of audio
  for the tagger and 6 for CLAP, plus the beat tracker on music.
  `koo-core/src/heard.rs` maps sounds, music, beats, level changes and
  silences onto the timeline, and finds beats, downbeats and onsets (a
  6 dB rise over the 50 ms before) near a cut.
* **Sound model files:** our exports, built by `scripts/models/` and
  hosted (with those scripts and the licence texts) at
  [Chris-Kode/efficientat-mn10-as-onnx](https://huggingface.co/Chris-Kode/efficientat-mn10-as-onnx),
  [Chris-Kode/beat-this-final0-onnx](https://huggingface.co/Chris-Kode/beat-this-final0-onnx)
  and [Chris-Kode/larger-clap-general-onnx](https://huggingface.co/Chris-Kode/larger-clap-general-onnx);
  each model's `config.rs` pins their URLs to a commit. JKU's server was
  unreachable, so the Beat This! export was made from the
  `safe-models/beat-this-final0` mirror, whose weights have not been
  compared with JKU's. Each graph takes 32 kHz mono samples and holds
  its model's own front end, so analysis decodes once for all three:
  `efficientat-mn10-as.onnx` (20 MB; windows × samples in, 527
  probabilities out; matches EfficientAT's pipeline to 2e-6),
  `beat-this-final0.onnx` (85 MB; up to 30 s in, beat and downbeat logits
  per 20 ms frame out; a windowed-sinc resampler to 22.05 kHz stands in
  for soxr), `larger-clap-general-audio.onnx` (78 MB; 10 s windows in,
  unit vectors out; resampled to 48 kHz inside; dynamic int8, cosine 0.997
  to the float model) and `larger-clap-general-text.onnx` (141 MB; RoBERTa
  token ids in; 8-bit block weights and an int8 embedding table, cosine
  0.998). The text tower's tokenizer comes from LAION's repository at a
  pinned commit (`bpe.rs` encodes it). `KOOEDITOR_TAGGER_MODEL`,
  `KOOEDITOR_BEATS_MODEL`, `KOOEDITOR_CLAP_AUDIO_MODEL` and
  `KOOEDITOR_CLAP_TEXT_MODEL` point at local copies instead.
* **Models** (`koo-models`): one folder per task (voice activity,
  transcription, word timing, speakers, sounds and music, beats, sound
  search). Its `mod.rs` holds the task's interface, the trait analysis
  calls (`VoiceActivity`, `Transcriber`, `Aligner`, `Diarizer`, `Tagger`,
  `BeatTracker`, `SoundSearch`); `MODELS`, the models that can do it, the
  default first; and `runner`, which builds the runner of one of them. In
  it, one folder per model: `mod.rs` is the model as the user sees it,
  `config.rs` its files (pinned to a commit, with their sizes) and the
  settings it runs with, `runner.rs` the code that loads and runs it
  (behind its feature) and finds its own files. What models share sits
  at the crate root: the catalog types, the audio rates, the downloads,
  ONNX Runtime and the BPE tokenizer. Analysis only orchestrates: it asks
  each task's runner for the selected model (`Task::model`: the one the
  user picked, else the default) and never names a model; a new pick of
  transcription model applies from the next transcript, and results made
  already stay. A new model is a folder under its task, its `MODEL` in
  `MODELS` and an arm in `runner`; a model in several sizes (Whisper) has
  a `MODEL` for each in one folder. Nothing downloads on its own.
  Settings → Models (the gear in the top bar, or cmd-,) lists each task
  with its model, picked there when the task has more than one (saved per
  user in the store), and downloads one or all of them, with progress
  and cancel, through `<file>.part`; a failed download stays there with
  Retry. `LocalAnalysis::model_tasks`
  says which tasks a build runs; "Analyze" stays disabled until each has
  its picked model downloaded. A runner whose files are missing fails
  ("… is not downloaded") and its step is left out. While a model is
  missing, the top bar's warning button
  lists them; its notices (warnings and errors, such as a failed
  download) can be dismissed for the session.
* **Hearing tools:** `view_range` and `view_detail` open with the sound
  script instead of the bare transcript; `get_audio` returns it for any
  range, `get_music` lists tempo, beats, downbeats and level changes,
  `search_sounds` ranks 2 s steps by a description or by an example
  range, and `view_audio` draws up to 30 s as a spectrogram lane per track
  (ffmpeg's `showspectrumpic`) with words, sounds and beats marked
  (`MediaService::audio_picture`). `blade` and `ripple_delete` take
  `align: beat | downbeat | onset`; a target clear of words wins over a
  nearer one on a word, and the cut still moves off words after.
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
* **Agent:** a tool for every command (76), `create_fx`/`update_fx` with
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
* Inside connected speech (no pause between words) more than half of
  the cuts still clip the tail or the onset of a word by more than 20 ms
  (on hand-labelled English; nearly 9 in 10 before the aligner), and may
  be heard; the agent's results say so. The aligner's 80 ms steps are the
  limit: `facebook/wav2vec2-base-960h` (Apache-2.0, English only) left
  4 points more cuts clean. whisper.cpp's DTW timestamps were tried and
  came out 0.2–0.9 s late on real speech, so they are off.
* Only English was measured against human labels. Spanish read speech
  was only compared with another aligner (they agree about as well as on
  English read speech). Japanese and Korean words are aligned as Whisper
  splits them, not as Qwen's own word segmenters would.
* Dutch and every other language outside the 11 keep Whisper's times as
  measured on the audio.
* The aligner needs about 600 MB of memory on top of Whisper (1.2 GB
  peak while it runs on 30 s), a 589 MB download and ~4 s to load; it runs
  at about 6 s per minute of audio on 4 CPU threads.
* Speakers were measured on four AMI test meetings (four people each,
  headset mix; pyannote's word-based reference): 4.1% of the speech of
  one person at a time went to the wrong voice. On two meetings'
  Whisper words, 98% of the words said by one person alone got a speaker
  (98.5% of them right), and 59% of those said over someone else (96%
  right; weighing overlaps by the windows labelled 8 points more of
  them than leaving ties unattributed, no less right). Voices are
  told apart within one item only; across items "Speaker 1" is not the
  same person. Diarization runs at about 3% of the audio's length on 4
  CPU threads (10 s for 5 minutes); the clustering fits on at most 2,000
  voiceprints (about an hour of conversation) and matches the rest to
  its voices.
* ONNX Runtime adds about 25 MB to the binary, and the first build
  downloads it (from pyke's CDN). Its build script unpacks the archive
  very slowly when unoptimized; `CARGO_PROFILE_DEV_BUILD_OVERRIDE_OPT_LEVEL=3`
  makes the first debug build take minutes instead of hours.
* The sound models were checked on a 64 s collage of real recordings
  (speech, ESC-50 effects, a drum loop), not measured against labels:
  laughter, glass, knocks, a dog, applause, the music and its tempo were
  found, rain under speech was not. Sounds the tagger is unsure of carry
  "?" in the script.
* Silero misses some quiet or short speech. On 5 minutes of an AMI
  meeting (headset mix), its ranges covered 89% of the labelled speech
  and left 13 s of it in silences of 0.5 s or more; the energy VAD
  covered 92% and left 5.5 s, but called 33 s outside the labels speech
  (Silero 5 s). Where both found speech, Silero's ranges started 10 ms
  and ended 30 ms later (medians). Whisper may still invent words over
  sounds Silero takes for speech; the script shows them as speech, marked
  "?" when unsure.
* The Beat This! export was verified here from a mirrored copy of the
  `final0` weights; the hosted file must be built from the official
  checkpoint (`export_beat_this.py` downloads it from JKU's server).
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
* **HTML** renders in headless Chromium with an engine-owned clock (GSAP,
  CSS animations, Web Animations, rAF and timers all follow clip time), so
  frames are deterministic and exportable, with alpha. GSAP 3.15 and its
  motion-graphics plugins are vendored in `koo-html/src/gsap` and preloaded;
  templates build their animation in `kooTimeline`, which the engine reruns
  when parameters or the clip length change.
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
