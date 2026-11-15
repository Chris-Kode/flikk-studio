# Analysis and models

The user starts analysis from the "⋯" menu of an item in the media pool,
or from its right-click menu. The command is "Analyze". When the item has
results, the command is "Analyze again". "Analyze again" runs every step
again on the models that the user picks now.

Flikk also queues every other item in the background when it arrives in
the pool. The method of arrival is not important: an import, the media
pool's Generate or an approval. `EditorState::analyze_new` does this. It
runs after every edit and looks at each pool item one time. No setting
turns it off.

Flikk never analyzes pictures. Items that are already in a project when
it opens stay as they were. If the user undoes an import and does it
again, the item is new again.

Automatic analysis never spends money without a request from the user.
While a provider is picked for transcription, some items wait in
`EditorState::provider_asks`. These are the items whose sound the
provider would hear. Such an item has no voiceover script in place of its
words, and it is not heard only (see below). A dialog
(`ui/provider_ask.rs`) lists them under "Analyze with <provider>?". The
window opens and closes the dialog with the list, and the dialog follows
the list when more items arrive.

- Analyze queues the items as the user asked, so they go to the
  provider.
- Cancel, Escape or the backdrop leave them unanalyzed until the user
  asks for them.
- Analyze is dimmed, with a note, while the provider has no key.

When the user picks a provider for transcription, Flikk drops the analysis
that waits in the background and would go to that provider.
`cancel_background_speech` does this and names their files. Flikk then asks
about those items in the same way. When the user goes back to this machine,
Flikk queues the items that the dialog asked about, because they cost
nothing then.

A last guard is `JobProgress::asked`. Sometimes a job was queued in the
background, and nobody asked for it after that. Such a job skips itself if
a provider transcribes at the time it runs. The item then stays unanalyzed.

Music and sound effects that a provider made are heard only
(`MediaItem::heard_only`, applied in `LocalAnalysis::analyze_with`). This
is true for each requester and for each transcription choice. The
library's sounds follow the same rule. A heard-only analysis has sounds
and music, beats and sound search. It has no voice activity, transcript,
speakers or shot changes. Thus these items never go to a provider, and
Flikk never asks about them.

The result of a heard-only item is complete when it covers those steps.
`coverage` calculates what a plain request wants for speech, a script or
sound only. The key of the job contains the steps, so Flikk can tell it
apart from a full job. Flikk cannot know that imported audio is music, so
it analyzes imported audio in full. Flikk analyzes the library's sounds
separately, without speech or shots, on local models only. Thus they cost
nothing.

The models are on disk before a project opens (see Models). Thus
"Analyze" is disabled only in these conditions:

- A provider is picked for transcription and has no key
  (`analysis_ready`). The notice of the key tells the user this.
- The item is queued or in analysis.

Flikk does not analyze images.

Every step (`AnalysisSteps`: transcript, speakers, sounds and music,
beats, sound search, shot changes) is always on. The user cannot turn a
step off. Each step runs when the model for its task is downloaded. If
the model of a step is missing, the step stays empty in the results.
Voice activity then uses an energy VAD as a fallback. Speakers need the
transcript, and beats and sound search need the sound pass.

Flikk analyzes items one at a time on a `JobQueue`. There are two groups:

- The requests of the user or the agent (`AnalysisService::analyze`):
  "Analyze", `view_media` and the Analyze button of the provider dialog.
  These go before the background group.
- The background group (`analyze_in_background`): new pool items and
  library sounds.

When the user asks for an item that waits in the background, the item
moves up with the requests (`JobQueue::push_first`). Each group runs in
the sequence of the queue.

Flikk writes a result to `cache/analysis/<fp>.json` only when every step
succeeded. If not, Flikk keeps the result for the session, and the job
fails with the names of the steps. `ANALYSIS_VERSION` (1) marks the
results. Flikk does not read the results of another version, or the
sound vectors next to them. Thus the item is not analyzed until the user
analyzes it again.

## Models (`flikk-models`)

There is one folder for each task: voice activity, transcription, word
timing, speakers, sound tagging, beats, sound search, segmentation, frame
interpolation, matting and depth. The `mod.rs` of the folder holds these
items:

- The interface of the task: the trait that analysis calls
  (`VoiceActivity`, `Transcriber`, `Aligner`, `Diarizer`, `Tagger`,
  `BeatTracker`, `SoundSearch`, `Segmenter`, `Interpolator`, `Matter`,
  `DepthEstimator`).
- `MODELS`: the models that can do the task, with the default first.
- `runner`, which builds the runner of one of these models.

In the task folder, there is one folder for each model:

- `mod.rs` is the model as the user sees it.
- `config.rs` holds its files and the settings that it runs with. The
  files are pinned to a commit, with their sizes and SHA-256.
- `runner.rs` is the code that loads and runs the model, behind its
  feature. It also finds its own files.

The crate root holds the parts that the models share. These are the catalog
types, the audio rates, the downloads, ONNX Runtime (`onnx.rs`) and the BPE
tokenizer (`bpe.rs`). There is one ONNX Runtime environment for each
process. The aligner, the diarizer, the sound models, segmentation, frame
interpolation, matting and depth share it.

`onnx::GpuSessions` runs RIFE and Depth Anything on the GPU:

- Core ML on macOS.
- DirectML on Windows.
- WebGPU on x86-64 Linux. Its Dawn library is next to the binary, which
  finds it by rpath. The editor does not start without it.

