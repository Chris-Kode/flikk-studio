# Media and playback

* `flikk-media` does these tasks:
  * ffprobe. It snaps variable rates to standard rates.
  * Fingerprints.
  * Single frames, with a 96 MiB in-memory cache.
  * Playback streams (RGBA frames as PAM over a pipe) and f32 PCM streams.
  * Proxies: short side ≤ 720, x264 veryfast CRF 26, no B-frames,
    half-second GOP.
  * 6 × 5 collages, stamped with timeline and source timecodes.
  * Waveform peaks (200/s).
  * Spectrogram pictures (`showspectrumpic`). Each track has one lane. The
    picture marks words, sounds and beats.

  `flikk-media` writes derived files atomically.
* Cache layout under `paths::cache()`:
  * `proxies/<fp>.mp4`
  * `waveforms/<fp>-<stream>.peaks`
  * `collages/`
  * `analysis/<fp>.json` and `<fp>.vectors`
  * `analysis/<fp>.motion`: camera paths
  * `mattes/<fp>/`:
    * subjects: `p-<key>.matte` for each prompted frame, and
      `t-<key>.mattes` for each tracked subject
    * people: `a-<model>-<chunk>.alpha` for each 32 frames
  * `interpolated/<generation>/`: optical flow frames at preview sizes, as
    QOI (see below)
  * `depth/<fp>/<model>/`: `<frame>.depth` for each source frame, a
    grayscale PNG
  * `coreml/<model>-<sizes>/`: depth models compiled for the GPU, 420 MB
    for each size
  * `html/`: pages and browser profiles
  * `render/<job>/`: scratch
  * `agent/frames/`: images for the activity log
  * `frames/`: stills that go to video generation
  * `frame-cache/<generation>/<ab>/<key>.frame`: the render cache (see
    below)
  * `hdr/`: tone-mapping tables
  * `respeak/`: the takes of a fixed sentence from a provider. They stay
    until the words are cut out of them.
  * `voice-sources/`: speech cut as FLAC for a provider to clone or match.
    The voices dialog removes these files.

  The proxy of an HDR item is `proxies/<fp>-<mapping>.mp4` (see below).
* **Disk caches** (`flikk-core/src/disk_cache.rs`).
  * The render cache and the optical flow frames are files under
    `<root>/<generation>/<ab>/`. A 16-byte key gives the name of each
    file. Each file starts with a magic and its key.
  * The code writes each file to a temporary file and then renames it.
  * An in-memory index removes the least recently used files past a size
    limit. The index is read from the folder on first use. It is ordered
    by modified time, and reads touch this time.
  * When the code opens one disk cache, it removes the other generations
    under its root. Thus a new layout or app version removes the old files
    immediately.
* **Sweep** (`flikk-core/src/cache_sweep.rs`). At startup, off the UI
  thread, the sweep removes these files:
  * Files that were not used for 30 days, from `proxies/`, `depth/`,
    `collages/`, `waveforms/`, `frames/`, `agent/`, `voices/`,
    `voice-previews/`, `hdr/` and the pages in `html/`.
  * Scratch (`voice-sources/`, `upload/`, `respeak/`, `render/`) and
    partial writes (`.part`, `.tmp`, `.tmp-…`) that are one day old.
  * Proxies of a fingerprint that are one day older than its newest proxy.
    These proxies were made for a tone map that changed after.

  Readers mark files as used (`mark_used`). This sets the modified time, a
  maximum of one time each day. The sweep does not remove mattes, analysis
  results or the disk caches. If the proxy of a video is missing, the pool
  makes it again.
  * Writes to the user's data go through a temp file with a unique name
    (`flikk_os::fs`). The user's data is library items, settings,
    credentials, the agent's stores, chats, proposals, autosaves and
    backups.
  * If a crash leaves such a temp file, the code removes it one day later
    (`flikk_os::fs::sweep_temps`). It also removes library item names that
    were claimed and never written, one day later.
