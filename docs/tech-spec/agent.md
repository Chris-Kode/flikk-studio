# Agent

* **Backends** (`flikk-agent/src/backend/`): Claude runs as
  `claude -p --input-format stream-json --output-format stream-json` with
  only the flikk MCP server (`--strict-mcp-config`, `--tools ""`,
  `--permission-mode dontAsk`, no setting sources or slash commands), the
  system prompt, `--model`/`--effort`, and `--resume` or `--session-id`.
  Codex runs as `codex exec --json --sandbox read-only` with the user's
  config and rules ignored, shell and patch tools off, and the MCP server,
  developer instructions, model and reasoning effort as `-c` overrides,
  resuming its thread. Each turn runs in an empty workdir under
  `agent/work`, so the user's own CLAUDE.md/AGENTS.md are not picked up. A
  backend that lost its session is retried fresh. `FLIKK_CLAUDE` and
  `FLIKK_CODEX` point at the CLIs.
* **Models and thinking:** two pickers next to Send list what the installed
  CLI offers (Claude's stream-JSON `initialize` request, Codex's
  `model/list` on `codex app-server`), cached in `agent/models.json` and
  refreshed in the background; each model brings its thinking levels.
  Choices are kept per backend in the user settings.
* **MCP server:** `flikk_agent::mcp::run()`, launched by the backend as
  `$FLIKK_MCP`, else the `flikk-mcp` binary next to the editor, else
  `flikk mcp`. It connects to the editor over TCP on 127.0.0.1 with the
  turn's token (`FLIKK_BRIDGE`/`FLIKK_TOKEN`).
* **Permissions** (`flikk-core/src/bus.rs`): one agent role, the editor.
  Permissions are per `ToolCategory`: read and view are always allowed;
  edit categories are Off or Propose (never auto-applied); skills and
  styles are Off, Ask or Allow (deleting one always asks); project memory
  (rules, facts, saving to the library) and deliver (renders, exports) always need the
  user's approval, asked with Approve/Deny cards above the composer and
  carried out by `agent_link/approve.rs`. The call waits: the host holds
  its MCP reply until the user decides and answers it with the outcome
  (a generated voiceover's media id, for one), so the turn carries on; an
  outcome whose call is gone (the turn ended or was cancelled) reaches the
  agent in the next turn's `<approvals>` block. Backends allow a tool call
  an hour for this. Generate media (music, sound effects, voiceovers and
  voices, pictures, videos and upscales: whatever spends the user's money
  or credits with a provider, `ToolCategory::costs`) is Off or Ask each
  time (the default). No setting lets it run unasked: a saved "Allow" from
  before reads as asking (`Permissions::get`), the handlers that ran one as
  a tool job are gone, and the only code that calls a provider for the
  agent is `agent_link/approve.rs`, on the user's click. `Permissions::choices`
  says what each category can be set to, and Settings → Agent draws its
  rows from it. A "Web" switch (off by default) gives Claude WebSearch/WebFetch and Codex web
  search. Both read pages as text only, so the same switch lists the `flikk`
  tool `view_web_image`: it downloads a PNG, JPEG, GIF or WebP (at most
  25 MB), returns it scaled to 1568 px on its long side, and with `add` keeps
  the original under `paths::web()` and adds it to the pool's "Web Images"
  bin as the user's edit. The `flikk` tool `generate_image` (Generate
  category, so it asks each time) is
  offered while someone can make the picture (`image::route`): the image
  provider picked in Settings → Integrations when one is set with a key,
  whatever the agent's own setting says (see "Generating pictures"), else
  the user's ChatGPT plan. The "Image generation" switch (off by default,
  either backend) is for the ChatGPT plan only: it turns on, and the tool
  and its prompt section are offered that way, only while Codex's saved
  sign-in carries a paid ChatGPT plan (`chatgpt_plan_type` in its ID token,
  not `free`); the switch is dimmed with a tooltip otherwise, and dimmed
  with the provider and model in its tooltip while an image provider is
  set, keeping its value for when it is not. The ChatGPT plan makes
  pictures from a description only. It sends the request Codex's own image tool
  sends (`POST chatgpt.com/backend-api/codex/images/generations`: the
  prompt, `gpt-image-2`, automatic size and quality, opaque or transparent
  background) with Codex's ChatGPT sign-in from `$CODEX_HOME/auth.json`
  (bearer token and `ChatGPT-Account-ID`, `originator: flikk`). Flikk never
  refreshes or writes that login: an expired one asks the user to open
  Codex, and plans without image generation (free) answer 403. The picture
  goes to `paths::generated()` and the pool's "Images Generated" bin, as the
  user's edit. Codex turns run with its own `features.image_generation`
  off, so there is one image tool. Web calls show as `web_search` and
  `web_fetch`, read from each backend's stream, whatever the backend calls
  them.
* **Tools** (`flikk-agent/src/tools/`): 106 in the catalog (104 plus
  `load_tools` and `run_tool`). Remove, move and detail operations are flags
  of their `set_*`/`view_*` sibling (for example `view_range` with `detail`).
  Each turn lists 26 core tools (`load_tools` among them) plus `run_tool`, and the groups (`edit`,
  `transform`, `captions`, `color`, `audio`, `effects`, `overlays`,
  `generate`, `memory`, `deliver`) the turn is likely to need, picked from the
  workspace, the selection and words in the message. A chat keeps the list
  it started with (`agent/tool_groups.json`) so the backend's prompt cache
  holds; later groups come through `load_tools` and are called with
  `run_tool`. External MCP clients see every tool. Each turn's use, with
  result sizes, is logged to `agent/tool_usage.jsonl`. Results are compact
  JSON that leave out neutral defaults.
