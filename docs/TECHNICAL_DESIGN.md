# Flikk — technical design

Engineering companion to the PRD ("AI-native video editor with agent chat",
Oct 26 2026). The PRD says *what*; this file says *how* the code does it
today. When the two disagree, the PRD wins on behaviour and this file wins on
structure. Keep it in step with the code: when a change alters something
described here, update the section in the same commit.

## 1. Shape of the system

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
  `Author::External` commands go into a change set for review (§4).
* **Engines behind traits.** The UI, the compositor and the agent host call
  the traits in `flikk_core::services`: `MediaService` (`flikk-media`),
  `AnalysisService` (`flikk-analysis`), `FxService` (`flikk-gpu`), `HtmlService`
  (`flikk-html`), `LetteringService` (`flikk-deliver`) and `GenerationService`
  (`flikk-cloud`). `EditorState.services` holds each as an
  `Option<Arc<dyn …>>`; an engine that fails to start (no ffmpeg, no GPU
  adapter, no browser) is `None` and callers degrade quietly.

### Crates

| Crate | Role | Depends on |
| --- | --- | --- |
| `flikk-core` | Model, time, ids, commands, ops, bus, change sets, history, queries, captions, cuts, grade, scopes, masks and subject mattes, speed ramps, depth maps, audio FX DSP, FX format and built-ins, compositor, blockouts (rough 3D scenes built from plain solids, their models and cycles, renderer, review and clip), job queue, service traits, online capabilities, user library, paths | os, serde, serde_json, thiserror, uuid |
| `flikk-os` | What every crate needs from the system, std only: files written whole (temp file and rename, durable or private) and outputs that never replace a file, the sweep of crash-left temp files, child processes killed on drop and the ffmpeg/ffprobe paths, poison-tolerant locks and panic messages, the 30 s retry back-off | — |
| `flikk-store` | `.flikk` project files, autosave, backups, recent files; SQLite for settings, style profiles and rules, source facts, fingerprints | core, rusqlite, chrono |
| `flikk-media` | ffprobe, fingerprint, proxies, frame and PCM decode, playback streams, collages, waveforms, spectrogram pictures | core, blake3, png |
| `flikk-analysis` | `AnalysisService`: the analysis pipeline over `flikk-models`' task interfaces, plus shot changes, silences, loudness, the energy VAD and the analysis cache; `SubjectService` (`LocalSubjects`): subject masks found and tracked, person masks matted, and their matte caches; `InterpolationService` (`LocalInterpolation`): optical flow frames for slow motion, and their cache; `DepthService` (`LocalDepth`): depth maps per frame, smoothed, and their cache | core, models |
| `flikk-models` | A folder per task (its interface and the models that can do it) and per model (files pinned to a commit, settings, runner); downloads; shared ONNX Runtime and BPE tokenizer | core, ureq, whisper-rs, ort, mlx-rs (macOS) |
| `flikk-cloud` | Online providers (ElevenLabs, fal, Pika): the provider catalog, `SpeechService` and `GenerationService`, the shared HTTP client, API keys in the system's credential store | core, ureq, keyring |
| `flikk-agent` | Backend adapters (Claude, Codex), MCP server (`flikk-mcp` bin) and bridge, tool catalog and groups, prompt builder, skill and style tools, backend sessions, blockout changes by asking (`reblock`) | core, base64, png |
| `flikk-deliver` | Render graph → FFmpeg, render queue, presets, SRT/ASS, OTIO, FCPXML, frame export, cosmic-text `Letterer` | core, cosmic-text |
| `flikk-gpu` | `FxService` on wgpu: GLSL effects, transitions, generators, masks and layer compositing | core, wgpu, naga |
| `flikk-html` | `HtmlService`: HTML generators in headless Chromium over CDP with a virtual clock, GSAP preloaded | core, tungstenite, GSAP (vendored) |
| `flikk` | GPUI app (gpui-kit): `EditorState`, workspaces and panels, agent link, persistence, playback, wiring of all of the above | all, cpal, image |

