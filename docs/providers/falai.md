# fal

fal (<https://fal.ai>) runs models from many vendors behind one queue
API. One key gives access to all of them.

The editor uses fal in four roles:

- a video provider (see "Video");
- an image provider (see "Image");
- a source of music and sound effects (see "Audio");
- a video upscaler (see "Upscaling").

fal does not do voiceovers or transcription for the editor. For these
capabilities, the editor answers `Unavailable`.

This file describes the client in `crates/flikk-cloud/src/fal/` as it
is. The code is the source of each fact.

## Access

| | |
| --- | --- |
| Queue | `https://queue.fal.run` |
| Upload | `https://rest.alpha.fal.ai/storage/upload/initiate?storage_type=fal-cdn-v3` |
| Billing | `https://api.fal.ai/v1/account/billing?expand=credits` |
| Auth | `Authorization: Key <key>` on each call to the queue, the upload and the billing |
| No auth | the signed `PUT` of an upload, and the download of a result from the CDN of fal |
| Keys | you make them at <https://fal.ai/dashboard/keys> |
| Env var | `FAL_KEY` |

The key is in the credential store of the system, under the service
`flikk` and the account `fal`. If the system has no credential store,
the key is in an owner-only `credentials.json` in the user-data folder.
The environment variable has priority over the two.

## Job flow

All work goes through the queue of fal (`queue.rs`).

1. **Upload** (`files.rs`, `url`): local files go to the storage of fal.
   This is the same flow that the clients of fal use.
   - `POST` the initiate URL with `{"content_type", "file_name"}`. The
     answer has `upload_url` and `file_url`.
   - `PUT` the raw bytes to `upload_url` with the `content-type`.
   - The request then names `file_url`.
   - If the initiate or the `PUT` fails, the editor sends the file inline
     as a `data:` URI. This is possible only up to 8 MiB. A bad key or no
     credits does not cause this fallback.
   - The editor uploads frames, references, edit pictures and the source
     of an upscale.
2. **Submit**: `POST https://queue.fal.run/<endpoint>` with the JSON
   input.
   - The answer must have `request_id`.
   - The editor uses the `status_url`, `response_url` and `cancel_url` of
     the answer. If they are missing, it builds them from
     `{queue}/{endpoint}/requests/{id}`.
   - The editor does not send a submit again after a 5xx. fal can have
     the request already, and charge for it. The editor sends it again
     only after a 429 or a connection that did not start.
3. **Poll**: `GET status_url`.
   - `IN_QUEUE` gives `queue_position`. `IN_PROGRESS` means that the
     model runs. `COMPLETED` ends the poll.
   - A failed request is also `COMPLETED`, with an `error` field.
   - A 404 in the first 60 s after the submit means "look again".
   - 408, 425, 429, 5xx, network errors and bad JSON also mean "look
     again". A failed look does not stop the job, because the request
     continues at fal.
   - If the status is not documented and has an `error` field, the
     editor maps the error. If its name has `fail`, `error`, `cancel`,
     `expire`, `timeout`, `abort` or `reject` in it, the job fails. Else
     the editor waits for 10 minutes, then cancels it.
4. **Result**: `GET response_url`.
   - After `COMPLETED`, the job cannot stop. The result is made and paid
     for.
   - The editor reads `video.url` for video and upscale. It reads
     `images[0].url` for an image. For audio, it reads the first of
     `audio`, `audio_file` and `audios[0]`.
   - The extension comes from `content_type` or `file_name`.
   - The download goes to a `.part` file, without the key.
5. **Cancel**: `PUT cancel_url` with no body.

   | Answer | Result in the editor |
   | --- | --- |
   | `status: "ALREADY_COMPLETED"` | The editor gets the result and keeps it |
   | 404 | Stopped |
   | Other 2xx, such as `CANCELLATION_REQUESTED` | The editor continues to poll |
   | Other answers | The editor sends the cancel again after 15 s |

   After a cancel, a request that stays `IN_QUEUE` for 60 s is cancelled.
   A request that is `IN_PROGRESS` continues, and the editor keeps what it
   makes, because fal charges for it.

## Timers

| Constant | Value | Use |
| --- | --- | --- |
| `POLL` | 3 s | Time between two looks. After failed looks, it doubles to a maximum of 60 s. |
| `TICK` | 250 ms | Time between two checks for a stop from the user |
| `FRESH` | 60 s | Time after the submit in which a 404 means "look again" |
| `STRANGE` | 10 min | Wait on a status that is not documented |
| `CANCEL_AGAIN` | 15 s | Time before the editor sends a cancel again |
| `STOPPING` | 60 s | Wait on a queued request after a cancel |
| `PATIENCE` | 2 h | Maximum time for one request. Then the editor cancels it. |
| `RESULT_PATIENCE` | 10 min | Maximum wait for the result after `COMPLETED` |
| HTTP back-off | 2, 4, 8, 16 s | Retries of a request that can be sent again |

The HTTP agent has a connect timeout of 30 s and a response timeout of
60 minutes.

## Errors

fal answers with these body shapes (`mod.rs`, `error`):

- `{"detail": "..."}`
- `{"detail": [{"loc", "msg", "type"}]}`: a validation error. The editor
  joins the items as `field: msg; …`.
- `{"detail": {"type", "message"}}`
- `{"error": {"type", "message"}}`: the platform API.

The kind of the error comes from the first of these that exists: the
`x-fal-error-type` header, `error_type`, `detail.type` and `error.type`.

The editor maps the error (`mod.rs`, `failure`). It examines the rows in
this sequence:

| Condition | Result in the editor |
| --- | --- |
| 401, or kind `rejected_key` | bad key |
| 402, or the message has "balance" or "exhausted" | out of credits |
| The kind has `content_policy`, or the message has "content policy" | refused prompt |
| 403 | not allowed |
| 422 | refused prompt |
| 429 | busy |
| Other | failed, with the message of fal |

The editor also refuses some requests before it sends them:

- a model that is not in the catalog;
- references of a kind that the model has no field for;
- an image edit on a model without an edit endpoint;
- a shape that has no `image_size` name.

## Balance

`mod.rs`, `account`, reads `credits.current_balance` and
`credits.currency`. Settings → Integrations shows USD as `$x.xx`. It
shows other currencies as `x.xx CUR`. If the key cannot read the billing
(403), the editor still accepts the key and shows no balance.

## Video

`models.rs`, `VIDEO`, and `video.rs`. The first model is the default.

The request has these fields:

- `prompt`. In reference mode, the editor rewrites the tags (see below).
- `duration`, in whole seconds, as a string or a number, as the model
  uses it. If the user asks for no length, the editor sends `"auto"`,
  `null`, or no field, as the model uses it.
- `resolution`: the requested resolution, else the first of the model.
  MiniMax gets it in upper case.
- `aspect_ratio`: some endpoints never get it. Some get it only when the
  user asks for a shape. Some get a word, such as `auto` or `adaptive`,
  when the user asks for no shape.
- The sound switch of the model, when it has one.
- First frame: the start field of the model. Last frame: `end_image_url`.
- References: one list of URLs for each kind of media.

The editor writes references as `@Image1`. It rewrites them for each
model:

| Style | Tag |
| --- | --- |
| `At` | `@Image1` |
| `Spaced` | `Image 1` |
| `Character` | `character1` |
| `RefToken` | `<IMAGE_REF_0>`, counted from 0 |

| Model | Endpoints | Lengths | Resolutions | Start field | Sound field | References (pictures / clips / sounds / total / s) |
| --- | --- | --- | --- | --- | --- | --- |
| `bytedance/seedance-2.0` | text-, image-, reference-to-video | 4–15 s | 720p, 480p | `image_url` | `generate_audio` | 9 / 3 / 3 / 12 / 15, `At` |
| `bytedance/seedance-2.0/fast` | as above | 4–15 s | 720p, 480p | `image_url` | `generate_audio` | as above |
| `bytedance/seedance-2.5` | text-, image-, reference-to-video | 4–30 s | 720p, 1080p, 480p | `image_url` | `generate_audio` | 30 / 10 / 10 / 50 / 30 |
| `kling/v4` | text-, image-to-video | 3–30 s | 720p, 1080p | `start_image_url` | — | — |
| `fal-ai/kling-video/o3/pro` | text-, image-, reference-to-video | 3–15 s | — | `image_url` | `generate_audio` | 4 pictures, `At` |
| `minimax/h3-max` | text-, image-, reference-to-video | 1–15 s | 768P, 1080P, 480P | `image_url` | — | 9 / 3 / 3 / 12 / 15, `Spaced` |
| `alibaba/wan-3.0` | text-, image-to-video | 2–30 s | 720p, 1080p, 480p | `start_image_url` | `audio` | — |
| `alibaba/happy-horse/v1.1` | text-, image-, reference-to-video | 3–15 s | 720p, 1080p | `image_url` | — | 9 pictures, `Character` |
| `google/gemini-omni-flash/v1.1` | text-, image-, reference-to-video | 3–10 s | 720p, 1080p, 4k, 360p | `image_url` | — | 10 / 3 / 0 / 13 / 30, `RefToken` |
| `blackforestlabs/flux-3-max` | text-, image-to-video | 5–20 s | 720p, 1080p | `image_url` | `generate_audio` | — |
| `lightricks/ltx-2.5/pro` | `text-to-video/pro`, `image-to-video/pro` | 6, 8, 10 s | 1080p, 720p | `image_url` | `generate_audio` | — |

- On Seedance 2.5, Kling v4, Kling O3, MiniMax and Happy Horse, the first
  frame sets the shape.
- Happy Horse takes no last frame. All other models take one.
- MiniMax always gets `prompt_expansion_mode: "balanced"`.

## Image

`models.rs`, `IMAGE`, and `image.rs`. The first model is the default.

- The editor sends the shape as `aspect_ratio` or as `image_size`.
  `image_size` uses the names of fal: `square_hd`, `landscape_16_9`,
  `portrait_16_9`, `landscape_4_3` and `portrait_4_3`.
- FLUX 3 and Grok get the resolution in lower case.
- GPT Image and Seedream 5.5 get `background: transparent` or `opaque`.
- Edit pictures go in `image_urls`. Ideogram takes `image_url` and
  `reference_image_urls`.

| Model | Text / edit endpoint | Shape | Resolutions | Pictures | Transparent |
| --- | --- | --- | --- | --- | --- |
| `google/nano-banana-2.1` | the id / `/edit` | `aspect_ratio`, `auto` | 1K, 2K, 4K | 14 | — |
| `fal-ai/nano-banana-pro` | the id / `/edit` | `aspect_ratio`, `auto` | 1K, 2K, 4K | 14 | — |
| `openai/gpt-image-2` | the id / `/edit` | `image_size`, `auto` | — | 16 | ✓ |
| `openai/gpt-image-2.6/flare` | `/text-to-image` / `/edit` | `image_size`, `auto` | — | 16 | ✓ |
| `openai/gpt-image-2.6/sunburst` | `/text-to-image` / `/edit` | `image_size`, `auto` | — | 16 | ✓ |
| `blackforestlabs/flux-3` | `/text-to-image` / `/edit-image` | `aspect_ratio`, `auto` | 1K, 1.5K, 2K, 4K | 10 | — |
| `blackforestlabs/flux-3/klein` | `/text-to-image` / `/edit-image` | `image_size` | — | 10 | — |
| `bytedance/seedream/v5.5` | `/text-to-image` / `/edit` | `image_size`, `auto_2K` | — | 10 | ✓ |
| `bytedance/seedream/v5/pro` | `/text-to-image` / `/edit` | `image_size`, `auto_2K` | — | 10 | — |
| `ideogram/v4.5` | the id / `/edit` | `image_size` | — | 5 | — |
| `xai/grok-imagine-image/v2.0` | `/text-to-image` / `/edit` | `aspect_ratio`, `auto` | 1K, 1.5K, 2K | 5 | — |
| `alibaba/qwen-image-3` | `/text-to-image` / `/edit` | `image_size` | — | 3 | — |
| `krea/v3` | `/text-to-image` / `/edit` | `image_size`, `auto` | — | 8 | — |
| `fal-ai/recraft/v4.1/pro` | `/text-to-image` / — | `image_size` | — | text only | — |

## Audio

`audio.rs`. The endpoint is the id of the model. The editor cuts the
prompt to the limit of the model. It fits the length to the range and
step of the model. Music sections go into the prompt as timed parts. If
the model has no instrumental switch, the editor adds "Instrumental
only, no vocals." to the prompt.

### Music

| Model | Prompt | Length | Other fields |
| --- | --- | --- | --- |
| `elevenlabs/music/v2.5` (default) | `prompt`, 4,100 | `music_length_ms`, 3–300 s | `force_instrumental`; `output_format: mp3_48000_320`. Sections go in a `composition_plan` in place of the prompt. |
| `sonilo/v1.1/text-to-music` | `prompt`, 2,000 | `duration` 5–600 s, 30 s if the user asks for no length | `num_samples: 1` |
| `google/lyria-3.5` | `prompt`, 5,000 | the model selects it | — |
| `fal-ai/lyria3/pro` | `prompt`, 5,000 | the model selects it | — |
| `fal-ai/stable-audio-3/medium/text-to-audio` | `prompt`, 2,000 | `duration` 5–380 s | `output_format: wav` |

A request to ElevenLabs Music on fal can take 10 minutes. With a
composition plan, it takes approximately 5 minutes.

### Sound effects

| Model | Prompt | Length | Other fields |
| --- | --- | --- | --- |
| `elevenlabs/sound-effects/v3` (default) | `text`, 450 | `duration_seconds` 0.5–30 s, step 0.5 | `loop`; `output_format: mp3_44100_192` |
| `sonilo/v1.1/text-to-sound-effects` | `prompt`, 2,000 | `duration` 0.5–180 s | `audio_format: wav` |
| `mirelo-ai/sfx2.0/text-to-audio` | `prompt`, 2,000 | `duration` 0.5–600 s | `num_samples: 1`; `upload_audio_format: wav` |
| `fal-ai/stable-audio-3/small/sfx/text-to-audio` | `prompt`, 2,000 | `duration` 1–120 s, step 0.5 | `output_format: wav` |

Mirelo makes two takes by default, and fal charges for each take. Thus
the editor sends `num_samples: 1`.

## Upscaling

`upscale.rs`. The editor uploads the source and sends it as `video_url`.
The targets are 720p, 1080p, 2k and 4k. Gemini has no 2k.

| Model | Scale | Fields | Limits |
| --- | --- | --- | --- |
| `topaz/upscale/video/precision` (default) | factor 1–4 | `upscale_factor`, `model: "Proteus"`, `H264_output: true` | — |
| `topaz/upscale/video/generative` | factor 1–4 | `upscale_factor`, `model: "Starlight Precise 2.6"`, `H264_output: true` | — |
| `fal-ai/seedvr/upscale/video` | resolution | `upscale_mode: "target"`, `target_resolution` (2k as 1440p, 4k as 2160p) | — |
| `clarityai/crystal-video-upscaler` | factor 1–8 | `scale_factor` | 5120 × 2880 at most |
| `blackforestlabs/flux-video-upscale` | factor 1.5–3 | `upscale_factor`, `creativity: 0` | 20 s at most |
| `google/gemini-omni-flash/v1.2/upscale` | resolution | `resolution` | 15 s at most |

Proteus is the default because it fits most footage and keeps the
picture of the source. FLUX gets `creativity: 0`, so that it does not
add new detail.

## Sources

The code has no links to the documents of fal. We did not compare this
file with them.

- <https://fal.ai/dashboard/keys>
- The endpoints in "Access"