* **Seeing and hearing:** `view_range` (collages, opening with the sound
  script), `view_frame` (annotated, with a `zoom` region under a pixel
  grid, or with `masks` showing what masks cut away tinted, naming subjects
  still being found), `get_layout`, `get_scopes`, `preview_fx`, `get_audio` (the sound
  script for any range), `get_music` (tempo, downbeats, level changes; every
  beat with `beats`), `search_sounds` (2 s steps ranked by a description or
  an example range) and `view_audio` (up to 30 s of spectrogram lanes).
  `view_media` (`tools/pool.rs`) sees a pool item on its own, placed or
  not, in its own seconds: it puts the item alone on a scratch timeline at
  its own size and frame rate, from its first frame, so the views above
  run on it unchanged (a video's collage and script, with `detail`; one
  frame with `t` and `zoom` in its pixels; a picture whole or zoomed; with
  `audio`, or for audio, the whole script and a spectrogram up to 30 s).
  An item whose analysis is missing is analysed first (`analyze`, as the
  pool's Analyze) and the call waits on that job, stopping with the turn;
  `list_media` says which items are analysed.
  `blade` and `ripple_delete` take `align: beat | downbeat | onset`; a
  target clear of words wins over a nearer one on a word.
* **Subject masks:** `set_mask` with `shape.type: subject` (`tools/masks.rs`)
  takes `include` and `exclude` points and a `box`, in picture fractions
  or frame pixels, on the frame at `at`; each list given replaces that
  frame's own. Before proposing, it runs the model on that frame and says
  what it kept (its share of the picture, its box in frame pixels, the
  model's confidence), with advice when it found nothing, took nearly the
  whole picture or is unsure; with `track` it says how many frames are
  left and how long that takes here. `get_clip` lists a subject's prompts
  on the timeline and how far it is tracked. The prompt steers the agent to
  subjects for objects and one person among several, and to check with
  `view_frame` `masks`.
* **Person masks:** `remove_background` (`tools/masks.rs`, transform
  group) gives each of `clips` a person mask, or changes its own: `keep:
  background` inverts it, `edge`, `feather` and `expansion` set its edge,
  `restore` takes the clips' person masks away. `set_mask` takes
  `shape.type: person` (with `edge`) too; `get_clip` says how many frames
  are matted, and `view_frame` names person masks not matted there yet.
* **Speed ramps:** `set_speed` (`tools/speed.rs`) takes one `speed`, a
  `ramp` of [position 0..1, speed] points or a `preset` (`ramp_in`,
  `ramp_out`, `hero`, `bullet`, `montage`, at `speed` or the clip's),
  `keep_length`, `interpolation` (`nearest`, `blend`, `optical_flow`) and
  `ramp_audio` (`stretch`, `mute`); `speed` alone on a ramped clip ends the
  ramp. `get_clip` reports a ramp as its points. The tool retimes one
  clip; a video's linked audio clip is retimed by its own call (the
  inspector ramps both).
* **Depth:** `add_depth_effect` (`tools/depth.rs`) puts a `lens_blur`,
  `fog` or `camera` on clips and says when depth maps cannot be made yet;
  `set_depth_focus` focuses a clip's lens blur (or a camera's pivot) on
  what is at a point of the picture at `at`: it makes that frame's map off
  the UI thread, then proposes the focus, keyed at `at` with `key` or when
  it already has keyframes. Depth masks are `set_mask`'s `depth` shape.
* **Generating audio:** `generate_audio` (`tools/generate.rs`) makes music
  (a prompt and a length, or `sections` with lengths of their own, so it
  changes on cue), a sound effect (optionally looping) or a
  voiceover (a script of up to 10,000 characters, one voice from
  `find_voices`, its stability and similarity) with the provider picked
  for it. The file goes to `paths::generated()` with a `<file>.json` of
  what was asked, and into the pool's "Music Generated", "Sound Effects
  Generated" or "Voiceover Generated" bin as the user's edit (`CommandBus::execute_shared_as`, which also adds it to the
  drafts waiting for review), so a rejected placement keeps what was paid
  for; `MediaItem::generated` records the provider, model and prompt, and
  for a voiceover who read it, so the pool can regenerate it. With
  `at` or `range` it is proposed on an audio track free over its length,
  else a new one. The approval runs it in the background, proposes the
  placement into the chat's set and answers the waiting call with the
  media id. A
  refused prompt (an artist's name) comes back with the provider's rewrite.
* **Music and sound effect providers:** ElevenLabs (its own API), fal and
  Pika. On fal, music: ElevenLabs Music v2.5 (the default), Sonilo 1.1,
  Lyria 3.5, Lyria 3 Pro, Stable Audio 3; sound effects: ElevenLabs Sound
  Effects v3 (the default), Sonilo 1.1, Mirelo SFX 2.0, Stable Audio 3
  SFX. On Pika, music: Pika Music (the default), Sonilo 1.1, MiniMax Music
  3.0, Lyria 3.5, Lyria 3 Pro; sound effects: Pika SFX (the default),
  Sonilo 1.1, ElevenLabs Sound Effects v2. Each model's
  `cloud::AudioModel` says the lengths it makes (a range and a step, or
  none when it picks the length itself) and whether it loops; the media
  pool's Generate dialog offers only those (no Length row for a model that
  picks, no Loop switch for one that can't), and `AudioModel::check`
  refuses the rest before anything is spent: in `Cloud::generate`, and
  before the agent's request waits for approval. A hosted model is a
  `HostedAudio` entry in `fal/audio.rs` or `pika/audio.rs` naming its
  fields; `audio.rs` writes a request in them, and asks in words for what
  a model has no field for: no vocals ("Instrumental only, no vocals."),
  and sections as timed lines in the prompt (`0:00–0:12 Intro: soft
  piano`) with their lyrics, or in a lyrics field as `[Name]` blocks. An
  ElevenLabs model on fal gets the same composition plan as ElevenLabs'
  own API, and Sonilo on Pika gets `segments`. fal runs them on its queue,
  Pika as media jobs, and `audio::fetch` names the file by what it is
  (WAV, FLAC, M4A, Ogg, else MP3). Mirelo is sent `num_samples: 1`: it
  makes, and bills, two takes by default.
* **Generating video:** `generate_video` (`tools/video.rs`) makes a shot
  with the provider and model picked for video (on fal: Seedance 2.0, 2.0
  Fast and 2.5, Kling 4.0 and O3 Pro, MiniMax H3 Max, Wan 3.0, Happy Horse
  1.1, Gemini Omni Flash 1.1, FLUX 3 Max, LTX 2.5 Pro; on Pika: Pika H3
  Flex and Pika 2.5, and Seedance, Kling, MiniMax H3, Hailuo, Wan, Happy
  Horse, Gemini Omni Flash, FLUX and Grok Imagine as Pika hosts them): from the prompt alone, from a first frame (a picture in the pool,
  or the program at a time, grabbed with `compose::program` into
  `paths::cache()/frames` just before) to an optional last one, or after
  references (pictures, clips and sounds in the pool, which the prompt names
  `@Image1`, `@Video1`, `@Audio1`). The agent writes the prompt as a brief
  (`catalog::VIDEO_PROMPT`: what each reference is for, the clip in a
  sentence, timed shots of one action and one camera move, the sound, and
  limits last), the shape video models follow best. Each model's `cloud::VideoModel` (its
  lengths, shapes, resolutions and reference limits) refuses what it can't
  make before anything is spent. Models name their endpoints and fields
  differently (`duration` as text or a number, `image_url` or
  `start_image_url`, a word for "the model picks the shape"), so each is
  an entry in `fal/models.rs` saying how it takes a request; references
  named `@Image1` are renamed to what the model reads (`Image 1`,
  `character1`, `<IMAGE_REF_0>`). fal runs it on its queue
  (`fal/queue.rs`), uploading the files it follows to fal's storage first;
  Pika runs it as a media job (`pika/jobs.rs`, [`pika.md`](pika.md)) after
  uploading to Pika, each model an entry in `pika/models.rs` as on fal, and
  sends a last frame on Pika 2.5 to Pikaframes.
  `cloud::GenerationTask` carries where it is (queued, running, coming
  down) to the media pool's row, which can stop it until it is made where
  the provider can stop it (fal can; Pika can't, so its job is marked
  `unstoppable` once taken). It
  lands in the "Video Generated" bin and, with `at` or `range`, is proposed
  on a video track free over its length (else a new one on top), its sound
  on an audio track beside it. The media pool's Video form makes the same
  request, starting on a pool picture or the frame at the playhead. However
  a generated item reaches the pool (the media pool or an approval),
  `analyze_new` queues it once like any new item (see [Analysis and models](analysis-and-models.md)): a video is
  asked about first while a provider is picked to transcribe; a
  voiceover, whose script stands in for its words, never is, nor are
  music and sound effects, which are heard only.
  Items already in a project when it opens are left as they were. Settings → Integrations has a "Video sound"
  switch under Video (`CloudSettings::silent_video`; sound costs more on
  some models): the media pool's form starts its Sound switch from it, and
  with it off the agent's videos are made silent whatever it asks. Each
  model's `cloud::VideoSound` says whether it makes sound at all, always
  (no switch: Pika H3 Flex, Kling 4.0, Kling 3.0 Turbo on Pika), or on a
  switch; only the last are made silent. Beside it, a resolution
  (`CloudSettings::video_resolution`: Auto, 480p, 720p, 1080p, 2K, 4K):
  `VideoModel::resolution_for` fits it to the model picked (the largest of
  its own at or below it, else its smallest), the media pool's form starts
  on it, and the agent's videos use it when they ask for none. Auto leaves
  the model's default.