Crates other than `flikk` depend only on `flikk-core` and `flikk-os`, never
on each other, except `flikk-analysis`, which runs its models through
`flikk-models`. Analysis
reaches an online transcriber only through `flikk_core::cloud::SpeechService`,
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
| FFmpeg | Spawn `ffmpeg`/`ffprobe` binaries (`FLIKK_FFMPEG` / `FLIKK_FFPROBE`, else the copy the release packages ship beside the editor, else `PATH`) | No libav linking on Windows/macOS |
| Playback | A 720p proxy (`cache/proxies/<fp>.mp4`) is made on import and used once it exists; streams pipe RGBA from a long-lived ffmpeg (`-hwaccel auto`, software fallback); frames are composed on the GPU and painted as images | Smooth playback on a laptop without a GPU video surface |
| Fingerprint | BLAKE3 over file size + head, tail and 8 evenly spaced 1 MiB chunks (whole file under 10 MiB) | Fast on multi-GB files; good enough to detect replacement (F2) |
| Compositor | One `flikk_core::compose::program` draws the frame for the viewer, the agent's views and composited render ranges | The preview matches the export |
| Whisper | `whisper-rs` (whisper.cpp), ggml model in `paths::models()`; on the GPU (Metal on macOS, Vulkan on Windows and Linux), else the CPU | Local, word timestamps, any language; no per-use cost |
| Word times | Forced alignment of Whisper's words with Qwen3-ForcedAligner-0.6B (Apache-2.0; our ONNX export, 4-bit weights, 589 MB) on ONNX Runtime (`ort`, linked statically at build time) | Whisper's own word times drift by up to a second in conversation; the aligner puts 94 % of word starts within 100 ms on hand-labelled English (Whisper: 48 %). One permissively licensed model for 11 languages |
| Hearing | The model never hears audio: analysis turns it into the sound script (words, `[sounds]`, `♪` music with tempo and changes, silences), beats to cut on, vectors to search by description, and spectrogram pictures | Neither backend takes audio input, and models place sounds in time far worse than in text; the transcript and collages already work this way |
| Sounds | EfficientAT mn10_as (MIT; 527 AudioSet classes) on 2 s windows every second, each followed by 4 s of silence, grouped into about 180 names for the script | Small (20 MB) and fast on the CPU; trained on 10 s clips, it answers nonsense for 2 s alone, and 6 s gets nine in ten of its 10 s answers at half the cost |
| Beats | Beat This! `final0` (MIT) on each stretch of music, 30 s chunks, its own minimal peak picking | State of the art for beats and downbeats without a DBN; runs only where the tagger heard music |
| Sound search | LAION's larger_clap_general (Apache-2.0): a vector per 4 s window every 2 s, int8, in `<fp>.vectors`; the text tower encodes a description at query time | Finds sounds outside the tagger's vocabulary ("sizzling bacon") and sounds like an example; 2 s steps scored by the lower of their two windows place a sound to about 2 s |
| Caches | Derived media data is keyed by fingerprint under `paths::cache()` | Re-opening a project never recomputes (principle 4, V6) |
| Agent ↔ editor | The backend CLI launches an MCP server over stdio; it connects to the editor over `127.0.0.1:<port>` with a token, and the editor serves calls in `AgentLink::pump` on the UI thread | One code path for Windows and macOS; the bus stays single-threaded |
| Tool schema | Times in tool arguments are seconds, `"M:SS.mmm"`, `"H:MM:SS.s"`, timecode `"HH:MM:SS:FF"`, frames `"f:1234"` or `@time`/`@range`; results are seconds to the millisecond | What models read and write reliably |
| Persistence | Projects are `.flikk` files (versioned JSON, atomic writes, media paths absolute and relative to the file) saved where the user picks; unsaved changes autosave to `paths::autosave()`, saved files get timed copies in `paths::backups()`. Undo history and pending change sets last the session. User data is SQLite (WAL) at `paths::database()` | Projects move between folders and machines like any pro editor's; one database per user |
| Library | FX programs (`.flikkfx`), presets (`.flikkpreset`: effects, audio effects or a grade with their values) sounds (`.flikksound`, their audio in `paths::assets()`, `library/assets/`, a folder per content) and voices (`.flikkvoice`: the provider's voice id, how it was made, the stability and similarity it reads best at, a sample under `paths::assets()/voices/`) under `paths::library()`, shared by every project (`flikk-core::library`). A voice lives on the provider's account; its file lists it, plays it and makes it again without a request. Using a program copies it into the project with its `origin`; presets carry the project programs they use; using a sound adds a media item on its file to the pool's Library bin. Removing a sound leaves the audio. Sounds without results are analyzed when they are added and on launch, for sounds, music, beats and vectors only (`AnalysisService::analyze_sound`), so nothing reaches a provider | A `.flikk` file stays complete anywhere; library edits reach a project only when its copy is updated; audio is too heavy to copy per project, and a project using it must not go offline |
| User data | Only under `paths::home()` (`$FLIKK_HOME` or `~/.flikk`); project files and exports go where the user saves them | AGENTS.md hard rule 2 |
| Online providers | `flikk-cloud`: a catalog of providers, each a folder with its client behind `Api` and the capabilities it offers (`Capability`: transcription, music, sound effects, voiceover, video, image, upscale) on which models; the user picks one per capability in Settings → Integrations (`CloudSettings`, per user in the store). Nothing runs online unless picked, and transcription stays local by default | A new provider or capability is a catalog entry, and settings, analysis and the agent find it there |
| API keys | In the system's credential store (Keychain, Credential Manager, Secret Service; `keyring`) under the service "flikk"; where there is none, an owner-only `credentials.json` in `paths::home()`. A provider's environment variable (`ELEVENLABS_API_KEY`, `FAL_KEY`, `PIKA_API_KEY`) wins | Never in the database, `.flikk` files or logs |

| Speed ramps | Speed keys on clip time, straight between them; source time is their integral, solved in closed form both ways (`flikk-core/src/speed.rs`). A ramped clip's `speed` is its mean | Exact under trims and splits (a key is added at the cut, the shape is unchanged); every timeline → source mapping goes through `Clip::to_source` / `source_offset` |
| Slow motion | Per clip: frame repeat, blend (two neighbouring source frames mixed in the batch) or optical flow with RIFE v4 (MIT; FuryTMP's ONNX export with the time step made an input, pinned, 22 MB) on ONNX Runtime, on the GPU (`onnx::GpuSessions`). Flow frames are made off the UI thread and blended until they land; renders wait for each | One run makes the frame at any moment between two; blending keeps playback smooth while the CPU catches up, and the render matches the viewer once frames are made |
| Person masks | MODNet (Apache-2.0; Xenova's ONNX export, pinned) per frame at a 512 short side, the edge eased onto the full-size frame by a guided filter, eased toward the frame before where it barely changed; mattes kept by fingerprint and model, made ahead of the playhead | One click, no prompt and no green screen; a matting model gives soft hair where a segmenter gives a hard edge, and is fast enough to run ahead of playback. SAM 2 stays for objects and one person among several. Robust Video Matting (GPL-3.0) was ruled out by its license |
| Depth maps | Depth Anything V2 Small (Apache-2.0; onnx-community's ONNX export, pinned) on ONNX Runtime, per source frame at 518 on the longer side, spread over each frame's 2nd–98th percentile, smoothed over ±2 frames, kept by fingerprint and model; effects read it as `getDepth(uv)` | One permissively licensed model gives relative depth for any picture; what effects need is what is in front of what, not metres. The larger sizes are CC-BY-NC |
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
`flikk_core::jobs::JobQueue`: FIFO, at most `limit` jobs at once on threads of
its own, a key already queued or running returns that job, and a job that
panics fails without stopping the queue.

## 4. Editing model

* **Project** (`flikk-core/src/model.rs`): media, bins, one `Timeline` (tracks
  of kind Video, Audio, Title, Caption, Overlay; markers; ducking; a timeline
  grade; `SequenceSettings`) and the project's FX programs.
* **Clips** carry speed, enabled, name, color, a transform, a grade, an
  effect stack, audio effects, volume automation, `transition_in` /
  `transition_out` (resolved by `fx::transitions`), masks and a `retime`.
* **Speed ramps** (`flikk-core/src/speed.rs`, `Clip::retime`): speed keys
  on clip time; between them the speed changes straight, before the first
  and after the last it holds. Source time is the integral
  (`Clip::source_offset`, inverse `Clip::timeline_offset`), behind
  `Clip::to_source` / `to_timeline` and so behind the viewer, compositor,
  render, transcript and caption mapping, handles, waveforms and silence
  bands. `SetSpeedRamp` takes points over the clip (0..1, speed) and keeps
  the source it plays: the clip's length follows the ramp, or with
  `keep_length` the speeds are scaled to fit; presets (`SpeedPreset`: Ramp
  in, Ramp out, Hero, Bullet, Montage) are shapes in multiples of the
  clip's speed. Trims and splits slice the keys (one added at the cut), so
  each frame keeps its speed; `SetSpeed` on a ramped clip scales the ramp.
  `SetRetime` picks how slowed frames are drawn (`Interpolation`: repeat,
  blend, optical flow) and whether ramped audio is stretched (pitch kept)
  or muted (`RampAudio`). Without keys a clip plays at `speed` as before,
  and old projects load unchanged.
* **Transform:** position, scale, rotation, opacity, per-axis stretch, skew,
  anchor, crop, flip, blend mode, `fit` (`Contain` with bars, or `Cover`,
  filling the frame and panned by position) and a corner pin; all keyframable.
  A pinned picture is placed through a homography (`Placement::homography`),
  others through one affine matrix shared by the GPU and CPU compositors.
  `compose::GroupMove` moves, scales and turns several clips about a common
  pivot and turns that back into each clip's transform.
* **Masks** (`flikk-core/src/mask.rs`): rectangles, ellipses and closed paths
  (straight or smooth) in the picture's own uv, with feather, expansion,
  opacity and invert, combined in order by add, subtract or intersect, each
  keyframable as a whole. Captions and titles take no masks or corner pins:
  renders burn them in with ASS, which cannot draw them.
* **Stabilization** (`Clip::stabilization`, `flikk-core/src/stabilize.rs`):
  on, a smoothness (0..1), Smooth or Locked, and a crop (Auto zoom or
  None); off by default and left out of `.flikk` files until set, so older
  files open unchanged. `Command::SetStabilization` sets it on video
  clips (see §7, "Stabilization").
* **Subject masks** (`MaskShape::Subject`, `flikk-core/src/subject.rs`): the
  mask keeps the object its `Subject`'s prompts pick: on some source
  frames, clicks on the object (or off it) and a box, in picture uv, so
  trims, splits and speed changes keep them on their frames. With `track`
  the object is followed over every frame the clip shows; otherwise each
  frame shows the nearest prompted frame's matte. `refine` eases the edge
  onto the picture's (hair). Feather, expansion, opacity, invert and mode
  work as for shapes; the prompts are not keyframed.
* **Person masks** (`MaskShape::Person { edge }`, `flikk-core/src/person.rs`):
  the mask keeps every person a matting model finds in each frame, with no
  prompt; `edge` (0..1, keyframable with the rest of the state) eases the
  edge onto the picture's. Invert keeps the background instead. Feather,
  expansion, opacity and mode work as for shapes, so it combines with
  shape and subject masks. Old projects open unchanged (a new shape kind).
* **Cuts in speech** (`flikk-core/src/cuts.rs`) go in the pause between two
  words, clear of both (40 ms after the earlier, 30 ms before the later), on
  a frame boundary when the pause holds one, with a 10 ms audio fade at the
  join. The transcript panel's word delete and the agent's `cut_words` use
  it; the agent's `blade` and `ripple_delete` move a cut that falls on a word
  into the nearest pause and say so (`snap: false` turns it off).
* **Depth masks** (`MaskShape::Depth`): keep what lies between `near` and
  `far` from the camera (relative distance, 0 nearest to 1 farthest; see
  "Depth maps"), fading over `softness` past each, from the picture's
  depth map at the source time; the background, the foreground or a slice
  between. They combine, invert, fade and keyframe (`near`, `far`,
  `softness` move) like shapes; feather and expansion do not apply. To
  grade or add an effect to one depth only, a copy of the clip on the track
  above takes the mask and the change. Without its map yet the mask is
  left out.
* **Fixing words** (`flikk-core/src/respeak*`): words typed over others in
  the transcript are said in the speaker's voice and cut in where the old
  ones were. `respeak::fix` takes the cuts either side of them from
  `Speech::around` (as a word delete places them), the rest of their
  sentence (back to the end of the one before, on to its own end, in that
  voice or in words fixed before) and up to 50 words either side as
  context, and the quietest pauses between words within 20 s for room
  tone. The provider reads the whole sentence with the new words in it,
  the context as `previous_text`/`next_text`, so the delivery fits;
  `respeak::make` times the take's words (`AnalysisService::time_script`:
  as a voiceover's, the aligner or the edges measured on the levels), cuts
  the new words out with up to 80 ms before and 150 ms after (half the
  pause to the next word at most), brings them to the speaker's speech
  level in the original sentence (20 ms frames within 25 dB of the
  loudest) and its tilt (a one-pole split at 1.5 kHz, the high band moved
  by the ratio of high to low energy, at most ×4), lays the longest pause
  found under them looped with 50 ms equal-power crossfades, and pads them
  to the pauses the old words had before and after. The result is a WAV in
  `paths::generated()` (with its `.json`), a voiceover item whose script is
  the new words, so analysis times them and the transcript shows them, in
  italics as every voiceover's words. The take goes to `cache/respeak/`
  and is deleted once used. `Command::ReplaceSpeech` cuts it in: on the
  speech's track the range goes and the new file plays there with a 10 ms
  fade at both joins (in the pauses), keeping the replaced clip's gain and
  audio effects; shorter, the rest ripples as a delete of the range's end
  would; longer, everything after moves on every unlocked track, the
  picture over the range slows to fill it, other audio leaves a gap and
  titles and captions over its end last longer; the same source heard on
  another audio track is cut out of the range. One undo step; a `Fix`
  finds its words again by clip and source time when the timeline moved
  while the provider read. Words in a sped-up, slowed or ramped clip, or
  on a picture track only, are refused; a ramped picture held over longer
  words keeps its ramp's shape, slower. A source with clips on audio tracks is now read
  for words there only (`query::words_in_range`), so the old words don't
  come back from the picture over them.
* **Queries** (`flikk-core/src/query.rs`): topmost clip at a time, sample
  points, transcript↔timeline word mapping, typed timestamp and chat
  reference resolution, effective style rules.

### Change sets

`Author::Agent` and `Author::External` commands go into the author's
open `ChangeSet` (`flikk-core/src/changeset.rs`, `bus.rs`), planned against that
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

### Sync by sound

A second camera or a separate audio recorder is lined up with the main clip
by what they both heard (`flikk-core/src/sync/`), for podcasts and multicam.

* **Measuring** (`sync/measure.rs`, pure Rust with its own radix-2 FFT in
  `sync/fft.rs`): each recording's stream is decoded through
  `MediaService::audio` to mono at 8 kHz, whole (up to 6 hours). A coarse
  pass turns each into an onset envelope (spectral flux of 16 log-spaced
  band energies, 100 a second, its local mean removed and its peaks capped)
  and cross-correlates the two over every lag at once, each lag normalized
  by the overlap it leaves (at least 3 s, or 30% of the shorter), so
  offsets of hours and recordings that start late or stop early cost the
  same. A fine pass reads 3–12 windows of 8 s spread over the overlap from
  both and finds each window's lag with band-limited (150–3,800 Hz)
  GCC-PHAT (β 0.8), to a fraction of a sample by a parabola. Flat first:
  the windows' median offset and those within 1.5 ms of it; a robust line
  (Theil–Sen, then least squares on the windows within 1.5 ms) wins when
  more windows agree with it and the drift it shows adds up to 2 ms or more
  over the overlap. The result is the follower's source time
  `offset + r · (1 + drift)` for the reference's source time `r`, with a
  confidence (0.35 from how far the coarse peak stands above the best lag
  elsewhere, 0.65 from the share of fine windows that agree); under 0.5 it
  is not used and the follower is left where it was. A follower that does
  not match the reference is tried through another follower that did
  (`sync::measure_group`). The measuring runs off the UI thread.
* **Placing** (`sync/plan.rs`): `Command::SyncClips` and
  `Command::PlaceSynced` carry what was measured, not positions, so a
  proposal planned again after the user moved the reference still lands in
  sync. A recording on the timeline is the clips that play one media item
  from the same point (a video and the sound that landed with it); they
  move together. The reference is the one on the lowest video track (the
  earliest there), else the first selected; the agent can name it. A
  follower that would overlap another clip goes to a free track of its kind,
  else a new one; one that would start before zero loses its head. A
  speed-ramped reference or follower is refused: its sound does not keep
  one offset, and a drift fix is a constant speed that would replace the
  ramp.
  `PlaceSynced` lays pool items down whole from the playhead, each on a
  free or new track, the group moved later together when one would start
  before it. "Recorder sound" disables the cameras' sound clips when the
  group holds a sound-only recording.
* **Drift** is corrected by the follower's speed (`speed × (1 + drift)`).
  Speeds within 0.1% of 1 (`sync::varispeed`, under two cents of pitch)
  play by resampling, as a fast clock does, not by time-stretching: output
  frame `n` is the input at `n · speed`, Catmull-Rom interpolated. The
  preview mixer uses it in place of WSOLA, and the render decodes such a
  clip's span the way the preview does, runs it through the same code and
  reads the result as FLAC from the job's work folder
  (`flikk-deliver/src/varispeed.rs`) instead of `atempo`, so preview and
  export put every sample at the same time. WSOLA and `atempo` were not
  used for this: each wanders up to ±10 ms around the nominal time. A
  follower video at such a speed is composited in renders, as any retimed
  clip is.
* **UI:** the timeline's clip menu offers "Sync by Audio" when the
  selection holds two or more recordings with sound, and "Sync by Audio,
  Recorder Sound" when it holds a sound-only one and a camera. The media
  pool takes cmd/ctrl- or shift-click to add items to its selection; an
  item's menu then offers "Sync" (and "Sync, Recorder Sound"). The edit is
  one undo step; the status line says only what did not match and drift
  that was corrected. The agent's `sync_audio` (`tools/sync.rs`) takes
  `clips` (or pool `media` with `at`), `reference` and
  `replace_camera_audio`, runs as a job and proposes the edit, reporting
  each follower's offset, drift and confidence, or why it was not synced.

## 5. Compositor and FX

* **One FX format** (`flikk-core/src/fx/mod.rs`; its module docs are the
  authoritative GLSL and HTML conventions). An `FxAsset` is a GLSL program
  (effect, transition or generator) or an HTML/CSS/JS document (generator:
  titles, lower thirds, motion graphics, image layouts) plus typed
  parameters (number, bool, color, point, text, choice, image). Built-ins:
  51 GLSL programs in `fx/shaders.rs` (27 effects, 17 transitions,
  7 generators) and 15 HTML templates in `fx/templates.rs`. Programs the
  user or the agent writes live in `Project::fx` and are edited in the FX
  code editor. Three more built-in effects, in `fx/depth.rs`, read depth
  (see "Depth effects"). An `FxInstance` uses an asset with parameter values and
  keyframes: on a clip's effect stack, as a transition, or as a generator
  clip's content.
* **GLSL** follows ShaderToy/gl-transitions: authors write `effect(uv)`,
  `transition(uv)` or `generate(uv)`; `flikk-gpu` wraps it as `#version 450`,
  compiles it with naga on a thread with a 16 MiB stack, caches the result
  and reports errors against the author's lines. `iResolution` is in
  sequence pixels whatever size the effect runs at.
* **HTML** renders in headless Chromium (Chrome, Chromium, Edge or Brave, or
  `FLIKK_CHROME`) over CDP with an engine-owned clock (`clock.js`: time,
  timers, rAF, CSS animations, Web Animations and GSAP all follow clip time,
  `Math.random` is seeded), so frames are deterministic and exportable, with
  alpha. GSAP 3.15 and its plugins are vendored in `flikk-html/src/gsap` and
  preloaded; templates build their animation in `flikkTimeline`, which the
  engine reruns when parameters or the clip length change. Pages lay out on
  the sequence's shape with a 1080 CSS px short side, so templates keep
  their proportions at any resolution; in portrait, bottom elements rise
  above the controls vertical apps draw. A render runs three browsers of its
  own (`flikk_html::crew`): each seeks through every frame, so scripts run as
  in one browser, and they take turns capturing the frames the compositor
  will ask for next.
* **Program frame** (`flikk_core::compose::program`): for each layer, the
  picture (a media frame at the speed-aware source time, an overlay, or a
  generator; a slowed clip that blends or flows shows the moment between
  two source frames: `Pictures::pair` reads both, the batch mixes them, or
  `InterpolationService::between` gives the optical flow frame once made,
  at 1/32 steps so frames are reused) → stabilization (a stabilized
  clip's frame moved, turned and zoomed inside its own frame by its
  correction) → clip grade, then timeline grade → effect stack → masks (a
  coverage raster multiplied into alpha; a subject's matte at the source
  time drawn into it, see "Subject masks", and a person mask's, see
  "Person masks") → transform and blend mode;
  transitions render each side onto its own canvas and mix them. Titles and
  captions are lettered on top by `LetteringService` (cosmic-text
  `flikk_deliver::Letterer`). The whole frame is one `FxBatch` on the GPU: one
  upload per source picture, every pass in one submit, one readback (already
  BGRA); submit and wait are separate so playback pipelines frames.
  Without a GPU, `CpuBatch` and `compose/cpu.rs` composite in memory
  (transitions become a dissolve; effects and GLSL generators are skipped).
* **Depth effects** (`flikk-core/src/depth.rs`, `fx/depth.rs`): a GLSL
  effect that calls `float getDepth(vec2 uv)` reads the picture's depth map
  (distance 0 nearest to 1 farthest, sampled like `getColor`); `flikk-gpu`
  declares it and binds the map after the image parameters
  (`FxBatch::apply_depth`). The compositor asks the depth service for the
  map of the source frame a clip shows (only for clips with such an effect
  or a depth mask) and uploads it once for the clip's chain; until the map
  is made those effects are left out. Built-ins: **Lens Blur**
  (`depth_blur`: `focus`, `range`, `blur` in pixels; a scatter-as-gather
  over two spirals of taps, what lies behind a pixel blurring no further
  over it than the pixel itself, so a sharp subject keeps its edge and a
  blurred foreground spills over it; keyframe `focus` for a rack focus),
  **Depth Fog** (`depth_fog`: `tint`, `density`, `start`) and **3D
  Camera** (`depth_parallax`: `motion` Dolly, Zoom (dolly zoom about
  `focus`), Orbit or Pan along `angle`, `amount` −1..1, eased over the
  clip; each output pixel searches the layers of distance from the front
  for the surface it shows, so near things cover far ones and the gaps
  they open are filled from the edge beside them, and the picture zooms in
  just enough to keep the frame's edges covered all through the move).
  The inspector's "Pick focus" on an effect with a `focus` arms the
  viewer: the next click on the clip's picture makes that frame's map and
  sets the focus to the distance there (a median over a small disc), keyed
  at the playhead when `focus` already has keyframes
  (`depth::focus_command`, `ui/viewer/focus.rs`).
* **Grade** (`flikk-core/src/grade.rs`): one pipeline (exposure, contrast, saturation,
  temperature/tint, lift/gamma/gain, offset, shadows/highlights, hue,
  curves, a LUT file), baked into a 65³ LUT for export.
  `scopes.rs` reads waveform, parade, vectorscope and histogram and matches
  shots.
* **Audio effects** (`flikk-core/src/audiofx.rs`, `audiofx/dsp.rs`) run in the
  preview mixer and map to FFmpeg filters in `flikk-deliver/src/audio_fx.rs`,
  table by table, so preview and export match.

## 6. Media and playback

* `flikk-media`: ffprobe (variable rates snapped to standard ones),
  fingerprints, single frames (96 MiB in-memory cache), playback streams
  (RGBA frames as PAM over a pipe) and f32 PCM streams, proxies (short side
  ≤ 720, x264 veryfast CRF 26, no B-frames, half-second GOP), 6 × 5 collages stamped
  with timeline and source timecodes, waveform peaks (200/s) and spectrogram
  pictures (`showspectrumpic`, one lane per track, words, sounds and beats
  marked). Derived files are written atomically.
* Cache layout under `paths::cache()`: `proxies/<fp>.mp4`,
  `waveforms/<fp>-<stream>.peaks`, `collages/`, `analysis/<fp>.json` and
  `<fp>.vectors`, `analysis/<fp>.motion` (camera paths), `mattes/<fp>/` (subjects: `p-<key>.matte` per prompted
  frame, `t-<key>.mattes` per tracked subject; people:
  `a-<model>-<chunk>.alpha` per 32 frames), `interpolated/<generation>/`
  (optical flow frames at preview sizes, QOI, below), `depth/<fp>/<model>/`
  (`<frame>.depth` per source frame, a grayscale PNG), `coreml/<model>-<sizes>/` (depth
  models compiled for the GPU, 420 MB a size), `html/` (pages and browser
  profiles), `render/<job>/` (scratch), `agent/frames/` (images for the
  activity log), `frames/` (stills handed to video generation),
  `frame-cache/<generation>/<ab>/<key>.frame` (the render cache, below),
  `hdr/` (tone-mapping tables), `respeak/` (a provider's takes of a fixed
  sentence, until the words are cut out of them), `voice-sources/`
  (speech cut as FLAC for a provider to clone or match, removed with the
  voices dialog). An HDR item's proxy is
  `proxies/<fp>-<mapping>.mp4` (below).
* **Disk caches** (`flikk-core/src/disk_cache.rs`): the render cache and
  optical flow frames are files named by a 16-byte key under
  `<root>/<generation>/<ab>/`, each starting with a magic and its key,
  written to a temporary file and renamed. An in-memory index (read from
  the folder on first use, ordered by modified time, which reads touch)
  evicts the least recently used past a size limit. Opening one removes
  the other generations under its root, so a new layout or app version
  drops the old files at once.
* **Sweep** (`flikk-core/src/cache_sweep.rs`): at startup, off the UI
  thread, files unused for 30 days go from `proxies/`, `depth/`,
  `collages/`, `waveforms/`, `frames/`, `agent/`, `voices/`,
  `voice-previews/`, `hdr/` and the pages in `html/`; scratch
  (`voice-sources/`, `upload/`, `respeak/`, `render/`) and partial writes
  (`.part`, `.tmp`, `.tmp-…`) a day old; and proxies of a fingerprint a
  day older than its newest, made for a tone map since changed. Readers
  mark files used (`mark_used`: the modified time, at most once a day).
  Mattes, analysis results and the disk caches are not swept. The pool
  makes a video's proxy again when it is missing. Writes to the user's data
  (library items, settings, credentials, the agent's stores, chats,
  proposals, autosaves, backups) go through a uniquely named temp file
  (`flikk_os::fs`); one a crash left is removed a day later
  (`flikk_os::fs::sweep_temps`), as are library item names claimed and
  never written.
* **HDR** (`flikk-core/src/hdr.rs`): probing marks a video stream whose
  transfer is PQ (`smpte2084`: HDR10, Dolby Vision) or HLG (`arib-std-b67`:
  iPhone and Android HLG, the base layer of Dolby Vision 8.4) with an `Hdr`
  (primaries, matrix, range, bit depth, the peak from MaxCLL or the
  mastering display, read off the first frame, and a Dolby Vision profile);
  the pool badges it "HDR". Each item's `ToneMap` (Auto by default, or Off;
  an exposure in stops and a peak in nits, set by `Command::SetToneMap` from
  the inspector's Color tab or the agent's `set_tone_map`) gives a
  `Mapping`, whose filters go on every decode of the original file (frames,
  playback, proxies, collages, which use the default tone map) and on every
  segment the render graph opens, mapped before any scaling so the viewer
  and the export match. The source's light (PQ absolute; HLG on a 1,000 nit
  display, system gamma per channel as zimg does) is divided by the nits
  that land on SDR white (100, halved per stop of exposure), taken from
  BT.2020 to BT.709 primaries, and FFmpeg's Möbius curve (knee 0.3, `desat`
  0) brings the source's peak (PQ: its metadata, else 1,000; HLG: 1,000) to
  white on the brightest channel; BT.1886's inverse encodes it. The output
  is BT.709 limited-range 10-bit 4:4:4, tagged so, and exports are tagged
  BT.709. With `zscale` and `tonemap` in the installed FFmpeg (`ffmpeg
  -filters`, checked once per process) zimg does it; otherwise the same
  steps are baked into a 65³ `.cube` over the BT.2020 signal
  (`cache/hdr/`) and applied with `lut3d` (`FLIKK_TONEMAP=lut` forces
  this). Proxies of HDR items are made already mapped, one per mapping;
  changing an item's tone map starts its proxy again (the pool's poll), and
  until it exists the original is decoded and mapped. Frames between source
  frames (optical flow) are made from mapped pictures, proxy or original,
  and keyed by the mapping; the render cache's globals hold the method, and
  its clip keys the item's tone map. Hardware encodes keep the BT.709 tags.
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

### Render cache

`flikk/src/render_cache*` keeps heavy frames on disk so playback stays
real time however heavy the edit (Resolve's smart cache, Premiere's render
bar).

* **What is rendered:** the ranges `compose::needs_compositor` picks
  (effects, generators, HTML generators, transitions, masks, subject masks,
  retimed media…), drawn by `compose::submit_program` like the viewer and
  the export, at the sequence's shape with a 720 short side (smaller
  sequences at their own size), masks cut (not tinted).