RIFE and Depth Anything are the only ONNX models that are faster on the
GPU. The others were slower or equal, as measured on an M5 with Core ML.
The exception is WeSpeaker, which gives NaN there on real audio. If a load,
run or answer fails on the GPU, Flikk runs it again on the CPU. That model
then stays on the CPU.

On macOS, depth keeps its compiled models: they take 17 s to compile and 4
s to load. DirectML and WebGPU on Linux did not run yet.

Analysis only controls the sequence of the work. It asks the runner of
each task for the selected model (`Task::model`: the model that the user
picked, or else the default). Analysis never names a model. A new pick of
transcription model applies from the next transcript. Results that exist
stay as they are.

A new model is a folder under its task, its `MODEL` in `MODELS` and an
arm in `runner`. A model in several sizes (Whisper) has a `MODEL` for
each size in one folder.

Every model is necessary: each task of `Task::ALL` on the model picked
for it (`EditorState::required_models`). While one is missing, the
welcome page shows a setup list in place of Recent (`state/setup.rs`,
`ui/welcome.rs`). The list has these rows:

- One row for the full set:
  - It shows the total size and one Download button. The button starts
    each missing model at the same time.
  - Its progress counts against the bytes that were missing when each
    download of the set started (from here or from Settings).
  - It has Cancel. A failed download stays in place with Retry.
- Whisper small or large, each with its size. Flikk holds this row while
  the set downloads.
- The agent CLIs that Flikk found, and their versions.
  `flikk_agent::backend::installed_version` runs `--version` off the UI
  thread. If Flikk finds neither CLI, the row shows Install… and Check
  again.
- FFmpeg and a browser, only when Flikk does not find them.

After an update that adds a task, the list comes back with only the files
of that task to download.

While the list shows, Flikk does not create or open a project, for all
types of request (`EditorState::projects_wait`, checked in
`projects.rs`). The app opens on the list for all methods of start. If
the app started for a `.flikk` file, that file opens when the set is
complete. New project, Open… and the drop zone are held until then. If a
shortcut asks for a project, the status line tells why. The welcome page
also shows the status line.

The demo opens without the models. `FLIKK_DEMO_SETUP` seeds it, and its
buttons only pretend.

Nothing downloads automatically. Settings → Models (the gear in the top
bar, or cmd-,) lists each task with its model. When a task has more than
one model, the user picks it there. The store saves the pick for each
user. The page downloads one model or all of them, with progress and
cancel, through `<file>.part`. A failed download stays there with Retry.

Flikk keeps a part when a download is cancelled or cut. The next download
asks the server for the rest (a range request). If a server sends the
full file again, the download starts again from the beginning.

Before a file gets its name, Flikk checks it against its pinned SHA-256
(`ModelFile::sha256`). This value is the host's LFS object id for the
pinned commit. For the small files, it is measured on bytes that were
checked against the host's git hash. If a file has the wrong size or does
not match, Flikk deletes it and the download fails. Thus Retry downloads
it clean.

Flikk locks a part while it writes the part. Thus a cancelled download
that is still in the process of stopping, or another copy of the app,
completes its work with the part first.