* **Blockouts** (`flikk-core/src/blockout/`, `tools/video.rs`,
  `reblock.rs`, `ui/blockout*`, `ui/chat/shot.rs`): video generation is
  expensive and each take invents camera, staging and timing again, so
  `generate_video` takes a `blockout`: a rough 3D scene the agent writes
  as JSON (`blockout::Scene`: objects, models, a camera or several with
  the cuts between them, an optional sun direction; metres, Y up, the ground at `y = 0`, at most 30 s). Nothing
  in it is ready-made: there are no built-in people, props or actions,
  so the agent is not held to what Flikk thought of. An object is a
  plain solid (box, sphere, cylinder, cone, capsule, dome, wedge, roof,
  pyramid), a joint with no shape that only carries others, or a copy of
  a model. Things are built by hanging solids from one another
  (`parent`: a part's place and turn are in its parent's space) at the
  point they turn about (`anchor`: base, middle, or top for a limb hung
  from its joint). They move by keys, for what happens once, and by a
  `Cycle`, for what repeats: a few keys over one round, added to what
  the keys make of the object, running by the clock (`seconds`), by the
  ground covered (`metres`: the round follows how far the thing the part
  belongs to has travelled, so a wheel keeps up with the road at any
  speed) or by the strides its thing's feet have taken (`strides`: arms
  in time with legs); a round that returns to its start eases to rest
  when the thing stands still, and `from` and `until` have a cycle go on
  between two times only, in and out over 0.3 s. A thing built once goes under
  `models` and is placed with `model`; a copy can be scaled and shaded,
  and its `parts` give, by part name, the fields that replace the
  model's (another pose, no walk). `build.rs` writes every copy out into
  `Part`s named `copy/part`, swaps names for places in the list, works
  out how each part's keys move it (`motion::Moves`) and the order parts
  are placed in, gives each joint the box its parts fill, and tabulates
  the ground each moving thing has covered. An object may also face
  something about the vertical (`face`: the way it travels, the camera,
  an object or a point), keep spinning, come and go (`hidden`), shake
  without repeating (`shake`: metres and degrees either way, and about
  how long a wobble takes), and say what it stands on (`on`: the ground
  or an object by name). A camera has a place, a `look_at` point or
  object, a width (`lens` in millimetres on a frame 36 mm along its
  longer side, as Blender fits a sensor, or `fov` in degrees), a roll, a
  hand-held `shake`, and may ride an object. The shot opens on `camera`;
  `cameras` holds more by name and `cuts` says when the picture goes
  over to which (`main` is the first), each camera moving on whether or
  not the picture is its own. `build.rs` turns each into a `motion::Rig`,
  the names it looks at and rides swapped for places among the parts.
  A camera, or a key of one, may say what its picture holds in place of
  where it stands (`frame`: the objects, a shot size from `close_up` to
  `very_wide`, and `left`, `centre` or `right`). It then keeps the way
  it looks from its `at` and stands as far off as fits the box of the
  solids that reach into the share of the subjects' height the size
  keeps (`motion::framed_from`, the fit Blender's camera-to-selected
  makes): the top 17% filling three quarters of the picture's height
  for a close-up, all of it filling a quarter for a very wide. That
  depends on the picture's shape, so it is worked out as each frame is
  drawn (`Scene::stood`), with things standing as they do when the
  camera or the key gets there, and a third is reached by looking a
  third of half the picture's width to the other side.
  Everything keys move follows a smooth curve through them
  (`motion::Track`), place, turn, size, grey, and the camera's field of
  view, roll and what it looks at: where a value turns back or rests it
  stops, between stops its pace changes least from key to key (a
  tridiagonal solve, as Blender's animation curves place their handles),
  and each pace is held within three times the straight one either
  side, so the curve never goes past a key. Two keys give a straight
  move. An ease times one stretch of the curve (linear, in, out, in-out,
  `back`, `bounce`) and `cut` holds and then jumps, ending one curve and
  starting the next (a camera's cut to another set-up). A key with a
  place and no time is passed on the way: `motion::key_times` spreads
  such keys by the ground between them, between the timed keys either
  side, so an orbit or a route is a list of places. Angles are taken as
  written (a wheel turns from 0 to 360); the review says when a turn of
  270 degrees or more stops short of a whole one. Shake, the camera's
  and an object's, is fractal noise: smooth noise at three rates, each
  twice as quick and half as strong as the last, seeded for an object
  by its name.
  At a time, `Scene::moment` places every part once, in the order built
  (`build::placing_order`): each after what it rides, and after what it
  faces and stands on. Where that goes round in a loop (two people
  facing each other) the part placed first turns to where the other
  stands before anything faces, which a first, rough pass gives. A part
  with `on` is set down: its solids are put where its limbs have them,
  the lowest point of them is found (`Solid::lowest`) and a ray is cast
  down from over it onto the solids of what it stands on, the ground
  where it meets none, so a walker's feet stay on a ramp or a stage and
  the height written is how far above that it is. `solid.rs` has the
  solids: where a ray meets each, its lowest point and whether a point
  is inside it.
  A part's front can be turned to something (`aim`: an object, the
  camera, a point; a key retargets it): the least turn that puts its +Z
  on the target, no more than 85 degrees from how it would else face. A
  part can be the end of a limb brought somewhere (`reach`: `to`, and
  optionally `from` and `until`): the part it rides and the part that
  one rides are the limb (`build::Limb`, hung on the upper of the two),
  and `Scene::bend` solves it in closed form each frame from the two
  lengths, bent the way its pose has it bent (a limb posed straight
  bends forward when it hangs, back when held out), each part turned
  the least that puts the joint and the end in place; out of reach it
  goes straight toward the target, and it moves there and back over
  0.35 s. A foot can step (`step`): `Scene::planted` says where it is
  from the strides its thing has taken by then (`build::strides`,
  tabulated a frame: a stride is as long as legs of that length take at
  the speed, 2.3 leg lengths times the 0.3rd power of speed squared
  over gravity and leg length, unless `metres` gives it). It is down
  where the thing passes over it for three quarters of a leg's length
  of ground, at most 0.6 of the stride, so a run has both feet in the
  air, and swings to its next landing in between, lifted an eighth of
  the leg; standing still it is as posed. Its leg is bent to it as a
  limb is, and a thing with `on` is held no higher than its grounded
  legs reach (`Scene::lift`), which is the rise and fall of a walk.
  Because a landing is found from a count of strides, any frame is
  still drawn without the ones before it.
  A key with `parent` gives its object to another from that second on,
  from where it is at that moment (`"none"` lets it go where it is):
  `build::take_hold` works out, in order of time, where each is within
  what takes it, the scene built as far as the one before (Blender's
  Child Of with its inverse set). What is held is placed last each
  frame, with all that rides it, and a limb that has taken what it
  reached for leaves for where it took it from, so reaching and holding
  never wait on each other.
  `world::World` is the scene at a moment as rays meet it: the solids
  where they stand, a ray from the camera trying only those listed for
  its 16 px tile of the picture, a ray toward the sun only those listed
  for its cell of a grid across the light (`world::Cells`, each solid's
  cells widened by as far as a ray toward the sun's edge strays), and
  rays any other way a tree of boxes (`world::Solids`). All three are
  rebuilt each frame. `Scene::render` (`render.rs`) makes the picture on
  the CPU: one ray a pixel, lit by the sun, the sky from above, the
  ground from below and a fill from across the sun that reaches round
  what turns from it, with metre tiles on the ground and haze. Light is
  worked out as light and turned into screen greys at the end, pixels
  between two greys scattered so gradients show no steps. Every picture
  has lines, from which solid each pixel shows: darker than both sides,
  a pixel wide on the side away from the camera, strong round a thing
  and light between its parts, and a solid's own edges a little lighter
  where they stick out. `Detail::Full` also softens shadows (one ray a
  pixel toward a spot on the sun's disc, seven more where the pixels
  around disagree, evened out over 5 px on the same surface), darkens
  surfaces where the picture shows another close (eight looks along a
  spiral within 0.7 m at every fourth pixel, shared with the pixels
  around on the same surface, round solids too), and draws pixels an
  edge runs through again from four rays, the line taking the share of
  the pixel the nearer solid leaves and the rest of its width in the
  pixel outside. `Detail::Draft` is what plays. The values kept a pixel
  on the way (about 50 bytes in full) stay allocated from one picture to
  the next (`render::Sheets`). With no GPU and no model, a 1280×720
  frame of the demo shot takes about 36 ms as a draft and 85 ms in full
  on four cores.
  Larger worlds are written short. A box, a cylinder or a capsule takes
  a `taper` (its top against its base, inside the same size): cylinder,
  cone and capsule share one ray test for a round side whose radius
  changes with height. `repeat` (`repeat.rs`) lays one object out as a
  row or a grid (`step`), a `ring` or a scatter over an `area` (each
  place the candidate, of twelve, furthest from those before it), every
  copy the whole object with its keys moved over, called `name 1` and
  on and riding a joint of the object's name that stands for them all;
  `vary` turns, scales, shades and puts each out of step by a hash of
  the name and the copy's number. A solid with `cut` is not drawn but
  taken out of the object it names: every solid is convex, so a ray is
  inside one over a single stretch (`Solid::span`, the ordinary test
  from outside each way), and a ray that meets a cut solid in a hole
  goes on to where it leaves the hole, the hole's far side turned round
  being the surface seen. `hills` (`land.rs`) makes the ground roll:
  smooth noise on a 256 × 256 grid that repeats beyond the haze, walked
  by rays in strides four times what its steepest slope makes sure is
  clear and met between the last two places; every height is then above
  the ground under that thing (`Part::lands`, cameras and points too),
  so scenes are written as on a level floor. A scene holds 6000 parts.
  What made more slow was measured: the tree of boxes is rebuilt in
  under 3 ms at 13 000 parts, so it still is each frame; parts nothing
  ever moves are placed once (`Part::still`), and the review sets its
  moments up on several threads.
  Nothing is simulated, but what is let go falls (`flight.rs`): a key
  with `"parent": "none"` leaves a thing with the speed it was carried
  at over the frame before, and a key with `throw` gives it the speed
  that brings it to an object, the camera or a point, after the seconds
  it says or those of the throw that takes the least speed, aimed at
  where a moving target will be. From there its place at any time is
  its speed and gravity, so a frame still draws on its own. Where it
  comes to rest is found once at build, frame by frame, as its lowest
  point comes down on a solid that is not its own or its thrower's, or
  on the ground; thrown to an object it is caught, a hold by that object
  timed by the flight. The scene's `gravity`, or an object's own in its
  place, says how strongly things fall against the Earth's pull: at 0 it goes on in a straight line or stays
  where it was let go at rest, under 0 it rises. The review says when each lands or is caught,
  and doubts a throw over 30 m/s or one still in the air at the end.
  `Scene::render_outside` draws the scene from outside its shot
  (`Outside`: around, above, how near), toward the middle of a ball that
  holds the scene over its whole length, with the shot's camera, its
  move and the way each travelling thing goes, a tick a second, stroked
  over the picture against its depth. `Scene::render_pass` draws the
  shot as its depth, its lines or each thing in a colour of its own
  (`Pass`), and `Scene::thing_at` names what a place in the shot shows.
  Passes are shown, not sent: whether a video model follows one better
  than the greys is untested.
  `Scene::write_clip` pipes full frames to FFmpeg as a 24 fps H.264 MP4,
  1280 px on the longer side. `Scene::review` says a scene in words for
  a writer that can't watch it: what the camera sees at the start, the
  middle, the end and after each cut (which things, where across the
  picture, how tall, hidden behind what: found from spots on each thing
  and from a spread of rays through the picture), what looks like a
  mistake a viewer would see (`doubts`: the subject out of the opening
  picture, a walker going sideways, a turn the long way round) and what
  could be tidied (`notes`: a thing never in
  the picture, a key after the end, a camera never cut to). Between the
  moments it describes it watches the whole scene twelve times a second
  (`review::Watch`) and adds to the doubts: the camera's subject hidden
  or out of the picture, from when until when; the camera inside an
  object or under the ground; one thing going through another (the
  middle of a solid inside a solid of the other, the two moving past
  each other); something on stepping feet faster than 12.5 m/s; a
  walker with its feet off the ground and no `on`; a limb short of what
  it reaches for. What the camera looks at is named as a shot is called
  from its height in the picture. A thing is what a viewer would
  name: what stands by itself, and each copy, not their parts; the
  copies of a repeat are said as one, with how many of them show.
  `preview_blockout` (the Generate group, a read) gives the agent that
  text with a collage of eight frames and the set from straight above
  with the camera drawn in, so it looks at a scene and fixes it before
  it asks for the video.
  A scene is long to write and quick to change, so it is written whole
  only once. `Scene::changed` (`change.rs`) makes a change that says
  only what differs: the scene's JSON with `objects`, and each model's
  parts, held by name; a name that is there has the fields given put in
  place of its own, a new name is added, `null` removes. Every blockout
  is kept on disk under a short id (`tools/drafts.rs`, the newest 40 in
  `paths::cache()/blockouts`): `preview_blockout` keeps a scene given as
  `blockout` and from then on takes `id` and `changes`, `generate_video`
  takes `blockout_id`, and the request's summary names the id, so the
  outcome the agent is told carries it. What the user changes on the
  card goes into the same kept scene (`Blockout::with_scene`), and it
  stays after they approve or deny, so another take, or the next try
  after a denial, starts from the scene as it was left. The backend runs
  that change a scene in the dialog, and the second look at a drafted
  one, answer with a change too.
  A `video::Shot` is the request with what it is made from: the
  `GenerateRequest::Video` as the user reads it, the frames to grab, the
  placement, and the `Blockout` (scene, clip path under
  `paths::cache()/upload`, clip size). Models follow references or start
  on a frame, not both, so `Shot::sent` lowers it for the provider: the
  blockout's clip is the first clip followed (`@Video1`) and a first
  frame the first picture (`@Image1`); `parse` adds a line to the prompt
  saying what each is unless the prompt names them. `Shot::check` refuses
  what the model can't make (a model that follows no clips, a last
  frame), and `Shot::prepare` grabs the frames and writes the clip just
  before the provider is called, in the approval's job or the tool's.
  The request waiting on the user is a card of its own (`ShotCard`, one
  entity per waiting `generate_video`, kept by `ChatPanel::sync_shots`):
  the blockout playing in a loop (`BlockoutPlayer`: draft frames drawn
  off the UI thread, the newest wanted next, narrower while drawing
  can't keep up, and the frame it stops on drawn again in full; drawn on
  the GPU when there is one, see [Blockouts on the GPU](blockout-gpu.md)), a tile for the first frame and each reference with the tag the
  prompt calls it, and the prompt as it is sent, in a field. A tile's menu
  fills it with a picture made for it (`EditorState::generate_then`, the
  user's own generation: it runs as a row of the media pool, lands in the
  Images Generated bin and fills the tile; an empty first frame beside a
  blockout is made after the blockout's opening frame, a filled tile
  after its picture), the program's frame at the playhead (grabbed at
  once) or a pool item, or empties it. Frames the agent named by time are
  grabbed when the card appears, so the tiles show what is sent. Every
  change is written into the waiting request
  (`AgentHost::revise_approval`), so Approve makes what the card shows;
  it is off while the model can't make the shot, with why beside it.
  A request without a blockout has a Blockout tile whose button has one
  written, for when the agent judged it not worth one, where the model
  picked follows clips: `Reblock::draft` is the same kind of backend run,
  given the shot's prompt without its role lines (`Shot::look`), its
  length and its shape. A first draft whose review has doubts is handed
  back once with the review to be put right, and stands if the second
  answer has no scene. What it answers becomes the shot's blockout, and
  an answer without a scene shows as why on the tile. Whenever a blockout
  or a first frame beside one comes or goes on the card,
  `Shot::settle_roles` puts the lines saying what `@Video1` and `@Image1`
  are into the prompt, or takes its own lines out again. Tiles have one
  size whatever the video's shape; the player paints its frame fitted
  inside its box (`paint_image` on a canvas: an image element takes its
  height from its width, so an upright frame overflowed a wide box).
  Clicking the blockout opens its dialog (`blockout::editor`): the scene
  large with play, seek and Undo, and a thread where the user says what
  should change. Above the picture the player's `Looking` is chosen:
  the shot, the set from outside (dragged round, the wheel bringing it
  nearer), or the shot's depth, lines or parts; a button lays the thirds
  over the shot, and a click on a thing in the shot puts its name into
  the ask (`Picked`). Each ask is one backend run without tools
  (`reblock::Reblock`: `TurnSpec::mcp` is `None`, so Claude gets an empty
  MCP config and no allowed tools, Codex no MCP server) given the scene,
  its review, the earlier asks and the format's schema; it answers with
  a sentence
  and a change to the scene, read from the reply's outermost braces and
  made by `Scene::changed`. Save puts the scene into the shot, and into
  the kept blockout, under a new clip path. Nothing in the dialog reaches a video provider.
