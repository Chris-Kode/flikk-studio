# Compositor and FX

* **One FX format** (`flikk-core/src/fx/mod.rs`). The module docs of this
  file are the authoritative GLSL and HTML conventions.
  * An `FxAsset` is a GLSL program or an HTML/CSS/JS document, plus typed
    parameters. A GLSL program is an effect, a transition or a generator.
    An HTML/CSS/JS document is a generator: titles, lower thirds, motion
    graphics or image layouts.
  * The parameter types are number, bool, color, point, text, choice and
    image.
  * Built-ins: 51 GLSL programs in `fx/shaders.rs` (27 effects,
    17 transitions, 7 generators) and 15 HTML templates in
    `fx/templates.rs`.
  * Programs that the user or the agent writes are in `Project::fx`. The
    FX code editor is the editor for these programs.
  * Three more built-in effects, in `fx/depth.rs`, read depth (see "Depth
    effects").
  * An `FxInstance` uses an asset with parameter values and keyframes. It
    is on the effect stack of a clip, or it is a transition, or it is the
    content of a generator clip.
* **GLSL** follows ShaderToy/gl-transitions.
  * Authors write `effect(uv)`, `transition(uv)` or `generate(uv)`.
  * `flikk-gpu` wraps the code as `#version 450` and compiles it with
    naga. The compiler runs on a thread with a 16 MiB stack.
  * `flikk-gpu` caches the result. It reports errors against the lines of
    the author.
  * `iResolution` is in sequence pixels, at all sizes that the effect runs
    at.
* **HTML** renders in headless Chromium over CDP. The browser is Chrome,
  Chromium, Edge or Brave, or `FLIKK_CHROME`.
  * The engine owns the clock (`clock.js`). Time, timers, rAF, CSS
    animations, Web Animations and GSAP all follow clip time. `Math.random`
    is seeded. Thus frames are deterministic and exportable, with alpha.
  * GSAP 3.15 and its plugins are vendored in `flikk-html/src/gsap`. The
    engine preloads them.
  * Templates build their animation in `flikkTimeline`. The engine runs
    `flikkTimeline` again when the parameters or the clip length change.
  * Pages lay out on the shape of the sequence, with a 1080 CSS px short
    side. Thus templates keep their proportions at all resolutions.
  * In portrait, bottom elements go up, above the controls that vertical
    apps draw.
  * A render runs three browsers of its own (`flikk_html::crew`). Each
    browser seeks through every frame, so scripts run as in one browser.
    The browsers take turns. Each turn captures the frames that the
    compositor will ask for next.
* **Program frame** (`flikk_core::compose::program`). For each layer, the
  compositor does these steps in this sequence:
  1. The picture: a media frame at the speed-aware source time, an
     overlay, or a generator.
     * A slowed clip that blends or flows shows the moment between two
       source frames.
     * `Pictures::pair` reads the two frames, and the batch mixes them.
       Alternatively, `InterpolationService::between` gives the optical
       flow frame after it is made.
     * The steps are 1/32, so the compositor uses frames again.
  2. Stabilization: the frame of a stabilized clip moves, turns and zooms
     inside its own frame by its correction.
  3. Clip grade, then timeline grade.
  4. Effect stack.
  5. Masks: a coverage raster that the compositor multiplies into alpha.
     The matte of a subject at the source time goes into this raster (see
     "Subject masks"). The matte of a person mask also goes into it (see
     "Person masks").
  6. Transform and blend mode.

  Transitions render each side onto its own canvas and mix them.
  `LetteringService` (cosmic-text `flikk_deliver::Letterer`) letters titles
  and captions on top.
  * The whole frame is one `FxBatch` on the GPU. It has one upload for
    each source picture, all passes in one submit and one readback. The
    readback is already BGRA.
  * Submit and wait are separate, so playback puts frames in a pipeline.
  * Without a GPU, `CpuBatch` and `compose/cpu.rs` composite in memory.
    Then transitions become a dissolve, and the compositor skips effects
    and GLSL generators.
* **Depth effects** (`flikk-core/src/depth.rs`, `fx/depth.rs`).
  * A GLSL effect that calls `float getDepth(vec2 uv)` reads the depth map
    of the picture. The distance is 0 nearest to 1 farthest. The effect
    samples it as it samples `getColor`.
  * `flikk-gpu` declares the function. It binds the map after the image
    parameters (`FxBatch::apply_depth`).
  * The compositor asks the depth service for the map of the source frame
    that a clip shows. It asks only for clips with such an effect or a
    depth mask.
  * The compositor uploads the map one time for the chain of the clip.
    Until the map is made, the compositor leaves out those effects.
  * Built-ins:
    * **Lens Blur** (`depth_blur`: `focus`, `range`, `blur` in pixels). It
      is a scatter-as-gather over two spirals of taps. What lies behind a
      pixel blurs no further over it than the pixel itself. Thus a sharp
      subject keeps its edge, and a blurred foreground spills over it.
      Keyframe `focus` for a rack focus.
    * **Depth Fog** (`depth_fog`: `tint`, `density`, `start`).
    * **3D Camera** (`depth_parallax`). `motion` is Dolly, Zoom (dolly
      zoom about `focus`), Orbit or Pan along `angle`. `amount` is −1..1,
      eased over the clip.
      * Each output pixel searches the layers of distance from the front
        for the surface that it shows. Thus near things cover far things.
      * Near things open gaps. The edge adjacent to each gap fills it.
      * The picture zooms in only enough to keep the edges of the frame
        covered during all of the move.
  * The inspector has "Pick focus" on an effect with a `focus`. This
    command arms the viewer.
    * The next click on the picture of the clip makes the map of that
      frame. The click sets the focus to the distance at that point, as a
      median over a small disc.
    * If `focus` already has keyframes, the focus is keyed at the playhead
      (`depth::focus_command`, `ui/viewer/focus.rs`).
* **Grade** (`flikk-core/src/grade.rs`): one pipeline (exposure, contrast,
  saturation, temperature/tint, lift/gamma/gain, offset,
  shadows/highlights, hue, curves, a LUT file). For export, the pipeline
  is baked into a 65³ LUT. `scopes.rs` reads waveform, parade, vectorscope
  and histogram. It also matches shots.
* **Audio effects** (`flikk-core/src/audiofx.rs`, `audiofx/dsp.rs`) run in
  the preview mixer. They map to FFmpeg filters in
  `flikk-deliver/src/audio_fx.rs`, table by table. Thus preview and export
  match.
