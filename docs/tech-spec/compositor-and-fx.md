# Compositor and FX

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