* **Generating pictures:** `Capability::Image` is a row in Settings →
  Integrations like video, with fal and Pika as providers. On fal: Nano
  Banana 2.1 (the default) and Pro, GPT Image 2, GPT Image 2.6 Flare and
  Sunburst, FLUX 3 and FLUX 3 Klein, Seedream 5.5 and 5.0 Pro, Ideogram
  4.5, Grok Imagine Image 2.0, Qwen Image 3, Krea 3 and Recraft V4.1 Pro.
  On Pika: Nano Banana 2.1 (the default) and Pro, GPT Image 2, 2.6 Flare
  and 2.6 Sunburst, Seedream 5.5, 5.0 Pro and Lite, Ideogram 4.5, Grok Imagine Image
  2.0, Muse Image and Recraft 4.1. A `GenerateRequest::Image` carries a
  prompt, an optional shape and resolution, a transparent background, and
  the pictures it edits or follows (`ImageMode::Edit` when there are any:
  image to image), which the prompt names `@Image1`, `@Image2` and the
  models read as "image 1". Each model's `cloud::ImageModel` (shapes,
  whether it can pick one, resolutions, how many pictures it follows,
  transparency) refuses what it can't make before anything is spent. Each
  model is an entry in `fal/models.rs` or `pika/models.rs` (`IMAGE`)
  saying how it takes a request: `aspect_ratio` and `resolution`, or fal's
  named `image_size` (only the five ratios fal names), or Pika's `size` in
  pixels (Ideogram), or no shape at all (Muse, Recraft on Pika); the
  pictures it follows in
  `image_urls`, or one to edit plus `reference_image_urls` (Ideogram on
  fal). fal runs it on its queue (`fal/image.rs`), Pika as a media job
  (`pika/image.rs`), both after uploading the pictures as for video; the
  file comes back as `images[0]`, and `picture::fetch` names it by what
  its first bytes say it is. It lands in the "Images Generated" bin with a
  `<file>.json`, and is not analyzed. The media pool's Image form (the
  Generate menu, or "Generate image…" on a picture, which it then edits)
  takes the prompt, pictures in the pool and the frame at the playhead
  (grabbed when it is made), the model's shapes (starting on the
  sequence's), resolutions and transparency. With a provider picked, the
  agent's `generate_image` makes the same request through it, whatever the
  agent's "Image generation" switch says, taking `aspect`, `resolution`,
  `references` (pictures in the pool) and `frames_at` (the program at
  those times, grabbed just before); the approval carries which way it
  goes (`via`), so an older ChatGPT request still reads. The agent writes
  the prompt as `catalog::IMAGE_PROMPT` lays out (a few sentences with
  what the picture is and its subject first, words in it quoted, an edit
  as the change and then what stays), the shape image models follow best.
  The shot card's first-frame form starts from the shot's prompt, told to
  show only its opening instant.
