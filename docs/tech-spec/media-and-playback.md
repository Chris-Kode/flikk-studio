# Media and playback

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

## Render cache

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
