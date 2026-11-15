# Agent

* **Backends** (`flikk-agent/src/backend/`): Flikk runs the agent through
  the Claude CLI or the Codex CLI.
  - Claude runs as
    `claude -p --input-format stream-json --output-format stream-json`.
    It gets only the flikk MCP server (`--strict-mcp-config`,
    `--tools ""`, `--permission-mode dontAsk`, no setting sources or slash
    commands). It also gets the system prompt, `--model`/`--effort`, and
    `--resume` or `--session-id`.
  - Codex runs as `codex exec --json --sandbox read-only`. It runs without
    the user's config and rules, and with its shell and patch tools off.
    The MCP server, the developer instructions, the model and the reasoning
    effort go to Codex as `-c` overrides. Codex resumes its thread.
  - Each turn runs in an empty workdir under `agent/work`. Thus the backend
    does not read the user's own CLAUDE.md/AGENTS.md.
  - If a backend lost its session, Flikk tries it again with a new session.
  - `FLIKK_CLAUDE` and `FLIKK_CODEX` give the location of the CLIs.
* **Models and thinking:** Two pickers next to Send list the models that
  the installed CLI offers. For Claude, the list comes from its stream-JSON
  `initialize` request. For Codex, the list comes from `model/list` on
  `codex app-server`. Flikk keeps the list in `agent/models.json` and
  refreshes it in the background. Each model has its own thinking levels.
  The user settings keep the choices for each backend.
* **MCP server:** The server is `flikk_agent::mcp::run()`. The backend
  starts it as `$FLIKK_MCP`. If there is no `$FLIKK_MCP`, the backend uses
  the `flikk-mcp` binary next to the editor. Else it uses `flikk mcp`. The
  server connects to the editor over TCP on 127.0.0.1 with the token of the
  turn (`FLIKK_BRIDGE`/`FLIKK_TOKEN`).
* **Permissions** (`flikk-core/src/bus.rs`): The agent has one role, the
  editor. Each `ToolCategory` has its own permission.
  - Read and view are always allowed.
  - Edit categories are Off or Propose. Flikk never applies an edit
    automatically.
  - Skills and styles are Off, Ask or Allow. A delete of a skill or a style
    always asks.
  - Project memory (rules, facts, saves to the library) and deliver
    (renders, exports) always need the user's approval. Approve/Deny cards
    above the composer ask for it. `agent_link/approve.rs` does the
    approved work.
  - The call waits. The host keeps its MCP reply until the user decides.
    Then the host answers the call with the outcome, for example the media
    id of a generated voiceover. Thus the turn continues.
  - Sometimes the call is gone, because the turn ended or a cancel stopped
    it. Then the outcome goes to the agent in the `<approvals>` block of the
    next turn. For this wait, backends allow one hour for a tool call.
  - Generate media is Off or Ask each time (the default). It includes
    music, sound effects, voiceovers and voices, pictures, videos and
    upscales. It is all work that spends the user's money or credits with a
    provider (`ToolCategory::costs`).
  - No setting lets generate media run without a question:
    - A saved "Allow" from before reads as a question (`Permissions::get`).
    - Flikk no longer has the handlers that ran it as a tool job.
    - The only code that calls a provider for the agent is
      `agent_link/approve.rs`, after the user's click.
  - `Permissions::choices` gives the possible settings of each category.
    Settings → Agent makes its rows from it.
  - A "Web" switch is off by default. It gives Claude WebSearch/WebFetch
    and gives Codex web search. The two backends read pages only as text.
    Thus the same switch also lists the `flikk` tool `view_web_image`. This
    tool:
    - downloads a PNG, JPEG, GIF or WebP of a maximum of 25 MB;
    - returns it scaled to 1568 px on its long side;
    - with `add`, keeps the original under `paths::web()` and adds it to the
      "Web Images" bin of the pool, as the user's edit.
  - The `flikk` tool `generate_image` is in the Generate category, so it
    asks each time. The agent gets it while a source can make the picture
    (`image::route`). The first source is the image provider picked in
    Settings → Integrations, when it has a key. This applies whatever the
    agent's own setting says (see "Generating pictures"). Else the source is
    the user's ChatGPT plan.
  - The "Image generation" switch is for the ChatGPT plan only. It is off
    by default and applies to either backend. It turns on only while
    Codex's saved sign-in carries a paid ChatGPT plan (`chatgpt_plan_type`
    in its ID token, not `free`). Only then does the agent get the tool and
    its prompt section through the ChatGPT plan.
  - At other times, the switch is dimmed and has a tooltip. While an image
    provider is set, the switch is dimmed and its tooltip shows the provider
    and the model. The switch keeps its value for the time when no provider
    is set.
  - The ChatGPT plan makes pictures only from a description. Flikk sends
    the same request as Codex's own image tool
    (`POST chatgpt.com/backend-api/codex/images/generations`). The request
    has the prompt, `gpt-image-2`, automatic size and quality, and an opaque
    or transparent background. It uses Codex's ChatGPT sign-in from
    `$CODEX_HOME/auth.json` (bearer token and `ChatGPT-Account-ID`,
    `originator: flikk`).
  - Flikk never refreshes or writes that login. If the login expired, Flikk
    asks the user to open Codex. Plans without image generation (free)
    answer 403.
  - The picture goes to `paths::generated()` and to the "Images Generated"
    bin of the pool, as the user's edit.
  - Codex turns run with Codex's own `features.image_generation` off. Thus
    there is one image tool.
  - Web calls show as `web_search` and `web_fetch`. Flikk reads them from
    the stream of each backend, whatever name the backend uses for them.
* **Tools** (`flikk-agent/src/tools/`): The catalog has 106 tools: 104,
  plus `load_tools` and `run_tool`.
  - Remove, move and detail operations are flags of their
    `set_*`/`view_*` sibling, for example `view_range` with `detail`.
  - Each turn lists 26 core tools (`load_tools` is one of them) plus
    `run_tool`. It also lists the groups that the turn will probably need:
    `edit`, `transform`, `captions`, `color`, `audio`, `effects`,
    `overlays`, `generate`, `memory`, `deliver`. Flikk picks the groups
    from the workspace, the selection and the words in the message.
  - A chat keeps the list that it started with (`agent/tool_groups.json`).
    Thus the prompt cache of the backend stays valid. Later groups come
    through `load_tools`, and the agent calls them with `run_tool`.
  - External MCP clients see every tool.
  - Flikk logs the tool use of each turn, with the result sizes, to
    `agent/tool_usage.jsonl`.
  - Results are compact JSON without neutral defaults.