* **HDR** (`flikk-core/src/hdr.rs`).
  * **Probe.** The probe marks a video stream with an `Hdr` when its
    transfer is PQ or HLG:
    * PQ is `smpte2084`: HDR10 and Dolby Vision.
    * HLG is `arib-std-b67`: iPhone and Android HLG, and the base layer of
      Dolby Vision 8.4.

    The `Hdr` holds the primaries, matrix, range and bit depth. It also
    holds the peak from MaxCLL or the mastering display, read off the
    first frame. It also holds a Dolby Vision profile. The pool badges the
    stream "HDR".
  * **Tone map.** Each item has a `ToneMap`: Auto (the default) or Off, an
    exposure in stops and a peak in nits.
    * `Command::SetToneMap` sets it. The command comes from the Color tab
      of the inspector or from the agent's `set_tone_map`.
    * The `ToneMap` gives a `Mapping`. The filters of the `Mapping` go on
      every decode of the original file: frames, playback, proxies and
      collages. Collages use the default tone map.
    * The filters also go on every segment that the render graph opens.
    * The mapping occurs before any scaling, so the viewer and the export
      match.
  * **Steps.** The mapping does these steps:
    1. The light of the source is PQ absolute, or HLG on a 1,000 nit
       display. For HLG, system gamma applies to each channel, as zimg
       does it.
    2. The code divides this light by the nits that land on SDR white.
       This value is 100, halved for each stop of exposure.
    3. The code takes the light from BT.2020 to BT.709 primaries.
    4. FFmpeg's Möbius curve (knee 0.3, `desat` 0) brings the peak of the
       source to white on the brightest channel. For PQ, the peak is the
       metadata, else 1,000. For HLG, the peak is 1,000.
    5. The inverse of BT.1886 encodes the result.

    The output is BT.709 limited-range 10-bit 4:4:4, with tags for these
    values. Exports are tagged BT.709.
  * **Engines.** If the installed FFmpeg has `zscale` and `tonemap`, zimg
    does the steps. The code checks this with `ffmpeg -filters`, one time
    for each process.
    * If not, the code bakes the same steps into a 65³ `.cube` over the
      BT.2020 signal (`cache/hdr/`). Then `lut3d` applies the `.cube`.
    * `FLIKK_TONEMAP=lut` forces this second method.
  * **Proxies.** Proxies of HDR items are made already mapped, one for
    each mapping.
    * A change to the tone map of an item starts its proxy again (the poll
      of the pool).
    * Until the proxy exists, the code decodes and maps the original.
  * **Keys.** Frames between source frames (optical flow) are made from
    mapped pictures, proxy or original. The mapping is part of their key.
    The globals of the render cache hold the method. Its clip keys hold the
    tone map of the item.
  * Hardware encodes keep the BT.709 tags.
* **Viewer** (`ui/viewer*`).
  * **Playback.** Each playback stream reads 4 frames ahead on its own
    thread (`compose/pictures.rs`). The player submits frame N+1 before it
    reads N back. Audio plays through cpal.
  * **Resolution.** If frames cost more than the frame time, the player
    drops to 1/2 or 1/4 resolution. When they cost less than half of it,
    the player probes back up.
  * **Frame cache.** Paused and full-resolution frames are cached
    (768 MiB, LRU). The key is the project version and the previewed change
    set. When the playhead rests, the 4 s after it render ahead.
  * **Tools.** The viewer zooms and pans, with a safe-area overlay.
    * Gizmos move, scale, stretch, rotate, skew and corner-pin one or more
      items, with snapping.
    * A mask mode draws and edits masks. It shows the selected picture
      whole. A tint shows what its masks cut away
      (`compose::MaskView::Tinted`, a toggle).
  * **Subject tool.** When the mouse moves, the tool shows a preview of
    what a click will keep.
    * The preview runs off the UI thread, and the newest hover wins.
    * A click keeps the subject. A drag puts a box around it.
    * On the picked subject, a click adds a point to the prompt of the
      frame. Alt makes a point off the object. Shift makes a new subject.
    * A click on a point removes it. Points also drag.
  * When the version of the subjects changes, the viewer drops cached
    frames. Thus mattes show as they are made.

## Render cache

`flikk/src/render_cache*` keeps heavy frames on disk. Thus playback stays
real time for all edits, also very heavy edits. Compare Resolve's smart
cache and Premiere's render bar.

