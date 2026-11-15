# UI

Pool items and library sounds play alone (`state/source.rs`). The item is
the one clip of a scratch project. Thus it goes through the compositor and
audio mixer of the program. The timeline and playhead do not move.

Sounds play from the pool and the library. Pictures open in the viewer, with a
bar for seek. They stay until the user goes back to Program or moves the
timeline.

The workspaces are `Workspace::ALL`, and `flikk/src/app.rs` renders them:

- Edit: media pool and effects library on the left, viewer, inspector,
  timeline.
- Captions: transcript and caption style panel.
- Color: wheels, curves, scopes.
- Audio: mixer and meters.
- Deliver.

The agent chat is docked on the right in each workspace.

Settings opens from the gear in the top bar, from cmd-, or from the gear
of the chat for its Agent page. Settings has five pages:

- Agent.
- Integrations: the key and account of each provider, and which provider
  does each capability on which model.
- Analysis: the steps.
- Models: download and select models. Delete the transcription size that
  is not selected.
- Storage: the folders that are important to a user.
  - The Cache: all the data that the editor makes again automatically. It
    has one Clear, which removes the data that it counts.
  - Models.
  - The library (effects, presets, sounds and voices, with their audio in
    `library/assets/`).
  - The output that providers and Codex generated.
  - Backups.
  - A row that turns background rendering off and sets its size limit.

  The Storage page does not show analysis results. Clear does not remove
  them with the cache, because to make them again can have a cost.

The FX code editor replaces the inspector while a program is open. When
nothing is selected, the inspector shows the sequence: format, resolution,
and Fit/Fill for all footage. Its Reframe asks the agent to fill the frame
and keep each subject in shot.

The user asked for a clean, minimal UI, and CLAUDE.md says "let the layout
do the explaining":

- Do not put secondary labels, helper text or descriptions under
  controls. Use icons with tooltips, placeholders in inputs, and groups
  made by spacing.
- Colors come from `cx.theme()` only. Change sets use `theme.green`
  (added), `theme.red` (removed) and `theme.yellow` (modified). The UI draws
  them as tints and outlines, never as banners.
- The default is the dark theme, with compact density (`text_sm`/`text_xs`,
  28 px rows).
- An empty state is a single muted line or an icon, not a paragraph.
- When possible, use gpui-kit components (`gpui_kit::component::*`:
  button, input, slider, switch, select, tab, menu, popover, tooltip,
  scroll), not hand-made widgets.

## How to see the app

`scripts/screenshot.sh out.png [wait]` builds the editor and renders it
under Xvfb (with `FLIKK_HOME=/tmp/flikk-dev` by default). The cloud
containers have no input automation. Thus, use these variables to seed the
states that you must see:

| Variable | Values | Seeds |
| --- | --- | --- |
| `FLIKK_DEMO` | `1` | A demo project with stand-in engines |
| `FLIKK_DEMO_WORKSPACE` | `Edit`, `Captions`, `Color`, `Audio`, `Deliver` | The workspace to open |
| `FLIKK_DEMO_FX` | `effects`, `generator`, `transition`, `library`, `code` | An effects or library view |
| `FLIKK_DEMO_SELECTION` | `clips`, `transition` | A selection |
| `FLIKK_DEMO_CHAT` | any | Chat references |
| `FLIKK_DEMO_CHAT_PICKER` | `@query` | The `@` list of the composer, open |
| `FLIKK_DEMO_VIEW` | `captions`, `retype` | The captions view of the transcript, or new text typed over two words |
| `FLIKK_DEMO_PLAY` | `1` | Playback |
| `FLIKK_DEMO_COMPOSE` | seconds | Each compositor feature on real media that FFmpeg made |
| `FLIKK_DEMO_DEPTH` | `1` | A lens blur (focus pick armed), a 3D camera and a depth mask on the selected clip |
| `FLIKK_DEMO_SETTINGS` | `agent`, `integrations`, `analysis`, `models`, `storage` | Settings, open on that page |
| `FLIKK_DEMO_STABILIZE` | seconds | A shaky clip that FFmpeg made, stabilized and selected. `FLIKK_DEMO_STABILIZE_MODE` = `off`, `locked` or `none`. |
| `FLIKK_DEMO_SPEED` | `1` | The selected clip, with a ramp (Hero) and blend, and its Speed tab open |
| `FLIKK_DEMO_PERSON` | a video or picture file | That file over a gradient with a person mask, selected. It uses the real media engine and person masks. |
| `FLIKK_DEMO_HDR` | `1` | The media of the selected clip is HDR (HLG). The seed shows its pool badge and the tone map of the Color tab. |
| `FLIKK_DEMO_GENERATE` | `music`, `sfx`, `voiceover`, `voices`, `voices-describe`, `voices-audio` | The Generate form of the media pool, or the Voices dialog on that tab |
| `FLIKK_DEMO_SETUP` | `missing`, `downloading`, `failed` | The setup list of the welcome page in that state, independent of the disk contents. Download, Cancel and Retry only pretend. It does not need `FLIKK_DEMO`. |
| `FLIKK_DEMO_ASK` | `1` | ElevenLabs is the transcriber for the run. The seed asks if ElevenLabs must analyze the video and audio of the demo. |
| `FLIKK_DEMO_SHOT` | `card`, `blockout`, `upright`, `plain` | A video in the chat that waits for the user, with a blockout, a first frame and a reference. `blockout` opens the dialog of the blockout. `upright` is the same in a 9:16 picture. `plain` is an upright video from only its prompt, with empty tiles. |

```sh
FLIKK_DEMO=1 FLIKK_HOME=/tmp/flikk-demo cargo run -p flikk
```

`scripts/cloud-session-start.sh` prepares cloud sessions with these
items: `cargo fetch`, `glslc` and the Vulkan headers for whisper.cpp. It
also prepares lavapipe, so the window opens under Xvfb. In these sessions,
Chromium for the HTML engine is under `/opt/pw-browsers/`.
