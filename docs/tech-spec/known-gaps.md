# Known gaps

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

## Analysis accuracy

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
