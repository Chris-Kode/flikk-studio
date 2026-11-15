# Known gaps

* The pending change sets and the undo history are in memory only. After
  a restart, they are lost.
* We wrote the Codex backend from its docs. We did not verify it end to
  end, as we did for `claude -p`.
* There is no GPU video surface. The viewer paints read-back frames as
  images. The Windows and macOS builds did not run here.
* These P1/P2 PRD items are not built:
  * nested sequences;
  * a music playlist UI (the `SetPlaylist` command and the agent tool
    exist);
  * voice cleanup;
  * voice input.
* Mouse and keyboard interactions (drags, drops, context menus,
  shortcuts) had only two checks: code review and screenshots of seeded
  states.
* Blockouts:
  * Only the agent's `generate_video` takes a blockout. The Video form of
    the media pool does not take one.
  * The user can ask for a scene on the card, or change a scene in the
    dialog. The run that writes this scene does not see it. That run has
    no tools. It reads the review of the scene instead. The review tells
    where things are. It does not tell if a limb bends in the correct
    direction.
  * The feet move with the cycle that their writer gave. Nothing solves
    them against the ground. Thus the feet slip where the round of the
    cycle does not match the stride.
  * A scene with built characters takes a long time to write. For a shot
    with a creature of approximately twenty parts, the backend used more
    than three minutes.
  * The pool and the project do not keep a blockout. The blockout is in
    the waiting request. Its clip is in the upload scratch.
  * The step that makes a picture for a tile had a code review only. This
    step needs the key of an image provider.
  * In a debug build, the editor draws the scene coarse. Unoptimized code
    draws approximately twenty times more slowly.
* Preview and export are different:
  * The preview of noise reduction uses an expander. The export uses
    `afftdn`.
  * The limiter and the time-stretch are slightly different in shape.
* The FX code editor has no syntax highlighting, because the `syntect`
  feature of gpui-kit is not vendored. A shader that never returns can
  stop the GPU.
* An external agent can propose a program. Its later connections can use
  that program only after the user accepts it.
* If the editor crashes, its headless browser can continue to run.
* ElevenLabs:
  * Here, the only checks against the live API used an invalid key. They
    covered the account, the transcription upload, music, sound effects,
    voiceovers and the voice list. Each one answered "invalid key".
  * No request ran with a working key. Thus no test measured the word
    times of Scribe against the LibriSpeech set above.
  * No test tried the music output formats that each plan allows. The
    client starts at 320 kbps MP3. If the API refuses a format, the client
    tries a lower one.
  * The instrumental chunks of the music composition plan come from the
    docs only.
  * Nobody heard if Eleven v4 ever reads an audio tag aloud.
  * The voice calls come from the API reference only. They ran against no
    account. These calls are: shared library search, add, similar voices,
    design, clone, rename and delete.
  * Some reference pages of the shared library did not load. For those
    pages, the query and reply fields come from memory.
  * Nobody heard if Eleven v4 reads designed voices and instantly cloned
    voices as well as the voices of the provider.
* The fix of words had an end-to-end check here. The flite voice of
  ffmpeg replaced the provider. Its take was louder and brighter than the
  original.
  * The check covered these items:
    * the words;
    * the context;
    * the cuts;
    * the level and tilt match;
    * room tone;
    * the `ReplaceSpeech` layout: same length, longer with the picture
      slowed, shorter with a ripple;
    * undo.
  * No ElevenLabs request ran. Thus it is not known if Eleven v4 accepts
    `previous_text`/`next_text`. If a refusal names them, the client sends
    the request again without them.
  * Nobody heard how well an instant clone matches the speaker in a
    single word.
  * The aligner was not downloaded here. Thus only the levels gave the
    times of the words in the take.
  * The match is a level and a tilt. It is not a full EQ match or reverb
    match.
  * The picture does not sync again to the new words. The lips move as
    they did before.
  * If a new word is longer than the old word, the picture under it slows.
    This is visible on a long insert.
  * The editor cannot fix words in sped-up, slowed or ramped clips.
  * The new words appear in the transcript after their analysis ends (a
    few seconds).
* CPU fallbacks (no GPU adapter) are slow in debug builds. On machines
  without a GPU, use release builds.
