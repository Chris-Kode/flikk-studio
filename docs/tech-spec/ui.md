# UI

Pool items and library sounds play on their own (`state/source.rs`): the
item is the one clip of a scratch project, so it goes through the program's
compositor and audio mixer while the timeline and playhead stay put. Sounds
play from the pool and the library; pictures open in the viewer, with a bar
to seek on, until the user goes back to Program or moves the timeline.

Workspaces (`Workspace::ALL`, rendered in `flikk/src/app.rs`): Edit
(media pool and effects library on the left, viewer, inspector, timeline),
Captions (transcript and caption style panel), Color (wheels, curves,
scopes), Audio (mixer and meters) and Deliver. The agent chat is docked on
the right in every workspace. Settings (the gear in the top bar, cmd-,, or
the chat's gear for its Agent page) has five pages: Agent, Integrations
(each provider's key and account, who does each capability on which
model), Analysis (the steps), Models (download, pick, and delete the transcription size not picked) and
Storage (the folders a user has a stake in: the Cache, everything the
editor makes again by itself, with one Clear that removes what it counts;
models, the library (effects, presets, sounds and voices, their audio in
`library/assets/`), what providers and Codex generated, and backups; and
a row that turns background rendering off and sets its size limit. Analysis results are
not shown and not cleared with the cache, as making them again can
cost). The FX code editor replaces
the inspector while a program is open. With nothing selected the inspector
shows the sequence: format, resolution, and Fit/Fill for all footage (whose
Reframe asks the agent to fill the frame and keep each subject in shot).

The user asked for a clean, minimal UI, and CLAUDE.md says "let the layout
do the explaining":

* No secondary labels, helper text or descriptions under controls. Use icons
  with tooltips, placeholders in inputs, and grouping by spacing.
* Colors come from `cx.theme()` only. Change sets use `theme.green` (added),
  `theme.red` (removed) and `theme.yellow` (modified), drawn as tints and
  outlines, never as banners.
* Dark theme by default, compact density (`text_sm`/`text_xs`, 28 px rows).
* Empty states are a single muted line or an icon, not a paragraph.
* Prefer gpui-kit components (`gpui_kit::component::*`: button, input,
  slider, switch, select, tab, menu, popover, tooltip, scroll) over
  hand-rolled widgets.

## Looking at the app

`scripts/screenshot.sh out.png [wait]` builds the editor and renders it
under Xvfb (with `FLIKK_HOME=/tmp/flikk-dev` by default). There is
no input automation in the cloud containers, so seed the states you need to
see:

| Variable | Values | Seeds |
| --- | --- | --- |
| `FLIKK_DEMO` | `1` | A demo project with stand-in engines |
| `FLIKK_DEMO_WORKSPACE` | `Edit`, `Captions`, `Color`, `Audio`, `Deliver` | The workspace to open |
| `FLIKK_DEMO_FX` | `effects`, `generator`, `transition`, `library`, `code` | An effects or library view |
| `FLIKK_DEMO_SELECTION` | `clips`, `transition` | A selection |
| `FLIKK_DEMO_CHAT` | any | Chat references |
| `FLIKK_DEMO_CHAT_PICKER` | `@query` | The composer's `@` list, open |
| `FLIKK_DEMO_VIEW` | `captions`, `retype` | The transcript's captions view, or typing over two words |
| `FLIKK_DEMO_PLAY` | `1` | Playback |
| `FLIKK_DEMO_COMPOSE` | seconds | Every compositor feature on real FFmpeg-made media |
| `FLIKK_DEMO_DEPTH` | `1` | A lens blur (focus pick armed), a 3D camera and a depth mask on the selected clip |
| `FLIKK_DEMO_SETTINGS` | `agent`, `integrations`, `analysis`, `models`, `storage` | Settings, open on that page |
| `FLIKK_DEMO_STABILIZE` | seconds | A shaky clip made with FFmpeg, stabilized and selected; `FLIKK_DEMO_STABILIZE_MODE` = `off`, `locked` or `none` |
| `FLIKK_DEMO_SPEED` | `1` | The selected clip ramped (Hero) and blending, its Speed tab open |
| `FLIKK_DEMO_PERSON` | a video or picture file | That file over a gradient with a person mask, selected, on the real media engine and person masks |
| `FLIKK_DEMO_HDR` | `1` | The selected clip's media is HDR (HLG): its pool badge and the Color tab's tone map |
| `FLIKK_DEMO_GENERATE` | `music`, `sfx`, `voiceover`, `voices`, `voices-describe`, `voices-audio` | The media pool's Generate form, or the Voices dialog on that tab |
| `FLIKK_DEMO_SETUP` | `missing`, `downloading`, `failed` | The welcome page's setup list in that state, whatever is on disk (Download, Cancel and Retry only pretend); needs no `FLIKK_DEMO` |
| `FLIKK_DEMO_ASK` | `1` | ElevenLabs picked to transcribe for the run, and the question whether to analyze the demo's video and audio with it |
| `FLIKK_DEMO_SHOT` | `card`, `blockout`, `upright`, `plain` | A video waiting on the user in the chat, with a blockout, a first frame and a reference; `blockout` opens the blockout's dialog; `upright` is the same in a 9:16 picture; `plain` is an upright video from its prompt alone, its tiles empty |

```sh
FLIKK_DEMO=1 FLIKK_HOME=/tmp/flikk-demo cargo run -p flikk
```

`scripts/cloud-session-start.sh` prepares cloud sessions: `cargo fetch`,
`glslc` and the Vulkan headers for whisper.cpp, and lavapipe so the window
opens under Xvfb. Chromium for the HTML engine is under `/opt/pw-browsers/`
there.