* **Seeing and hearing:** These tools let the agent see and hear the edit:
  - `view_range` gives collages. They start with the sound script.
  - `view_frame` gives an annotated frame. With `zoom`, it shows a region
    under a pixel grid. With `masks`, it shows with a tint the areas that
    the masks cut away. It also names the subjects that Flikk has not found
    yet.
  - The agent also has `get_layout`, `get_scopes` and `preview_fx`.
  - `get_audio` gives the sound script for any range.
  - `get_music` gives the tempo, the downbeats and the level changes. With
    `beats`, it gives every beat.
  - `search_sounds` gives 2 s steps in a rank order from a description or
    an example range.
  - `view_audio` gives spectrogram lanes for a maximum of 30 s.
  - `view_media` (`tools/pool.rs`) sees one pool item alone, placed or not,
    in the seconds of the item. It puts the item alone on a scratch
    timeline at its own size and frame rate, from its first frame. Thus
    the views above run on it without change:
    - for a video, its collage and script, with `detail`;
    - one frame with `t` and `zoom` in its pixels;
    - a picture, whole or zoomed;
    - with `audio`, or for audio, the whole script and a spectrogram of a
      maximum of 30 s.
  - If an item has no analysis, Flikk analyses it first (`analyze`, as the
    Analyze of the pool). The call waits for that job and stops with the
    turn. `list_media` tells which items have an analysis.
  - `blade` and `ripple_delete` take `align: beat | downbeat | onset`. A
    target with no words on it wins over a nearer target on a word.
* **Subject masks:** `set_mask` with `shape.type: subject`
  (`tools/masks.rs`) takes `include` and `exclude` points and a `box`.
  - The points and the box are in picture fractions or frame pixels, on the
    frame at `at`. Each list that the call gives replaces the list of that
    frame.
  - Before it proposes the mask, the tool runs the model on that frame. It
    then tells what it kept: its share of the picture, its box in frame
    pixels and the confidence of the model.
  - It gives advice when it found nothing, when it took almost the whole
    picture, or when it is not sure.
  - With `track`, it tells how many frames remain and how long that takes
    on this computer.
  - `get_clip` lists the prompts of a subject on the timeline. It also
    tells how far the tracking of the subject has gone.
  - The prompt tells the agent to use subjects for objects and for one
    person among several. It also tells the agent to check with
    `view_frame` `masks`.
* **Person masks:** `remove_background` (`tools/masks.rs`, transform group)
  gives each of `clips` a person mask, or changes the mask that it has.
  - `keep: background` inverts the mask.
  - `edge`, `feather` and `expansion` set its edge.
  - `restore` removes the person masks of the clips.
  - `set_mask` also takes `shape.type: person` (with `edge`).
  - `get_clip` tells how many frames have a matte. `view_frame` names the
    person masks that have no matte at that frame yet.
* **Speed ramps:** `set_speed` (`tools/speed.rs`) takes these inputs:
  - one `speed`;
  - a `ramp` of [position 0..1, speed] points;
  - a `preset` (`ramp_in`, `ramp_out`, `hero`, `bullet`, `montage`), at
    `speed` or at the speed of the clip;
  - `keep_length`;
  - `interpolation` (`nearest`, `blend`, `optical_flow`);
  - `ramp_audio` (`stretch`, `mute`).

  If a clip has a ramp, `speed` alone ends the ramp. `get_clip` gives a
  ramp as its points. The tool changes the timing of one clip. The linked
  audio clip of a video needs its own call. The inspector ramps the two
  clips.
* **Depth:** Two tools add depth to clips:
  - `add_depth_effect` (`tools/depth.rs`) puts a `lens_blur`, `fog` or
    `camera` on clips. It tells when Flikk cannot make depth maps yet.
  - `set_depth_focus` focuses the lens blur of a clip (or the pivot of a
    camera). The focus goes on the object at a point of the picture at
    `at`. It
    makes the map of that frame off the UI thread, then proposes the focus.
    The focus gets a key at `at` with `key`, or when it already has
    keyframes.
  - Depth masks are the `depth` shape of `set_mask`.