* On systems other than macOS, subject tracking runs on the CPU. It uses
  the CPU provider of ONNX Runtime, as every model here does. A frame
  takes approximately 4 s on 4 cores. Thus a minute of 30 fps footage
  takes hours. We did not try the GPU providers (DirectML, CUDA). If the
  prompt changes, the tracker tracks the full clip again, not only from
  that frame.
* The MLX runner:
  * It did not run on a Mac. We checked its numbers on Linux, with MLX on
    the CPU. Thus its speed on Apple GPUs is not measured.
  * MLX loads its Metal kernels from `mlx.metallib`. mlx-sys builds this
    file under `~/.mlx/lib/` (or `MLX_RS_METAL_PATH`) with the Metal
    compiler of Xcode.
  * A packaged app must ship this file next to the binary. MLX looks
    there first.
  * Its weights also come from the repository of a third party (pinned).
* A subject mask follows one object. Several objects need several masks.
  The matte is the 256² logits of the model, stretched over the picture.
  Thus fine detail (hair) depends on `refine`. `refine` only softens the
  edge.
* The SAM 2.1 graphs come from the Hugging Face repository of a third
  party (pinned to a commit). A mirror under the account of the project
  can keep them reachable.
* Hardware encoding:
  * It did not run on a real hardware encoder. This container has no GPU.
    Thus the probe found no encoder. Its FFmpeg 6.1 lists NVENC, Quick Sync
    and VA-API. The trials fail, as they should.
  * A stand-in `FLIKK_FFMPEG` that accepts `h264_nvenc` checked the
    fallback. It also checked the NVENC argument path (graph, piped
    composited frames, audio).
  * VideoToolbox and AMF come from the encoder options of FFmpeg only. We
    only parsed the `hwupload` tail of VA-API.
  * We did not measure the CRF-to-QP/bitrate mapping against libx264 (no
    VMAF runs). We did not measure the speed-up.
* Speed ramps and optical flow:
  * Optical flow runs on the CPU (ONNX Runtime) only. Here, RIFE took
    approximately 0.25 s for a 256 × 128 frame.
  * At 720p and 1080p, a frame probably takes seconds. Thus flow is slower
    than playback, and the viewer shows blended frames until the flow
    frames exist. Flow also makes renders slower.
  * No GPU, Mac or Windows was available for a measurement.
  * The editor keeps frames raw (approximately 3.7 MB each at 720p). They
    accumulate until the user clears Previews.
  * The viewer makes flow from proxies. Renders make flow from originals.
    Thus the two match in motion, but not pixel for pixel.
* Speed ramps and audio:
  * Ramped audio in the viewer (our WSOLA stretch) and in the render
    (`atempo` moved in 50 ms steps) are slightly different in shape. The
    same difference occurs for one speed.
  * OTIO and FCPXML exports do not include ramps. They also do not include
    speed.
  * The protected-range check sees the two halves of a split ramped clip
    as different pieces. Thus it can warn on a split that changes nothing.
  * Agent views (stills) show blended frames until the flow frames exist.
* The RIFE graph comes from the Hugging Face repository of a third party
  (`walterlow/RIFE_fp32_timestep`, pinned to a commit). A mirror under the
  account of the project can keep it reachable.
* Person masks:
  * MODNet runs on the CPU. Here, a 768 × 512 frame takes 0.27 s on 4
    cores. Thus the preview is not real time without a GPU provider.
  * We did not try a GPU provider. We did not try macOS and Windows.
  * The ONNX file comes from the repository of a third party (Xenova,
    pinned to a commit). A mirror under the Hugging Face account of the
    project can keep it reachable.
  * The smoothing over time starts again at a chunk that was made before
    the chunk ahead of it. Thus the first frame of a chunk can step.
  * MODNet learned from portraits. These cases leave holes or halos: full
    bodies far from the camera, several people that overlap, and people
    against busy backgrounds of their own colour. An Expand value below 0
    trims a halo.
  * We checked its quality by eye on one photograph and on a 2 s clip
    from that photograph. We did not measure it.