* **Upscaling video:** `Capability::Upscale` is a row in Settings →
  Integrations like video. On fal: Topaz Precision (Proteus, the default)
  and Starlight, SeedVR2, Crystal, FLUX Video Upscale and Gemini Omni
  Flash 1.2. On Pika: Topaz Proteus (the default), Starlight Precise and
  Starlight Fast, and Pika H3 Flex's own upscale (`draft-to-video`, for
  sources up to 768p). A `GenerateRequest::Upscale` carries the source
  (`cloud::UpscaleSource`: its path, name, size, length and whether it has
  sound) and a resolution ("1080p", "2k", "4k": the lines on the short
  side), or none for the model's default (1080p where it can, else its
  smallest). Each model's `cloud::UpscaleModel` (its resolutions, how it
  is told how much larger: the resolution itself, a factor in a range, or
  whole steps such as Topaz's ×2, ×3, ×4 on Pika, the longest and largest
  source it takes, the most pixels it makes) works out the factor and
  refuses what it can't make before anything is spent. Each model is an
  entry in `fal/upscale.rs` or `pika/upscale.rs` with its endpoint and a
  function writing its input; Pika's uploads stop at 50 MB, which is
  checked first. The source is uploaded as for video and the job followed
  the same way. Upscalers drop or encode the sound again, so `upscaled.rs`
  puts the picture made together with the source's own sound (copied as it
  is, else as AAC) into `<stem>.mov`, keeping the provider's file when
  FFmpeg can't. It lands in the "Video Upscaled" bin, named
  `upscale-<source name>`, and is analyzed like footage. In the media pool,
  "Upscale…" on a video (`media_pool/upscale.rs`) offers the resolutions
  the model can make it at (`UpscaleModel::targets_for`, starting on its
  default) and, when a clip shows it, "Replace clips":
  `EditorState::upscale` then runs `Command::SwapMedia` once the file
  lands, as an undo step of its own. `SwapMedia { from, to }` points every
  clip of `from` on an unlocked picture track at `to`, at the same source
  times; sound clips keep `from`, whose sound the upscaled video carries
  anyway.
  The agent's `upscale_video` (`tools/upscale.rs`, Generate) takes a pool
  video, a resolution (default: the model's default for it) and `replace`,
  and is approved like `generate_video`. Where a generated file goes once
  in the pool is a `generate::Destination`: the pool only, a placement on
  the timeline, or `Replace(media)`, which proposes `SwapMedia` in the
  agent's change set for review.
* **Voiceovers:** Eleven v4 (`elevenlabs/speech.rs`) takes its delivery from
  the script: audio tags, ellipses and capitals, with no SSML, so
  `cloud::check_script` refuses a `<break>` before any credits are spent and
  offers the script with pauses in its place. Only stability and similarity
  are sent; speed is left to retiming the clip. The media pool's form picks
  the voice (`/v2/voices`, samples cached under `paths::cache()`) and puts
  tags in at the cursor, and the system prompt teaches the agent the same
  rules. A script runs to one request, about 10 minutes; longer ones are
  several clips, one voice each. The form lists the user's own voices
  first, starts a library voice on the stability and similarity it last read
  with (`EditorState::keep_reading` keeps them), and its + button opens the
  Voices dialog over it.
* **Voices** (`ui/voices.rs`, `state/voices.rs`, `elevenlabs/voices.rs`,
  `elevenlabs/design.rs`): one dialog, from the pool's Generate menu, the
  voiceover form and the transcript. *Find* lists the account's voices
  (`/v2/voices`, read once per session into `EditorState::account_voices`)
  or searches the shared Voice Library (`/v1/shared-voices`, by words,
  gender, age, language and use, 30 a page), each with its sample; a shared
  one is added to the account (`/v1/voices/add/{owner}/{voice}`, a voice
  slot), and one with a custom rate shows it. *Describe* designs voices from
  a description (`/v1/text-to-voice/design` on `eleven_ttv_v3`, 20–1,000
  characters, checked before credits are spent), plays each and keeps the
  one picked (`/v1/text-to-voice`). *From audio* clones a voice
  (`/v1/voices/add`, multipart) from files or pool items, sent as their
  sound when they are video, with background-noise removal and a consent
  box it needs. In the transcript, a speaker's words open the dialog as one
  WAV of where they speak (`state::voices::speaker_spans`, padded and
  joined stretches of the source they say most in, up to 3 minutes,
  `flikk_analysis::cut_speech`): to clone it, or to find shared voices that
  sound like the first 30 s (`/v1/similar-voices`). Whatever is picked,
  added or made is kept in the library (`library::save_voice`) and the next
  voiceover starts on it. The library's Voices section reads a voiceover on
  a voice, plays its sample, renames it (on the account too, for voices the
  user made), removes it, or deletes it from the account after asking;
  one the account no longer has is dimmed, and a designed one can be
  designed again from its description. A voiceover in the pool can save
  the voice that read it. The agent's `find_voices` lists the user's
  library first, with how each reads, or searches the Voice Library with
  `library` and filters; `add_voice` and `design_voice` (Generate media,
  so approved each time) add or design one and keep it in the library.
  Their approval cards are heard first: `find_voices` remembers where each
  voice listed can be heard and the request carries it, so the card of an
  `add_voice` or a voiceover plays the voice's sample (the library's copy
  when it has one). Approving a design opens the Voices dialog on the
  agent's description, designing; the user hears the takes and keeps one
  (or any other voice), which settles the request, and closing the dialog
  leaves it waiting. Cloning is not a tool: it needs the speaker's consent, so the prompt
  sends the user to the dialog.
