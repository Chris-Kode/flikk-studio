# Tech specs

These files tell you how Flikk is built and why. They go with the PRD
("AI-native video editor with agent chat", Oct 26 2026). The PRD gives
the behaviour. These files give the structure of the code today.

If the PRD and these files disagree, the PRD is correct for behaviour.
These files are correct for structure.

Keep these files correct. If a change makes a file incorrect, update the
file in the same commit.

## The system

Each file describes one area of the code as it is.

| Spec | Contents |
| --- | --- |
| [Shape of the system](architecture.md) | The crates, their contents and their dependencies |
| [Key decisions](key-decisions.md) | A table of decisions, each with its reason |
| [Threading](threading.md) | The work on the UI thread, the work on other threads, and the job queues |
| [Editing model](editing-model.md) | The command bus, change sets and sync by sound |
| [Compositor and FX](compositor-and-fx.md) | How the CPU and the GPU compose a program frame |
| [Media and playback](media-and-playback.md) | Decode, proxies, the player and the render cache |
| [Analysis and models](analysis-and-models.md) | Transcription, speech, sounds, masks, stabilization and depth |
| [Agent](agent.md) | The agent host, its tools, blockouts and external agents |
| [Persistence](persistence.md) | Project files, user data, releases and updates |
| [Deliver](deliver.md) | The render queue and export |
| [UI](ui.md) | Panels, the kit and how to look at the app |
| [PRD to code map](prd-code-map.md) | The location of each PRD requirement in the code |
| [Known gaps](known-gaps.md) | Functions that are missing or approximate |

## Decisions

Each file records one decision that changes more than one crate.

| Spec | Decision |
| --- | --- |
| [GPU turns and the Jobs list](gpu-turns.md) | GPU work uses turns, one unit at a time. The title bar shows the jobs that use them. |
| [Blockouts on the GPU](blockout-gpu.md) | The blockout ray tracer runs as WGSL compute. The CPU renderer is the reference and the fallback. |

## Providers

Each file describes the API of an external service as the editor uses it.

| Spec | Contents |
| --- | --- |
| [Pika](pika.md) | The Pika developer API: access, operations and the parts that the editor uses |

## How to add a file

1. Write the file in ASD-STE100 Simplified Technical English. Refer to
   rule 6 in [AGENTS.md](../../AGENTS.md).
2. Put the file in the correct group and add its row to this file.
3. Give the file the name of its subject, in lower case with hyphens.
4. Make links to other files by name, such as `[Agent](agent.md)`. Do not
   use section numbers.

A system file describes the code as it is. If a file for the area
exists, add to that file. Do not start a new one.

A decision file has these sections: Status, Problem, Decision, How it
works, Alternatives that we rejected, Verification and Open items. It
must tell the reader which checks we did not do.

## Language of the files

All files in this folder obey ASD-STE100. When you change a file, write
the new text in ASD-STE100.
