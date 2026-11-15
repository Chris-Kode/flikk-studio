# Providers

Each file in this folder describes the API of an external service as
the editor uses it. The clients are in `crates/flikk-cloud/src/`.

| Provider | Capabilities in the editor |
| --- | --- |
| [ElevenLabs](elevenlabs.md) | Transcription, music, sound effects, voiceovers and voices |
| [fal](falai.md) | Video, images, music, sound effects, voiceovers and upscaling |
| [Pika](pika.md) | Video, images, music, sound effects, voiceovers, cloned voices and upscaling |

## How to add a file

1. Give the file the name of the provider, in lower case.
2. Add its row to the table in this file.
3. Use these sections where they apply: Access, Job flow, Errors,
   Balance, a section for each capability, and Sources.

If a change to a client makes a file incorrect, update the file in the
same commit.