`LocalAnalysis::model_tasks` tells which tasks a build runs. If the files
of a runner are missing (deleted by hand), the runner fails ("… is not
downloaded"). Flikk then skips its step. Nothing warns about a missing
model, because the welcome page covers it.

The user cannot delete a model that the editor needs there. Only the
transcription size that is not in use has Delete, for a full or a partial
download. When the user picks the other size, Flikk downloads it, and the
size on disk stays in use. `EditorState::picked_model` is the pick, and
`Task::model` is the size that analysis runs on. Flikk switches when the
new size is on disk. Thus the set stays complete, and projects open in
the meantime.

A failed download shows on its row with Retry, and the old size stays in
use. Cancel, or a new pick, stops the switch and its download.

The user can dismiss the notices of the top bar for the session. These
are warnings such as offline media or a provider without its key.

Cargo features of `flikk-analysis` gate the runners: `whisper`,
`aligner`, `diarizer`, `sounds`, `subjects`, `interpolation`, `people`.
All of them are default features. Without a feature, its models stay in
the list, but its step uses the fallback that this document describes
below:

- Without `subjects`, subject masks keep nothing.
- Without `interpolation`, optical flow blends.
- Without `people`, person masks keep nothing.

The `mlx` feature of `flikk-models` adds the MLX subject mask model and
makes it the default. It is for macOS only, because mlx-sys links Apple's
frameworks. `flikk` enables it with `metal` on macOS. mlx-sys builds MLX
from source. Thus a macOS build needs CMake and Xcode's Metal compiler.

Settings → Models lists segmentation as "Subject masks" and matting as
"Person masks". Neither is part of analysis. Settings → Models also lists
frame interpolation ("Optical flow", RIFE) in the same way. Flikk blends
slowed frames while it makes the interpolated frames.

`LocalInterpolation` makes flow frames on its own thread, with the newest
request first. It keeps the last 48 frames in memory. It keeps the frames
with a short side up to 720 (the viewer's and the render cache's) as QOI in
a disk cache under `cache/interpolated/`. This cache is 4 GB at most, and
the least recently used frames go out first. The Cache in Settings →
Storage counts and clears it.

A render keeps its larger frames in memory. It asks for each frame only
while it is on that frame.

Depth ("Depth maps", the `depth` feature, default on) is also not part of
analysis.

## Transcription online

The user can pick a provider for transcription in Settings →
Integrations (ElevenLabs Scribe v2, or v2 Medical). Then the provider
does the speech steps, and no local speech model runs or is necessary.
Flikk skips voice activity, Whisper, the aligner, the second pass on the
holes and the speaker models. The Models page shows the provider on
their rows.

Flikk encodes the stream again as lossless mono FLAC at its own rate, up
to 48 kHz (`cache/upload/<fp>.flac`, deleted after). It sends the file in
one request with word times, speakers and audio events. It asks for
speakers (`diarize`) when the speakers step is on. The filler words stay,
because the removal of dead time needs them.

Words keep the times that the provider aligned. The edges measured on the
audio are tuned to Whisper's late starts and early ends, and they moved
good times off the words. Confidence is `exp(logprob)`.
`speakers::apply` numbers the provider's voices, so the ids stay when
Flikk analyzes an item again. Flikk changes a three-letter language into
ISO 639-1.

Speech is where the words are. Words less than 250 ms apart make one
range, which starts 60 ms early, as Silero's ranges do. The transcript
records what made it (`engine`: "elevenlabs/scribe_v2", or the Whisper
model's id).

The other models become optional. If the model of a sound step is
missing, Flikk skips the step, and the step does not fail. When the sound
pass does not run, the provider's audio events take its place:

- "laughter" becomes a sound.
- An event with "music" becomes music without tags or beats.

A provider error (no key, no credits, network) causes the transcript step
to fail, as other errors do. Flikk does not try the step again with
Whisper.

## Speech

* **Voice activity:** Silero VAD v6.2 (`voice_activity/silero/`), run by
  whisper.cpp. The file is `ggml-silero-v6.2.0.bin` from
  ggml-org/whisper-vad at a pinned commit, or `FLIKK_VAD_MODEL`. Speech
  starts at 0.5 and continues until the value is under 0.35. Pauses under
  250 ms stay in the speech, and each range starts 60 ms early. Music is
  not speech for Silero.

  The energy VAD (`flikk-analysis/src/vad.rs`) took a music bed for
  speech. It replaces Silero without the `whisper` feature, or for the
  session when Flikk cannot get the model.

  Silero hears the audio in one pass and also in 4 s blocks. Each block
  comes after the second before it. A window is speech when one of the two
  finds speech. The reason is that music suppresses the one pass for
  seconds after it. Also, a block that starts during a sentence doubts the
  rest of that sentence.

  A silence never takes a transcribed word. Thus the quiet last word of a
  phrase that Silero let go of stays.
* **Transcription:** Whisper small (264 MB, the default) or large
  (large-v3-turbo, 874 MB), both with 8-bit weights. The user picks the
  size in Settings → Models. `FLIKK_WHISPER_MODEL` (a path or a ggml file
  name in `paths::models()`) applies to the size that is picked.

  Flikk builds whisper.cpp for the GPU: Metal on macOS, Vulkan on Windows
  and Linux, with flash attention. Thus a build needs Xcode or the Vulkan
  SDK (`glslc` and the Vulkan headers on Linux). Without a GPU, it runs
  on the CPU. On Windows, the editor delay-loads `vulkan-1.dll`
  (`flikk/build.rs`). Where the DLL is missing,
  `flikk/src/vulkan_fallback.rs` makes Vulkan fail to start. Thus Whisper
  also runs on the CPU there.

  Whisper hears only speech (`transcription/whisper/`). Some inputs caused
  errors: music, a long silence, or a 30 s window that starts with one of
  them. With these inputs, Whisper invented words, which often repeated
  what was said before. Also, its times became loose.

  Whisper hears the stretches that Silero finds, as the VAD filter of
  faster-whisper does. Pauses under 2 s stay, with 400 ms on each side.
  Whisper hears each stretch separately, without the text of the window
  before. It uses the language of the longest stretch. Flikk drops its
  notes, such as "(upbeat music)".

  A stretch that fits one window starts with 0.5 s of silence. Without
  it, Whisper took speech at the very start of what it heard for a cut
  phrase, and dropped it. This removed the first words of an item that
  starts with talk. Also, when Whisper started on speech, it more often
  skipped the last phrase of the stretch.

  A longer stretch continues in windows that whisper.cpp starts where
  Whisper finished a segment. Silence before it moved those windows. In a
  conversation, one window then ended at its first sentence and skipped
  the rest. Decoding is a beam search of 5, as OpenAI's transcribe does.

  Sometimes Whisper gives one sentence a segment that continues over
  seconds of speech that it skips. Sometimes it takes the short last phrase
  of a stretch for silence. Thus, after the word alignment, Flikk finds
  Silero's speech of 600 ms or more with no word within 250 ms. Whisper
  hears that speech again separately. Flikk aligns the words that it finds
  there against that audio only, and adds them.

  In the measurements below, each such hole held words that were lost.
  There were none in other conditions.
* **Word times** (`word_timing/qwen3_aligner/`, `aligner` feature):
  Qwen3-ForcedAligner-0.6B hears up to 30 s of audio with Whisper's
  words. It gives the start and end of each word, in 80 ms steps. Then
  the starts move 20 ms earlier, never into the word before. The edges of
  words next to a pause of 100 ms or more move 20 ms into the pause,
  never past its middle. The reason is that its starts after a pause came
  late, and its ends before a pause came early, against forced-aligned
  LibriSpeech.

  It knows English, Spanish, French, German, Italian, Portuguese,
  Russian, Chinese, Cantonese, Japanese and Korean. Its tokenizer comes
  from Qwen's repository. The model comes from ours (below), or from
  `FLIKK_ALIGNER_MODEL`.

  Other languages keep word edges measured on the audio levels
  (`flikk-analysis/src/align.rs`). The result for the session also keeps
  them when the model cannot load. Words with no letters and no digits
  stay between the aligned words around them.

  That measurement runs only where the aligner cannot run. Before the
  aligner, it moved words over the pause between two speakers, and the
  aligner followed. This occurred at 12 of 55 turns in four
  conversations, and at 4 without it. The word error rate there went from
  5.9% to 7.1%. After the aligner, it made cuts worse. The aligner's
  times never go past the audio that it got, so the words stay in
  sequence.
* **Voiceover words:** Flikk analyzes a voiceover immediately after it is
  made, when the transcript step is on. Its words are its script
  (`cloud::spoken_words`: no audio tags, punctuation on the word before).
  Thus neither Whisper nor a provider hears it, and it costs no credits.

  Flikk divides the rest of the script over the remaining speech by
  written length (`align::spread`). The aligner hears 30 s from the end
  of the last placed word, with the words expected in the first 25 s.
  Words that it puts in the last 3 s go again with the next stretch
  (`follow_script`). Thus an error in the duration of the script does not
  accumulate over a long read.

  A stretch cannot hear past a long pause. Thus Flikk first shortens
  silences over 10 s to 3 s on each side (`Shortened`). This applies to
  silences in which neither the voice activity nor the levels hear
  anything. A word placed over such a silence keeps the side with more
  speech.

  Sometimes a passage of the script is not said, and it is longer than
  the model can squeeze where it is. Then the words after it are late. A
  stretch never hears before the end of the last stretch. Such a stretch
  squeezes many of its words into one 80 ms step, in scattered runs. When
  the stretch is well heard, a tenth of its words at most are squeezed. A
  passage squeezed in place gives one run.

  Then short stretches from words further on find the fewest words to
  leave behind (`catch_up`). Flikk hears the stretch before again, up to
  that point. Flikk keeps it if it follows the audio, except for one
  squeezed run.

  Flikk measured this on espeak-ng reads against their word events. The
  mean start error was 30–40 ms through 7 minutes. A 48-word unsaid
  passage left 0.5 s mean error (8.7 s before), confined to the area
  around it.

  The language is not known ("und"), so the aligner runs on any script.
  Its models are optional. Without the aligner, the words keep edges
  measured on the audio levels. "Analyze again" aligns them when the
  aligner is there. The analysis of a voiceover always uses this path.
* **Alignment model file:** Qwen publishes PyTorch weights only, so
  `qwen3-forced-aligner-0.6b-q4.onnx` is our export. It is hosted with
  its build scripts at
  [Chris-Kode/qwen3-forced-aligner-0.6b-onnx](https://huggingface.co/Chris-Kode/qwen3-forced-aligner-0.6b-onnx).
  Its `config.rs` pins the commit. The file has 589,447,679 bytes, with
  SHA-256 `228f127b…9ab49df`.

  The graph takes 16 kHz samples (a multiple of 160) and the text's token
  ids. It returns the argmax of the 5,000 timestamp classes for each
  token. It contains these parts:

  - The Whisper log-mel features of `WhisperFeatureExtractor` (400-point
    frames, hop 160, 128 mels).
  - The audio encoder's convolutions per second of features. Its layers
    have attention over the full chunk, as Qwen's CPU path does.
  - The audio between `<|audio_start|>` and `<|audio_end|>`, before the
    text.
  - The causal text model.

  The export used `torch.onnx.export(dynamo=True)` at opset 18. ONNX
  Runtime's basic optimizer folded the constants. `Gemm` nodes became
  `MatMul` + `Add`. `MatMulNBitsQuantizer` quantized the weights (4 bits,
  block 32, symmetric). The embedding table became
  `GatherBlockQuantized`.

  The export matches Qwen's own pipeline exactly before quantization.
  After quantization, it is within 0.5 points of clean cuts. Dynamic int8
  quantization breaks it. The repository contains the Apache-2.0 licence
  text. It states that the export is a modified (exported, quantized)
  Qwen3-ForcedAligner-0.6B.
* **Speakers** (`speakers/pyannote/`, `diarizer` feature) follow
  pyannote's speaker-diarization-3.1 on ONNX Runtime. Two models do the
  work:

  - pyannote segmentation-3.0 (MIT) hears 10 s windows 2.5 s apart. Every
    17 ms, it tells which of up to three voices of the window speak.
  - WeSpeaker ResNet34-LM (CC-BY-4.0, trained on VoxCeleb) changes the
    voice of each window into a 256-number voiceprint. It does this only
    where the voice speaks alone. It uses Kaldi filterbank features
    (`fbank.rs`, equal to `kaldi-native-fbank` to 1e-5).

  Flikk clusters the voiceprints with centroid linkage at pyannote's
  threshold (0.7046). Clusters of fewer than 12 join the nearest larger
  cluster. Flikk then averages the windows back into who speaks in each
  frame.

  A word goes to the voice that speaks through most of the word. Flikk
  weighs overlaps by how many windows heard each voice. Below 0.6 of the
  speech, the word keeps no speaker. Flikk numbers the speakers in the
  sequence of their appearance, and attributes them after the aligner.

  Both models come from their own Hugging Face repositories:
  `onnx-community/pyannote-segmentation-3.0` (6 MB) and
  `Wespeaker/wespeaker-voxceleb-resnet34-LM` (26 MB). Its `config.rs`
  pins the commits. When they cannot load, every word is "Speaker 1" for
  the session.

## Sounds, music and beats

* After the transcript (`sounds` feature), Flikk decodes the audio again
  at 32 kHz. It streams the audio through the sound tagger
  (`sound_tagging/efficientat/`) and CLAP's audio tower
  (`sound_search/clap/`) together. Then the beat tracker
  (`beats/beat_this/`) hears each stretch of music that the tagger found,
  with 1 s on each side.

  Runs of tagged windows become `Analysis::sounds` and `music`. A run
  starts up to 2 s before its sound, so Flikk shrinks it by that much. A
  sound shorter than a second moves onto the loudest stretch of the 10 ms
  levels around it. Silence alone scores 0.25 as music, so music needs
  0.5 to start. Flikk drops bells, claps and other musical sounds inside
  music.

  Music carries tags (genre, mood, vocals, lead instruments), beats and
  downbeats. Its tempo is the mean beat interval near the median. The
  vectors go to `cache/analysis/<fp>.vectors`. If Flikk cannot get a
  model, the sounds stay out. Flikk keeps what the other models found for
  the session.

  On 4 CPU threads, the pass takes about 2 minutes per hour of audio for
  the tagger and 6 for CLAP. The beat tracker on music adds to this.
* `flikk-core/src/heard.rs` maps sounds, music, beats, level changes and
  silences onto the timeline. It also finds beats, downbeats and onsets
  near a cut. An onset is a 6 dB rise over the 50 ms before.
* **Sound model files:** these are our exports. `scripts/models/` builds
  them. They are hosted, with those scripts and the licence texts, at
  [Chris-Kode/efficientat-mn10-as-onnx](https://huggingface.co/Chris-Kode/efficientat-mn10-as-onnx),
  [Chris-Kode/beat-this-final0-onnx](https://huggingface.co/Chris-Kode/beat-this-final0-onnx)
  and [Chris-Kode/larger-clap-general-onnx](https://huggingface.co/Chris-Kode/larger-clap-general-onnx).
  Each model's `config.rs` pins their URLs to a commit.

  JKU's server was unreachable. Thus the Beat This! export came from the
  `safe-models/beat-this-final0` mirror. Nobody compared its weights with
  JKU's weights.

  Each graph takes 32 kHz mono samples and holds its model's own front
  end. Thus analysis decodes one time for all three:

  - `efficientat-mn10-as.onnx` (20 MB): windows × samples in, 527
    probabilities out. It matches EfficientAT's pipeline to 2e-6.
  - `beat-this-final0.onnx` (85 MB): up to 30 s in, beat and downbeat
    logits per 20 ms frame out. A windowed-sinc resampler to 22.05 kHz
    replaces soxr.
  - `larger-clap-general-audio.onnx` (78 MB): 10 s windows in, unit
    vectors out. It resamples to 48 kHz inside. It uses dynamic int8,
    with cosine 0.997 to the float model.
  - `larger-clap-general-text.onnx` (141 MB): RoBERTa token ids in. It
    has 8-bit block weights and an int8 embedding table, with cosine
    0.998.

  The text tower's tokenizer comes from LAION's repository at a pinned
  commit (`bpe.rs` encodes it). `FLIKK_TAGGER_MODEL`,
  `FLIKK_BEATS_MODEL`, `FLIKK_CLAP_AUDIO_MODEL`,
  `FLIKK_CLAP_TEXT_MODEL`, `FLIKK_CLAP_VOCAB` and
  `FLIKK_CLAP_MERGES` point at local copies in place of these.

## Fitting music to a length

`flikk-core/src/music_fit*` makes a song end exactly where it should, on
its real ending. It uses seamless edits in place of a hard cut or a fade
(like Premiere's Remix). On a media clip on an audio track, the
timeline's clip menu has "Fit To". Its choices are Sequence End (the end
of the other tracks), In/Out, or Duration…. The agent has `fit_music`.
Both build a `music_fit::Request` on the UI thread and run it off that
thread.

* **Bars:** the bars come from the analysis' downbeats, from the clip's
  in point on. Flikk fills missed downbeats where a gap holds a whole
  number of bars. It moves each downbeat onto the attack within 40 ms of
  it, so a cut keeps the hit whole. The attack is the largest rise of the
  high-passed level, 3 ms before it.

  Quiet bars at the end (8 dB under the song's median) are the ring of
  the final chord, and they join the ending. The part before the first
  bar is the head. The part after the last bar is the ending.
* **Features** (`music_fit/features.rs`) come from PCM decoded at
  22.05 kHz mono through `MediaService::audio`. For each quarter of each
  bar, there is a 12-bin chroma (55 Hz–5 kHz) and 12 MFCCs. The MFCCs use
  26 mel bands, 2048-sample frames and hop 512. Each bar also has its RMS
  level. A bar-to-bar similarity mixes chroma agreement (cubed, 0.5),
  timbre (MFCC distance over its median, 0.35) and level (0.15).
* **Planner** (`music_fit/planner.rs`): a dynamic program over (bars
  played, bar, bars since the last jump) finds the cheapest path from the
  first bar to the last. The path has these properties:

  - It keeps the first and last 4 bars (fewer in short songs).
  - It runs at least 4 bars between jumps.
  - It can jump back (loops) or forward (skips).

  The cost of a jump from bar i into bar j is the sum of these items:

  - 1.
  - 5 × (1 − how much j sounds like i + 1 and j − 1 like i).
  - A phrase penalty: 0 when the bars moved are a multiple of 8, 0.4 of
    4, 1.5 of 2, else 3.
  - 0.5 off a 4-bar boundary.
  - The level change.

  Flikk scores each length of path together with where the path leaves
  the ending. Flikk can trim the ending's ring down to its hit and half a
  bar, under a fade at least a beat long. An early end, or a cut into the
  hit, costs by the beat. The quality is the weakest jump's score times
  how close the end lands.
* **Applying:** `Command::FitMusic { clip, at, segments, crossfade,
  fade_out }` (category Audio) replaces the clip with its segments back
  to back from `at`. It keeps the clip's gain, effects and edge
  transitions. Each join gets a 30 ms `EndOnCut` transition. Audio tracks
  play it as an equal-power crossfade (`audiofx::spans`) in the viewer
  and the render alike. Thus the downbeat after the cut never fades.

  Flikk drops keyframes. The command refuses clips at another speed,
  because fitting keeps the tempo. It also refuses results that would run
  over another clip on the track. It is one undo step for the user and
  one reviewable item for the agent. The agent's result lists each skip
  and loop with its source span, place and score.

## Subject masks

* **Model** (`flikk-models/src/segmentation/sam2/`): SAM 2.1 tiny (Meta,
  Apache-2.0), as square-zero-labs' ONNX export of `transformers`'
  `Sam2VideoModel` with the memory modules (190 MB). Its authors checked
  it against `propagate_in_video_iterator`, with a worst per-frame IoU of
  0.9967. It is pinned to a commit. It has these parts:

  - A vision encoder (1024² picture → features at three scales).
  - A decoder (features, labelled points → 256² logits, a score, the
    object's pointer).
  - A memory encoder.
  - The memory attention.
  - The pointers' temporal positions.
  - `constants.json`.

  Flikk stretches pictures to the model's square with a triangle filter
  (PIL's bilinear) and normalizes them. A box is two corner points
  labelled 2 and 3, before the clicks. We checked it here against a
  Python reference of the same loop on `transformers`' bedroom clip. The
  per-frame IoU was ≥ 0.9988.
* **On MLX** (`flikk-models/src/segmentation/sam2_mlx/`, feature `mlx`,
  the default model on macOS): this is the same network, written over
  `mlx-rs` from SAM 2's modules. These modules are:

  - Hiera with windowed and pooled attention.
  - The FPN neck.
  - The prompt encoder and two-way decoder.
  - The memory encoder.
  - The memory attention with axial rotary positions.

  It runs on the GPU in unified memory. It uses the MLX conversion of
  Meta's checkpoint in half precision (90 MB, from avbiswas' `sam2-mlx`,
  pinned). It computes in f32. Pictures go through the same
  `plan::square`.

  Its decoder chooses as SAM 2 does. For one click or none, it takes the
  best of the three multimask answers. Else it takes the single answer,
  unless that answer is unstable. The ONNX decoder always takes the best of
  three. Thus a box or several clicks can differ slightly (IoU 0.95 on the
  bedroom clip's box).

  If both decoders make the same choice, the IoU is 1.0000. One click gives
  IoU 1.0000 and the same scores.

  Tracking attends to the memory that exists, as SAM 2 does: every
  prompted frame (`plan::bank` without a cap) and the tracked frames. It
  does not use the padding that the fixed bank of the ONNX graphs needs.
  Against the ONNX runner over 8 frames, the per-frame IoU was 0.97–1.0.
  With the same padding, it was 1.0000 on 7 of the frames and 0.9977 on
  the other.

  We checked it on Linux, with MLX on the CPU (a local build of mlx-sys
  that links OpenBLAS). It did not run on a Mac yet.
* **Tracking** (`runner.rs`, `Segmenter::track`): Flikk segments each
  prompted frame by itself, and binarizes its memory. Flikk tracks the
  frames after the first prompted frame forward from it. It tracks the
  frames before it backward. Each frame attends to a fixed bank of 7
  frames of memory and 16 object pointers:

  - The memory: the nearest prompted frames before and after, then the
    latest tracked frame. Repeats of the latest frame fill the bank.
  - The pointers: the prompted frames passed and the latest tracked
    frame, with the same padding.

  The temporal positions are as `transformers` gives them. The model
  controls occlusion. A hidden object gives an empty matte, and the model
  finds it again when it comes back.
* **Service** (`flikk-analysis/src/subjects.rs`, `LocalSubjects`): frames
  come from the media engine (the proxy when there is one). Flikk numbers
  them on the media's rate as the engine decodes them. Tracking reads
  forward with one stream and backward in chunks of 24.

  Flikk keeps each prompted frame by fingerprint, frame and prompt. Thus
  a frame previewed in the viewer is not segmented again when the click
  lands in a subject. Flikk keeps a tracked subject by fingerprint and
  `Subject::key` (all its prompts). This covers every frame that its clip
  shows and its prompted frames.

  Both keys hold the model's id. Thus, when the user picks the other
  model in Settings → Models, Flikk makes them again. Flikk also makes
  the runners again for that model. Only the runner of its own model
  reads an `Encoded` frame.

  Flikk cleans the mattes: it removes islands and holes under 16 cells.
  It keeps them to 1/16 of a logit up to ±8, and run-length codes them
  (about 6 KB a frame). They stay in memory and under `cache/mattes/`
  (`store.rs`).

  Previews run on their own model instance and keep the last 3 encoded
  frames. Thus selection stays quick while a subject tracks on the other
  instance. The first click on a frame encodes it, in about 2–4 s on a
  laptop CPU. Later clicks take 50–80 ms.

  A tracking job waits 1.2 s before it starts, so some quick clicks run
  one job. A newer subject of the same mask cancels its older job.
  Tracking costs about 4 s a frame on 4 CPU cores. Flikk does not try a
  failed frame or job again until the model is downloaded again.
* **Drawing** (`Matte::field`): Flikk upsamples the logits as the model
  does (bilinear between cell centres) to the mask raster. The edge is
  one pixel soft, from the logits over their slope. `refine` eases it
  onto the decoded frame's luma with a guided filter, within three radii
  of the edge. The radius is a 135th of the height, and ε is 10⁻³.

  Flikk draws a feathered or expanded subject from signed distances in
  place of this, as it does for shapes. These are the logits near the
  edge, and an exact distance transform further out. A subject that has
  no matte yet stays out of the raster. Layout bounds a picture by its
  subjects' mattes.

## Stabilization

* **Measuring** (`flikk-analysis/src/motion.rs`, `LocalStabilizer`, Rust
  alone, no model): Flikk reads the frames one time, in sequence, through
  the media engine (the proxy when there is one). The frames are at most
  320 px wide, in grey, with half-size and quarter-size copies.

  Flikk compares each frame with the frame before it:

  1. The full picture at quarter size gives its overall move (±8 px
     there).
  2. Flikk uses a 10 × 6 grid of 16 px blocks. Only the blocks with
     texture in both directions count (the structure tensor's smaller
     eigenvalue).
  3. Flikk finds each block again by least absolute difference at half
     size, around that move.
  4. Flikk refines each block at full size to a fraction of a pixel (a
     parabola through the neighbours).

  RANSAC (200 rounds, 1 px) fits a move and turn to the blocks. Least
  squares on the blocks that agree then refines them. Thus a subject that
  moves by itself stays out. If too few blocks agree (under 4, or a
  third), or the turn is over 3°, Flikk uses the median move.

  Flikk does not fit scale. Shake zooms very little between frames. Also,
  a fitted zoom was mostly noise that the correction then added back.

  The path (`stabilize::CameraPath`: each frame's time and its step from
  the frame before) goes to `cache/analysis/<fp>.motion`. Flikk keeps it
  with the analysis results: Clear in Settings → Storage does not remove
  them. Paths that Flikk reads stay in memory for the session.

  The measurement runs as a job on its own `JobQueue` (one at a time).
  `Stabilizer::jobs` lists it, and the inspector shows its progress.
  Renders first measure the paths that they need
  (`composed::measure_motion`, part of the progress). The agent's
  `get_shake` waits for it. The measurement takes about 8 s per minute of
  30 fps 720p footage in a release build here, with the decoding.
* **Correcting** (`flikk-core/src/stabilize.rs`, `Plan`): the steps over
  the source range that the clip shows add up into the camera's pose per
  frame. The pose is where frame 0's centre went, and its turn. Flikk
  composes them exactly as similarities.

  Smooth runs a Gaussian over each part of the pose
  (σ = 0.12 s × 25^smoothness, so 0.12–3 s). Flikk reflects each end
  through a line fitted to the σ frames there. Thus a pan continues, and
  the first frames are also smooth. Locked holds the mean pose. Each
  frame moves from where the camera was to where the target puts it
  (W = S ∘ C⁻¹).

  With Auto zoom, the picture zooms in by the least amount that hides the
  edges on every frame of the range. There is one zoom for the clip, so
  it does not pump. The maximum is 1.4 ×. Frames that would need more get
  only the part of their correction that fits (binary search). A running
  minimum, then a Gaussian no wider, eases this part. Thus the path stays
  smooth and still under the zoom.

  Flikk keeps the plans for the last 24 clip settings and ranges.

  The compositor draws the frame's correction into the picture's own
  frame before its effects, masks and the clip's transform. This is
  `compose::Ctx::stabilized`, an extra composite in the same batch. Thus
  the viewer, the agent's views and composited renders draw it alike
  (`needs_compositor` is true for a stabilized clip).

  Until its path is measured, a clip shows unstabilized and asks for the
  path. Until then, the render cache keeps no frame of it
  (`Span::stabilized`, `worker::ready`). Flikk finds the frame by source
  time through `Clip::to_source`. Thus speed changes and ramps find the
  correct frame. A slowed clip can make a moment between two frames
  (blended or optical flow). That moment takes their corrections in
  proportion (`Plan::between`).
* **Checked here** on a 6 s 1280 × 720 clip made with FFmpeg. The clip is a
  still with a sine-and-random crop jitter of ±20 px. It also has a slow
  pan, a ±1° wobble and a picture that moves across it. The steps matched
  the known moves to about a pixel and the turns to 0.0005 rad.

  The shake score is the RMS of what remains of the path after Flikk
  removes moves slower than 0.25 s. It is in percent of the frame width,
  plus the turn. The score went from 1.9 to 0.13 (Smooth, zoom 1.11),
  0.13 (Locked) and 0.09 (no crop). A still clip scores 0.00.
* **Agent:** `stabilize` (Transform) proposes it, keeps the values that
  the agent does not give, and starts the measurement. `get_shake` (with
  the transform group) measures when it must. It gives these values for
  each clip:

  - Its score.
  - Its label: steady under 0.25, slightly shaky, shaky, very shaky over
    2.
  - The jitter in percent and degrees.
  - The zoom that its settings (or the default settings) would take.

  `get_clip` shows the settings.

## Person masks

* **Model** (`flikk-models/src/matting/modnet/`): MODNet (Ke et al.,
  Apache-2.0 code and weights). It is the official photographic portrait
  checkpoint, as Xenova's ONNX export (`Xenova/modnet`, Apache-2.0). It
  is pinned to commit `fa2fa546`, with 25.9 MB in full precision.
  `FLIKK_MODNET` points at a local copy.

  There is one graph. Its input is a picture with its short side at 512,
  scaled to -1..1. The long side is in proportion, at most 1024, and both
  sides are a multiple of 32. Its output is coverage 0..1 at that size.
  Flikk resamples pictures by area averaging.
* **Service** (`flikk-analysis/src/people.rs`, `LocalPeople`): it is
  behind `LocalSubjects`' person methods of `SubjectService`. It shares
  the media engine and version of `LocalSubjects`.

  Flikk reads frames at a 512 short side through the media engine (the
  proxy when there is one). It mattes them in chunks of 32. It keeps them
  by fingerprint, model and chunk, as run-length coded bytes. That is
  about 30 KB a frame for a 768 × 512 matte, so about 55 MB a minute at
  30 fps.

  The chunks stay in memory (96 chunks) and under `cache/mattes/`. The
  Cache in Settings → Storage clears them. Every person mask on the same
  media shares them.

  One worker serves the newest request first. When Flikk draws a frame,
  it asks for the 3 s from that frame on, so playback finds them made. A
  render asks for the frames of its range that it shows, and waits
  (`complete_person`). When the newest request wants another chunk, the
  worker stops the current chunk and continues it later.

  If a frame is not made yet, Flikk shows the nearest made frame within a
  second. Else the mask stays out. Flikk eases each frame toward the
  frame before it. It eases by half where they agree, and not at all
  where the coverage moved by 0.25 or more. This stops the flicker on
  still edges.
* **Drawing** (`PersonMatte::field`): Flikk upsamples the coverage
  bilinearly to the mask raster. In the band where the neighbourhood is
  mixed, `edge` eases it toward a guided filter of itself on the decoded
  frame's luma. The radius is a 270th of the height times 1 + 3 × edge.
  ε is 10⁻⁴ + 10⁻³ × (1 − edge). Thus strands that the small matte blurs
  come back, and halos of background tighten.

  Expansion is a grey dilation (or erosion) by a square. Feather is three
  box blurs. Thus both keep the soft edge. Layout bounds a picture by its
  people.
* **Speed, here** (4 CPU cores, release build): MODNet takes 0.27 s for a
  768 × 512 frame. Through the service with decoding, it takes 0.34 s a
  frame. The edge takes 0.1 s on a 1024-wide raster. A 2 s clip of 60
  frames was matted in 17 s. After that, Flikk read it from the cache in
  under a millisecond.

  On the CPU, playback is faster than the matting. The frames ahead become
  available as it continues.
* **UI:** a person button (tooltip "Remove background") is in the
  inspector's masks section and in the viewer's mask tools. It adds the
  mask to the clip, or selects the mask that the clip has. Its row has
  the usual mode, invert and keyframe, and an Edge slider. A click on a
  person in mask mode selects the mask. Without the model, a notice leads
  to Settings → Models.

## Depth maps

* **Model** (`flikk-models/src/depth/depth_anything/`): Depth Anything V2
  Small (DINOv2 ViT-S and a DPT head; Apache-2.0). The Base and Large
  weights are CC-BY-NC. The model is onnx-community's
  `depth-anything-v2-small` export in full precision (99 MB,
  `onnx/model.onnx`), pinned to a commit. `FLIKK_DEPTH_MODEL` points at
  another copy.

  Flikk decodes pictures 640 wide. It resamples them bilinearly to 518 on
  the longer side, with both sides a multiple of 14 (518 × 294 for 16:9).
  It normalizes them as ImageNet. The output is relative disparity at
  that size. The model was trained at 518 on the shorter side. The longer
  side at 518 costs a third as much on the CPU.
* **Speed**, measured here (4 CPU cores, ONNX Runtime's CPU provider):

  - 0.55–0.58 s a frame at 518 × 392 (a 4:3 picture).
  - 0.40 s at 518 × 294 (16:9; with Python's ONNX Runtime on the same
    graph).
  - The first run: 1.4 s, with the load.
  - 62 frames of a 640 × 480 clip in 35.6 s through the service.

  Thus a second of 30 fps footage takes 12–18 s.
* **Service** (`flikk-analysis/src/depth.rs`, `LocalDepth`): frames come
  from the media engine (the proxy when there is one). Flikk numbers them
  on the media's rate (an image is one frame).

  Flikk spreads the disparity of each frame over its own 2nd to 98th
  percentile. The reason is that the model's answer has an unknown scale
  and shift per frame. Flikk changes the disparity into distance and
  keeps it at 384 on the longer side. It keeps it as bytes (about 80 KB a
  frame) under `cache/depth/` and in memory.

  Flikk smooths the map shown for a frame over time with the two frames
  on each side. The weight depends on how near they are (σ 1.2 frames).
  Pixel by pixel, it also depends on how well each frame agrees after a
  least-squares fit onto this frame (σ 0.08). Thus moving things keep
  this frame's depth, and the range becomes steady. When all neighbours
  are made, the result is fixed. Thus a render matches the viewer,
  because the render makes them all first.

  A request for a frame queues it, the next two seconds and the frames
  before it that the smoothing reads. These go before what was queued,
  on one worker thread. Meanwhile, the nearest made frame within 12
  replaces it. Flikk does not try a failed frame again in the session.
  Settings → Storage clears the maps with the Cache.
