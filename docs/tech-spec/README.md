# Tech specs

How Flikk is built, and why. Engineering companion to the PRD ("AI-native
video editor with agent chat", Oct 26 2026): the PRD says *what*; these
files say *how* the code does it today. When the two disagree, the PRD
wins on behaviour and these files win on structure.

Keep them in step with the code: when a change alters something described
here, update the file in the same commit.

## The system

One file per area, each describing the code as it is.

| Spec | Covers |
| --- | --- |
| [Shape of the system](architecture.md) | The crates, what each holds and how they depend on each other |
| [Key decisions](key-decisions.md) | The table of choices, each with its reason |
| [Threading](threading.md) | What runs on the UI thread, what runs off it, job queues |
| [Editing model](editing-model.md) | The command bus, change sets, sync by sound |
| [Compositor and FX](compositor-and-fx.md) | How a program frame is composed, on the CPU and the GPU |
| [Media and playback](media-and-playback.md) | Decoding, proxies, the player, the render cache |
| [Analysis and models](analysis-and-models.md) | Transcription, speech, sounds, subject and person masks, stabilization, depth |
| [Agent](agent.md) | The agent host, its tools, blockouts, driving the editor from outside |
| [Persistence](persistence.md) | Project files, user data, releases and updates |
| [Deliver](deliver.md) | The render queue and export |
| [UI](ui.md) | Panels, the kit, looking at the app |
| [PRD to code map](prd-code-map.md) | Where each PRD requirement lives |
| [Known gaps](known-gaps.md) | What is missing or approximate |

## Decisions

One file per decision that shapes more than one crate: what was decided,
why, what was turned down, what was verified and what is still open.

| Spec | Decision |
| --- | --- |
| [GPU turns and the Jobs list](gpu-turns.md) | GPU work takes turns, one unit at a time, and what takes them is listed in the title bar |
| [Blockouts on the GPU](blockout-gpu.md) | The blockout ray tracer runs as WGSL compute, with the CPU renderer as reference and fallback |

## Providers

Notes on an outside service's API as the editor uses it.

| Spec | Covers |
| --- | --- |
| [Pika](pika.md) | Pika's developer API: access, operations, what the editor uses |

## Adding one

- Put it in the group it belongs to and add its row here.
- Name the file after its subject, in lower case with hyphens.
- A system file describes the code as it is; extend the one that covers
  the area before starting a new one.
- A decision file keeps the sections Status, Problem, Decision, How it
  works, Turned down, Verified, Open, and says plainly what was not
  verified.
- Link between files by name (`[Agent](agent.md)`), not by section
  number.