* **Keys** (`render_cache/key.rs`): the timeline is cut at every clip and
  transition edge on the tracks that show; each piece that needs the
  compositor is a span hashed (BLAKE3) over serialized state: the clips
  shown there and the clips touching them (transitions draw from them),
  their tracks, their media items (fingerprint, path, info, proxy), the
  timeline without its tracks, markers and ducking, the project's FX
  programs, the caption styles, the frame size, which engines are wired
  (and whether the depth model is here), the subject and depth models, the
  app version and a format number. A frame's key is
  its span's hash and its index. Serialized values go through
  `serde_json::Value`, whose maps sort their keys. Only fields known not to
  draw are left out (clip audio, name and colour; track name and lock; bins
  and tags), so a property added to a clip, track or the timeline later is
  in the key without being listed. An edit changes the keys of the frames
  it changes and no others; nothing is invalidated, frames no longer asked
  for age out.
* **Store** (`render_cache/store.rs`): a disk cache (above) under
  `cache/frame-cache/v<FORMAT>-<app version>/`. Frames are only ever shown
  in the viewer, so an opaque one (nearly all: the program is composed on
  black) is kept as JPEG at quality 90, one with transparency as QOI
  (BGRA). On 720p frames checked on the CPU, JPEG was about 9× smaller
  than QOI on a clean photo (94 KB against 850 KB) and 5× on a grainy one
  (520 KB against 2.6 MB), decoding in 5–12 ms; it encodes in 25–45 ms,
  on a writer thread of the worker's own. A file that is missing or does
  not decode, or whose key does not match, is dropped and the frame
  composed live.
* **Background render** (`render_cache.rs`, `render_cache/worker.rs`):
  `EditorState.render` polls 4 times a second. 200 ms after the shown
  project (`display_project`: a previewed draft or a drag included) last
  changed it is planned and measured off the UI thread; 1.5 s after the
  last play, scrub, edit or delivery a worker thread renders the spans not
  on disk, from the playhead on, then from the start, pipelined as in
  playback. Any play, playhead move, edit or job in the render queue drops
  the worker, which stops after the frame in hand. A span waits (retried every 5 s) while its media is offline or
  its subject masks' mattes are still being made; the depth maps its
  clips read are made first (`complete_depth`), so a frame is only kept
  as it will finally look. A stale or cancelled render can only write
  frames under the keys of what it drew, so it never shows a wrong frame.
  The first 9 frames of a span are composed one at a time and timed; when
  the median takes under a quarter of a frame's time the span is cheap
  (for the session, by its hash): drawn live as fast as read, so the rest
  of it is not kept.
* **Reading:** playback (`viewer/player.rs`) takes a frame from the
  in-memory cache, else from disk when the cached frame is at least three
  quarters of the viewer's width, else composes it; paused frames and
  render-ahead use disk frames at least as wide as the viewer. A source
  item in the viewer, a zoomed viewer and tinted masks always compose.
* **Render bar:** a 2 px strip along the top of the ruler: `theme.success`
  over cached frames, `theme.danger` over frames still to render; nothing
  where no compositor is needed or the span is cheap.