* **Generating audio:** `generate_audio` (`tools/generate.rs`) makes audio.
  - It makes one of these, with the provider picked for it:
    - music, from a prompt and a length, or from `sections` with their own
      lengths, so that the music changes on cue;
    - a sound effect, which can loop;
    - a voiceover, from a script of a maximum of 10,000 characters, one
      voice from `find_voices`, and its stability and similarity.
  - The file goes to `paths::generated()` with a `<file>.json` of the
    request. It also goes into the "Music Generated", "Sound Effects
    Generated" or "Voiceover Generated" bin of the pool, as the user's
    edit.
  - This edit uses `CommandBus::execute_shared_as`, which also adds the
    file to the drafts that wait for review. Thus a rejected placement
    keeps the media that the user paid for.
  - `MediaItem::generated` records the provider, the model and the prompt.
    For a voiceover, it also records the voice that read it. Thus the pool
    can generate it again.
  - With `at` or `range`, the tool proposes the file on an audio track that
    is free over its length. Else it proposes it on a new track.
  - The approval runs the request in the background. It proposes the
    placement into the set of the chat and answers the waiting call with
    the media id.
  - If the provider refuses a prompt (for example, an artist's name), the
    result has the rewrite of the provider.
* **Music and sound effect providers:** The providers are ElevenLabs (its
  own API), fal and Pika. On fal and Pika, these models are available:

  | Provider | Music | Sound effects |
  | --- | --- | --- |
  | fal | ElevenLabs Music v2.5 (the default), Sonilo 1.1, Lyria 3.5, Lyria 3 Pro, Stable Audio 3 | ElevenLabs Sound Effects v3 (the default), Sonilo 1.1, Mirelo SFX 2.0, Stable Audio 3 SFX |
  | Pika | Pika Music (the default), Sonilo 1.1, MiniMax Music 3.0, Lyria 3.5, Lyria 3 Pro | Pika SFX (the default), Sonilo 1.1, ElevenLabs Sound Effects v2 |

  - The `cloud::AudioModel` of each model tells the lengths that it makes.
    These are a range and a step, or none when the model picks the length
    itself. It also tells if the model loops.
  - The Generate dialog of the media pool offers only those options. It
    has no Length row for a model that picks the length. It has no Loop
    switch for a model that cannot loop.
  - `AudioModel::check` refuses the other requests before Flikk spends
    anything. It does this in `Cloud::generate`, and before the agent's
    request waits for approval.
  - A hosted model is a `HostedAudio` entry in `fal/audio.rs` or
    `pika/audio.rs` that names its fields. `audio.rs` writes a request in
    these fields. If a model has no field for an item, `audio.rs` asks for
    the item in words:
    - no vocals: "Instrumental only, no vocals.";
    - sections: timed lines in the prompt (`0:00–0:12 Intro: soft piano`)
      with their lyrics, or `[Name]` blocks in a lyrics field.
  - An ElevenLabs model on fal gets the same composition plan as the own
    API of ElevenLabs. Sonilo on Pika gets `segments`.
  - fal runs these models on its queue, and Pika runs them as media jobs.
    `audio::fetch` names the file by its type (WAV, FLAC, M4A, Ogg, else
    MP3).
  - Flikk sends `num_samples: 1` to Mirelo. By default, Mirelo makes two
    takes and bills the two.
* **Generating video:** `generate_video` (`tools/video.rs`) makes a shot
  with the provider and the model picked for video.
  - These models are available:

    | Provider | Models |
    | --- | --- |
    | fal | Seedance 2.0, 2.0 Fast and 2.5, Kling 4.0 and O3 Pro, MiniMax H3 Max, Wan 3.0, Happy Horse 1.1, Gemini Omni Flash 1.1, FLUX 3 Max, LTX 2.5 Pro |
    | Pika | Pika H3 Flex and Pika 2.5, and Seedance, Kling, MiniMax H3, Hailuo, Wan, Happy Horse, Gemini Omni Flash, FLUX and Grok Imagine as Pika hosts them |

  - The shot starts from one of these:
    - the prompt alone;
    - a first frame, to an optional last frame. A first frame is a picture
      in the pool, or the program at a time. Flikk grabs the program frame
      with `compose::program` into `paths::cache()/frames` just before the
      request.
    - references: pictures, clips and sounds in the pool. The prompt names
      them `@Image1`, `@Video1`, `@Audio1`.
  - The agent writes the prompt as a brief (`catalog::VIDEO_PROMPT`). The
    brief gives the purpose of each reference, the clip in a sentence, and
    timed shots of one action and one camera move. Then it gives the sound,
    and the limits last. Video models follow this shape best.
  - The `cloud::VideoModel` of each model gives its lengths, shapes,
    resolutions and reference limits. It refuses what the model cannot make
    before Flikk spends anything.
  - Models use different names for their endpoints and fields. Examples are
    `duration` as text or a number, `image_url` or `start_image_url`, and a
    word for "the model picks the shape". Thus each model is an entry in
    `fal/models.rs` that tells how it takes a request.
  - Flikk renames references named `@Image1` to the name that the model
    reads (`Image 1`, `character1`, `<IMAGE_REF_0>`).
  - fal runs the request on its queue (`fal/queue.rs`). Before that, Flikk
    uploads the files that the model follows to the storage of fal.
  - Pika runs the request as a media job (`pika/jobs.rs`,
    [`pika.md`](pika.md)) after an upload to Pika. Each model is an entry in
    `pika/models.rs`, as on fal. Flikk sends a last frame on Pika 2.5 to
    Pikaframes.
  - `cloud::GenerationTask` carries the state of the request (queued,
    running, coming down) to the row of the media pool. The row can stop
    the request until it is made, where the provider can stop it. fal can.
    Pika cannot, so Flikk marks its job `unstoppable` once Pika takes it.
  - The result goes into the "Video Generated" bin. With `at` or `range`,
    Flikk proposes it on a video track that is free over its length. Else
    it proposes it on a new track on top. Its sound goes on an audio track
    next to it.
  - The Video form of the media pool makes the same request. It starts on a
    pool picture or on the frame at the playhead.
  - A generated item can come into the pool from the media pool or from an
    approval. In the two cases, `analyze_new` puts it in the queue once,
    like any new item (see [Analysis and models](analysis-and-models.md)):
    - While a provider is picked to transcribe, Flikk asks about a video
      first.
    - Flikk never asks about a voiceover, because its script is in place of
      its words.
    - Flikk never asks about music and sound effects, because they are
      only heard.
  - Flikk does not change the items that are already in a project when it
    opens.
  - Settings → Integrations has a "Video sound" switch under Video
    (`CloudSettings::silent_video`). Sound costs more on some models. The
    Sound switch of the media pool's form starts from this switch. If it is
    off, Flikk makes the agent's videos silent, whatever the agent asks.
  - The `cloud::VideoSound` of each model tells if it makes sound at all.
    Some models always make sound and have no switch: Pika H3 Flex,
    Kling 4.0, Kling 3.0 Turbo on Pika. Other models make sound on a switch.
    Flikk makes only the last group silent.
  - Next to the switch is a resolution (`CloudSettings::video_resolution`:
    Auto, 480p, 720p, 1080p, 2K, 4K). `VideoModel::resolution_for` fits it
    to the model picked: the largest of its own at or below it, else its
    smallest. The media pool's form starts on this resolution. The agent's
    videos use it when they ask for none. Auto keeps the model's default.
* **Blockouts** (`flikk-core/src/blockout/`, `tools/video.rs`,
  `reblock.rs`, `ui/blockout*`, `ui/chat/shot.rs`): Video generation is
  expensive. Each take makes the camera, the staging and the timing again.
  Thus `generate_video` takes a `blockout`. A blockout is a rough 3D scene
  that the agent writes as JSON (`blockout::Scene`).
  - **The scene:**
    - A scene has objects, models, one or more cameras with the cuts
      between them, and an optional sun direction. The units are metres, Y
      is up, and the ground is at `y = 0`. The maximum length is 30 s.
    - Nothing in the scene is ready-made: there are no built-in people,
      props or actions. Thus the agent is not limited to the ideas of
      Flikk.
    - An object is a plain solid (box, sphere, cylinder, cone, capsule,
      dome, wedge, roof, pyramid). It can also be a joint with no shape
      that only carries other objects, or a copy of a model.
    - The agent builds things when it hangs solids from other solids
      (`parent`). The place and turn of a part are in the space of its
      parent.
    - A part hangs at the point that it turns about (`anchor`: base,
      middle, or top for a limb that hangs from its joint).
  - **Motion by keys and cycles:**
    - Keys move a thing for events that occur once.
    - A `Cycle` moves a thing for events that repeat. A cycle is a few keys
      over one round. Flikk adds it to the motion that the keys give the
      object.
    - A cycle runs by the clock (`seconds`), or by the ground covered
      (`metres`). With `metres`, the round follows the distance that the
      thing of the part has travelled. Thus a wheel stays with the road at
      all speeds.
    - A cycle can also run by the strides of the feet of its thing
      (`strides`). Thus arms move in time with legs.
    - If a round returns to its start, it eases to rest when the thing
      stands still.
    - `from` and `until` let a cycle run only between two times. The cycle
      eases in and out over 0.3 s.
  - **Models and copies:**
    - A thing that the agent builds once goes under `models`. The agent
      places it with `model`.
    - A copy can have a scale and a shade. Its `parts` give, by part name,
      the fields that replace the fields of the model (for example, another
      pose, or no walk).
    - `build.rs` does these steps:
      - It writes every copy out into `Part`s named `copy/part`.
      - It changes the names in the list to places.
      - It calculates how the keys of each part move it (`motion::Moves`),
        and the order in which Flikk places the parts.
      - It gives each joint the box that its parts fill.
      - It makes a table of the ground that each moving thing has covered.
  - **Other object fields:** An object can also do these things:
    - face a target about the vertical (`face`: the direction that it
      travels, the camera, an object or a point);
    - spin without stop;
    - come and go (`hidden`);
    - shake without repetition (`shake`: metres and degrees to each side,
      and about how long one wobble takes);
    - tell what it stands on (`on`: the ground, or an object by name).
  - **Cameras:**
    - A camera has a place, a `look_at` point or object, and a width. The
      width is `lens` in millimetres on a frame 36 mm along its longer side
      (as Blender fits a sensor), or `fov` in degrees.
    - A camera also has a roll and a hand-held `shake`. It can ride an
      object.
    - The shot opens on `camera`. `cameras` holds more cameras by name.
      `cuts` tells when the picture goes to which camera (`main` is the
      first).
    - Each camera continues its move when the picture is not its own.
    - `build.rs` turns each camera into a `motion::Rig`. It changes the
      names that the camera looks at and rides into places among the
      parts.
  - **Framing:**
    - A camera, or a key of a camera, can tell what its picture holds
      instead of its place (`frame`). `frame` gives the objects, a shot
      size from `close_up` to `very_wide`, and `left`, `centre` or `right`.
    - The camera then keeps its view direction from its `at`. It stands at
      the distance that fits a box. The box holds the solids that reach into
      the share of the subjects' height that the shot size keeps
      (`motion::framed_from`). This is the fit that Blender's
      camera-to-selected makes.
    - For a close-up, the top 17% fills three quarters of the picture
      height. For a very wide shot, all of it fills a quarter.
    - This fit depends on the shape of the picture. Thus Flikk calculates
      it when it draws each frame (`Scene::stood`). The things stand as they
      do when the camera or the key gets there.
    - For a third, the camera looks to the other side by a third of half
      the picture width.
  - **Curves:**
    - All values that keys move follow a smooth curve through the keys
      (`motion::Track`). These values are the place, turn, size and grey,
      and the field of view, the roll and the look target of the camera.
    - Where a value turns back or rests, the curve stops. Between stops,
      the pace changes as little as possible from key to key. This is a
      tridiagonal solve, as Blender's animation curves place their handles.
    - Each pace stays within three times the straight pace on each side.
      Thus the curve never goes past a key.
    - Two keys give a straight move.
    - An ease sets the timing of one part of the curve (linear, in, out,
      in-out, `back`, `bounce`).
    - `cut` holds and then jumps. It ends one curve and starts the next,
      for example for the cut of a camera to another set-up.
    - A thing passes a key with a place and no time on its way.
      `motion::key_times` spreads these keys by the ground between them,
      between the timed keys on each side. Thus an orbit or a route is a
      list of places.
    - Flikk uses angles as written: a wheel turns from 0 to 360. The review
      tells when a turn of 270 degrees or more stops before a full turn.
    - The shake of the camera and of an object is fractal noise. It is
      smooth noise at three rates. Each rate is twice as fast and half as
      strong as the last. The seed for an object is its name.
  - **Placement at a time:**
    - At a given time, `Scene::moment` places each part once, in the build
      order (`build::placing_order`). Each part comes after the part that
      it rides, and after the parts that it faces and stands on.
    - Sometimes this order makes a loop, for example two people that face
      each other. Then the part placed first turns to the place of the
      other before any part faces. A first, rough pass gives that place.
    - Flikk sets down a part with `on`. It puts the solids of the part
      where its limbs have them and finds their lowest point
      (`Solid::lowest`). Then it casts a ray down from above that point
      onto the solids of the object that the part stands on. If the ray
      meets no solid, the part stands on the ground.
    - Thus the feet of a walker stay on a ramp or a stage. The height in
      the scene is the distance above that surface.
    - `solid.rs` has the solids. For each solid, it finds where a ray meets
      it, its lowest point, and if a point is inside it.
  - **Aim, reach and step:**
    - `aim` turns the front of a part to a target: an object, the camera or
      a point. A key can change the target. The part turns as little as
      possible to put its +Z on the target. The turn is a maximum of 85
      degrees from the direction that the part would else face.
    - `reach` brings the end of a limb to a place (`to`, and optionally
      `from` and `until`). The limb is the part that the end rides and the
      part that this part rides (`build::Limb`, hung on the upper of the
      two).
    - `Scene::bend` solves the limb in closed form for each frame, from the
      two lengths. The limb bends the way its pose bends it. A limb with a
      straight pose bends forward when it hangs, and back when the pose
      holds it out. Each part turns as little as possible to put the joint
      and the end in place.
    - If the target is out of reach, the limb goes straight toward it. The
      limb moves there and back over 0.35 s.
    - A foot can step (`step`). `Scene::planted` gives the place of the
      foot from the strides that its thing has taken by then
      (`build::strides`, in a table for each frame).
    - A stride is as long as legs of that length take at the speed, unless
      `metres` gives it. This length is 2.3 leg lengths times the 0.3rd
      power of speed squared over gravity and leg length.
    - The foot is down while its thing passes over it for three quarters
      of a leg length of ground. This is a maximum of 0.6 of the stride, so
      a run has the two feet in the air.
    - Between landings, the foot swings to its next landing and lifts an
      eighth of the leg. When the thing stands still, the foot is as posed.
    - Flikk bends the leg to the foot as it bends a limb. A thing with `on`
      stays no higher than its grounded legs reach (`Scene::lift`). This
      gives the rise and fall of a walk.
    - A count of strides gives each landing. Thus Flikk can draw any frame
      without the frames before it.
  - **Holds:**
    - A key with `parent` gives its object to another object from that
      second. The object starts from its place at that moment. The value
      `"none"` releases the object at its place.
    - `build::take_hold` calculates, in time order, the place of each held
      object in its holder. For each hold, the scene is built up to the hold
      before it. This is Blender's Child Of with its inverse set.
    - Flikk places a held object last in each frame, with all the parts that
      ride it.
    - A limb that took its reach target moves to the place where it took
      the target from. Thus reach and hold never wait for each other.
  - **World and render:**
    - `world::World` is the scene at a moment, as rays meet it. It has the
      solids at their places.
    - A camera ray tests only the solids listed for its 16 px tile of the
      picture.
    - A ray toward the sun tests only the solids listed for its cell of a
      grid across the light (`world::Cells`). Flikk widens the cells of each
      solid by the distance that a ray toward the edge of the sun strays.
    - Rays in other directions use a tree of boxes (`world::Solids`).
      Flikk builds all three again for each frame.
    - `Scene::render` (`render.rs`) makes the picture on the CPU with one
      ray for each pixel. The light comes from the sun, the sky from above,
      the ground from below, and a fill from across the sun. The fill
      reaches round surfaces that turn away from the sun. The ground has
      metre tiles, and there is haze.
    - Flikk calculates light as light and changes it into screen greys at
      the end. It scatters pixels between two greys, so gradients show no
      steps.
    - Every picture has lines, from the solid that each pixel shows. A line
      is darker than the two sides. It is one pixel wide on the side away
      from the camera. It is strong round a thing and light between its
      parts. The own edges of a solid are a little lighter where they stick
      out.
    - `Detail::Full` adds these steps:
      - It softens shadows. One ray for each pixel goes toward a spot on
        the disc of the sun. Seven more go where the pixels around
        disagree. Flikk evens out the result over 5 px on the same surface.
      - It darkens surfaces where the picture shows another surface close
        by. It uses eight looks along a spiral within 0.7 m at every fourth
        pixel. It shares the result with the pixels around on the same
        surface. This also applies to round solids.
      - It draws again, from four rays, the pixels that an edge runs
        through. The line takes the share of the pixel that the nearer
        solid leaves. The rest of its width is in the pixel outside.
    - `Detail::Draft` is the detail for playback.
    - The values that Flikk keeps for each pixel during a render (about 50
      bytes in full) stay allocated from one picture to the next
      (`render::Sheets`).
    - A 1280×720 frame of the demo shot takes about 36 ms as a draft and
      85 ms in full. This is with no GPU and no model, on four cores.
  - **Larger worlds:** The scene format has short forms for larger worlds.
    - A box, a cylinder or a capsule takes a `taper`: its top against its
      base, inside the same size. Cylinder, cone and capsule share one ray
      test for a round side with a radius that changes with height.
    - `repeat` (`repeat.rs`) lays out one object as a row or a grid
      (`step`), a `ring`, or a scatter over an `area`. In a scatter, each
      place is the candidate (of twelve) furthest from the places before
      it.
    - Every copy is the whole object with its keys moved over. The names of
      the copies start at `name 1`. The copies ride a joint with the name of
      the object, which stands for all of them.
    - `vary` turns, scales and shades each copy, and moves it out of step,
      by a hash of the name and the copy number.
    - Flikk does not draw a solid with `cut`. It removes the solid from the
      object that it names.
    - Every solid is convex, so a ray is inside a solid over one stretch
      only (`Solid::span`, the usual test from outside in each direction).
      If a ray meets a cut solid in a hole, it continues to the point where
      it leaves the hole. The far side of the hole, turned round, is the
      visible surface.
    - `hills` (`land.rs`) makes the ground roll. It is smooth noise on a
      256 × 256 grid that repeats beyond the haze. Rays walk it in strides
      of four times the distance that its steepest slope makes sure is
      clear. A ray meets the ground between its last two places.
    - Every height is then above the ground under that thing
      (`Part::lands`, also for cameras and points). Thus the agent writes
      scenes as on a level floor.
    - A scene holds 6000 parts.
    - The causes of slow scenes were measured. The tree of boxes is built
      again in less than 3 ms at 13 000 parts, so Flikk still builds it
      each frame. Flikk places parts that nothing ever moves only once
      (`Part::still`). The review prepares its moments on several threads.
  - **Falls and throws** (`flight.rs`):
    - Flikk does not simulate physics, but a thing that is let go falls.
    - A key with `"parent": "none"` releases a thing with the speed at
      which it was carried over the frame before.
    - A key with `throw` gives the thing the speed that brings it to an
      object, the camera or a point. The flight takes the seconds that the
      key gives, or the seconds of the throw with the least speed. The aim
      is the future place of a moving target.
    - From that point, the place of the thing at any time comes from its
      speed and gravity. Thus a frame still draws by itself.
    - The build finds once, frame by frame, where the thing comes to rest.
      That is where its lowest point comes down on a solid that is not its
      own or its thrower's, or on the ground.
    - If the thing goes to an object, that object catches it. The catch is
      a hold by that object, timed by the flight.
    - The `gravity` of the scene, or an object's own `gravity` in its place,
      tells how strongly things fall, relative to the pull of the Earth. At
      0, a thing continues in a straight line, or stays at rest where it was
      let go. Under 0, it rises.
    - The review tells when each thing lands or is caught. It adds a doubt
      for a throw over 30 m/s, or for a thing still in the air at the end.
  - **Other views:**
    - `Scene::render_outside` draws the scene from outside its shot
      (`Outside`: around, above, how near). It looks toward the middle of a
      ball that holds the scene over its full length.
    - This view shows the camera of the shot, its move, and the path of
      each travelling thing, with a tick for each second. Flikk strokes these
      over the picture against its depth.
    - `Scene::render_pass` draws the shot as its depth, its lines, or each
      thing in its own colour (`Pass`). `Scene::thing_at` names what a place
      in the shot shows.
    - Flikk shows passes but does not send them. Nobody has tested if a
      video model follows a pass better than the greys.
  - **Clip and review:**
    - `Scene::write_clip` pipes full frames to FFmpeg as a 24 fps H.264
      MP4, 1280 px on the longer side.
    - `Scene::review` describes a scene in words for a writer that cannot
      watch it. It gives these items:
      - What the camera sees at the start, the middle, the end and after
        each cut. This is which things, where across the picture, how
        tall, and hidden behind what. Flikk finds this from spots on each thing and
        from a spread of rays through the picture.
      - Possible mistakes that a viewer would see (`doubts`). Examples are
        the subject out of the opening picture, a walker that goes
        sideways, and a turn the long way round.
      - Possible tidy-ups (`notes`): a thing never in the picture, a key
        after the end, a camera that no cut goes to.
    - Between the moments that it describes, the review watches the whole
      scene twelve times a second (`review::Watch`). It adds these doubts:
      - the subject of the camera hidden or out of the picture, from when
        until when;
      - the camera inside an object or under the ground;
      - one thing through another: the middle of a solid inside a solid of
        the other, while the two move past each other;
      - a thing on feet that step, faster than 12.5 m/s;
      - a walker with its feet off the ground and no `on`;
      - a limb that does not get to the target that it reaches for.
    - The review names the camera target with a shot name, from the height
      of the target in the picture.
    - A thing is what a viewer would name: an object that stands by itself,
      and each copy, but not their parts. The review gives the copies of a
      repeat as one item, with the number that show.
    - `preview_blockout` is in the Generate group and is a read. It gives
      the agent that text and a collage of eight frames. It also gives the
      set from straight above, with the camera drawn in. Thus the agent looks at a scene and fixes it
      before it asks for the video.
  - **Changes and kept blockouts:**
    - A scene takes long to write but is quick to change. Thus the agent
      writes it in full only once.
    - `Scene::changed` (`change.rs`) applies a change that gives only the
      differences. The change is the JSON of the scene, with `objects` and
      the parts of each model held by name.
    - If a name exists, the given fields replace its own fields. A new name
      adds an item. `null` removes an item.
    - Flikk keeps every blockout on disk under a short id
      (`tools/drafts.rs`, the newest 40 in `paths::cache()/blockouts`).
    - `preview_blockout` keeps a scene given as `blockout`. After that, it
      takes `id` and `changes`. `generate_video` takes `blockout_id`. The
      summary of the request names the id, so the outcome that the agent
      gets carries it.
    - The user's changes on the card go into the same kept scene
      (`Blockout::with_scene`). The scene stays after the user approves or
      denies. Thus another take, or the next try after a denial, starts from
      the scene as it was left.
    - The backend runs that change a scene in the dialog also answer with a
      change. The second look at a drafted scene also answers with a change.
  - **The shot:**
    - A `video::Shot` is the request together with its sources:
      - the `GenerateRequest::Video` as the user reads it;
      - the frames to grab;
      - the placement;
      - the `Blockout` (scene, clip path under `paths::cache()/upload`, clip
        size).
    - Models follow references or start on a frame, but not the two. Thus
      `Shot::sent` lowers the shot for the provider. The clip of the
      blockout is the first clip followed (`@Video1`). A first frame is the
      first picture (`@Image1`).
    - `parse` adds a line to the prompt that tells what each one is, unless
      the prompt names them.
    - `Shot::check` refuses what the model cannot make, for example a model
      that follows no clips, or a last frame.
    - `Shot::prepare` grabs the frames and writes the clip just before Flikk
      calls the provider. It runs in the job of the approval or of the
      tool.
  - **The shot card:**
    - The request that waits for the user has its own card (`ShotCard`).
      There is one entity for each waiting `generate_video`, and
      `ChatPanel::sync_shots` keeps them. The card has these items:
      - The blockout in a loop (`BlockoutPlayer`). The player draws draft
        frames off the UI thread. It draws the newest wanted frame next. It
        draws narrower frames while it cannot keep up. It draws the frame
        that it stops on again in full. It draws on the GPU when there is
        one (see [Blockouts on the GPU](blockout-gpu.md)).
      - A tile for the first frame and for each reference, with the tag
        that the prompt uses for it.
      - The prompt as Flikk sends it, in a field.
    - The menu of a tile can empty the tile. It can also fill the tile
      with one of these:
      - A picture made for it (`EditorState::generate_then`). This is the
        user's own generation. It runs as a row of the media pool, goes
        into the Images Generated bin and fills the tile. For an empty first
        frame next to a blockout, Flikk makes the picture from the opening
        frame of the blockout. For a filled tile, Flikk makes it from the
        picture of the tile.
      - The frame of the program at the playhead, grabbed at once.
      - A pool item.
    - Flikk grabs the frames that the agent named by time when the card
      appears. Thus the tiles show what Flikk sends.
    - Each change goes into the waiting request
      (`AgentHost::revise_approval`). Thus Approve makes what the card
      shows. Approve is off while the model cannot make the shot, and the
      reason shows next to it.
  - **Blockout drafts:**
    - A request without a blockout has a Blockout tile when the model
      picked follows clips. The button of the tile writes a blockout. This
      is for when the agent judged that a blockout was not worth it.
    - `Reblock::draft` is the same type of backend run. It gets the prompt
      of the shot without its role lines (`Shot::look`), its length and its
      shape.
    - If the review of a first draft has doubts, Flikk gives the draft
      back once with the review, for a correction. The first draft stays if
      the second answer has no scene.
    - The answer becomes the blockout of the shot. If the answer has no
      scene, the tile shows the reason.
    - When a blockout, or a first frame next to one, comes or goes on the
      card, `Shot::settle_roles` changes the prompt. It puts in the lines
      that tell what `@Video1` and `@Image1` are, or it takes its own lines
      out again.
    - Tiles have one size for all video shapes. The player paints its frame
      to fit inside its box (`paint_image` on a canvas). An image element
      takes its height from its width, so an upright frame overflowed a wide
      box.
  - **The blockout dialog:**
    - A click on the blockout opens its dialog (`blockout::editor`). The
      dialog shows the scene large, with play, seek and Undo. It also has a
      thread where the user tells what to change.
    - Above the picture, the user chooses the `Looking` of the player:
      - the shot;
      - the set from outside (a drag turns it, and the wheel brings it
        nearer);
      - the depth, lines or parts of the shot.
    - A button puts the thirds over the shot. A click on a thing in the shot
      puts its name into the ask (`Picked`).
    - Each ask is one backend run without tools (`reblock::Reblock`).
      `TurnSpec::mcp` is `None`, so Claude gets an empty MCP config and no
      allowed tools, and Codex gets no MCP server. The run gets the scene,
      its review, the earlier asks and the schema of the format.
    - The run answers with a sentence and a change to the scene. Flikk reads
      the change from the outermost braces of the reply and applies it with
      `Scene::changed`.
    - Save puts the scene into the shot and into the kept blockout, under a
      new clip path. Nothing in the dialog goes to a video provider.
* **Generating pictures:** `Capability::Image` is a row in Settings →
  Integrations, as video is. Its providers are fal and Pika.
  - These models are available:

    | Provider | Models |
    | --- | --- |
    | fal | Nano Banana 2.1 (the default) and Pro, GPT Image 2, GPT Image 2.6 Flare and Sunburst, FLUX 3 and FLUX 3 Klein, Seedream 5.5 and 5.0 Pro, Ideogram 4.5, Grok Imagine Image 2.0, Qwen Image 3, Krea 3 and Recraft V4.1 Pro |
    | Pika | Nano Banana 2.1 (the default) and Pro, GPT Image 2, 2.6 Flare and 2.6 Sunburst, Seedream 5.5, 5.0 Pro and Lite, Ideogram 4.5, Grok Imagine Image 2.0, Muse Image and Recraft 4.1 |

  - A `GenerateRequest::Image` carries a prompt, an optional shape and
    resolution, and a transparent background. It also carries the pictures
    that it edits or follows. If there are pictures, the mode is
    `ImageMode::Edit` (image to image).
  - The prompt names these pictures `@Image1`, `@Image2`. The models read
    them as "image 1".
  - The `cloud::ImageModel` of each model gives its shapes, if it can pick
    one, its resolutions, how many pictures it follows, and its
    transparency. It refuses what the model cannot make before Flikk spends
    anything.
  - Each model is an entry in `fal/models.rs` or `pika/models.rs`
    (`IMAGE`). The entry tells how the model takes a request. The shape is
    one of these:
    - `aspect_ratio` and `resolution`;
    - fal's named `image_size` (only the five ratios that fal names);
    - Pika's `size` in pixels (Ideogram);
    - no shape at all (Muse, Recraft on Pika).
  - The pictures that the model follows go in `image_urls`. Ideogram on fal
    takes one picture to edit plus `reference_image_urls`.
  - fal runs the request on its queue (`fal/image.rs`). Pika runs it as a
    media job (`pika/image.rs`). The two run after an upload of the
    pictures, as for video.
  - The file comes back as `images[0]`. `picture::fetch` names it by the
    type that its first bytes give.
  - The file goes into the "Images Generated" bin with a `<file>.json`.
    Flikk does not analyze it.
  - The Image form of the media pool opens from the Generate menu, or from
    "Generate image…" on a picture. In the second case, the form edits that
    picture. The form takes these inputs:
    - the prompt;
    - pictures in the pool;
    - the frame at the playhead, grabbed when the picture is made;
    - the shapes of the model (it starts on the shape of the sequence);
    - the resolutions and the transparency.
  - With a provider picked, the agent's `generate_image` makes the same
    request through that provider. This applies whatever the agent's "Image
    generation" switch says. The tool takes `aspect`, `resolution`,
    `references` (pictures in the pool) and `frames_at` (the program at
    those times, grabbed just before).
  - The approval carries the path of the request (`via`). Thus an older
    ChatGPT request still reads correctly.
  - The agent writes the prompt as `catalog::IMAGE_PROMPT` tells. The
    prompt is a few sentences. It gives what the picture is and its subject
    first, and puts words in the picture in quotes. For an edit, it gives
    the change and then what stays. Image models follow this shape best.
  - The first-frame form of the shot card starts from the prompt of the
    shot. Flikk tells it to show only the opening instant of the shot.
* **Upscaling video:** `Capability::Upscale` is a row in Settings →
  Integrations, as video is.
  - These models are available:

    | Provider | Models |
    | --- | --- |
    | fal | Topaz Precision (Proteus, the default) and Starlight, SeedVR2, Crystal, FLUX Video Upscale and Gemini Omni Flash 1.2 |
    | Pika | Topaz Proteus (the default), Starlight Precise and Starlight Fast, and Pika H3 Flex's own upscale (`draft-to-video`, for sources up to 768p) |

  - A `GenerateRequest::Upscale` carries the source
    (`cloud::UpscaleSource`: its path, name, size, length and if it has
    sound). It also carries a resolution ("1080p", "2k", "4k": the lines on
    the short side). With no resolution, Flikk uses the default of the
    model: 1080p where it can, else its smallest.
  - The `cloud::UpscaleModel` of each model gives these items:
    - its resolutions;
    - how Flikk tells it the scale: the resolution itself, a factor in a
      range, or whole steps such as Topaz's ×2, ×3, ×4 on Pika;
    - the longest and largest source that it takes;
    - the most pixels that it makes.
  - It calculates the factor and refuses what the model cannot make before
    Flikk spends anything.
  - Each model is an entry in `fal/upscale.rs` or `pika/upscale.rs`, with
    its endpoint and a function that writes its input. Pika's uploads stop
    at 50 MB, and Flikk checks this first.
  - Flikk uploads the source as for video, and follows the job in the same
    way.
  - Upscalers drop the sound or encode it again. Thus `upscaled.rs` puts
    the new picture together with the source's own sound into
    `<stem>.mov`. It copies the sound as it is, else it encodes it as AAC.
    If FFmpeg cannot do this, Flikk keeps the provider's file.
  - The file goes into the "Video Upscaled" bin with the name
    `upscale-<source name>`. Flikk analyzes it like footage.
  - In the media pool, "Upscale…" on a video (`media_pool/upscale.rs`)
    offers the resolutions that the model can make for it
    (`UpscaleModel::targets_for`). It starts on the default of the model.
    When a clip shows the video, it also offers "Replace clips".
  - With "Replace clips", `EditorState::upscale` runs `Command::SwapMedia`
    when the file arrives. This is a separate undo step.
  - `SwapMedia { from, to }` points every clip of `from` on an unlocked
    picture track at `to`, at the same source times. Sound clips keep
    `from`, because the upscaled video carries the sound of `from` anyway.
  - The agent's `upscale_video` (`tools/upscale.rs`, Generate) takes a pool
    video, a resolution (default: the default of the model for it) and
    `replace`. It needs approval as `generate_video` does.
  - A `generate::Destination` tells where a generated file goes after it
    is in the pool. It is one of these:
    - the pool only;
    - a placement on the timeline;
    - `Replace(media)`, which proposes `SwapMedia` in the agent's change set
      for review.
* **Voiceovers:** Eleven v4 (`elevenlabs/speech.rs`) takes its delivery
  from the script: audio tags, ellipses and capitals, with no SSML.
  - Thus `cloud::check_script` refuses a `<break>` before Flikk spends any
    credits. It offers the script with pauses in place of the `<break>`.
  - Flikk sends only stability and similarity. For speed, the user changes
    the timing of the clip.
  - The form of the media pool picks the voice (`/v2/voices`, with samples
    cached under `paths::cache()`). It puts tags in at the cursor. The
    system prompt teaches the agent the same rules.
  - A script fits in one request, about 10 minutes. A longer script is
    several clips, with one voice each.
  - The form lists the user's own voices first. It starts a library voice
    on the stability and similarity of its last reading
    (`EditorState::keep_reading` keeps them). Its + button opens the Voices
    dialog over the form.
* **Voices** (`ui/voices.rs`, `state/voices.rs`, `elevenlabs/voices.rs`,
  `elevenlabs/design.rs`): One dialog opens from the Generate menu of the
  pool, from the voiceover form and from the transcript.
  - *Find* lists the voices of the account (`/v2/voices`, read once for
    each session into `EditorState::account_voices`). It can also search
    the shared Voice Library (`/v1/shared-voices`) by words, gender, age,
    language and use, 30 for each page. Each voice has its sample.
  - Flikk adds a shared voice to the account
    (`/v1/voices/add/{owner}/{voice}`). This uses a voice slot. If a voice
    has a custom rate, the dialog shows it.
  - *Describe* designs voices from a description (`/v1/text-to-voice/design`
    on `eleven_ttv_v3`, 20–1,000 characters). Flikk checks the description
    before it spends credits. The dialog plays each voice and keeps the one
    picked (`/v1/text-to-voice`).
  - *From audio* clones a voice (`/v1/voices/add`, multipart) from files or
    pool items. For a video, Flikk sends its sound. The tab has
    background-noise removal and a consent box, which the clone needs.
  - In the transcript, the words of a speaker open the dialog as one WAV of
    the places where the speaker speaks
    (`state::voices::speaker_spans`). The WAV has padded and joined parts
    of the source where the speaker says most, up to 3 minutes
    (`flikk_analysis::cut_speech`). The user can clone it, or find shared
    voices that sound like the first 30 s (`/v1/similar-voices`).
  - Flikk keeps the voice that the user picks, adds or makes in the library
    (`library::save_voice`). The next voiceover starts on it.
  - The Voices section of the library can do these things with a voice:
    - read a voiceover on it;
    - play its sample;
    - rename it (on the account too, for voices that the user made);
    - remove it;
    - delete it from the account, after a question.
  - If the account no longer has a voice, the voice is dimmed. The user can
    design a designed voice again from its description.
  - A voiceover in the pool can save the voice that read it.
  - The agent's `find_voices` lists the user's library first, with how each
    voice reads. With `library` and filters, it searches the Voice Library.
  - `add_voice` and `design_voice` are Generate media, so they need
    approval each time. They add or design a voice and keep it in the
    library.
  - The user hears a voice on its approval card first. `find_voices`
    remembers where the user can hear each voice in its list, and the
    request carries it. Thus the card of an `add_voice` or a voiceover plays
    the sample of the voice. If the library has a copy, the card plays that
    copy.
  - The approval of a design opens the Voices dialog on the agent's
    description, in design mode. The user hears the takes and keeps one, or
    any other voice. This settles the request. If the user closes the
    dialog, the request continues to wait.
  - Cloning is not a tool, because it needs the consent of the speaker.
    Thus the prompt sends the user to the dialog.
* **Fixing words:** In the transcript, Retype… (from the menu of the
  selection, or Enter) turns the selected words into an editor. Enter or
  the mic button says the typed text in the voice of the speaker
  (`ui/transcript/retype.rs`, `state/respeak.rs`).
  - The voice is the library voice linked to that speaker
    (`SavedVoice::speakers`: fingerprint and speaker id).
  - If there is no linked voice, the Voices dialog opens to clone the voice
    of the speaker from the recording. Flikk links and uses the voice that
    the user keeps there, cloned or found.
  - The user's click is their permission to spend the credits. The request
    runs as a row of the media pool, which shows if it fails. The result
    comes in as the user's edits: the item in the Voiceover Generated bin,
    then the cut.
  - The agent's `replace_words` (`tools/respeak.rs`, Generate) takes the
    words by their start times (`from`, `to`, checked against `words`). It
    also takes the new `text`. Without a linked voice, it takes a `voice`
    from `find_voices`. It cannot clone.
  - The approval card plays the voice. The approval makes the words off the
    UI thread (`approve::Started::Respeak`). It adds them to the pool and
    proposes the `ReplaceSpeech` in the set of the chat.
* **Finding sounds:** `find_sounds` (`tools/sounds.rs`) searches the user's
  library and the audio of the project.
  - It searches by words: name, category, tags, and the request to the
    provider. Where the analysis left vectors, it also searches by sound.
    It tells how many items had no vectors.
  - `use_library_sound` adds a sound to the "Library" bin of the pool, as
    the user's edit, like generated audio. With `at`, it proposes the sound
    in the same way.
  - `save_sound_to_library` waits for approval.
  - The ♫ menu of the media pool does the same from a form. Music runs for
    the length of In/Out.
* **Prompt** (`flikk-agent/src/prompt.rs`): The system prompt has these
  parts:
  - the tool guidance (with or without web and images);
  - the shared rules (seeing and hearing, time formats, references, cuts in
    pauses, protected ranges);
  - the part of the editor;
  - the FX conventions;
  - the language.

  Each message carries these items:
  - an `<editor_context>` (project summary, format, workspace, playhead,
    selection, in/out, selected words);
  - the approved style rules;
  - the picked style as a `<style>` block;
  - a skill line;
  - `<approvals>`;
  - a `<references>` block, when the user referenced objects.
* **Chat** (`ui/chat*`): Each project has its sessions in
  `paths::projects()/<project>.chats.json`. Each session resumes its own
  backend conversation (`agent/sessions.json`).
  - The tool calls of one run go into one card.
  - Some calls run off-thread, for example a `flikk` job, or the backend's
    own web search and fetch. Such a call shows with a spinner from
    `AgentEvent::ToolStarted` until its `ToolFinished` replaces it. If it
    still runs when the turn ends, Flikk marks it Stopped.
  - The change-set card is above the composer. There is one card for each
    set, for all the turns that added to it. It stays with its outcome
    until the next message. It has Preview, Review (a checklist with
    ticks), Reject and Accept.
  - `ask_user` shows a maximum of six choices.
  - Skills run as `/name …`. The remaining text of the message is their
    input. Flikk has no built-in skills.
  - Skills and styles are Markdown files under `paths::skills()` and
    `paths::styles()` (`flikk-core/src/skills.rs`). The user manages them
    in Settings, and the agent manages them too. The style picker is next
    to send.
  - cmd/ctrl-K opens the quick command bar.
* **References:** The `@` picker of the composer and drops insert chips.
  - Drops come from these sources:
    - Alt-drag of clips, transitions and markers from the timeline;
    - media, effects, audio effects, transitions and transcript words;
    - "Add to chat" menu items.
  - Flikk sends the chips as tokens that the agent reads:
    - `@clip <id8>`, `@media <id8>`;
    - `@marker <id8 or name>`, `@track <id8 or name>`, `@bin <id8>`;
    - `@effect <clip id8>/<effect id8>`, `@transition <clip id8>/in|out`;
    - `@fx <builtin key or project id8>`, `@audiofx <kind>`;
    - `@range <start>-<end>` (timeline seconds, three decimals);
    - `@time <secs>`.
  - `<id8>` is the `Display` of the id.
  - A click on a chip shows the object in the editor.

## Driving the editor from an external agent

While the editor runs, it publishes its MCP bridge in
`paths::home()/bridge.json` (address, token, pid; owner-only). Then any MCP
client can use the same tools as the built-in agent:

```sh
claude mcp add flikk -- flikk mcp          # Claude Code
```

For Codex, add this to `~/.codex/config.toml`:

```toml
[mcp_servers.flikk]
command = "flikk"
args = ["mcp"]
```

External calls use the current role and permissions of the agent. Their
edits go into an "External agent" change set. This set goes to review when
the client disconnects, or after 90 s without a call. External clients
cannot use `ask_user`.
