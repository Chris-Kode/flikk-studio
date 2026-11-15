# Analysis and models

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

## Models (`flikk-models`)

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

## Transcription online

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

## Speech

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

## Sounds, music and beats

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

## Fitting music to a length

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

## Subject masks

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

## Stabilization

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

## Person masks

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

## Depth maps

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