* **Settings → Storage:** the frames count in the Cache row and go with
  its Clear (old generations go when the store opens); the "Background
  rendering" row sets the size limit (2–50 GB, 10 by default) and turns
  background rendering off (`RenderCacheSettings` in the store's `meta`).

## 7. Analysis and models

The user asks for analysis from an item's "⋯" menu in the media pool (or
its right-click menu): "Analyze", or "Analyze again" once it has results,
which runs every step anew on the models picked now. Every other item is
also queued as it reaches the pool, however it got there (an import, the
media pool's Generate, an approval), in the background
(`EditorState::analyze_new`, called after every edit, which looks at each
pool item once; there is no setting to turn it off). Pictures are never
analyzed, and items already in a project when it opens are left as they
were; an import undone and done again is new again. Automatic analysis
never spends money unasked: while a provider is picked to transcribe, an
item whose sound it would hear (one without a voiceover's script standing
in for its words, and not heard only, below) waits in
`EditorState::provider_asks`, and a dialog
(`ui/provider_ask.rs`, opened and closed by the window with the list, which
it follows as more arrive) lists them under "Analyze with <provider>?".
Analyze queues them as asked for, so they go to the provider; Cancel,
Escape or the backdrop leave them unanalyzed until asked for. Analyze is
dimmed, with a note, while the provider has no key. Picking a provider to
transcribe drops the analysis still waiting in the background that would
go to it (`cancel_background_speech`, which names their files) and asks
about those items the same way; going back to this machine queues the
items asked about, as they cost nothing then. As a last guard, a job
queued in the background and not asked for since skips itself, leaving the
item unanalyzed, when a provider transcribes by the time it runs
(`JobProgress::asked`). Music and sound effects a provider made are heard
only, whoever asks and whatever is picked to transcribe
(`MediaItem::heard_only`, applied in `LocalAnalysis::analyze_with`, the
rule the library's sounds follow): sounds and music, beats and sound
search, with no voice activity, transcript, speakers or shot changes, so
they never reach a provider and are never asked about. Their result is
whole when it covers those steps (`coverage` works out what a plain
request wants for speech, a script or sound only), and the job's key
carries the steps, so it is told apart from a full one. Imported audio
can't be told to be music, so it is analyzed in full. The library's sounds are
analyzed on their own without speech or shots, on local models only, so
that it costs nothing. The models are on
disk before a project opens (see Models), so "Analyze" is disabled only
while a provider picked to transcribe has no key (`analysis_ready`; the
key's own notice says so), and while the item is queued or analyzing;
images are not analyzed. Every step
(`AnalysisSteps`: transcript, speakers, sounds and music, beats, sound
search, shot changes) is always on and cannot be turned off: each runs
whenever the model for its task is downloaded, and one whose model is
missing stays empty in the results (voice activity falls back to an
energy VAD). Speakers need the transcript; beats and sound search, the
sound pass. Items are analyzed one at a time on a `JobQueue`: what the
user or the agent asks for (`AnalysisService::analyze`: "Analyze",
`view_media`, the provider dialog's Analyze) goes ahead of what is queued
in the background (`analyze_in_background`: new pool items, library
sounds), and an
item waiting there moves up with them when asked for
(`JobQueue::push_first`); each group runs in the order queued. A result is written to
`cache/analysis/<fp>.json` only when every step succeeded; otherwise it is
kept for the session and the job fails, naming the steps.
`ANALYSIS_VERSION` (1) marks the results: those of another version, and the
sound vectors beside them, are not read, so the item is not analyzed until
the user analyzes it again.

### Models (`flikk-models`)

One folder per task (voice activity, transcription, word timing, speakers,
sound tagging, beats, sound search, segmentation, frame interpolation, matting, depth). Its `mod.rs` holds the
task's interface, the trait analysis calls (`VoiceActivity`, `Transcriber`,
`Aligner`, `Diarizer`, `Tagger`, `BeatTracker`, `SoundSearch`, `Segmenter`, `Interpolator`, `Matter`, `DepthEstimator`); `MODELS`, the
models that can do it, the default first; and `runner`, which builds the
runner of one of them. In it, one folder per model: `mod.rs` is the model as
the user sees it, `config.rs` its files (pinned to a commit, with their
sizes and SHA-256) and the settings it runs with, `runner.rs` the code that loads and
runs it (behind its feature) and finds its own files. What models share
sits at the crate root: the catalog types, the audio rates, the downloads,
ONNX Runtime (`onnx.rs`; one environment per process, shared by the
aligner, the diarizer, the sound models, segmentation, frame interpolation, matting and depth) and the BPE tokenizer
(`bpe.rs`). `onnx::GpuSessions` runs RIFE and Depth Anything on the GPU:
Core ML on macOS, DirectML on Windows, WebGPU on x86-64 Linux (its Dawn
library sits beside the binary, which finds it by rpath, and the editor
does not start without it). They are the only ONNX models that gain from
it: the others came out slower or level, measured on an M5 with Core ML,
except WeSpeaker, which gives NaN there on real audio. A load, run or
answer that fails on the GPU is run again on the CPU, where that model then
stays. On macOS depth keeps its compiled models: they take 17 s to compile
and 4 s to load. DirectML and WebGPU on Linux have not been run yet.

Analysis only orchestrates: it asks each task's runner for the selected
model (`Task::model`: the one the user picked, else the default) and never
names a model; a new pick of transcription model applies from the next
transcript, and results made already stay. A new model is a folder under
its task, its `MODEL` in `MODELS` and an arm in `runner`; a model in several
sizes (Whisper) has a `MODEL` for each in one folder.

Every model is needed: each task of `Task::ALL` on the model picked for
it (`EditorState::required_models`). While one is missing, the welcome
page shows a setup list in place of Recent (`state/setup.rs`,
`ui/welcome.rs`): one row for the whole set, with its total size and one
Download that starts each missing model at once, its progress counted
against the bytes missing when each of the set's downloads started (from
here or Settings), Cancel, and a failed download
in place with Retry; Whisper small or large, each with its size, held
while the set downloads; the agent CLIs found and their versions
(`flikk_agent::backend::installed_version` runs `--version` off the UI
thread; with neither, Install… and Check again); and FFmpeg and a browser,
only when they are not found. After an update that adds a task, the list
comes back with only its files to fetch. No project is created or opened
while it shows, however it is asked for (`EditorState::projects_wait`,
checked in `projects.rs`): the app opens on it whatever way it was
started, and a `.flikk` file it was started for opens once the set is
complete. New project, Open… and the drop zone are held meanwhile; a
shortcut that asks says why on the status line, which the welcome page
shows too. The demo opens without the models. `FLIKK_DEMO_SETUP` seeds
it, its buttons only pretending.

Nothing downloads on its own. Settings → Models (the gear in the top bar, or
cmd-,) lists each task with its model, picked there when the task has more
than one (saved per user in the store), and downloads one or all of them,
with progress and cancel, through `<file>.part`; a failed download stays
there with Retry. A part is kept when a download is cancelled or cut, and
the next one asks the server for the rest (a range request; a server that
sends the whole file again starts it over). Before a file takes its name
it is checked against its pinned SHA-256 (`ModelFile::sha256`: the host's
LFS object id for the pinned commit, or measured on bytes checked against
the host's git hash for the small files); one that comes out the wrong
size or does not match is deleted and fails, so Retry fetches it clean. A part is locked while it is written, so a cancelled
download still winding down, or another copy of the app, is done with it
first. `LocalAnalysis::model_tasks` says which tasks a build
runs. A runner whose files are missing (deleted by hand) fails ("… is not
downloaded") and its step is left out; nothing warns about a missing
model, as the welcome page covers it. A model the editor needs can't be
deleted there: only the transcription size not in use has Delete, whole
or part downloaded. Picking the other size downloads it while the size on
disk stays in use (`EditorState::picked_model` is the pick, `Task::model`
the size analysis runs on), and switches once it is on disk; so the set
stays complete and projects open meanwhile. A failed download shows on
its row with Retry, the old size still in use; Cancel, or picking again,
gives the switch up and stops its download. The top bar's notices
(warnings such as offline media or a provider without its key) can be
dismissed for the session.

Cargo features of `flikk-analysis` (all default) gate the runners: `whisper`,
`aligner`, `diarizer`, `sounds`, `subjects`, `interpolation`, `people`. Without one, its models stay
listed but its step falls back as described below (without `subjects`,
subject masks keep nothing; without `interpolation`, optical flow blends;
without `people`, person masks keep nothing). `flikk-models`' `mlx` (macOS only: mlx-sys links
Apple's frameworks) adds the MLX subject mask model and makes it the
default; `flikk` turns it on with `metal` on macOS. mlx-sys builds
MLX from source, so a macOS build needs CMake and Xcode's Metal compiler. Segmentation is listed under Settings → Models
as "Subject masks" and matting as "Person masks", but neither is part of
analysis. Frame interpolation ("Optical flow", RIFE) is listed the same way;
slowed frames are blended while its frames are made.
`LocalInterpolation` makes flow frames on one thread of its own, the
newest asked first, keeps the last 48 in memory and those with a short
side up to 720 (the viewer's and the render cache's) as QOI in a disk
cache under `cache/interpolated/`, 4 GB at most, least recently used
first out (counted and cleared with the Cache in Settings → Storage). A render's larger frames
stay in memory: it asks for each only while it is on it.
Depth ("Depth maps", the `depth` feature, default on) is not part of
analysis either.

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
  `FLIKK_VAD_MODEL`), run by whisper.cpp: speech from 0.5 until under
  0.35, pauses under 250 ms kept, 60 ms before each range. Music is not
  speech to it. The energy VAD (`flikk-analysis/src/vad.rs`), which took a
  music bed for speech, stands in without the `whisper` feature, or for the
  session when the model can't be had. Silero hears the audio in one go and
  in 4 s blocks, each after the second before it, and a window is speech
  when either finds it: music holds it down for seconds after, and a block
  that starts during a sentence doubts the rest of it. A silence never takes
  a transcribed word, so the quiet last word of a phrase Silero let go of
  stays.
* **Transcription:** Whisper small (264 MB, the default) or large
  (large-v3-turbo, 874 MB), both with 8-bit weights, picked in Settings →
  Models, or `FLIKK_WHISPER_MODEL` (a path or a ggml file name in
  `paths::models()`) for whichever size is picked. whisper.cpp is built for
  the GPU (Metal on macOS, Vulkan on Windows and Linux, with flash
  attention), so building needs Xcode or the Vulkan SDK (`glslc` and the
  Vulkan headers on Linux); without a GPU it runs on the CPU. On Windows
  the editor delay-loads `vulkan-1.dll` (`flikk/build.rs`), and where
  the DLL is missing `flikk/src/vulkan_fallback.rs` makes Vulkan fail
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
  ours (below), or `FLIKK_ALIGNER_MODEL`. Other languages keep word
  edges measured on the audio levels (`flikk-analysis/src/align.rs`), and so
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
* `flikk-core/src/heard.rs` maps sounds, music, beats, level changes and
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
  commit (`bpe.rs` encodes it). `FLIKK_TAGGER_MODEL`,
  `FLIKK_BEATS_MODEL`, `FLIKK_CLAP_AUDIO_MODEL`,
  `FLIKK_CLAP_TEXT_MODEL`, `FLIKK_CLAP_VOCAB` and
  `FLIKK_CLAP_MERGES` point at local copies instead.

### Fitting music to a length

`flikk-core/src/music_fit*` makes a song end exactly where it should, on its
real ending, with seamless edits instead of a hard cut or a fade (like
Premiere's Remix). The timeline's clip menu offers "Fit To" → Sequence End
(the end of the other tracks), In/Out, or Duration… on a media clip on an
audio track; the agent has `fit_music`. Both build a `music_fit::Request`
on the UI thread and run it off it:

* **Bars:** the analysis' downbeats from the clip's in point on, missed
  ones filled where a gap holds a whole number of bars, each moved onto the
  attack within 40 ms of it (the largest rise of the high-passed level,
  3 ms before it), so a cut keeps the hit whole. Quiet bars at the end
  (8 dB under the song's median) are the ring of the final chord and join
  the ending. Before the first bar is the head; after the last, the
  ending.
* **Features** (`music_fit/features.rs`), from PCM decoded at 22.05 kHz
  mono through `MediaService::audio`: per quarter of each bar a 12-bin
  chroma (55 Hz–5 kHz) and 12 MFCCs (26 mel bands, 2048-sample frames,
  hop 512), and the bar's RMS level. A bar-to-bar similarity mixes chroma
  agreement (cubed, 0.5), timbre (MFCC distance over its median, 0.35) and
  level (0.15).
* **Planner** (`music_fit/planner.rs`): a dynamic program over (bars
  played, bar, bars since the last jump) finds the cheapest path from the
  first bar to the last that keeps the first and last 4 bars (fewer in
  short songs), runs at least 4 bars between jumps, and may jump back
  (loops) or forward (skips). A jump from bar i into bar j costs 1, plus
  5 × (1 − how much j sounds like i + 1 and j − 1 like i), plus a phrase
  penalty (0 when the bars moved are a multiple of 8, 0.4 of 4, 1.5 of 2,
  else 3), 0.5 off a 4-bar boundary, and the level change. Each length of
  path is scored with where it leaves the ending: the ending's ring can be
  trimmed down to its hit and half a bar under a fade at least a beat long;
  ending early or cutting into the hit costs by the beat. The quality is
  the weakest jump's score times how close the end lands.
* **Applying:** `Command::FitMusic { clip, at, segments, crossfade,
  fade_out }` (category Audio) replaces the clip with its segments back to
  back from `at`, keeping its gain, effects and edge transitions; each join
  gets a 30 ms `EndOnCut` transition, which audio tracks play as an
  equal-power crossfade (`audiofx::spans`) in the viewer and the render
  alike, so the downbeat after the cut is never faded. Keyframes are
  dropped. It refuses clips at another speed (fitting keeps the tempo) and
  results that would run over another clip on the track. One undo step for
  the user, one reviewable item for the agent, whose result lists each
  skip and loop with its source span, place and score.

### Subject masks

* **Model** (`flikk-models/src/segmentation/sam2/`): SAM 2.1 tiny (Meta,
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
* **On MLX** (`flikk-models/src/segmentation/sam2_mlx/`, feature `mlx`, the
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
* **Service** (`flikk-analysis/src/subjects.rs`, `LocalSubjects`): frames come
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

### Stabilization

* **Measuring** (`flikk-analysis/src/motion.rs`, `LocalStabilizer`, Rust
  alone, no model): frames are read once in order through the media
  engine (the proxy when there is one) at most 320 px wide, in grey, with
  half- and quarter-size copies. Each frame is compared with the one
  before: the whole picture at quarter size for its overall move (±8 px
  there), then a 10 × 6 grid of 16 px blocks, those with texture both ways
  (the structure tensor's smaller eigenvalue), found again by least
  absolute difference at half size around that move and refined at full
  size to a fraction of a pixel (a parabola through the neighbours). A
  move and turn are fitted to the blocks by RANSAC (200 rounds, 1 px)
  and refined by least squares on the blocks that agree, so a subject
  that moves on its own is left out; with too few agreeing (under 4, or a
  third) or a turn over 3°, the median move stands in. Scale is not
  fitted: shake hardly zooms between frames, and a fitted zoom was mostly
  noise that the correction then added back. The path
  (`stabilize::CameraPath`: each frame's time and its step from the one
  before) goes to `cache/analysis/<fp>.motion`, kept with the analysis
  results (Clear in Settings → Storage leaves them); paths read stay in
  memory for the session. It runs as a job on its own `JobQueue` (one at a
  time), listed by `Stabilizer::jobs`, with the inspector showing its
  progress; renders measure the paths they need first
  (`composed::measure_motion`, part of the progress), and the agent's
  `get_shake` waits for it. About 8 s per minute of 30 fps 720p footage
  in a release build here, decoding included.
* **Correcting** (`flikk-core/src/stabilize.rs`, `Plan`): the steps over the
  source range the clip shows add up into the camera's pose per frame
  (where frame 0's centre went, its turn), composed exactly as
  similarities. Smooth runs a Gaussian over each part of the pose
  (σ = 0.12 s × 25^smoothness, so 0.12–3 s), each end reflected through a
  line fitted to the σ frames there, so a pan carries on and the first
  frames are smoothed too; Locked holds the mean pose. Each frame is moved
  from where the camera was to where the target puts it (W = S ∘ C⁻¹).
  With Auto zoom the picture is zoomed in by the least that hides the
  edges on every frame of the range (one zoom for the clip, so it does
  not pump), at most 1.4 ×; frames that would need more get only the part
  of their correction that fits (binary search), eased by a running
  minimum then a Gaussian no wider, so the path stays smooth and still
  under the zoom. Plans are kept for the last 24 clip settings and
  ranges. The compositor draws the frame's correction into the picture's
  own frame before its effects, masks and the clip's transform
  (`compose::Ctx::stabilized`, an extra composite in the same batch), so
  the viewer, the agent's views and composited renders
  (`needs_compositor` is true for a stabilized clip) draw it alike. Until
  its path is measured, a clip shows unstabilized, and asks for it; the
  render cache keeps no frame of it until then (`Span::stabilized`,
  `worker::ready`). The frame is looked up by source time through
  `Clip::to_source`, so speed changes and ramps find the right one; a
  moment a slowed clip makes between two frames (blended or optical flow)
  takes their corrections in proportion (`Plan::between`).
* **Checked here** on a 6 s 1280 × 720 clip made with FFmpeg (a still
  with a sine-and-random crop jitter of ±20 px, a slow pan, a ±1°
  wobble and a picture moving across it): the steps matched the known
  moves to about a pixel and the turns to 0.0005 rad; its shake score
  (the RMS of what is left of the path once moves slower than 0.25 s are
  taken out, in percent of the frame width, plus the turn) went from 1.9
  to 0.13 (Smooth, zoom 1.11), 0.13 (Locked) and 0.09 (no crop), and a
  still clip scores 0.00.
* **Agent:** `stabilize` (Transform) proposes it, the values not given
  kept, and starts measuring; `get_shake` (with the transform group)
  measures when it has to and gives each clip's score, its label (steady
  under 0.25, slightly shaky, shaky, very shaky over 2), the jitter in
  percent and degrees, and the zoom its settings (or the default ones)
  would take; `get_clip` shows the settings.

### Person masks

* **Model** (`flikk-models/src/matting/modnet/`): MODNet (Ke et al., Apache-2.0
  code and weights), the official photographic portrait checkpoint as
  Xenova's ONNX export (`Xenova/modnet`, Apache-2.0, pinned to commit
  `fa2fa546`, 25.9 MB, full precision; `FLIKK_MODNET` points at a local
  copy). One graph: a picture with its short side at 512 (the long side
  in proportion, at most 1024, both a multiple of 32), scaled to -1..1 →
  coverage 0..1 at that size. Pictures are resampled by area averaging.
* **Service** (`flikk-analysis/src/people.rs`, `LocalPeople`, behind
  `LocalSubjects`' person methods of `SubjectService`, sharing its media
  engine and version): frames are read at a 512 short side through the
  media engine (the proxy when there is one), matted in chunks of 32 and
  kept by fingerprint, model and chunk, as run-length coded bytes (about
  30 KB a frame for a 768 × 512 matte, so about 55 MB a minute at 30 fps)
  in memory (96 chunks) and under `cache/mattes/` (cleared with the Cache
  in Settings → Storage). Every person mask on the same media
  shares them. One worker serves the newest request first: drawing a frame
  asks for the 3 s from it on, so playback finds them made; a render asks
  for the frames of its range it shows and waits (`complete_person`). When
  the newest request wants another chunk the worker leaves the one it is
  on and goes on with it later. A frame not made yet shows the nearest made
  one within a second, else the mask is left out. Each frame is eased
  toward the one before (by half where they agree, not at all where the
  coverage moved by 0.25 or more), which stops the flicker on still edges.
* **Drawing** (`PersonMatte::field`): the coverage is upsampled bilinearly
  to the mask raster, then within the band where the neighbourhood is
  mixed it is eased by `edge` toward a guided filter of itself on the
  decoded frame's luma (radius a 270th of the height times 1 + 3 × edge,
  ε 10⁻⁴ + 10⁻³ × (1 − edge)): strands the small matte blurs come back
  and halos of background tighten. Expansion is a grey dilation (or
  erosion) by a square, feather three box blurs, so both keep the soft
  edge. Layout bounds a picture by its people.
* **Speed, here** (4 CPU cores, release build): MODNet takes 0.27 s for a
  768 × 512 frame (0.34 s a frame through the service with decoding), and
  the edge 0.1 s on a 1024-wide raster; a 2 s clip of 60 frames was
  matted in 17 s, and read from the cache in under a millisecond after.
  Playback outruns it on the CPU: frames ahead fill in as it goes.
* **UI:** a person button (tooltip "Remove background") in the
  inspector's masks section and in the viewer's mask tools adds the mask
  to the clip (or selects the one it has); its row has the usual mode,
  invert and keyframe, and an Edge slider. A click on a person in mask mode
  selects the mask. Without the model, a notice leads to Settings → Models.

### Depth maps

* **Model** (`flikk-models/src/depth/depth_anything/`): Depth Anything V2
  Small (DINOv2 ViT-S and a DPT head; Apache-2.0, the Base and Large
  weights being CC-BY-NC), as onnx-community's `depth-anything-v2-small`
  export in full precision (99 MB, `onnx/model.onnx`), pinned to a commit
  (`FLIKK_DEPTH_MODEL` points at another copy). Pictures are decoded
  640 wide, resampled bilinearly to 518 on the longer side, both sides a
  multiple of 14 (518 × 294 for 16:9), and normalized as ImageNet; out
  comes relative disparity at that size. The model was trained at 518 on
  the shorter side; the longer side at 518 costs a third as much on the
  CPU.
* **Speed**, measured here (4 CPU cores, ONNX Runtime's CPU provider):
  0.55–0.58 s a frame at 518 × 392 (a 4:3 picture), 0.40 s at 518 × 294
  (16:9; with Python's ONNX Runtime on the same graph), the first run
  1.4 s with loading; 62 frames of a 640 × 480 clip in 35.6 s through the
  service. So a second of 30 fps footage takes 12–18 s.
* **Service** (`flikk-analysis/src/depth.rs`, `LocalDepth`): frames come from
  the media engine (the proxy when there is one), numbered on the media's
  rate (an image is one frame). Each frame's disparity is spread over its
  own 2nd to 98th percentile (the model's answer has an unknown scale and
  shift per frame), turned into distance and kept at 384 on the longer side
  as bytes (about 80 KB a frame) under `cache/depth/` and in memory. The
  map shown for a frame is smoothed over time with the two frames either
  side: weighted by how near they are (σ 1.2 frames) and, pixel by pixel,
  by how well each agrees once fitted onto this frame by least squares
  (σ 0.08), so moving things keep this frame's depth while the range
  steadies. With every neighbour made the result is fixed, so a render
  (which makes them all first) matches the viewer. Asking for a frame
  queues it, the next two seconds and the frames before it smoothing reads,
  ahead of what was queued, on one worker thread; meanwhile the nearest
  made frame within 12 stands in. A failed frame is not tried again in the
  session. Settings → Storage clears the maps with the Cache.

## 8. Agent

* **Backends** (`flikk-agent/src/backend/`): Claude runs as
  `claude -p --input-format stream-json --output-format stream-json` with
  only the flikk MCP server (`--strict-mcp-config`, `--tools ""`,
  `--permission-mode dontAsk`, no setting sources or slash commands), the
  system prompt, `--model`/`--effort`, and `--resume` or `--session-id`.
  Codex runs as `codex exec --json --sandbox read-only` with the user's
  config and rules ignored, shell and patch tools off, and the MCP server,
  developer instructions, model and reasoning effort as `-c` overrides,
  resuming its thread. Each turn runs in an empty workdir under
  `agent/work`, so the user's own CLAUDE.md/AGENTS.md are not picked up. A
  backend that lost its session is retried fresh. `FLIKK_CLAUDE` and
  `FLIKK_CODEX` point at the CLIs.
* **Models and thinking:** two pickers next to Send list what the installed
  CLI offers (Claude's stream-JSON `initialize` request, Codex's
  `model/list` on `codex app-server`), cached in `agent/models.json` and
  refreshed in the background; each model brings its thinking levels.
  Choices are kept per backend in the user settings.
* **MCP server:** `flikk_agent::mcp::run()`, launched by the backend as
  `$FLIKK_MCP`, else the `flikk-mcp` binary next to the editor, else
  `flikk mcp`. It connects to the editor over TCP on 127.0.0.1 with the
  turn's token (`FLIKK_BRIDGE`/`FLIKK_TOKEN`).
* **Permissions** (`flikk-core/src/bus.rs`): one agent role, the editor.
  Permissions are per `ToolCategory`: read and view are always allowed;
  edit categories are Off or Propose (never auto-applied); skills and
  styles are Off, Ask or Allow (deleting one always asks); project memory
  (rules, facts, saving to the library) and deliver (renders, exports) always need the
  user's approval, asked with Approve/Deny cards above the composer and
  carried out by `agent_link/approve.rs`. The call waits: the host holds
  its MCP reply until the user decides and answers it with the outcome
  (a generated voiceover's media id, for one), so the turn carries on; an
  outcome whose call is gone (the turn ended or was cancelled) reaches the
  agent in the next turn's `<approvals>` block. Backends allow a tool call
  an hour for this. Generate media (music, sound effects, voiceovers and
  voices, pictures, videos and upscales: whatever spends the user's money
  or credits with a provider, `ToolCategory::costs`) is Off or Ask each
  time (the default). No setting lets it run unasked: a saved "Allow" from
  before reads as asking (`Permissions::get`), the handlers that ran one as
  a tool job are gone, and the only code that calls a provider for the
  agent is `agent_link/approve.rs`, on the user's click. `Permissions::choices`
  says what each category can be set to, and Settings → Agent draws its
  rows from it. A "Web" switch (off by default) gives Claude WebSearch/WebFetch and Codex web
  search. Both read pages as text only, so the same switch lists the `flikk`
  tool `view_web_image`: it downloads a PNG, JPEG, GIF or WebP (at most
  25 MB), returns it scaled to 1568 px on its long side, and with `add` keeps
  the original under `paths::web()` and adds it to the pool's "Web Images"
  bin as the user's edit. The `flikk` tool `generate_image` (Generate
  category, so it asks each time) is
  offered while someone can make the picture (`image::route`): the image
  provider picked in Settings → Integrations when one is set with a key,
  whatever the agent's own setting says (see "Generating pictures"), else
  the user's ChatGPT plan. The "Image generation" switch (off by default,
  either backend) is for the ChatGPT plan only: it turns on, and the tool
  and its prompt section are offered that way, only while Codex's saved
  sign-in carries a paid ChatGPT plan (`chatgpt_plan_type` in its ID token,
  not `free`); the switch is dimmed with a tooltip otherwise, and dimmed
  with the provider and model in its tooltip while an image provider is
  set, keeping its value for when it is not. The ChatGPT plan makes
  pictures from a description only. It sends the request Codex's own image tool
  sends (`POST chatgpt.com/backend-api/codex/images/generations`: the
  prompt, `gpt-image-2`, automatic size and quality, opaque or transparent
  background) with Codex's ChatGPT sign-in from `$CODEX_HOME/auth.json`
  (bearer token and `ChatGPT-Account-ID`, `originator: flikk`). Flikk never
  refreshes or writes that login: an expired one asks the user to open
  Codex, and plans without image generation (free) answer 403. The picture
  goes to `paths::generated()` and the pool's "Images Generated" bin, as the
  user's edit. Codex turns run with its own `features.image_generation`
  off, so there is one image tool. Web calls show as `web_search` and
  `web_fetch`, read from each backend's stream, whatever the backend calls
  them.
* **Tools** (`flikk-agent/src/tools/`): 106 in the catalog (104 plus
  `load_tools` and `run_tool`). Remove, move and detail operations are flags
  of their `set_*`/`view_*` sibling (for example `view_range` with `detail`).
  Each turn lists 26 core tools (`load_tools` among them) plus `run_tool`, and the groups (`edit`,
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
  `view_media` (`tools/pool.rs`) sees a pool item on its own, placed or
  not, in its own seconds: it puts the item alone on a scratch timeline at
  its own size and frame rate, from its first frame, so the views above
  run on it unchanged (a video's collage and script, with `detail`; one
  frame with `t` and `zoom` in its pixels; a picture whole or zoomed; with
  `audio`, or for audio, the whole script and a spectrogram up to 30 s).
  An item whose analysis is missing is analysed first (`analyze`, as the
  pool's Analyze) and the call waits on that job, stopping with the turn;
  `list_media` says which items are analysed.
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
  subjects for objects and one person among several, and to check with
  `view_frame` `masks`.
* **Person masks:** `remove_background` (`tools/masks.rs`, transform
  group) gives each of `clips` a person mask, or changes its own: `keep:
  background` inverts it, `edge`, `feather` and `expansion` set its edge,
  `restore` takes the clips' person masks away. `set_mask` takes
  `shape.type: person` (with `edge`) too; `get_clip` says how many frames
  are matted, and `view_frame` names person masks not matted there yet.
* **Speed ramps:** `set_speed` (`tools/speed.rs`) takes one `speed`, a
  `ramp` of [position 0..1, speed] points or a `preset` (`ramp_in`,
  `ramp_out`, `hero`, `bullet`, `montage`, at `speed` or the clip's),
  `keep_length`, `interpolation` (`nearest`, `blend`, `optical_flow`) and
  `ramp_audio` (`stretch`, `mute`); `speed` alone on a ramped clip ends the
  ramp. `get_clip` reports a ramp as its points. The tool retimes one
  clip; a video's linked audio clip is retimed by its own call (the
  inspector ramps both).
* **Depth:** `add_depth_effect` (`tools/depth.rs`) puts a `lens_blur`,
  `fog` or `camera` on clips and says when depth maps cannot be made yet;
  `set_depth_focus` focuses a clip's lens blur (or a camera's pivot) on
  what is at a point of the picture at `at`: it makes that frame's map off
  the UI thread, then proposes the focus, keyed at `at` with `key` or when
  it already has keyframes. Depth masks are `set_mask`'s `depth` shape.
* **Generating audio:** `generate_audio` (`tools/generate.rs`) makes music
  (a prompt and a length, or `sections` with lengths of their own, so it
  changes on cue), a sound effect (optionally looping) or a
  voiceover (a script of up to 10,000 characters, one voice from
  `find_voices`, its stability and similarity) with the provider picked
  for it. The file goes to `paths::generated()` with a `<file>.json` of
  what was asked, and into the pool's "Music Generated", "Sound Effects
  Generated" or "Voiceover Generated" bin as the user's edit (`CommandBus::execute_shared_as`, which also adds it to the
  drafts waiting for review), so a rejected placement keeps what was paid
  for; `MediaItem::generated` records the provider, model and prompt, and
  for a voiceover who read it, so the pool can regenerate it. With
  `at` or `range` it is proposed on an audio track free over its length,
  else a new one. The approval runs it in the background, proposes the
  placement into the chat's set and answers the waiting call with the
  media id. A
  refused prompt (an artist's name) comes back with the provider's rewrite.
* **Music and sound effect providers:** ElevenLabs (its own API), fal and
  Pika. On fal, music: ElevenLabs Music v2.5 (the default), Sonilo 1.1,
  Lyria 3.5, Lyria 3 Pro, Stable Audio 3; sound effects: ElevenLabs Sound
  Effects v3 (the default), Sonilo 1.1, Mirelo SFX 2.0, Stable Audio 3
  SFX. On Pika, music: Pika Music (the default), Sonilo 1.1, MiniMax Music
  3.0, Lyria 3.5, Lyria 3 Pro; sound effects: Pika SFX (the default),
  Sonilo 1.1, ElevenLabs Sound Effects v2. Each model's
  `cloud::AudioModel` says the lengths it makes (a range and a step, or
  none when it picks the length itself) and whether it loops; the media
  pool's Generate dialog offers only those (no Length row for a model that
  picks, no Loop switch for one that can't), and `AudioModel::check`
  refuses the rest before anything is spent: in `Cloud::generate`, and
  before the agent's request waits for approval. A hosted model is a
  `HostedAudio` entry in `fal/audio.rs` or `pika/audio.rs` naming its
  fields; `audio.rs` writes a request in them, and asks in words for what
  a model has no field for: no vocals ("Instrumental only, no vocals."),
  and sections as timed lines in the prompt (`0:00–0:12 Intro: soft
  piano`) with their lyrics, or in a lyrics field as `[Name]` blocks. An
  ElevenLabs model on fal gets the same composition plan as ElevenLabs'
  own API, and Sonilo on Pika gets `segments`. fal runs them on its queue,
  Pika as media jobs, and `audio::fetch` names the file by what it is
  (WAV, FLAC, M4A, Ogg, else MP3). Mirelo is sent `num_samples: 1`: it
  makes, and bills, two takes by default.
* **Generating video:** `generate_video` (`tools/video.rs`) makes a shot
  with the provider and model picked for video (on fal: Seedance 2.0, 2.0
  Fast and 2.5, Kling 4.0 and O3 Pro, MiniMax H3 Max, Wan 3.0, Happy Horse
  1.1, Gemini Omni Flash 1.1, FLUX 3 Max, LTX 2.5 Pro; on Pika: Pika H3
  Flex and Pika 2.5, and Seedance, Kling, MiniMax H3, Hailuo, Wan, Happy
  Horse, Gemini Omni Flash, FLUX and Grok Imagine as Pika hosts them): from the prompt alone, from a first frame (a picture in the pool,
  or the program at a time, grabbed with `compose::program` into
  `paths::cache()/frames` just before) to an optional last one, or after
  references (pictures, clips and sounds in the pool, which the prompt names
  `@Image1`, `@Video1`, `@Audio1`). The agent writes the prompt as a brief
  (`catalog::VIDEO_PROMPT`: what each reference is for, the clip in a
  sentence, timed shots of one action and one camera move, the sound, and
  limits last), the shape video models follow best. Each model's `cloud::VideoModel` (its
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
  a generated item reaches the pool (the media pool or an approval),
  `analyze_new` queues it once like any new item (see §7): a video is
  asked about first while a provider is picked to transcribe; a
  voiceover, whose script stands in for its words, never is, nor are
  music and sound effects, which are heard only.
  Items already in a project when it opens are left as they were. Settings → Integrations has a "Video sound"
  switch under Video (`CloudSettings::silent_video`; sound costs more on
  some models): the media pool's form starts its Sound switch from it, and
  with it off the agent's videos are made silent whatever it asks. Each
  model's `cloud::VideoSound` says whether it makes sound at all, always
  (no switch: Pika H3 Flex, Kling 4.0, Kling 3.0 Turbo on Pika), or on a
  switch; only the last are made silent. Beside it, a resolution
  (`CloudSettings::video_resolution`: Auto, 480p, 720p, 1080p, 2K, 4K):
  `VideoModel::resolution_for` fits it to the model picked (the largest of
  its own at or below it, else its smallest), the media pool's form starts
  on it, and the agent's videos use it when they ask for none. Auto leaves
  the model's default.
* **Blockouts** (`flikk-core/src/blockout/`, `tools/video.rs`,
  `reblock.rs`, `ui/blockout*`, `ui/chat/shot.rs`): video generation is
  expensive and each take invents camera, staging and timing again, so
  `generate_video` takes a `blockout`: a rough 3D scene the agent writes
  as JSON (`blockout::Scene`: objects, models, a camera or several with
  the cuts between them, an optional sun direction; metres, Y up, the ground at `y = 0`, at most 30 s). Nothing
  in it is ready-made: there are no built-in people, props or actions,
  so the agent is not held to what Flikk thought of. An object is a
  plain solid (box, sphere, cylinder, cone, capsule, dome, wedge, roof,
  pyramid), a joint with no shape that only carries others, or a copy of
  a model. Things are built by hanging solids from one another
  (`parent`: a part's place and turn are in its parent's space) at the
  point they turn about (`anchor`: base, middle, or top for a limb hung
  from its joint). They move by keys, for what happens once, and by a
  `Cycle`, for what repeats: a few keys over one round, added to what
  the keys make of the object, running by the clock (`seconds`), by the
  ground covered (`metres`: the round follows how far the thing the part
  belongs to has travelled, so a wheel keeps up with the road at any
  speed) or by the strides its thing's feet have taken (`strides`: arms
  in time with legs); a round that returns to its start eases to rest
  when the thing stands still, and `from` and `until` have a cycle go on
  between two times only, in and out over 0.3 s. A thing built once goes under
  `models` and is placed with `model`; a copy can be scaled and shaded,
  and its `parts` give, by part name, the fields that replace the
  model's (another pose, no walk). `build.rs` writes every copy out into
  `Part`s named `copy/part`, swaps names for places in the list, works
  out how each part's keys move it (`motion::Moves`) and the order parts
  are placed in, gives each joint the box its parts fill, and tabulates
  the ground each moving thing has covered. An object may also face
  something about the vertical (`face`: the way it travels, the camera,
  an object or a point), keep spinning, come and go (`hidden`), shake
  without repeating (`shake`: metres and degrees either way, and about
  how long a wobble takes), and say what it stands on (`on`: the ground
  or an object by name). A camera has a place, a `look_at` point or
  object, a width (`lens` in millimetres on a frame 36 mm along its
  longer side, as Blender fits a sensor, or `fov` in degrees), a roll, a
  hand-held `shake`, and may ride an object. The shot opens on `camera`;
  `cameras` holds more by name and `cuts` says when the picture goes
  over to which (`main` is the first), each camera moving on whether or
  not the picture is its own. `build.rs` turns each into a `motion::Rig`,
  the names it looks at and rides swapped for places among the parts.
  A camera, or a key of one, may say what its picture holds in place of
  where it stands (`frame`: the objects, a shot size from `close_up` to
  `very_wide`, and `left`, `centre` or `right`). It then keeps the way
  it looks from its `at` and stands as far off as fits the box of the
  solids that reach into the share of the subjects' height the size
  keeps (`motion::framed_from`, the fit Blender's camera-to-selected
  makes): the top 17% filling three quarters of the picture's height
  for a close-up, all of it filling a quarter for a very wide. That
  depends on the picture's shape, so it is worked out as each frame is
  drawn (`Scene::stood`), with things standing as they do when the
  camera or the key gets there, and a third is reached by looking a
  third of half the picture's width to the other side.
  Everything keys move follows a smooth curve through them
  (`motion::Track`), place, turn, size, grey, and the camera's field of
  view, roll and what it looks at: where a value turns back or rests it
  stops, between stops its pace changes least from key to key (a
  tridiagonal solve, as Blender's animation curves place their handles),
  and each pace is held within three times the straight one either
  side, so the curve never goes past a key. Two keys give a straight
  move. An ease times one stretch of the curve (linear, in, out, in-out,
  `back`, `bounce`) and `cut` holds and then jumps, ending one curve and
  starting the next (a camera's cut to another set-up). A key with a
  place and no time is passed on the way: `motion::key_times` spreads
  such keys by the ground between them, between the timed keys either
  side, so an orbit or a route is a list of places. Angles are taken as
  written (a wheel turns from 0 to 360); the review says when a turn of
  270 degrees or more stops short of a whole one. Shake, the camera's
  and an object's, is fractal noise: smooth noise at three rates, each
  twice as quick and half as strong as the last, seeded for an object
  by its name.
  At a time, `Scene::moment` places every part once, in the order built
  (`build::placing_order`): each after what it rides, and after what it
  faces and stands on. Where that goes round in a loop (two people
  facing each other) the part placed first turns to where the other
  stands before anything faces, which a first, rough pass gives. A part
  with `on` is set down: its solids are put where its limbs have them,
  the lowest point of them is found (`Solid::lowest`) and a ray is cast
  down from over it onto the solids of what it stands on, the ground
  where it meets none, so a walker's feet stay on a ramp or a stage and
  the height written is how far above that it is. `solid.rs` has the
  solids: where a ray meets each, its lowest point and whether a point
  is inside it.
  A part's front can be turned to something (`aim`: an object, the
  camera, a point; a key retargets it): the least turn that puts its +Z
  on the target, no more than 85 degrees from how it would else face. A
  part can be the end of a limb brought somewhere (`reach`: `to`, and
  optionally `from` and `until`): the part it rides and the part that
  one rides are the limb (`build::Limb`, hung on the upper of the two),
  and `Scene::bend` solves it in closed form each frame from the two
  lengths, bent the way its pose has it bent (a limb posed straight
  bends forward when it hangs, back when held out), each part turned
  the least that puts the joint and the end in place; out of reach it
  goes straight toward the target, and it moves there and back over
  0.35 s. A foot can step (`step`): `Scene::planted` says where it is
  from the strides its thing has taken by then (`build::strides`,
  tabulated a frame: a stride is as long as legs of that length take at
  the speed, 2.3 leg lengths times the 0.3rd power of speed squared
  over gravity and leg length, unless `metres` gives it). It is down
  where the thing passes over it for three quarters of a leg's length
  of ground, at most 0.6 of the stride, so a run has both feet in the
  air, and swings to its next landing in between, lifted an eighth of
  the leg; standing still it is as posed. Its leg is bent to it as a
  limb is, and a thing with `on` is held no higher than its grounded
  legs reach (`Scene::lift`), which is the rise and fall of a walk.
  Because a landing is found from a count of strides, any frame is
  still drawn without the ones before it.
  A key with `parent` gives its object to another from that second on,
  from where it is at that moment (`"none"` lets it go where it is):
  `build::take_hold` works out, in order of time, where each is within
  what takes it, the scene built as far as the one before (Blender's
  Child Of with its inverse set). What is held is placed last each
  frame, with all that rides it, and a limb that has taken what it
  reached for leaves for where it took it from, so reaching and holding
  never wait on each other.
  `world::World` is the scene at a moment as rays meet it: the solids
  where they stand, a ray from the camera trying only those listed for
  its 16 px tile of the picture, a ray toward the sun only those listed
  for its cell of a grid across the light (`world::Cells`, each solid's
  cells widened by as far as a ray toward the sun's edge strays), and
  rays any other way a tree of boxes (`world::Solids`). All three are
  rebuilt each frame. `Scene::render` (`render.rs`) makes the picture on
  the CPU: one ray a pixel, lit by the sun, the sky from above, the
  ground from below and a fill from across the sun that reaches round
  what turns from it, with metre tiles on the ground and haze. Light is
  worked out as light and turned into screen greys at the end, pixels
  between two greys scattered so gradients show no steps. Every picture
  has lines, from which solid each pixel shows: darker than both sides,
  a pixel wide on the side away from the camera, strong round a thing
  and light between its parts, and a solid's own edges a little lighter
  where they stick out. `Detail::Full` also softens shadows (one ray a
  pixel toward a spot on the sun's disc, seven more where the pixels
  around disagree, evened out over 5 px on the same surface), darkens
  surfaces where the picture shows another close (eight looks along a
  spiral within 0.7 m at every fourth pixel, shared with the pixels
  around on the same surface, round solids too), and draws pixels an
  edge runs through again from four rays, the line taking the share of
  the pixel the nearer solid leaves and the rest of its width in the
  pixel outside. `Detail::Draft` is what plays. The values kept a pixel
  on the way (about 50 bytes in full) stay allocated from one picture to
  the next (`render::Sheets`). With no GPU and no model, a 1280×720
  frame of the demo shot takes about 36 ms as a draft and 85 ms in full
  on four cores.
  Larger worlds are written short. A box, a cylinder or a capsule takes
  a `taper` (its top against its base, inside the same size): cylinder,
  cone and capsule share one ray test for a round side whose radius
  changes with height. `repeat` (`repeat.rs`) lays one object out as a
  row or a grid (`step`), a `ring` or a scatter over an `area` (each
  place the candidate, of twelve, furthest from those before it), every
  copy the whole object with its keys moved over, called `name 1` and
  on and riding a joint of the object's name that stands for them all;
  `vary` turns, scales, shades and puts each out of step by a hash of
  the name and the copy's number. A solid with `cut` is not drawn but
  taken out of the object it names: every solid is convex, so a ray is
  inside one over a single stretch (`Solid::span`, the ordinary test
  from outside each way), and a ray that meets a cut solid in a hole
  goes on to where it leaves the hole, the hole's far side turned round
  being the surface seen. `hills` (`land.rs`) makes the ground roll:
  smooth noise on a 256 × 256 grid that repeats beyond the haze, walked
  by rays in strides four times what its steepest slope makes sure is
  clear and met between the last two places; every height is then above
  the ground under that thing (`Part::lands`, cameras and points too),
  so scenes are written as on a level floor. A scene holds 6000 parts.
  What made more slow was measured: the tree of boxes is rebuilt in
  under 3 ms at 13 000 parts, so it still is each frame; parts nothing
  ever moves are placed once (`Part::still`), and the review sets its
  moments up on several threads.
  Nothing is simulated, but what is let go falls (`flight.rs`): a key
  with `"parent": "none"` leaves a thing with the speed it was carried
  at over the frame before, and a key with `throw` gives it the speed
  that brings it to an object, the camera or a point, after the seconds
  it says or those of the throw that takes the least speed, aimed at
  where a moving target will be. From there its place at any time is
  its speed and gravity, so a frame still draws on its own. Where it
  comes to rest is found once at build, frame by frame, as its lowest
  point comes down on a solid that is not its own or its thrower's, or
  on the ground; thrown to an object it is caught, a hold by that object
  timed by the flight. The scene's `gravity`, or an object's own in its
  place, says how strongly things fall against the Earth's pull: at 0 it goes on in a straight line or stays
  where it was let go at rest, under 0 it rises. The review says when each lands or is caught,
  and doubts a throw over 30 m/s or one still in the air at the end.
  `Scene::render_outside` draws the scene from outside its shot
  (`Outside`: around, above, how near), toward the middle of a ball that
  holds the scene over its whole length, with the shot's camera, its
  move and the way each travelling thing goes, a tick a second, stroked
  over the picture against its depth. `Scene::render_pass` draws the
  shot as its depth, its lines or each thing in a colour of its own
  (`Pass`), and `Scene::thing_at` names what a place in the shot shows.
  Passes are shown, not sent: whether a video model follows one better
  than the greys is untested.
  `Scene::write_clip` pipes full frames to FFmpeg as a 24 fps H.264 MP4,
  1280 px on the longer side. `Scene::review` says a scene in words for
  a writer that can't watch it: what the camera sees at the start, the
  middle, the end and after each cut (which things, where across the
  picture, how tall, hidden behind what: found from spots on each thing
  and from a spread of rays through the picture), what looks like a
  mistake a viewer would see (`doubts`: the subject out of the opening
  picture, a walker going sideways, a turn the long way round) and what
  could be tidied (`notes`: a thing never in
  the picture, a key after the end, a camera never cut to). Between the
  moments it describes it watches the whole scene twelve times a second
  (`review::Watch`) and adds to the doubts: the camera's subject hidden
  or out of the picture, from when until when; the camera inside an
  object or under the ground; one thing going through another (the
  middle of a solid inside a solid of the other, the two moving past
  each other); something on stepping feet faster than 12.5 m/s; a
  walker with its feet off the ground and no `on`; a limb short of what
  it reaches for. What the camera looks at is named as a shot is called
  from its height in the picture. A thing is what a viewer would
  name: what stands by itself, and each copy, not their parts; the
  copies of a repeat are said as one, with how many of them show.
  `preview_blockout` (the Generate group, a read) gives the agent that
  text with a collage of eight frames and the set from straight above
  with the camera drawn in, so it looks at a scene and fixes it before
  it asks for the video.
  A scene is long to write and quick to change, so it is written whole
  only once. `Scene::changed` (`change.rs`) makes a change that says
  only what differs: the scene's JSON with `objects`, and each model's
  parts, held by name; a name that is there has the fields given put in
  place of its own, a new name is added, `null` removes. Every blockout
  is kept on disk under a short id (`tools/drafts.rs`, the newest 40 in
  `paths::cache()/blockouts`): `preview_blockout` keeps a scene given as
  `blockout` and from then on takes `id` and `changes`, `generate_video`
  takes `blockout_id`, and the request's summary names the id, so the
  outcome the agent is told carries it. What the user changes on the
  card goes into the same kept scene (`Blockout::with_scene`), and it
  stays after they approve or deny, so another take, or the next try
  after a denial, starts from the scene as it was left. The backend runs
  that change a scene in the dialog, and the second look at a drafted
  one, answer with a change too.
  A `video::Shot` is the request with what it is made from: the
  `GenerateRequest::Video` as the user reads it, the frames to grab, the
  placement, and the `Blockout` (scene, clip path under
  `paths::cache()/upload`, clip size). Models follow references or start
  on a frame, not both, so `Shot::sent` lowers it for the provider: the
  blockout's clip is the first clip followed (`@Video1`) and a first
  frame the first picture (`@Image1`); `parse` adds a line to the prompt
  saying what each is unless the prompt names them. `Shot::check` refuses
  what the model can't make (a model that follows no clips, a last
  frame), and `Shot::prepare` grabs the frames and writes the clip just
  before the provider is called, in the approval's job or the tool's.
  The request waiting on the user is a card of its own (`ShotCard`, one
  entity per waiting `generate_video`, kept by `ChatPanel::sync_shots`):
  the blockout playing in a loop (`BlockoutPlayer`: draft frames drawn
  off the UI thread, the newest wanted next, narrower while drawing
  can't keep up, and the frame it stops on drawn again in full), a tile for the first frame and each reference with the tag the
  prompt calls it, and the prompt as it is sent, in a field. A tile's menu
  fills it with a picture made for it (`EditorState::generate_then`, the
  user's own generation: it runs as a row of the media pool, lands in the
  Images Generated bin and fills the tile; an empty first frame beside a
  blockout is made after the blockout's opening frame, a filled tile
  after its picture), the program's frame at the playhead (grabbed at
  once) or a pool item, or empties it. Frames the agent named by time are
  grabbed when the card appears, so the tiles show what is sent. Every
  change is written into the waiting request
  (`AgentHost::revise_approval`), so Approve makes what the card shows;
  it is off while the model can't make the shot, with why beside it.
  A request without a blockout has a Blockout tile whose button has one
  written, for when the agent judged it not worth one, where the model
  picked follows clips: `Reblock::draft` is the same kind of backend run,
  given the shot's prompt without its role lines (`Shot::look`), its
  length and its shape. A first draft whose review has doubts is handed
  back once with the review to be put right, and stands if the second
  answer has no scene. What it answers becomes the shot's blockout, and
  an answer without a scene shows as why on the tile. Whenever a blockout
  or a first frame beside one comes or goes on the card,
  `Shot::settle_roles` puts the lines saying what `@Video1` and `@Image1`
  are into the prompt, or takes its own lines out again. Tiles have one
  size whatever the video's shape; the player paints its frame fitted
  inside its box (`paint_image` on a canvas: an image element takes its
  height from its width, so an upright frame overflowed a wide box).
  Clicking the blockout opens its dialog (`blockout::editor`): the scene
  large with play, seek and Undo, and a thread where the user says what
  should change. Above the picture the player's `Looking` is chosen:
  the shot, the set from outside (dragged round, the wheel bringing it
  nearer), or the shot's depth, lines or parts; a button lays the thirds
  over the shot, and a click on a thing in the shot puts its name into
  the ask (`Picked`). Each ask is one backend run without tools
  (`reblock::Reblock`: `TurnSpec::mcp` is `None`, so Claude gets an empty
  MCP config and no allowed tools, Codex no MCP server) given the scene,
  its review, the earlier asks and the format's schema; it answers with
  a sentence
  and a change to the scene, read from the reply's outermost braces and
  made by `Scene::changed`. Save puts the scene into the shot, and into
  the kept blockout, under a new clip path. Nothing in the dialog reaches a video provider.
* **Generating pictures:** `Capability::Image` is a row in Settings →
  Integrations like video, with fal and Pika as providers. On fal: Nano
  Banana 2.1 (the default) and Pro, GPT Image 2, GPT Image 2.6 Flare and
  Sunburst, FLUX 3 and FLUX 3 Klein, Seedream 5.0 Pro, Ideogram 4.5, Grok
  Imagine Image 2.0, Qwen Image 3, Krea 3 and Recraft V4.1 Pro. On Pika:
  Nano Banana 2.1 (the default) and Pro, GPT Image 2, 2.6 Flare and 2.6
  Sunburst, Seedream 5.0 Pro and Lite, Ideogram 4.5, Grok Imagine Image
  2.0, Muse Image and Recraft 4.1. A `GenerateRequest::Image` carries a
  prompt, an optional shape and resolution, a transparent background, and
  the pictures it edits or follows (`ImageMode::Edit` when there are any:
  image to image), which the prompt names `@Image1`, `@Image2` and the
  models read as "image 1". Each model's `cloud::ImageModel` (shapes,
  whether it can pick one, resolutions, how many pictures it follows,
  transparency) refuses what it can't make before anything is spent. Each
  model is an entry in `fal/models.rs` or `pika/models.rs` (`IMAGE`)
  saying how it takes a request: `aspect_ratio` and `resolution`, or fal's
  named `image_size` (only the five ratios fal names), or Pika's `size` in
  pixels (Ideogram), or no shape at all (Muse, Recraft on Pika); the
  pictures it follows in
  `image_urls`, or one to edit plus `reference_image_urls` (Ideogram on
  fal). fal runs it on its queue (`fal/image.rs`), Pika as a media job
  (`pika/image.rs`), both after uploading the pictures as for video; the
  file comes back as `images[0]`, and `picture::fetch` names it by what
  its first bytes say it is. It lands in the "Images Generated" bin with a
  `<file>.json`, and is not analyzed. The media pool's Image form (the
  Generate menu, or "Generate image…" on a picture, which it then edits)
  takes the prompt, pictures in the pool and the frame at the playhead
  (grabbed when it is made), the model's shapes (starting on the
  sequence's), resolutions and transparency. With a provider picked, the
  agent's `generate_image` makes the same request through it, whatever the
  agent's "Image generation" switch says, taking `aspect`, `resolution`,
  `references` (pictures in the pool) and `frames_at` (the program at
  those times, grabbed just before); the approval carries which way it
  goes (`via`), so an older ChatGPT request still reads. The agent writes
  the prompt as `catalog::IMAGE_PROMPT` lays out (a few sentences with
  what the picture is and its subject first, words in it quoted, an edit
  as the change and then what stays), the shape image models follow best.
  The shot card's first-frame form starts from the shot's prompt, told to
  show only its opening instant.
* **Upscaling video:** `Capability::Upscale` is a row in Settings →
  Integrations like video. On fal: Topaz Precision (Proteus, the default)
  and Starlight, SeedVR2, Crystal, FLUX Video Upscale and Gemini Omni
  Flash 1.2. On Pika: Topaz Proteus (the default), Starlight Precise and
  Starlight Fast, and Pika H3 Flex's own upscale (`draft-to-video`, for
  sources up to 768p). A `GenerateRequest::Upscale` carries the source
  (`cloud::UpscaleSource`: its path, name, size, length and whether it has
  sound) and a resolution ("1080p", "2k", "4k": the lines on the short
  side), or none for the model's default (1080p where it can, else its
  smallest). Each model's `cloud::UpscaleModel` (its resolutions, how it
  is told how much larger: the resolution itself, a factor in a range, or
  whole steps such as Topaz's ×2, ×3, ×4 on Pika, the longest and largest
  source it takes, the most pixels it makes) works out the factor and
  refuses what it can't make before anything is spent. Each model is an
  entry in `fal/upscale.rs` or `pika/upscale.rs` with its endpoint and a
  function writing its input; Pika's uploads stop at 50 MB, which is
  checked first. The source is uploaded as for video and the job followed
  the same way. Upscalers drop or encode the sound again, so `upscaled.rs`
  puts the picture made together with the source's own sound (copied as it
  is, else as AAC) into `<stem>.mov`, keeping the provider's file when
  FFmpeg can't. It lands in the "Video Upscaled" bin, named
  `upscale-<source name>`, and is analyzed like footage. In the media pool,
  "Upscale…" on a video (`media_pool/upscale.rs`) offers the resolutions
  the model can make it at (`UpscaleModel::targets_for`, starting on its
  default) and, when a clip shows it, "Replace clips":
  `EditorState::upscale` then runs `Command::SwapMedia` once the file
  lands, as an undo step of its own. `SwapMedia { from, to }` points every
  clip of `from` on an unlocked picture track at `to`, at the same source
  times; sound clips keep `from`, whose sound the upscaled video carries
  anyway.
  The agent's `upscale_video` (`tools/upscale.rs`, Generate) takes a pool
  video, a resolution (default: the model's default for it) and `replace`,
  and is approved like `generate_video`. Where a generated file goes once
  in the pool is a `generate::Destination`: the pool only, a placement on
  the timeline, or `Replace(media)`, which proposes `SwapMedia` in the
  agent's change set for review.
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
  `flikk_analysis::cut_speech`): to clone it, or to find shared voices that
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
  so approved each time) add or design one and keep it in the library.
  Their approval cards are heard first: `find_voices` remembers where each
  voice listed can be heard and the request carries it, so the card of an
  `add_voice` or a voiceover plays the voice's sample (the library's copy
  when it has one). Approving a design opens the Voices dialog on the
  agent's description, designing; the user hears the takes and keeps one
  (or any other voice), which settles the request, and closing the dialog
  leaves it waiting. Cloning is not a tool: it needs the speaker's consent, so the prompt
  sends the user to the dialog.
