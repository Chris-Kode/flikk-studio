# ElevenLabs

ElevenLabs (<https://elevenlabs.io>) makes speech and audio. The editor
uses its API in these roles:

- a transcriber, with Scribe (see "Transcription");
- a source of music (see "Music");
- a source of sound effects (see "Sound effects");
- a voiceover provider (see "Voiceovers");
- the voices that read voiceovers: list, search, design, clone, rename
  and delete (see "Voices").

ElevenLabs does not make video or pictures, and it does not upscale. For
these capabilities, the editor answers `Unavailable`.

This file describes the client in `crates/flikk-cloud/src/elevenlabs/`
as it is. The code is the source of each fact.

## Access

| | |
| --- | --- |
| Base URL | `https://api.elevenlabs.io` |
| Auth | `xi-api-key: <key>` on each call |
| Keys | you make them at <https://elevenlabs.io/app/settings/api-keys> |
| Env var | `ELEVENLABS_API_KEY` |
| Plan | `GET /v1/user/subscription` |

The key is in the credential store of the system, under the service
`flikk` and the account `elevenlabs`. If the system has no credential
store, the key is in an owner-only `credentials.json` in the user-data
folder. The environment variable has priority over the two.

## Request flow

ElevenLabs has no job queue. Each request is one HTTP call, and the
answer is the result.

- The audio comes back in the body. The editor writes it to a `.part`
  file, then renames the file to `<stem>.<ext>`. The extension is the
  first part of the output format, such as `mp3` or `wav`.
- The `character-cost` header gives the credits that the request used.
  The editor records this number.
- The editor does not send a generation or a transcription again after a
  5xx. ElevenLabs can have done the work, and charged for it. The editor
  sends it again only after a 429 or a connection that did not start.
- The user can stop a generation while it waits to be sent again.
- The editor always names the model. The API defaults to older models.

### Output formats

A plan of ElevenLabs can refuse an output format. For each request, the
editor tries the formats in sequence, best first.

- A 400, 403 or 422 that names `output_format`, `feature_not_available`
  or `subscription_required` is a refused format. The editor then tries
  the next format.
- The editor remembers refused formats for the session. Later requests
  start with a format that the plan accepts.
- The editor always tries the last format.

| Request | Formats, best first |
| --- | --- |
| Voiceover | `wav_48000` (Pro), `mp3_44100_192` (Creator), `mp3_44100_128` |
| Sound effect | `wav_48000` (Pro), `mp3_44100_192` (Creator), `mp3_44100_128` |
| Sound effect that loops | `mp3_44100_192`, `mp3_44100_128` |
| Music | `mp3_48000_320`, `mp3_48000_192`, `mp3_44100_128` |

WAV is best for a voiceover, because the user edits it after. ElevenLabs
makes 48 kHz WAV only for effects that do not loop. The v2 music models
make 48 kHz MP3.

## Errors

The body of an error is one of these shapes. The shape changes with the
endpoint and its age.

- `{"detail": {"type", "code", "status", "message", "data"}}`
- `{"detail": [{"msg", …}]}`: a list of validation errors. The editor
  joins them with `; `.
- `{"detail": "..."}`

The editor maps the error (`mod.rs`, `error`). It examines the rows in
this sequence. "Code" is any of `detail.code`, `detail.status` and
`detail.type`.

| Condition | Result in the editor |
| --- | --- |
| Code `bad_prompt` or `bad_composition_plan` | refused prompt, with `data.prompt_suggestion` or `data.composition_plan_suggestion` as a suggestion |
| Code `quota_exceeded` or `insufficient_credits`, or HTTP 402 | out of credits |
| A code that has `limit_reached`, or that starts with `can_not_use` | not allowed (no voice slots, or no instant cloning on the plan) |
| Code `missing_permissions` or `insufficient_permissions` | not allowed (the key has a scope for other endpoints) |
| 401 | bad key |
| 403 | not allowed |
| 429 | busy |
| Other | failed, with the message of ElevenLabs |

## Plan

`GET /v1/user/subscription` gives `tier`, `character_count` and
`character_limit`. Settings → Integrations shows the plan and the
credits used. If the key cannot read the plan, the editor still accepts
the key and shows no plan.

## Transcription

`POST /v1/speech-to-text` (`transcribe.rs`), as a multipart upload of
the whole file in one request.

| Model | Name |
| --- | --- |
| `scribe_v2` (default) | Scribe v2 |
| `scribe_v2_medical` | Scribe v2 Medical |

- Fields: `model_id`, `timestamps_granularity: word`,
  `tag_audio_events: true`, `diarize`, and `language_code` when the user
  sets a language.
- The audio is FLAC, WAV or MP3. The editor sends FLAC from analysis.
- The editor keeps filler words. The cut of dead time needs the "um"s.
- Each word of `type: "word"` becomes a word with its times, its speaker
  (`speaker_id`) and a confidence of `exp(logprob)`.
- Each item of `type: "audio_event"`, such as "(laughter)", becomes a
  sound. The editor removes the brackets and writes the label in lower
  case.
- ElevenLabs charges by the minute of audio.

## Music

`POST /v1/music` (`music.rs`).

| Model | Name |
| --- | --- |
| `music_v2_5` (default) | Music v2.5 |
| `music_v2` | Music v2 |
| `music_v1` | Music v1 |

- A piece is 3 s to 5 min. The API accepts 10 minutes, but the plans
  sell 5.
- Without sections, the editor sends `prompt` (4,100 characters),
  `force_instrumental` and `music_length_ms`.
- With sections, the editor sends a `composition_plan` in place of the
  prompt. Then the music changes on cue.
  - A plan has a maximum of 30 sections.
  - Each section is 3 s to 2 min.
  - The sections together are 3 s to 5 min.
  - The editor refuses a plan that breaks these limits before it sends
    it.
- The shape of the plan changes with the model:
  - `music_v1`: `positive_global_styles`, `negative_global_styles` and
    `sections` with `lines`. The request also has
    `respect_sections_durations: true`.
  - v2 and later: `chunks`. The text of each chunk has the name of the
    section in brackets, then its lyrics or `{instrumental}`.
  - An instrumental plan adds "vocals" to the styles to avoid.
- The editor also sends this plan to the music models of ElevenLabs on
  other hosts, such as fal.

## Sound effects

`POST /v1/sound-generation` (`sound.rs`).

| Model | Name |
| --- | --- |
| `eleven_text_to_sound_v2` (default) | Sound Effects v2 |
| `eleven_text_to_sound_v3` | Sound Effects v3 |

- Fields: `text`, `model_id`, `loop`, and `duration_seconds` when the
  user asks for a length.
- A length is 0.5–30 s, in steps of 0.5 s.

## Voiceovers

`POST /v1/text-to-speech/{voice}` (`speech.rs`).

| Model | Name |
| --- | --- |
| `eleven_v4` | Eleven v4 |

- Fields: `text`, `model_id` and `voice_settings` with `stability` and
  `similarity_boost` (0–1).
- Eleven v4 reads the delivery from the script: audio tags, ellipses,
  punctuation and capitals. It does not read SSML.
- Eleven v4 takes only stability and similarity. Style and speed of the
  API do not apply to it. To change the speed, the user changes the
  timing of the clip.
- The model's `VoiceoverModel` (in `mod.rs`) says what it reads: 10,000
  characters, audio tags, stability and similarity, and every voice
  operation below. Before any provider gets a script, `Cloud::generate`
  checks it against the picked model, and refuses these scripts:
  - a script longer than the model takes;
  - a script with `<break>` tags. The refusal has a suggestion with
    pauses in place of the tags.
- The voice id goes in the path. Thus the editor accepts only an id of
  ASCII letters and digits. The agent can write the id.
- A word fix continues a recording. Thus the request also sends the
  speech around the new words as `previous_text` and `next_text`. Then
  the delivery fits. If a model refuses these fields, the editor sends
  the request again without them.

## Voices

`voices.rs` and `design.rs`. A voice is on the account at ElevenLabs.

| Operation | Endpoint | Notes |
| --- | --- | --- |
| List | `GET /v2/voices?page_size=100` | The account voices and the default voices. The editor reads 20 pages at most, with `next_page_token`. |
| Search | `GET /v1/shared-voices?page_size=30&page=…` | The shared library. Filters: `search`, `gender`, `age`, `language`, `use_cases`. |
| Similar | `POST /v1/similar-voices` | Multipart `audio_file`, `top_k: 20` |
| Add | `POST /v1/voices/add/{owner}/{voice}` | Adds a shared voice to the account, with `new_name` |
| Design | `POST /v1/text-to-voice/design?output_format=mp3_44100_128` | `voice_description`, `model_id: eleven_ttv_v3`, and `text` or `auto_generate_text: true` |
| Save a design | `POST /v1/text-to-voice` | `voice_name`, `voice_description`, `generated_voice_id` |
| Clone | `POST /v1/voices/add` | Multipart `name`, `files`, `remove_background_noise`, `description` |
| Rename | `POST /v1/voices/{voice}/edit` | Multipart `name` |
| Delete | `DELETE /v1/voices/{voice}` | A 404 is a success, because the voice is gone |

- The traits of a voice come from its labels, in this sequence: gender,
  age, accent, descriptive and use case.
- A shared voice has an owner (`public_owner_id`). It can also have a
  rate above 1 and a notice period in days.
- Design uses `eleven_ttv_v3`, because it follows a description most
  closely. A designed voice reads with each speech model.
- A design description is 20–1,000 characters. A design text is
  100–1,000 characters.
- Design previews come back as base64 MP3. The editor writes them to
  `voice-previews` in the cache folder. Each plan accepts
  `mp3_44100_128`.
- Voice ids go in paths. Thus the editor accepts only ids of ASCII
  letters and digits.
- The agent has no tool to clone a voice, because a clone needs the
  consent of the speaker.

## Sources

The code has no links to the documents of ElevenLabs. We did not
compare this file with them.

- <https://elevenlabs.io/app/settings/api-keys>
- The endpoints above, on `https://api.elevenlabs.io`