* ONNX Runtime adds approximately 25 MB to the binary. The first build
  downloads it (from pyke's CDN). When unoptimized, its build script
  unpacks the archive very slowly. With
  `CARGO_PROFILE_DEV_BUILD_OVERRIDE_OPT_LEVEL=3`, the first debug build
  takes minutes, not hours.
* Sync by sound:
  * We measured it only on material that FFmpeg made from synthetic
    speech (espeak-ng voices over room tone and tones). We did not measure
    it on real recordings from two devices in a room.
  * We did not try music-only overlap, very reverberant rooms, or
    recordings made far apart (sound that arrives 10+ ms later).
  * The measurement keeps the two onset envelopes in memory. Each envelope
    takes approximately 1.4 MB per hour. The coarse correlation of two
    recordings of one hour takes approximately 35 MB more.
  * The measurement reads each recording once in full, and then once more
    in windows. It shows no progress and has no cancel.
  * An hour takes approximately 10 s per recording in release. In debug
    builds, it takes approximately two times as long.
  * The editor finds linked clips by media, start, source in and speed.
    Thus a camera with its sound moved apart from its picture syncs alone.
  * The multi-selection of the pool is for sync only. A drag still takes
    one item.
  * It did not run on macOS or Windows.

* Stabilization:
  * We checked it only on shake that FFmpeg made (a fractal still,
    jittered by crop and rotate). We did not check it on real handheld
    footage.
  * We did not measure these cases:
    * rolling shutter wobble, motion blur and low light;
    * a subject that fills the frame;
    * a fast pan past the search (approximately 10 % of the width a
      frame).
  * The correction is a similarity. Thus it cannot remove rolling-shutter
    skew.
  * The code does not measure scale. Thus a shaky zoom does not become
    steady.
  * Subject and person masks come from mattes of the unstabilized frame.
    Depth effects and masks come from its depth map. Thus on a stabilized
    clip, they are offset by the correction. Shape masks follow the
    stabilized picture.
  * It did not run on a GPU here (only lavapipe and the CPU compositor).
    It did not run on macOS or Windows.
* The music fit:
  * We checked it on a synthetic 120 BPM song: intro, A, B, A, outro, and
    a final chord that rings for 5 s. We built the song with NumPy.
  * The check used the true downbeats of the song and the downbeats of
    Beat This!.
  * Every target from 20 s to 130 s landed exactly.
  * Joins showed no clicks. The largest sample step at a join was 0.03,
    against 0.81 inside the song.
  * With true downbeats, where the length allowed a 4-bar move, jumps
    paired the same chords in the same section (85–96 %).
  * Beat This! finds bars in the ring of the final chord. With its
    downbeats, the ending had approximately a bar to give. Some lengths
    took a weaker jump (25–60 %).
  * We did not check it on real songs, or by ear.
  * No length leaves the meter, because a jump moves whole bars. If whole
    bars cannot reach the target, the song ends up to half a bar off. Or
    the song trims the ring of its ending.
  * We checked the context menu by a build only, not on screen.
  * Each fit calculates the bar features again. No cache keeps them.
* Render cache:
  * Keys cover the project and the styles of the editor. They do not
    cover files behind them that the editor reads by path. Examples are
    the file of an image parameter, a LUT changed in place, and analysis
    data outside the project. Such a change shows only after the user
    clears the cache.
  * The app version is the key for built-in shaders and templates. Thus a
    development build that changes one of them shows old frames until the
    user clears the cache.
  * Absolute time is the key for frames. Thus a clip move (or a ripple)
    renders the moved range again.
  * We did not measure the disk reads of playback at real time on a GPU
    machine here, because the cloud containers have no GPU. We did not
    measure them on macOS or Windows.
  * We timed JPEG coding only on still frames, on the CPU.
  * The timing of the worker at the render size sets which spans are
    cheap. The viewer size has no effect. The editor finds these spans
    again in each session.
* HDR tone mapping:
  * We checked it only on PQ and HLG clips that FFmpeg made (`zscale`
    from SDR test sources, x265 with HDR10 metadata).
  * Against the frames of the viewer, exports match approximately as well
    as SDR exports do (41 dB).
  * The LUT method and the zimg method agree to 51–54 dB on smooth
    pictures. On saturated hard edges, they agree to approximately 30 dB,
    because the 65³ table is coarse there.
  * We did not try real phone footage (iPhone Dolby Vision 8.4 HLG,
    Android HLG, Samsung HDR10+).
  * We did not try Dolby Vision profile 5 files (IPT, no
    backward-compatible base layer). They still look wrong.
  * We did not try dynamic HDR10+ metadata. The editor ignores it.
  * We did not try HDR stills (AVIF, HEIC). The editor does not map them.
  * We did not try `libplacebo`/`tonemap_vaapi`. The editor does not use
    them, because they need a GPU device.
  * Items imported before this change keep no `Hdr` until the user
    imports them again.
  * The file name of a proxy holds the mapping, but not the method. Thus
    after a switch to the LUT, the editor uses a proxy made with zimg.
  * Collages always show the default tone map.
  * If HDR footage plays before its proxy exists, the editor maps it at
    full resolution. This is slow for 4K on a CPU.
* Depth maps:
  * Depth maps run on the CPU on all systems (the CPU provider of ONNX
    Runtime). We did not try CoreML, DirectML or CUDA.
  * A frame takes 0.4–0.6 s on 4 cores. Thus playback over footage
    without maps shows the effects late. Until then, the map of the
    nearest finished frame replaces the missing map. If there is no such
    map, the picture shows as it is.
  * A render makes the maps first. This can take much longer than the
    encode.
  * They did not run on a Mac or Windows, or on a real GPU. The depth
    shaders compiled and rendered here on lavapipe only.
* Depth is relative and per picture:
  * `focus`, `start` and mask ranges are fractions of the depth range of
    each shot. They are not distances.
  * Thus the same value can land in a different place after a cut. If the
    range of a shot changes (something walks near the camera), the values
    shift.
  * The smoothing keeps the range steady over ±2 frames only.
  * Maps are 8-bit at 384 on the longer side. Thus fine detail (hair,
    fences) is soft, and the lens blur can make a halo there.
* The 3D Camera fills the gaps that it uncovers. It stretches the edge
  next to each gap (no inpainting). Thus large moves on pictures with near
  objects smear. For those pictures, keep `amount` low.
* The ONNX file of the depth model comes from the Hugging Face repository
  of a third party (onnx-community, pinned to a commit). A mirror under
  the account of the project can keep it reachable.
* If the process exits while the depth worker is inside the model, ONNX
  Runtime can print an error during its shutdown.

## Analysis accuracy

* Sync by sound:
  * The test used a 10-minute camera recording (AAC in MP4) of synthetic
    speech over room tone, and a release build.
  * An offset is the source time of the follower for the source time of
    the reference (`offset + r · (1 + drift)`).
  * The table gives the error at three points of the overlap.

  | Follower | Truth | Measured | Error |
  | --- | --- | --- | --- |
  | Recorder (24-bit WAV) started 83.4567 s earlier | +83.4567 s | +83.45670 s | < 0.01 ms |
  | Second camera started 95.1234 s later, 300 Hz–3 kHz, −12 dB, white noise, AAC | −95.1234 s | −95.12334 s | 0.06 ms |
  | Recorder that covers 6:40–15:00 only (MP3), partial overlap | −400 s | −400.00001 s | 0.01 ms |
  | Recorder 0.0104% fast (`asetrate` 48,005 Hz) from 20.5 s earlier | +20.497865 s, drift −1.0415e-4 | +20.49790 s, −1.041e-4 | 0.04–0.06 ms |
  | 4 min of unrelated talk and 2 min of room tone, then the camera's sound (44.1 kHz) | +366.4509 s | +366.45090 s | < 0.01 ms |
  | Pink noise 3 dB under the speech (mean levels), three echoes (40–120 ms), −6 dB | −30 s | −30.00000 s | < 0.01 ms |
  | 20 s excerpt from 4:10 | −250 s | −250.00001 s | 0.01 ms |
  | Unrelated talk (same voices) | no match | confidence 0.09 | rejected |

  Every match got confidence 1.00, and all its fine windows agreed. We
  rendered the synced recorder through `SyncClips` and the WAV preset.
  This includes the recorder with drift, through the resampler. A new
  measurement against the camera gave an offset of 0.04 ms or less, and
  no drift. A 10-minute pair takes approximately 5 s in release
  (envelopes 2 s each) and 10 s in debug.

* Cuts in connected speech:
  * Connected speech has no pause between words. There, more than half of
    the cuts still clip the tail or the onset of a word by more than
    20 ms.
  * This result is on hand-labelled English. Before the aligner, the
    result was almost 9 in 10.
  * A person can possibly hear these clips. The results of the agent say
    this.
  * The 80 ms steps of the aligner are the limit.
    `facebook/wav2vec2-base-960h` (Apache-2.0, English only) left 4 points
    more cuts clean.
  * We tried the DTW timestamps of whisper.cpp. On real speech, they were
    0.2–0.9 s late. Thus they are off.
* Languages:
  * Only English had a measurement against human labels.
  * We compared Spanish read speech only with another aligner. The two
    agree approximately as well as on English read speech.
  * The editor aligns Japanese and Korean words as Whisper splits them. It
    does not align them as the word segmenters of Qwen would.
  * Dutch and each other language outside the 11 keep the times of
    Whisper, as measured on the audio.
* The aligner needs approximately 600 MB of memory in addition to
  Whisper. The peak is 1.2 GB while it runs on 30 s. It also needs a
  589 MB download and ~4 s to load. It runs at approximately 6 s per
  minute of audio on 4 CPU threads.
* Speakers:
  * We measured speakers on four AMI test meetings: four people each,
    headset mix, and the word-based reference of pyannote.
  * In these meetings, 4.1% of the speech of one person at a time went to
    the incorrect voice.
  * On the Whisper words of two meetings, 98% of the words that one
    person said alone got a speaker. Of these, 98.5% were correct.
  * Of the words that a person said over another person, 59% got a
    speaker. Of these, 96% were correct.
  * We weighed overlaps by the windows. This labelled 8 points more of
    these words than when we left ties unattributed. The fraction of
    correct labels did not decrease.
  * The editor tells voices apart within one item only. Across items,
    "Speaker 1" is not the same person.
  * Diarization runs at approximately 3% of the length of the audio on 4
    CPU threads (10 s for 5 minutes).
  * The clustering fits on 2,000 voiceprints at most (approximately an
    hour of conversation). It matches the rest to its voices.
* We checked the sound models on a 64 s collage of real recordings
  (speech, ESC-50 effects, a drum loop). We did not measure them against
  labels. They found laughter, glass, knocks, a dog, applause, the music
  and its tempo. They did not find rain under speech. Sounds that the
  tagger is not sure of have "?" in the script.
* Silero:
  * Silero misses some quiet or short speech.
  * On 5 minutes of an AMI meeting (headset mix), its ranges covered 89%
    of the labelled speech. They left 13 s of it in silences of 0.5 s or
    more.
  * The energy VAD covered 92% and left 5.5 s. But it called 33 s
    outside the labels speech. Silero called 5 s.
  * Where both found speech, the ranges of Silero started 10 ms later and
    ended 30 ms later (medians).
  * Whisper can still invent words over sounds that Silero takes for
    speech. The script shows them as speech, with "?" when it is not
    sure.
* Transcription:
  * We measured it on LibriSpeech test-other and dev-clean, with
    forced-aligned word times from `gilkeyio/librispeech-alignments`.
  * We made these sets into items of four types:
    * items that start to talk at once;
    * items that continue for minutes with short pauses;
    * items that fade out at the end of every phrase;
    * items that pass between two to four speakers.
  * 5,190 words went through the full pipeline, with Whisper small on 4
    CPU threads.
  * These changes took the word error rate from 7.7% to 6.7%:
    * the word edges, measured only where the aligner cannot measure
      them;
    * the silence before a stretch that fits one window;
    * the beam search;
    * the holes, heard again.
  * The same changes took the words left out from 66 to 38. At the end of
    a phrase, they went from 28 to 12. At its start, they went from 12 to
    7. Both items that lost their first word kept it.
  * Words heard on the far side of the pause between two phrases
    decreased from 26 to 8.
  * All of it takes approximately 15% more time on the CPU. The beam
    search is most of that time.
  * The editor told speakers apart as before (92% of the words in the
    conversations).
  * Whisper still sometimes skips a short phrase. It puts the words of
    the next phrase in its place. This leaves no hole.
  * We moved the edges of the aligner next to a pause. For the ends there
    that came more than 30 ms early, this took the fraction from 40% to
    25%.
  * For the starts that came more than 30 ms late, it took the fraction
    from 40% to 23%. The number within 50 ms stayed the same.
  * Silences that keep the words clipped leave 8 of 50 last words that
    fade more than 50 ms off. Before, this number was 15.
* We verified the Beat This! export here from a mirrored copy of the
  `final0` weights. Build the hosted file from the official checkpoint.
  `export_beat_this.py` downloads it from the server of JKU.
