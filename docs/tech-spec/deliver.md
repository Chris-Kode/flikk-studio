# Deliver

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
* **HDR** sources go through the same tone mapping as in the viewer ([Media and playback](media-and-playback.md)),
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