* **Fixing words:** in the transcript, Retype… (the selection's menu, or
  Enter) turns the selected words into an editor; Enter or the mic button
  says what is typed in the speaker's voice (`ui/transcript/retype.rs`,
  `state/respeak.rs`). The voice is the library voice linked to that
  speaker (`SavedVoice::speakers`: fingerprint and speaker id); without
  one, the Voices dialog opens on cloning theirs from what they say in the
  recording, and the voice kept there, cloned or found, is linked and
  used. The user's click is their leave to spend the credits; the request
  runs as a row of the media pool, which shows it fail, and lands as the
  user's edits (the item in the Voiceover Generated bin, then the cut).
  The agent's `replace_words` (`tools/respeak.rs`, Generate) takes the
  words by the times they start (`from`, `to`, checked against `words`),
  the new `text` and, without a linked voice, a `voice` from
  `find_voices`; it can't clone. The approval card plays the voice;
  approving makes the words off the UI thread
  (`approve::Started::Respeak`), adds them to the pool and proposes the
  `ReplaceSpeech` in the chat's set.
* **Finding sounds:** `find_sounds` (`tools/sounds.rs`) searches the user's
  library and the project's audio by words (name, category, tags, what a
  provider was asked) and, where analysis left vectors, by how they sound;
  it says how many had none. `use_library_sound` adds a sound to the pool's
  "Library" bin as the user's edit, like generated audio, and with `at`
  proposes it the same way. `save_sound_to_library` waits for approval.
  The media pool's ♫ menu makes the same from a form (Music runs as long as
  In/Out).
