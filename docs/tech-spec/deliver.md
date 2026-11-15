# Deliver

- **Render graph** (`flikk-deliver/src/graph.rs`): Each job has one FFmpeg
  `filter_complex`, and the sources open inside it. The graph contains
  these items:
  - Cuts and transforms.
  - Keyframes as FFmpeg expressions that match the curves of the viewer.
  - The grade as a `lut3d`.
  - Captions and titles, burned in with ASS (libass). The viewer letters
    them with the same timing code in `captions/motion.rs`. The render
    also uses this code when FFmpeg has no `ass` filter. It then sends the
    frames through the compositor with `Letterer`.
  - Gain, fades, equal-power crossfades and `atempo` retiming.
  - Ducking with `sidechaincompress`, audio effects and two-pass
    `loudnorm`.

  Under a speed ramp, the audio goes through `atempo` stages. There are
  sufficient stages to keep each stage at 0.5 or more. An `asendcmd`
  script in the work folder changes their tempo every 50 ms of timeline.
  The stretch of the viewer reads its grains where the ramp has the source
  (`Stretch::mapped`). If a ramp is muted, the graph does not include the
  clip.
- **HDR** sources get the same tone mapping as in the viewer
  ([Media and playback](media-and-playback.md)). It occurs after the trim
  of the segment and before its grade. Composited ranges get it from the
  media engine. The code tags video outputs as BT.709.
- **Composited ranges** (`composed.rs`) are ranges that need the
  compositor (`compose::needs_compositor`).
  - These ranges have effects, generators, transitions, blend modes, crop,
    flip, stretch/skew/anchor, corner pins, masks, or retimed or ramped
    media.
  - `compose::program` draws them from the original media. They go into
    the graph through a pipe as raw RGBA, so the captions and audio stay.
  - The code makes optical flow frames when it composes each frame
    (`speed::Waiting`). It never uses blended frames in their place.
  - First, the code finds (and tracks) the subject masks in the range. It
    also mattes the frames of the person masks in the range.
    `SubjectService::complete` and `complete_person` use half of the
    progress.
  - If the code cannot find a subject, the render fails. This is also true
    for people without their model.
  - A render of a person mask had a mean difference of 1.8/255 from the
    frame of the viewer (H.264 included).
  - Then the code makes each depth map that the depth effects and masks of
    the range read (`DepthService::complete`). These are the frames that
    their clips show there, with transitions, and the frames that
    smoothing reads. This step uses half of the remaining progress. Thus
    the frames compose as the viewer shows them. If the code cannot make a
    map, the render fails.
- **Queue** (`queue.rs`): Jobs run one after the other. Their progress
  comes from `-progress pipe:1`. The user can cancel a job. Jobs never
  overwrite a file.
- **Hardware encoding** (`hw.rs`): H.264 and HEVC go to a hardware
  encoder. The encoders are VideoToolbox (macOS), NVENC, Quick Sync,
  VA-API (Linux) and AMF (Windows).
  - The code uses an encoder when two conditions are true. The installed
    FFmpeg lists the encoder (`ffmpeg -encoders`). A trial encode of five
    frames with the arguments of the render is successful.
  - The probe runs one time in each session, off the UI thread. The trials
    run in parallel (approximately 0.1 s here).
  - The CRF of the preset maps to the rate control of each encoder:

    | Encoder | Rate control |
    | --- | --- |
    | NVENC | `-rc vbr -cq` |
    | Quick Sync | `-global_quality` |
    | VA-API and AMF | Constant QP, one step below the CRF |
    | VideoToolbox | An average bitrate from the CRF, size and rate. Its constant quality is only for Apple silicon. |

  - swscale changes the frames to 4:2:0 as for libx264. Thus the matrix
    and colour tags match. VA-API then does `hwupload` at the end of the
    graph.
  - Composited ranges send RGBA through a pipe into the same graph. Thus
    they also encode on hardware.
  - The Deliver workspace has an Encoder choice (Auto, Hardware,
    Software). The store keeps it for each user (`Store::render_encoder`).
  - If the hardware encode fails, Auto uses libx264 / libx265. Hardware
    fails instead.
  - For the agent, `queue_render` takes an optional `encoder`. If there
    is no `encoder`, it uses the choice of the user.
- **Presets:** 1080p60, 1080p30 and 4K30 H.264, 4K30 HEVC (x265 CRF 22),
  ProRes 422 HQ, WAV and Custom. The short side sets their size, so they
  keep the shape of the sequence ("1080p" is 1080 × 1920 for 9:16).
- **Exports:** SRT, ASS, OTIO, FCPXML and PNG frames.