* **What it renders:** the ranges that `compose::needs_compositor` picks
  (effects, generators, HTML generators, transitions, masks, subject
  masks, retimed media…).
  * `compose::submit_program` draws them, as for the viewer and the
    export.
  * The size has the shape of the sequence with a 720 short side. Smaller
    sequences render at their own size.
  * Masks cut. They are not tinted.
* **Keys** (`render_cache/key.rs`):
  * The code cuts the timeline at every clip and transition edge on the
    tracks that show. Each piece that needs the compositor is a span.
  * The span is hashed (BLAKE3) over serialized state. The state contains:
    * the clips shown there, and the clips that touch them (transitions
      draw from them)
    * their tracks
    * their media items (fingerprint, path, info, proxy)
    * the timeline without its tracks, markers and ducking
    * the FX programs of the project
    * the caption styles
    * the frame size
    * which engines are wired, and whether the depth model is here
    * the subject and depth models
    * the app version and a format number.
  * The key of a frame is the hash of its span and its index.
  * Serialized values go through `serde_json::Value`. Its maps sort their
    keys.
  * The key leaves out only fields that are known not to draw: clip audio,
    name and colour, track name and lock, bins and tags. Thus a property
    that is added later to a clip, track or the timeline is in the key. It
    is not necessary to list it.
  * An edit changes the keys of the frames that it changes, and no other
    keys. Nothing is invalidated. Frames that no request asks for any more
    age out.
* **Store** (`render_cache/store.rs`): a disk cache (see above) under
  `cache/frame-cache/v<FORMAT>-<app version>/`.
  * Frames show only in the viewer. Thus the store keeps an opaque frame
    as JPEG at quality 90. Nearly all frames are opaque, because the
    program is composed on black.
  * The store keeps a frame with transparency as QOI (BGRA).
  * On 720p frames checked on the CPU, JPEG was about 9× smaller than QOI
    on a clean photo (94 KB against 850 KB). It was 5× smaller on a grainy
    photo (520 KB against 2.6 MB).
  * JPEG decodes in 5–12 ms. It encodes in 25–45 ms, on a writer thread
    that the worker owns.
  * A file can be missing, or not decode, or have a key that does not
    match. In these conditions, the store drops the file and the frame is
    composed live.
* **Background render** (`render_cache.rs`, `render_cache/worker.rs`):
  * `EditorState.render` polls 4 times each second.
  * The shown project is `display_project`. It includes a previewed draft
    or a drag. 200 ms after its last change, it is planned and measured
    off the UI thread.
  * 1.5 s after the last play, scrub, edit or delivery, a worker thread
    renders the spans that are not on disk. It starts from the playhead,
    then from the start. It uses a pipeline, as playback does.
  * Any play, playhead move, edit or job in the render queue drops the
    worker. The worker stops after the frame that it has in hand.
  * A span waits while its media is offline. It also waits while the
    mattes of its subject masks are not complete. It tries again every
    5 s.
  * The depth maps that its clips read are made first (`complete_depth`).
    Thus the cache keeps a frame only as it will finally look.
  * A stale or cancelled render can write frames only under the keys of
    what it drew. Thus it never shows a wrong frame.
  * The first 9 frames of a span are composed one at a time and timed.
    If the median takes less than a quarter of the time of a frame, the
    span is cheap (for the session, by its hash).
  * A cheap span is drawn live, as fast as it is read. Thus the cache does
    not keep the rest of the span.
* **Reading:** playback (`viewer/player.rs`) takes a frame from the
  in-memory cache.
  * If the frame is not there, playback can take it from disk. The cached
    frame must be at least three quarters of the width of the viewer.
  * Else, playback composes the frame.
  * Paused frames and render-ahead use disk frames at least as wide as the
    viewer.
  * A source item in the viewer, a zoomed viewer and tinted masks always
    compose.
* **Render bar:** a 2 px strip along the top of the ruler.
  * It shows `theme.success` over cached frames.
  * It shows `theme.danger` over frames that are not rendered yet.
  * It shows nothing where no compositor is necessary, or where the span is
    cheap.
* **Settings → Storage:**
  * The frames count in the Cache row. Its Clear removes them. Old
    generations go when the store opens.
  * The "Background rendering" row sets the size limit (2–50 GB, 10 by
    default). It can also disable background rendering
    (`RenderCacheSettings` in the `meta` of the store).