* **Prompt** (`flikk-agent/src/prompt.rs`): the system prompt is the tool
  guidance (with or without web and images), the shared rules (seeing and hearing,
  time formats, references, cutting in pauses, protected ranges), the
  editor's part, the FX conventions and the language. Each message carries an `<editor_context>` (project
  summary, format, workspace, playhead, selection, in/out, selected words),
  the approved style rules, the picked style as a `<style>` block, a skill
  line, `<approvals>`, and a
  `<references>` block when the user referenced objects.
* **Chat** (`ui/chat*`): sessions per project in
  `paths::projects()/<project>.chats.json`, each resuming its own backend
  conversation (`agent/sessions.json`). Tool calls stack into one card per
  run. A call that runs off-thread (a `flikk` job, or the backend's own web
  search and fetch) shows with a spinner from `AgentEvent::ToolStarted`
  until its `ToolFinished` replaces it; one still running when the turn ends
  is marked Stopped. The change-set card sits above the composer, once per set however
  many turns grew it, and stays with its outcome until the next message;
  it has Preview, Review (a checklist with ticks), Reject and Accept. `ask_user` shows up to six choices. Skills run as
  `/name …`, the rest of the message being their input; none are built
  in. Skills and styles are Markdown files under `paths::skills()` and
  `paths::styles()` (`flikk-core/src/skills.rs`), managed in Settings and by
  the agent; the style picker sits beside send. cmd/ctrl-K opens the quick
  command bar.