* **Fixing words:** in the transcript, Retype… (the selection's menu, or
  Enter) turns the selected words into an editor; Enter or the mic button
  says what is typed in the speaker's voice (`ui/transcript/retype.rs`,
  `state/respeak.rs`). The voice is the library voice linked to that
  speaker (`SavedVoice::speakers`: fingerprint and speaker id); without
  one, the Voices dialog opens on cloning theirs from what they say in the
  recording, and the voice kept there, cloned or found, is linked and
  used. The user's click is their leave to spend the credits; the request
  runs as a row of the media pool, which shows it fail, and lands as the
  user's edits (the item in the Voiceover Generated bin, then the cut).
  The agent's `replace_words` (`tools/respeak.rs`, Generate) takes the
  words by the times they start (`from`, `to`, checked against `words`),
  the new `text` and, without a linked voice, a `voice` from
  `find_voices`; it can't clone. The approval card plays the voice;
  approving makes the words off the UI thread
  (`approve::Started::Respeak`), adds them to the pool and proposes the
  `ReplaceSpeech` in the chat's set.
* **Finding sounds:** `find_sounds` (`tools/sounds.rs`) searches the user's
  library and the project's audio by words (name, category, tags, what a
  provider was asked) and, where analysis left vectors, by how they sound;
  it says how many had none. `use_library_sound` adds a sound to the pool's
  "Library" bin as the user's edit, like generated audio, and with `at`
  proposes it the same way. `save_sound_to_library` waits for approval.
  The media pool's ♫ menu makes the same from a form (Music runs as long as
  In/Out).