* **References:** the composer's `@` picker and drops (Alt-drag clips,
  transitions and markers from the timeline; media, effects, audio effects,
  transitions and transcript words; "Add to chat" menu items) insert chips
  that are sent as tokens the agent reads: `@clip <id8>`, `@media <id8>`,
  `@marker <id8 or name>`, `@track <id8 or name>`, `@bin <id8>`,
  `@effect <clip id8>/<effect id8>`, `@transition <clip id8>/in|out`,
  `@fx <builtin key or project id8>`, `@audiofx <kind>`,
  `@range <start>-<end>` (timeline seconds, three decimals) and
  `@time <secs>`. `<id8>` is the id's `Display`. Clicking a chip reveals
  the object in the editor.

## Driving the editor from an external agent

While the editor runs it publishes its MCP bridge in
`paths::home()/bridge.json` (address, token, pid; owner-only). Any MCP
client can then use the same tools as the built-in agent:

```sh
claude mcp add flikk -- flikk mcp          # Claude Code
```

or for Codex, in `~/.codex/config.toml`:

```toml
[mcp_servers.flikk]
command = "flikk"
args = ["mcp"]
```

External calls act with the agent's current role and permissions. Their
edits land in an "External agent" change set that goes to review when the
client disconnects or after 90 s without a call; `ask_user` is not
available to them.