* **Prompt** (`flikk-agent/src/prompt.rs`): the system prompt is the tool
  guidance (with or without web and images), the shared rules (seeing and hearing,
  time formats, references, cutting in pauses, protected ranges), the
  editor's part, the FX conventions and the language. Each message carries an `<editor_context>` (project
  summary, format, workspace, playhead, selection, in/out, selected words),
  the approved style rules, the picked style as a `<style>` block, a skill
  line, `<approvals>`, and a
  `<references>` block when the user referenced objects.
* **Chat** (`ui/chat*`): sessions per project in
  `paths::projects()/<project>.chats.json`, each resuming its own backend
  conversation (`agent/sessions.json`). Tool calls stack into one card per
  run. A call that runs off-thread (a `flikk` job, or the backend's own web
  search and fetch) shows with a spinner from `AgentEvent::ToolStarted`
  until its `ToolFinished` replaces it; one still running when the turn ends
  is marked Stopped. The change-set card sits above the composer, once per set however
  many turns grew it, and stays with its outcome until the next message;
  it has Preview, Review (a checklist with ticks), Reject and Accept. `ask_user` shows up to six choices. Skills run as
  `/name …`, the rest of the message being their input; none are built
  in. Skills and styles are Markdown files under `paths::skills()` and
  `paths::styles()` (`flikk-core/src/skills.rs`), managed in Settings and by
  the agent; the style picker sits beside send. cmd/ctrl-K opens the quick
  command bar.
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
claude mcp add flikk -- flikk mcp          # Claude Code
```

or for Codex, in `~/.codex/config.toml`:

```toml
[mcp_servers.flikk]
command = "flikk"
args = ["mcp"]
```

External calls act with the agent's current role and permissions. Their
edits land in an "External agent" change set that goes to review when the
client disconnects or after 90 s without a call; `ask_user` is not
available to them.

## 9. Persistence

* **Project files** (`flikk-store/src/project_file.rs`): `.flikk` JSON
  (`format: "flikk-project"`, `version: 1`), written atomically; media keeps
  its absolute path and its path relative to the file, tried in that order.
  Agent settings are the user's, for every project, and are not saved in
  the file (a `settings` key older files carry is ignored). New, Open, Save,
  Save As and recent files (cmd/ctrl-N, O, S, shift-S) are in the project
  menu (`flikk/src/projects.rs`), with a dot while there are unsaved
  changes and a Save / Don't Save / Cancel question before New, Open, Quit
  or closing the window.
* **Autosave and backups:** unsaved changes autosave to
  `paths::autosave()/<id>.flikk` (1 s after the last change, at most 5 s after
  the first) for crash recovery; saved files get a backup in
  `paths::backups()` at most every 5 minutes, 10 kept.
* **Offline media** is flagged with a notice and relinked from a folder
  (`flikk/src/relink.rs`): matched by file name, confirmed by
  fingerprint when it can be.
* **Database** (`paths::database()`, SQLite in WAL mode,
  `flikk-store/src/schema.rs`): `meta` (agent settings, model picks, analysis
  steps, the provider per capability, recent projects), `style_profiles` (caption styles), `style_rules`,
  `source_facts` and `media_fingerprints`. Nothing per project.
  Imported Markdown notes become proposed style rules, one per bullet or
  paragraph (S5); facts tied to a path are suspended when its media is
  replaced (F2).
* **Caption styles** (`CaptionStyle`, `flikk-core/src/memory.rs`): font,
  weight, spacing, fill, stroke, shadow, background box, position and
  nudge, a spoken-word highlight (color, box, karaoke, word-by-word reveal)
  and intro/outro motion (fade, pop, slide), with twelve ready-made looks;
  sizes are at 1080p, scaled by the frame's short side.
* **Other user files** under `paths::home()`: `projects/` (chats),
  `library/` (with `library/assets/`, the library's audio and voice
  samples), `models/`, `cache/`, `agent/` (sessions, models, tool groups,
  tool usage, work dirs), `generated/` (music, sound effects and
  voiceovers with their `.json`), `logs/`, `updates/` (a new version
  while it downloads and unpacks), `bridge.json`, and `credentials.json`
  (owner-only, only where the system has no credential store).

### Releases and updates

* **Release** (`.github/workflows/release.yml`, run by hand on `main`):
  raises the workspace version (patch, minor or major) in `Cargo.toml` and
  every member's entry in `Cargo.lock`, commits "Release vX.Y.Z" to `main`,
  tags it and publishes a GitHub release with generated notes, then starts
  Deploy (a release made with `GITHUB_TOKEN` starts no `release` workflow).
* **Deploy** (`.github/workflows/deploy.yml`, on a published release or by
  hand with a tag): builds `flikk` and `flikk-mcp` in release on Linux
  x86-64 (Ubuntu 24.04), macOS Apple silicon (14.0 and later) and Windows
  x86-64, with `FLIKK_UPDATES` and `FLIKK_UPDATE_REPO` set, and attaches
  what `scripts/package.sh` makes: `flikk-<v>-<os>-<arch>.tar.gz` (`.zip`
  on Windows) and its `.sha256`. On macOS the archive holds `Flikk.app`
  (ad hoc signed, not notarized, `mlx.metallib` beside the binary);
  elsewhere a folder with the binaries and the libraries cargo put beside
  them (Dawn on Linux). Windows also gets `…-setup.exe`, built with Inno
  Setup from `scripts/windows/flikk.iss`: it installs for the user alone
  into `%LOCALAPPDATA%\Programs\Flikk`, without an administrator, so
  updates can write there, adds a Start menu entry and opens `.flikk`
  files. It fails when the tag is not the crate version. On Windows it
  builds into `D:\t`: whisper.cpp's Vulkan shader generator is a CMake
  project nested so deep in cargo's build folder that under the checkout
  its paths pass 260 characters, which MSBuild's file tracker can't open
  (error FTK1011). A local Windows build in a deep folder needs a short
  `CARGO_TARGET_DIR` too. Deploy builds the tag's source but packages
  with `scripts/` from its own commit (checked out into `.deploy/`), so a
  release rebuilt by hand from `main` gets packaging fixes made since.
* **Updates** (`flikk/src/update.rs`, `flikk/src/state/update.rs`): only
  packaged builds (`FLIKK_UPDATES` at build time) ask GitHub for the
  latest release, at launch and every 6 hours; `FLIKK_NO_UPDATES` turns
  that off. A newer version with this platform's archive is a notice under
  the bell that can't be dismissed and is listed first. Update downloads
  it to `paths::updates()`, checks its SHA-256, unpacks it with the
  system's `tar` (which reads zip on Windows) and lays its files over the
  install: the bundle on macOS, the executable's folder elsewhere. Each
  file is written beside its target and renamed over it; on Windows the
  target is renamed aside first (`*.flikk-old`, swept at the next launch),
  since a running executable can be renamed but not replaced. Restart
  settles unsaved changes like Quit, then starts the installed version.
  Whether the install folder can be written to is tried when the release
  is found: where it can't (Program Files, `/opt`, `/Applications` for a
  standard user), or where an update failed and it then can't, the notice
  offers Download…, which opens the release's page, in place of Update.

## 10. Deliver

* **Render graph** (`flikk-deliver/src/graph.rs`): one FFmpeg `filter_complex`
  per job, sources opened inside it. Cuts, transforms, keyframes as FFmpeg
  expressions matching the viewer's curves, the grade as a `lut3d`, captions
  and titles burned in with ASS (libass; the viewer letters them with the
  same timing code in `captions/motion.rs`, and so does the render when
  FFmpeg has no `ass` filter, by piping the frames through the compositor
  with `Letterer`), gain, fades, equal-power
  crossfades, `atempo` retiming, ducking with `sidechaincompress`, audio
  effects, and two-pass `loudnorm`. Under a speed ramp the audio runs
  through `atempo` stages (enough that each stays at 0.5 or more) whose
  tempo an `asendcmd` script in the work folder moves every 50 ms of
  timeline; the viewer's stretch reads its grains where the ramp has the
  source (`Stretch::mapped`). Muted ramps leave the clip out.
* **HDR** sources go through the same tone mapping as in the viewer (§6),
  after the segment's trim and before its grade; composited ranges get it
  from the media engine. Video outputs are tagged BT.709.
* **Composited ranges** (`composed.rs`): ranges that need the compositor
  (effects, generators, transitions, blend modes, crop, flip, stretch/skew/
  anchor, corner pins, masks, retimed or ramped media;
  `compose::needs_compositor`) are drawn by `compose::program` from the
  original media and piped into the graph as raw RGBA, which keeps
  captions and audio. Optical flow frames are made as each frame is
  composed (`speed::Waiting`), never blended in their place. Subject masks in
  the range are found (and tracked) first, and person masks' frames in it
  matted, `SubjectService::complete` and `complete_person` taking half the
  progress; a subject that cannot be found (or people without their model)
  fails the render. A render of a person mask matched the viewer's frame
  to a mean difference of 1.8/255 (H.264 included). Then every depth map
  the range's depth effects and masks read (the frames their clips show
  there, transitions included, and the frames smoothing reads) is made (`DepthService::complete`, half of what
  progress is left), so frames compose as the viewer shows them; a map
  that cannot be made fails the render.
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

Workspaces (`Workspace::ALL`, rendered in `flikk/src/app.rs`): Edit
(media pool and effects library on the left, viewer, inspector, timeline),
Captions (transcript and caption style panel), Color (wheels, curves,
scopes), Audio (mixer and meters) and Deliver. The agent chat is docked on
the right in every workspace. Settings (the gear in the top bar, cmd-,, or
the chat's gear for its Agent page) has five pages: Agent, Integrations
(each provider's key and account, who does each capability on which
model), Analysis (the steps), Models (download, pick, and delete the transcription size not picked) and
Storage (the folders a user has a stake in: the Cache, everything the
editor makes again by itself, with one Clear that removes what it counts;
models, the library (effects, presets, sounds and voices, their audio in
`library/assets/`), what providers and Codex generated, and backups; and
a row that turns background rendering off and sets its size limit. Analysis results are
not shown and not cleared with the cache, as making them again can
cost). The FX code editor replaces
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
under Xvfb (with `FLIKK_HOME=/tmp/flikk-dev` by default). There is
no input automation in the cloud containers, so seed the states you need to
see:

| Variable | Values | Seeds |
| --- | --- | --- |
| `FLIKK_DEMO` | `1` | A demo project with stand-in engines |
| `FLIKK_DEMO_WORKSPACE` | `Edit`, `Captions`, `Color`, `Audio`, `Deliver` | The workspace to open |
| `FLIKK_DEMO_FX` | `effects`, `generator`, `transition`, `library`, `code` | An effects or library view |
| `FLIKK_DEMO_SELECTION` | `clips`, `transition` | A selection |
| `FLIKK_DEMO_CHAT` | any | Chat references |
| `FLIKK_DEMO_CHAT_PICKER` | `@query` | The composer's `@` list, open |
| `FLIKK_DEMO_VIEW` | `captions`, `retype` | The transcript's captions view, or typing over two words |
| `FLIKK_DEMO_PLAY` | `1` | Playback |
| `FLIKK_DEMO_COMPOSE` | seconds | Every compositor feature on real FFmpeg-made media |
| `FLIKK_DEMO_DEPTH` | `1` | A lens blur (focus pick armed), a 3D camera and a depth mask on the selected clip |
| `FLIKK_DEMO_SETTINGS` | `agent`, `integrations`, `analysis`, `models`, `storage` | Settings, open on that page |
| `FLIKK_DEMO_STABILIZE` | seconds | A shaky clip made with FFmpeg, stabilized and selected; `FLIKK_DEMO_STABILIZE_MODE` = `off`, `locked` or `none` |
| `FLIKK_DEMO_SPEED` | `1` | The selected clip ramped (Hero) and blending, its Speed tab open |
| `FLIKK_DEMO_PERSON` | a video or picture file | That file over a gradient with a person mask, selected, on the real media engine and person masks |
| `FLIKK_DEMO_HDR` | `1` | The selected clip's media is HDR (HLG): its pool badge and the Color tab's tone map |
| `FLIKK_DEMO_GENERATE` | `music`, `sfx`, `voiceover`, `voices`, `voices-describe`, `voices-audio` | The media pool's Generate form, or the Voices dialog on that tab |
| `FLIKK_DEMO_SETUP` | `missing`, `downloading`, `failed` | The welcome page's setup list in that state, whatever is on disk (Download, Cancel and Retry only pretend); needs no `FLIKK_DEMO` |
| `FLIKK_DEMO_ASK` | `1` | ElevenLabs picked to transcribe for the run, and the question whether to analyze the demo's video and audio with it |
| `FLIKK_DEMO_SHOT` | `card`, `blockout`, `upright`, `plain` | A video waiting on the user in the chat, with a blockout, a first frame and a reference; `blockout` opens the blockout's dialog; `upright` is the same in a 9:16 picture; `plain` is an upright video from its prompt alone, its tiles empty |

```sh
FLIKK_DEMO=1 FLIKK_HOME=/tmp/flikk-demo cargo run -p flikk
```

`scripts/cloud-session-start.sh` prepares cloud sessions: `cargo fetch`,
`glslc` and the Vulkan headers for whisper.cpp, and lavapipe so the window
opens under Xvfb. Chromium for the HTML engine is under `/opt/pw-browsers/`
there.

## 12. PRD → code map

| PRD | Where |
| --- | --- |
| M1 import, bins, search | `ui/media_pool*`, `Command::ImportMedia` |
| M2 inspector, M4 proxies | `flikk-media`, `ui/media_pool*` |
| M3 analysis | `flikk-analysis`, `flikk-models` |
| E1, E2 timeline and tools | `ui/timeline*`, `flikk-core/src/edit/` |
| E3 transcript editing | `ui/transcript*`, `flikk-core/src/query.rs`, `cuts.rs`; fixing words: `flikk-core/src/respeak*`, `state/respeak.rs`, `tools/respeak.rs` |
| E4 silences | `AnalysisService::silences`, `ui/timeline/silence.rs` |
| E5 transform, keyframes | `model::Transform`, `mask.rs`, `ui/inspector*`, `ui/viewer/gizmo.rs`, `ui/viewer/masks.rs` |
| Subject masks | `flikk-core/src/subject.rs`, `mask.rs`, `compose.rs`, `flikk-models/src/segmentation/`, `flikk-analysis/src/subjects*`, `ui/viewer/masks.rs`, `ui/inspector/masks.rs`, `tools/masks.rs` |
| Stabilization | `flikk-core/src/stabilize.rs`, `compose.rs`, `flikk-analysis/src/motion.rs`, `ui/inspector/stabilize.rs`, `tools/stabilize.rs`, `flikk-deliver/src/composed.rs` |
| Person masks | `flikk-core/src/person.rs`, `mask.rs`, `compose.rs`, `flikk-models/src/matting/`, `flikk-analysis/src/people*`, `ui/inspector/masks.rs`, `ui/viewer.rs`, `tools/masks.rs` |
| Depth maps, depth effects and masks | `flikk-core/src/depth.rs`, `fx/depth.rs`, `mask.rs`, `compose.rs`, `flikk-gpu` (`wrap.rs`, `batch.rs`), `flikk-models/src/depth/`, `flikk-analysis/src/depth*`, `flikk-deliver/src/composed.rs`, `ui/inspector/effects.rs`, `ui/inspector/masks.rs`, `ui/viewer/focus.rs`, `tools/depth.rs` |
| E8 timecode readout | `Clip::to_source`, viewer |
| Speed ramps, slow motion | `flikk-core/src/speed.rs`, `edit/retime.rs` (`set_speed_ramp`), `compose.rs`, `compose/pictures.rs`, `flikk-models/src/interpolation/`, `flikk-analysis/src/interpolation.rs`, `audiofx/stretch.rs`, `flikk-deliver/src/graph.rs`, `ui/inspector/speed.rs`, `tools/speed.rs` |
| C1–C3, C6 | `flikk-core/src/captions.rs`, caption tracks, `CaptionStyle`, `ui/caption_style*`, `flikk-deliver` SRT/ASS, `ui/lettering.rs` |
| K1 grade | `flikk-core/src/grade.rs`, `scopes.rs`, `ui/inspector/color*`, `ui/scopes.rs` |
| A1–A3 mixer, loudness, ducking | `ui/mixer*`, `ui/inspector/audio.rs`, `audiofx.rs`, `AnalysisService::loudness`, render graph |
| Fit music to length | `flikk-core/src/music_fit*`, `Command::FitMusic`, `state/music_fit.rs`, `ui/timeline/menu.rs`, `tools/music.rs` |
| D1 render | `flikk-deliver`, `ui/deliver.rs`; hardware encoders `flikk-deliver/src/hw.rs` |
| Render cache | `flikk/src/render_cache*`, `ui/viewer/player.rs`, `ui/timeline/paint.rs` (render bar), `ui/editor_settings/storage.rs`, `flikk-store/src/render_cache.rs` |
| AG1–AG10 | `flikk-agent`, `agent_link*`, `ui/chat*`, `ui/settings.rs` |
| Online providers | `flikk-cloud`, `flikk-core/src/cloud/`, `pool.rs`, `state/cloud.rs`, `ui/editor_settings/integrations.rs`, `ui/media_pool/generate.rs`, `tools/generate.rs` |
| HDR tone mapping | `flikk-core/src/hdr.rs`, `flikk-media` probe, decode, proxy, `flikk-deliver` graph, `ui/inspector/tone_map.rs`, `ui/media_pool/hdr.rs`, `set_tone_map` |
| V1–V7 collages | `MediaService::collage`, `flikk-agent` view tools |
| S1, S3, S4, F1, F2, F4 | `flikk-core::memory`, `flikk-store`, cut planners |
| Sync by sound (podcasts, multicam) | `flikk-core/src/sync/`, `Command::SyncClips` / `PlaceSynced`, `state/sync.rs`, `ui/timeline/menu.rs`, `ui/media_pool.rs`, `ui/viewer/audio.rs`, `flikk-deliver/src/varispeed.rs`, `tools/sync.rs` |

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
* Blockouts: only the agent's `generate_video` takes one; the media pool's
  Video form does not. A scene asked for on the card or changed in the
  dialog is written without being seen: that run has no tools, and reads
  the scene's review instead, which says where things are but not
  whether a limb bends the right way. Feet are moved by the cycle their
  writer gave, not solved against the ground, so they slip where the
  cycle's round doesn't match the stride. A scene with built characters
  is long to write: a shot with a creature of twenty-odd parts took the
  backend over three minutes. A blockout is
  not kept in the pool or the project: it lives in the waiting request
  and its clip in the upload scratch. Making a picture for a tile was
  checked by code review only, as it needs an image provider's key. In a
  debug build the scene is drawn coarse, since drawing is some twenty
  times slower unoptimized.
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
* Fixing words was checked here end to end with ffmpeg's flite voice
  standing in for the provider (a take louder and brighter than the
  original): the words, context, cuts, level and tilt match, room tone,
  the `ReplaceSpeech` layout (same length, longer with the picture slowed,
  shorter with a ripple) and undo. No ElevenLabs request has run: whether
  Eleven v4 takes `previous_text`/`next_text` is unknown (a refusal naming
  them is retried without), and how well an instant clone matches the
  speaker in a single word is unheard. Without the aligner downloaded here
  the take's words were timed by the levels only. The match is a level and
  a tilt, not a full EQ or reverb match; the picture is not re-synced to
  the new words (lips keep moving as they did), and a word longer than
  the old one slows the picture under it, which shows on a long insert.
  Words in sped-up, slowed or ramped clips can't be fixed. The new words appear in the
  transcript once their analysis finishes (a few seconds).
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
  with a stand-in `FLIKK_FFMPEG` that accepts `h264_nvenc`; VideoToolbox
  and AMF are written from FFmpeg's encoder options, VA-API's `hwupload`
  tail only parsed. The CRF-to-QP/bitrate mapping is not measured against
  libx264 (no VMAF runs), nor the speed-up.
* Speed ramps: optical flow runs on the CPU (ONNX Runtime) only: RIFE took
  about 0.25 s for a 256 × 128 frame here; at 720p and 1080p it is likely
  seconds a frame, so flow lags playback (blended until made) and slows
  renders. No GPU, Mac or Windows was available to measure it. Frames are
  kept raw (about 3.7 MB each at 720p) and pile up until Previews is
  cleared. Flow is made from proxies in the viewer and from originals in
  renders, so the two match in motion but not pixel for pixel.
* Speed ramps: ramped audio in the viewer (our WSOLA stretch) and the
  render (`atempo` moved in 50 ms steps) differ slightly in shape, as for
  one speed. OTIO and FCPXML exports leave ramps out, as they do speed. The
  protected-range check sees the two halves of a split ramped clip as
  different pieces, so it may warn on a split that changes nothing.
  Agent views (stills) show blended frames until flow frames are made.
* The RIFE graph comes from a third party's Hugging Face repository
  (`walterlow/RIFE_fp32_timestep`, pinned to a commit); a mirror under the
  project's own account would keep it reachable.
* Person masks: MODNet runs on the CPU (0.27 s a 768 × 512 frame on 4
  cores here), so preview is not real time without a GPU provider, which
  was not tried; nor were macOS and Windows. The ONNX file comes from a
  third party's repository (Xenova, pinned to a commit); a mirror under
  the project's own Hugging Face account would keep it reachable. Easing
  over time starts again at a chunk made before the one ahead of it, so a
  chunk's first frame can step. MODNet is trained on portraits: full
  bodies far from the camera, several people overlapping and people
  against busy backgrounds of their own colour leave holes or halos
  (Expand below 0 trims a halo). Its quality was checked by eye on one
  photograph and a 2 s clip made from it, not measured.
* ONNX Runtime adds about 25 MB to the binary, and the first build
  downloads it (from pyke's CDN). Its build script unpacks the archive very
  slowly when unoptimized; `CARGO_PROFILE_DEV_BUILD_OVERRIDE_OPT_LEVEL=3`
  makes the first debug build take minutes instead of hours.
* Sync by sound was measured only on FFmpeg-made material from synthetic
  speech (espeak-ng voices over room tone and tones), not on real
  recordings from two devices in a room; music-only overlap, very
  reverberant rooms and recordings made far apart (sound arriving 10+ ms
  later) are untried. Measuring keeps both onset envelopes in memory
  (about 1.4 MB per hour each; the coarse correlation of two hour-long
  recordings takes about 35 MB more) and reads each recording once whole
  and once more in windows, with no progress shown and no cancel; an hour
  takes about 10 s per recording in release, about twice that in debug
  builds. Linked clips are found by media, start, source in and speed,
  so a camera whose sound was moved apart from its picture syncs alone.
  The pool's multi-selection is for syncing only; dragging still takes
  one item. Not run on macOS or Windows.

* Stabilization was checked on FFmpeg-made shake only (a fractal still
  jittered by crop and rotate), not on real handheld footage: rolling
  shutter wobble, motion blur, low light, a subject filling the frame or a
  fast pan past the search (about 10 % of the width a frame) are
  unmeasured, and the correction is a similarity, so it cannot remove
  rolling-shutter skew. Scale is not measured, so a shaky zoom is not
  steadied. Subject and person masks are drawn from mattes of the
  unstabilized frame, and depth effects and masks from its depth map, so
  on a stabilized clip they sit off by the correction; shape masks follow
  the stabilized picture. Not run on a GPU here (lavapipe and the CPU
  compositor only), nor on macOS or Windows.
* Fitting music was checked on a synthetic 120 BPM song (intro, A, B, A,
  outro, a final chord ringing 5 s; built with NumPy) with its true
  downbeats and with Beat This!'s: every target from 20 s to 130 s landed
  exactly, joins showed no clicks (largest sample step at a join 0.03,
  against 0.81 inside the song), and with true downbeats jumps paired the
  same chords in the same section (85–96 %) where the length allowed a
  4-bar move. With Beat This!'s, which finds bars in the final chord's
  ring, the ending had about a bar to give, and some lengths took a weaker
  jump (25–60 %). Not checked on real songs, nor by ear; no length leaves
  the meter (a jump moves whole bars), so where whole bars can't reach the
  target the song ends up to half a bar off or trims its ending's ring.
  The context menu was checked by building only, not on screen. Bar
  features are computed on every fit, not cached.
* Render cache: keys cover the project and the editor's styles, not files
  read by path behind them (an image parameter's file or a LUT changed in
  place, analysis data kept outside the project): such a change shows only
  after the cache is cleared. Built-in shaders and templates are keyed by
  the app version, so a development build that changes one keeps showing
  old frames until cleared. Frames are keyed by absolute time, so moving a
  clip (or a ripple) renders the moved range again. Playback's disk reads
  were not measured here at real time on a GPU machine (no GPU in the
  cloud containers), nor on macOS or Windows; JPEG coding was timed on
  still frames on the CPU only. Cheap spans are judged by the worker's
  timing at the render size, not the viewer's, and found again each
  session.
* HDR tone mapping was checked on FFmpeg-made PQ and HLG clips only
  (`zscale` from SDR test sources, x265 with HDR10 metadata): against the
  viewer's frames, exports match about as well as SDR exports do (41 dB),
  and the LUT and zimg methods agree to 51–54 dB on smooth pictures (about
  30 dB on saturated hard edges, where the 65³ table is coarse). Real phone
  footage (iPhone Dolby Vision 8.4 HLG, Android HLG, Samsung HDR10+) was not
  tried, nor were Dolby Vision profile 5 files (IPT, no backward-compatible
  base layer: they still look wrong), dynamic HDR10+ metadata (ignored), HDR
  stills (AVIF, HEIC; not mapped) or `libplacebo`/`tonemap_vaapi` (not used:
  they need a GPU device). Items imported before this keep no `Hdr` until
  they are imported again. A proxy's file name holds the mapping but not the
  method, so a proxy made with zimg is used after a switch to the LUT.
  Collages always show the default tone map. Playing HDR footage before its
  proxy exists maps it at full resolution, which is slow for 4K on a CPU.
* Depth maps run on the CPU everywhere (ONNX Runtime's CPU provider; no
  CoreML, DirectML or CUDA tried): 0.4–0.6 s a frame on 4 cores, so
  playback over footage not yet made shows the effects late (the nearest
  made frame's map stands in, else the picture as it is), and a render
  makes them first, which can take far longer than the encode. Not run on
  a Mac or Windows, nor on a real GPU: the depth shaders were compiled and
  rendered here on lavapipe only.
* Depth is relative and per picture: `focus`, `start` and mask ranges are
  fractions of each shot's own range of depth, not distances, so the same
  value can land elsewhere after a cut, and a shot whose range changes
  (something walks close to the camera) shifts them. Smoothing steadies
  the range over ±2 frames only. Maps are 8-bit at 384 on the longer side:
  fine detail (hair, fences) is soft, and the lens blur can halo there.
* The 3D Camera fills the gaps it uncovers by stretching the edge beside
  them (no inpainting), so large moves on pictures with near objects
  smear; keep `amount` modest for those.
* The depth model's ONNX file comes from a third party's Hugging Face
  repository (onnx-community, pinned to a commit); a mirror under the
  project's own account would keep it reachable.
* If the process exits while the depth worker is inside the model, ONNX
  Runtime can print an error as it tears down.

### Analysis accuracy

* Sync by sound, against a 10-minute camera recording (AAC in MP4) of
  synthetic speech over room tone; offsets as the follower's source time
  for the reference's (`offset + r · (1 + drift)`), error at three points
  of the overlap, release build:

  | Follower | Truth | Measured | Error |
  | --- | --- | --- | --- |
  | Recorder (24-bit WAV) started 83.4567 s earlier | +83.4567 s | +83.45670 s | < 0.01 ms |
  | Second camera started 95.1234 s later, 300 Hz–3 kHz, −12 dB, white noise, AAC | −95.1234 s | −95.12334 s | 0.06 ms |
  | Recorder covering 6:40–15:00 only (MP3), partial overlap | −400 s | −400.00001 s | 0.01 ms |
  | Recorder 0.0104% fast (`asetrate` 48,005 Hz) from 20.5 s earlier | +20.497865 s, drift −1.0415e-4 | +20.49790 s, −1.041e-4 | 0.04–0.06 ms |
  | 4 min of unrelated talk and 2 min of room tone, then the camera's sound (44.1 kHz) | +366.4509 s | +366.45090 s | < 0.01 ms |
  | Pink noise 3 dB under the speech (mean levels), three echoes (40–120 ms), −6 dB | −30 s | −30.00000 s | < 0.01 ms |
  | 20 s excerpt from 4:10 | −250 s | −250.00001 s | 0.01 ms |
  | Unrelated talk (same voices) | no match | confidence 0.09 | rejected |

  Every match scored confidence 1.00 with all its fine windows agreeing.
  Rendered through `SyncClips` and the WAV preset, the synced recorder
  (drifting one included, through the resampler) measured against the
  camera again at offset 0.04 ms or less and no drift. A 10-minute pair
  takes about 5 s in release (envelopes 2 s each) and 10 s in debug.

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
