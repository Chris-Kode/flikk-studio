# Pika

The developer API of Pika (the "Pika API Club", <https://dev.pika.art/>)
is now a paid, prepaid aggregator. One key gives access to Pika's own
models and to approximately 170 operations from other vendors. These
vendors include Seedance, Kling, Veo, Wan, MiniMax, ElevenLabs and more.

The editor uses the API in three roles:

- A video provider for Pika's own models, and for the video models of
  other vendors that fit its video request (see "What the editor uses").
- An image provider for the image models of other vendors (see "Image
  operations").
- A source of music and sound effects, from Pika and from other vendors
  (see "Audio operations").
- A voiceover provider, from Pika and from other vendors, with the
  voices the account clones (see "Voiceovers" and "Voices").

We read these sources on 2026-11-02: `llms.txt`, the specification of
each operation, the public catalog and `openapi.json` (links at the end).
If the prose and the schema are different, Pika states that the
`input_schema` of the catalog is the source of truth.

## Access

| | |
| --- | --- |
| Base URL | `https://api.dev.pika.art` |
| Auth | `X-API-Key: <key>` (or `Authorization: Bearer <key>` on `/v1/media`) |
| Keys | you make, rotate and revoke them at <https://dev.pika.art/keys> |
| Env var | `PIKA_API_KEY` (the name that the docs of Pika suggest) |
| Billing | prepaid in micro-USD (1,000,000 = $1). A $10/month membership gives back $10 of credits. There is no free tier and no sandbox. Pika charges only for successful generations |
| Free calls | `GET /catalog/apis`, `GET /catalog/apis/{api_id}?expand=inputs` (no key), `POST /catalog/apis/{api_id}/quote`, `GET /billing/balance` |

## Job flow

1. **Upload** (local files): `POST /v1/media/uploads` with
   `{"content_type": "image/png", "size_bytes": 12345}` → `upload_url`
   (valid 5 minutes), `headers`, `url`, `expires_in`, `max_bytes`.
   - `PUT` the raw bytes to `upload_url`. Send `headers` verbatim.
     `Content-Type` and `Content-Length` are signed. If you do not send
     them verbatim, the storage answers a bare 403.
   - The request then names `url`.
   - The limits are 20 MB for an image or audio and 50 MB for a video.
     `413` means too big. `415` means an unsupported type.
   - You can send a public URL as it is.
   - `DELETE /v1/media/uploads?url=…` erases an upload.
2. **Submit**: `POST /v1/media/{vendor}/{model}/{function}` with the JSON
   body → the job, `{"id": "media_…", "status": "queued"}`. The request
   can have an optional `Idempotency-Key` header and an optional
   `webhook_url`.
3. **Poll**: `GET /v1/media/jobs/{id}`.
   - `status` goes `queued` → `running` → `completed` or `failed`. It can
     skip `running`.
   - A completed job has `output.video.url` (and `content_type`). A
     failed job has `error.code` and `error.message`.
   - A terminal job has `usage` and `billing` (`state`: `settled` with
     `charge_micro_usd`, `pending`, `unavailable`).
   - `GET /v1/media/jobs/{id}/content` gives the same URL as `{"url"}`.
     Before the job is complete, it answers `409`.
4. **Cancel**: there is no cancel operation. `DELETE /v1/media/jobs/{id}`
   erases the media and the request of a finished job. On a running job,
   it answers `409`.

## Errors

There are two body shapes:

- A request error before a job exists is `{"message": "..."}`.
- Pika can refuse a submit after the job row exists. The answer is then
  the failed job:
  `{"id", "status": "failed", "error": {"code", "message"}}`.

| HTTP | Meaning |
| --- | --- |
| 401 | Key missing or invalid (`{"message":"Invalid API key"}`; `"missing API key"` without one) |
| 403 | Inactive key (`{"message"}`), or the balance / postpaid cycle cannot pay for it (failed job, `insufficient_balance` / `cycle_limit_exceeded`) |
| 404 | Unknown job, or unknown vendor/model/function |
| 409 | A request for the result before it is ready, or an idempotency key used again with a different body |
| 413 / 415 | Upload too big, or of a type that it does not accept |
| 422 | Validation: `{"message": "duration: Input should be less than or equal to 15"}`. An unpriceable combination is a failed job with `invalid_input` |
| 429 | Per-minute or per-day limit, or the queue of the org is full (failed job, `rate_limited`; honour `Retry-After`) |
| 503 | A Pika dependency is down (failed job, `provider_unavailable`) |

`error.code` is one of `invalid_input`, `content_moderation`,
`provider_error`, `provider_timeout`, `provider_unavailable`,
`rate_limited`, `insufficient_balance`, `membership_required`,
`cycle_limit_exceeded`, `admission_suspended`, `usage_limit_exceeded`,
`timed_out`, `internal`.

The editor maps the codes (`pika/mod.rs`, `failure`):

| `error.code` | Result in the editor |
| --- | --- |
| `insufficient_balance`, `cycle_limit_exceeded` | out of credits |
| `membership_required`, `usage_limit_exceeded` | not allowed |
| `invalid_input`, `content_moderation` | refused prompt |
| `rate_limited`, `provider_unavailable` | busy |
| Other codes | failed, with the message of Pika |

If there is no code, the editor maps the HTTP status:

| HTTP | Result in the editor |
| --- | --- |
| 401 | bad key |
| 403 | not allowed |
| 422 | refused prompt |
| 429 / 503 | busy |

## Balance

`GET /billing/balance` uses the same key and is free. It gives these
fields: `unit` (`micro_usd`), `currency`, `org_id`, `balance_micro_usd`
(prepaid plus comp credits), `prepaid_micro_usd`, `credit_micro_usd` and
`postpaid`.

`postpaid` is `null` for a prepaid org. When it is `active`,
`cycle.remaining_micro_usd` is the amount that the org can still spend.
Settings → Integrations shows the balance as the dollars that remain.

## Pika's video operations

All operations are async. All use `POST /v1/media/pika/…`.

### Pika H3 Flex — `pika-h3-flex/{text,image,reference}-to-video`

This model makes video with native, synchronized sound. No switch
disables the sound.

| Field | Type | Values / default |
| --- | --- | --- |
| `prompt` | string, required | 1–8000 chars |
| `duration` | integer | 5–15, default 5 |
| `aspect_ratio` | enum or null | `auto`, `21:9`, `16:9`, `3:2`, `4:3`, `5:4`, `1:1`, `4:5`, `3:4`, `2:3`, `9:16`, `9:21`. If left out: `auto` while `rewrite` is on, else the ratio of the first keyframe, else 16:9. A fixed ratio has priority over the frames (the frames are cropped) |
| `resolution` | enum | `480p`, `768p` (default) native; `1080p`, `2k`, `4k` are 768p upscaled |
| `seed` | integer | default 0 |
| `rewrite` | boolean | default true: Pika rewrites the prompt |
| `output_mode` | enum | `video` (default), `audio-stems` (adds dialogue, music and effects WAVs) |
| `channel_mode` | enum | `mono` (default), `stereo`; stems only |
| `image` | URL | image-to-video: first frame (JPEG, PNG, WebP; 24 MiB, 40 MP) |
| `last_frame_image` | URL | image-to-video: last frame; `image`, this, or both |
| `references` | `[{type: image\|video\|audio, url}]`, required | reference-to-video: 1–12, at most 9 images, 3 videos, 3 audio; at least one image or video; each 24 MiB, 64 MiB base64 together; clips and sounds 2–15 s, 15 s together per kind |

The prompt cites references as `@Image1`, `@Video1`, `@Audio1`. The
count is per kind, in sequence. The prose specification also mentions
flat `image_urls` / `video_urls` / `audio_urls` lists. The schema has
priority, and it takes `references`.

The price for each output second is $0.01 at 480p to $0.08–0.10 at 4K.
Reference-to-video costs more. It also adds the reference video seconds.

### Pika 2.5 — `pika-2.5/{text,image}-to-video`

This model makes no sound.

| Field | Type | Values / default |
| --- | --- | --- |
| `prompt` | string | text: required, 1–4000 chars; image: optional |
| `negative_prompt` | string | optional |
| `duration_s` | integer | `5` (default) or `10` |
| `resolution` | enum | `480p`, `720p`, `1080p` (default) |
| `aspect_ratio` | enum or null | text only: `5:2`, `16:9`, `4:3`, `1:1`, `4:5`, `9:16`; if left out, the model selects |
| `seed` | integer or null | random by default |
| `image` | URL, required | image-to-video: the picture to animate (the video has its shape) |

The price is $0.02–0.09 per second, by resolution.

### Pikaframes (Pika 2.5 Keyframe) — `pikaframes/image-to-video`

| Field | Type | Values / default |
| --- | --- | --- |
| `images` | URL[], required | 1–5 ordered keyframes; one image needs `loop` |
| `prompt` | string | default motion prompt |
| `transition_duration_s` | integer | 1–10, default 5; 5 at most each when there are several transitions |
| `transition_durations_s` | integer[] | per transition, overrides the above |
| `transition_prompts` | string[] | per transition, overrides `prompt` |
| `loop` | boolean | default false: adds a transition back to the first |
| `resolution` | enum | `720p`, `1080p` (default) |
| `negative_prompt`, `seed` | | optional |

### Not used by the editor

- `pika-h3-flex/prompt-rewriter`, `pika-h3-flex/audio-stems`
  (`draft-to-video` is used to upscale; see "Upscaling");
- Pikadditions and Pikaswaps (`video-to-video`: add or swap something in
  a clip), Pikaffects (`image-to-video`, `video-to-video`: named
  effects);
- Pika Dynamic Captions, Video Compose, Video Merge and Pika Soundtrack
  (sound for a full video);
- webhooks, idempotency keys, quotes, spend reports.

Some of these operations edit media in a way that the video request of
the editor has no field for.

## Other vendors' video models

Each model is at `POST /v1/media/{vendor}/{model}/{function}` and has its
own fields. The `input_schema` of the catalog is the reference.
`pika/models.rs` records how each model takes a request.

| Model | Text / first frame / references | Length | Aspect field | Resolutions | First / last frame fields | References | Sound |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Seedance 2.0 (`bytedance/seedance-2.0`) | text, image, reference-to-video | 4–15 or `"auto"` (required) | `ratio`: 1:1, 4:3, 3:4, 16:9, 9:16, 21:9, `adaptive` | 480p, 720p, 1080p, 4k (required) | `image_url`, `end_image_url` | `image_urls` 9, `video_urls` 3, `audio_urls` 3; clips and sounds 2–15 s, 15 s per kind | `generate_audio` |
| Seedance 2.0 Fast, 2.0 Mini | same | same | same | 480p, 720p (default) | same | same | same |
| Seedance 2.5 (`bytedance/seedance-2.5`) | same | 4–30 or `"auto"` | same (a plain string on image-to-video) | 480p, 720p (default), 1080p | same | 30 images, 10 videos, 10 sounds; 2–30 s, 30 s per kind; also `draft`, `omni_reference_task_type` (edit/extend) | same |
| Kling 4.0 (`kling/kling-4.0`) | text, image, omni-video | 3–30 | `aspect_ratio`: 21:9, 16:9, 9:16, 1:1 (text, omni) | 720p, 1080p | `image_url`, `last_frame_url`, `middle_frame_urls` | omni `contents`: `{type, url, id}` items cited `@id`; no stated limits | always |
| Kling 3.0 | text, image, omni-video | 3–15 | `aspect_ratio`: 16:9, 9:16, 1:1 | 720p, 1080p, 4k | `image_url`, `last_frame_url`; `elements` | omni `contents` | `audio`: `native`/`off` |
| Kling 3.0 Turbo | text, image | 3–15 | as 3.0 (text) | 720p, 1080p | `image_url` | — | always |
| MiniMax H3 (`minimax/h3`) | text, image, reference-to-video | 4–15 | `ratio` (text, reference) | `768P`, `2K` (default) | `first_frame_image`, `last_frame_image` | `image_urls` 9, `video_urls` 3, `audio_urls` 3, cited `Image 1`, `Video 1`, `Audio 1`; clips 15 s together | — |
| Hailuo 2.3 (`minimax/hailuo-2.3`) | text, image | 6 or 10 (1080p: 6 only) | none | 768p, 1080p | `first_frame_image` | — | — |
| Wan 3.0, Wan 3.0 Prime (`alibaba/wan3.0-video[-prime]`) | text, image, omni-video | 2–30 or `"auto"` | `ratio`: `adaptive`, 16:9, 4:3, 1:1, 3:4, 9:16 | 480p, 720p, 1080p (default) | `first_frame_url`, `last_frame_url` | `reference_image_urls` 10, `reference_video_urls` 5, `reference_audio_urls` 5; also `file_url`, `web_url` | `audio` |
| Happy Horse 1.0, 1.1 (`alibaba/happyhorse-1.x`) | text, image, reference-to-video | 3–15 | `ratio`: 16:9, 9:16, 1:1, 4:3, 3:4, 4:5, 5:4, 9:21, 21:9 | `720P`, `1080P` (default) | `image_url` | `reference_image_urls` 1–9 | — |
| Gemini Omni Flash (`google/gemini-omni-flash`) | text, image, reference-to-video | 3–10 (0: the model picks) | `aspect_ratio`: 16:9, 9:16 | none | `image_urls` (1–2: start, end) | `image_urls` 1–6 | — |
| FLUX 3 (`black-forest-labs/flux-3-video`) | text, image | 5–20 or `"auto"` | `aspect_ratio`: `auto`, 21:9, 2:1, 16:9, 4:3, 1:1, 3:4, 9:16 | 720p, 1080p | `keyframes: [{image_url, at_s?}]` (1–10) | — | `generate_audio` |
| Grok Imagine Video (`x-ai/grok-imagine-video`) | text, image, reference-to-video | 1–15 | `aspect_ratio`: 1:1, 16:9, 9:16, 4:3, 3:4, 3:2, 2:3 | 480p, 720p | `image_url` | `reference_image_urls` 1–3 | — |

The editor does not offer the models below. Its video request needs both
text and image to video, and always sends a length.

- Veo 3.1 and Veo 3.1 Fast: text-to-video only (with up to 3 reference
  images at 8 s), plus video extension;
- Veo 3.1 Lite, Hailuo 2.3 Fast, Grok Imagine Video 1.5: image-to-video
  only;
- Gemini Omni 1.1 Flash: no length field;
- the other video operations: video-to-video, video extension, motion
  control, avatars and lip sync. The editor offers upscaling separately
  (see "Upscaling").

The editor does not use the omni-video references of Kling, because
Pika states no limits for them. For most models, the prompt cites
references as `@Image1`, `@Video1`, `@Audio1`. The specifications of
Pika say this for every reference model. The fields of MiniMax H3 say
`Image 1`. Thus the editor rewrites its tags for MiniMax H3.

## What the editor uses

`crates/flikk-cloud/src/pika/`:

| Model (catalog id) | Text | First frame | First + last | References | Lengths | Shapes | Resolutions | Sound |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Pika H3 Flex (`pika/pika-h3-flex`, default) | text-to-video | image-to-video | image-to-video | reference-to-video (9 pictures, 3 clips, 3 sounds, 12 in all, 15 s per kind) | 5–15 s | the 11 ratios above | 768p, 480p, 1080p, 2k, 4k | always |
| Pika 2.5 (`pika/pika-2.5`) | text-to-video | image-to-video (the picture sets the shape) | Pikaframes, one transition | — | 5 or 10 s | `5:2`, `16:9`, `4:3`, `1:1`, `4:5`, `9:16` (text) | 1080p, 720p, 480p (720p or 1080p between two frames) | none |
| Seedance 2.0, 2.0 Fast, 2.0 Mini, 2.5 | ✓ | ✓ | ✓ | ✓ | as above | without `adaptive` | as above | switch |
| Kling 4.0, 3.0 | ✓ | ✓ (sets the shape) | ✓ | — | as above | as above | as above | 4.0 always, 3.0 switch |
| Kling 3.0 Turbo | ✓ | ✓ (sets the shape) | — | — | 3–15 s | as above | 720p, 1080p | always |
| MiniMax H3 | ✓ | ✓ (sets the shape) | ✓ | ✓ | 4–15 s | as above | 2k, 768p | — |
| Hailuo 2.3 | ✓ | ✓ | — | — | 6 or 10 s | — | 768p, 1080p | — |
| Wan 3.0, Wan 3.0 Prime | ✓ | ✓ | ✓ | ✓ (omni-video) | 2–30 s | without `adaptive` | 1080p, 720p, 480p | switch |
| Happy Horse 1.1, 1.0 | ✓ | ✓ | — | pictures | 3–15 s | as above | 1080p, 720p | — |
| Gemini Omni Flash | ✓ | ✓ | ✓ | pictures | 3–10 s | 16:9, 9:16 | none | — |
| FLUX 3 | ✓ | ✓ | ✓ (two keyframes) | — | 5–20 s | without `auto` | 720p, 1080p | switch |
| Grok Imagine Video | ✓ | ✓ | — | pictures | 1–15 s | as above | 720p, 480p | — |

- The request:
  - The editor sends the prompt as the user wrote it. For MiniMax H3, the
    editor rewrites the reference tags.
  - `rewrite`, `seed`, `draft`, `watermark` and similar fields keep their
    defaults.
  - If the user asks for no shape, the request leaves the field out.
  - If the user asks for no length, the request also leaves the field
    out. Or it sends `"auto"` where the model takes it. Seedance requires
    a length.
  - The editor always sends the resolution, in the letter case that the
    model uses.
- Before the editor uploads a file or spends credit, `VideoModel::check`
  and `pika/video.rs` refuse the requests that Pika would refuse:
  - references that are only sounds;
  - clips or sounds under 2 s (H3 Flex, Seedance);
  - 480p between two frames on Pika 2.5;
  - over 6 s at 1080p on Hailuo 2.3.
- The editor uploads files with `POST /v1/media/uploads` and a signed
  `PUT`.
- The job:
  - The editor polls the job every 3 s while it is queued or running.
  - The job cannot stop. Thus the media pool shows no stop button for it.
  - The editor follows the job for two hours at most.
  - The editor downloads the video from `output.video.url` without the
    key.
  - The editor does not record the charge of the job.

## Image operations

Each image model has two operations at
`POST /v1/media/{vendor}/{model}/…`:

- `text-to-image`;
- `image-to-image`: it edits or follows from 1 picture up to its limit of
  pictures, as `image_urls`.

Jobs, uploads and errors are the same as for video. A completed job has
`output.images[].url` (and `content_type`). We read this data on
2026-11-07 from the `input_schema` of the catalog.

`crates/flikk-cloud/src/pika/` (`models::IMAGE`, `image.rs`):

| Model (catalog id) | Shape | Resolution | Pictures | Transparent |
| --- | --- | --- | --- | --- |
| Nano Banana 2.1 (`google/gemini-nano-banana-2.1`, default) | `aspect_ratio`, `auto` | `resolution` 1K, 2K, 4K | 16 | — |
| Nano Banana Pro (`google/gemini-3-pro-image`) | as above | as above | 16 | — |
| GPT Image 2, 2.6 Flare, 2.6 Sunburst (`openai/gpt-image-2`, `-2.6-flare`, `-2.6-sunburst`) | `aspect_ratio`, no `auto` (1:1 left out) | `resolution` 1K, 2K, 4K | 16 | `background` |
| Seedream 5.5 (`bytedance/seedream-5.5`) | `aspect_ratio` (left out: picked from the prompt) | `size` 2K, 1.5K, 1K | 10 | `background` |
| Seedream 5.0 Pro (`bytedance/seedream-5.0-pro`) | `aspect_ratio` (left out: picked from the prompt) | `size` 2K, 1.5K, 1K | 10 | — |
| Seedream 5.0 Lite (`bytedance/seedream-5.0-lite`) | as above | `size` 2K, 3K, 4K | 14 | — |
| Ideogram 4.5 (`ideogram/ideogram-4.5`) | `size` in pixels: for each ratio, the larger size in its text-to-image enum. Editing accepts any `WxH` in multiples of 32 up to 4,194,304 pixels, and these sizes obey that rule | — | 5 | — |
| Grok Imagine Image 2.0 (`x-ai/grok-imagine-image-2.0`) | `aspect_ratio`, `auto` | `resolution` 1K, 2K | 5 | — |
| Muse Image (`meta/muse-image-1.0`) | none (`size` left at `auto`) | — | 10 | — |
| Recraft 4.1 (`recraft/recraft-4.1`) | none: its `size` answers 422 "not enabled on this environment yet" | — | text only | — |

`num_images`, `seed`, `quality`, `output_format` and similar fields keep
their defaults. Each request makes one picture.

The editor does not use these models:

- Seedream 4.0, 4.5 and 5.0 Flash;
- the 5.5, 5.0 Pro and 5.0 Flash layer decompositions;
- Gemini 3.1 Flash Image and Flash Lite;
- Ideogram 4.0 (and Transparent) and P-Image;
- GPT Image 2.5;
- Grok Imagine Image and Image Quality;
- Topaz Image Upscale.

Pika also validates the body of an image request before its key. We sent
these bodies, as `image.rs` writes them, with a made-up key:

- the text-to-image body of each model: no shape, each shape, each
  resolution, and transparent where available;
- the image-to-image body of each model: one picture, its maximum number
  of pictures, and pictures with a shape.

Each body got the answer 401. This test found that Recraft 4.1 refuses
`size`.

## Audio operations

The editor makes music and sound effects with these operations
(`pika/audio.rs`). `audio.rs` also has the fields of each request. All
are media jobs. A completed job has `output.audio.url` and
`content_type`.

| Operation | Prompt | Length | Other fields the editor sends |
| --- | --- | --- | --- |
| Pika Music (`pika/pika-audio/pika-music`, default) | `prompt`, 4,000 | `duration` 10–360 (default 30) | `lyrics` (`[Name]` blocks) when the user gives sung lines |
| Sonilo 1.1 (`sonilo/sonilo-v1.1-music/text-to-music`) | `prompt`, 2,000 | `duration` 5–360, whole seconds, required | `segments: [{start, prompt (200), label?}]`, up to 30; `label` one of intro, verse, pre-chorus, chorus, bridge, break, silence, outro |
| MiniMax Music 3.0 (`minimax/minimax-music-3.0/text-to-audio`) | `prompt`, 2,000 | none (up to 5 min) | `is_instrumental`; `lyrics` (3,500), or `lyrics_optimizer` to make the model write them |
| Lyria 3.5, Lyria 3 Pro (`google/lyria-3.5/text-to-audio`, `google/lyria-3-pro/text-to-audio`) | `prompt`, 4,096 | none (3 Pro: up to 184 s) | none: structure, tempo and lyrics go in the prompt |
| Pika SFX (`pika/pika-audio/pika-sfx`, default) | `prompt`, 4,000 | `duration` 1–20 (default 10) | |
| Sonilo 1.1 SFX (`sonilo/sonilo-v1.1-sfx/sound-effects`) | `prompt`, 2,000 | `duration` 1–180, whole seconds, required | `audio_format: wav` |
| ElevenLabs Sound Effects v2 (`elevenlabs/eleven-text-to-sound-v2/sound-effects`) | `text`, 450 | `duration_seconds` 0.5–30, required | `loop` |

The editor does not use these operations:

- Lyria 2, Lyria 3 (30 s), Seed Audio, Kling Audio;
- the video-to-audio operations (Sonilo, Kling, Pika Soundtrack);
- stems.

## Voiceovers

The editor reads voiceovers with these operations (`pika/speech.rs`,
with the fields each takes described as a `HostedSpeech` in
`crates/flikk-cloud/src/speech.rs`). All are media jobs. A completed job
has `output.audio.url`. We read the specifications on 2026-11-15.

| Operation | Script | Voice | Tuning | Other fields the editor sends | Delivery |
| --- | --- | --- | --- | --- | --- |
| ElevenLabs v4 (`elevenlabs/eleven-v4/text-to-speech`, default) | `text`, 10,000 | `voice`: a stock voice or a clone | `voice_settings.stability`, `voice_settings.similarity_boost` | — (MP3) | audio tags |
| MiniMax Speech 2.8 HD (`minimax/minimax-speech-2.8-hd/text-to-speech`) | `text`, 10,000 | `voice_id` | — | `format: flac`, `sample_rate: 44100` | interjections and `<#x#>` pauses |
| Pika Speech (`pika/pika-audio/pika-speech`) | `script`, 10,000 | `voice_preset` (76 presets) | — | — (MP3) | none: tags are read aloud |

- Each model reads with the voices `speech.rs` lists: the 17 stock
  voices of ElevenLabs v4 (`darian` first), the 17 MiniMax ids that fal
  also names, and Pika Speech's 76 presets. A voice outside the list,
  and not a clone that the model reads, is refused before the job.
- The editor does not send Pika Speech a `reference_audio`, MiniMax's
  speed, pitch or emotion, or the speech around a word fix.
- Pika's MiniMax page documents neither interjections nor `<#x#>`
  pauses. The editor writes them as for MiniMax on fal, which reads
  them; this is not verified through Pika.
- The editor does not use ElevenLabs v4 Turbo, Multilingual v2, Turbo
  2.5, Text to Dialogue, MiniMax 2.8 Turbo or Seed Audio.

## Voices

`pika/voices.rs`. The voice endpoints take the key as
`Authorization: Bearer <key>`; the editor also sends `X-API-Key`.

| Call | Endpoint | Notes |
| --- | --- | --- |
| List | `GET /v1/voices` | `{results: [{voice_id, name, kind, provider, created_at}]}`: stock voices first, then the account's clones, newest first. Only ElevenLabs v4 lists clones. |
| Clone | `POST /v1/voices` | `{name (1–100), sample_urls (1–25, uploaded first), consent_attested: true}`, `201` with the voice. Synchronous: it can take over a minute. Never sent again on a server error. |
| Delete | `DELETE /v1/voices/{voice_id}` | `204`; `404` counts as deleted. |

- A clone's id is `voice_` and 32 hex digits. The editor checks it
  before it goes in a path.
- The user confirms the speaker's consent in the Voices dialog. Without
  it, the editor sends nothing.
- An organization keeps 100 clones at most (`409`). The first 5 are
  free, deleted ones included; each one after costs $1.
- A clone reads only on its provider's models: ElevenLabs v4 here.
- If the list call fails, the editor shows the stock voices without the
  clones. Only a key that Pika refuses stops the list.
- The editor does not delete a stock voice. It tells the user to remove
  the voice from the library.
- A voiceover keeps the delivery marks of its model (`marks` in
  `GenerationInfo`). The editor uses them to find the words the take
  says.
- Pika can't rename a voice (the name is kept in the library only),
  take noise out of recordings, search a shared library or design a
  voice.

## Upscaling

Two operations make a video larger. Both are jobs, as above. Both take
the video as an uploaded URL (50 MB at most) and answer
`output.video.url`. We read this data on 2026-11-10 from the
`input_schema` of the catalog.

| Operation | Fields | Limits | Price |
| --- | --- | --- | --- |
| Topaz Video Upscale (`topaz/topaz-video-upscale/video-upscale`) | `video_url`; `upscale_factor` `x2` (default), `x3`, `x4` (`x1` only interpolates frames); `model` `proteus` (default), `starlight-mini`, `starlight-hq`, `starlight-sharp`, `starlight-fast-2`, `starlight-precise-2.6`; `h264_output` (default H.265); Proteus tuning (`compression`, `noise`, `halo`, `grain`, `recover_detail`), `target_fps`, `slowdown_factor` | output at most 4096 × 2160 pixels in all; Starlight takes sources up to 30 fps and, except Precise 2.6, never makes less than 720p | Proteus $0.19 per 1,000 output megapixels; Starlight $1.92 (Fast 2) or $3.84 per 1,000 1080p frames, $4.20 or $8.39 per 1,000 4K frames |
| Pika H3 Flex Draft to Video (`pika/pika-h3-flex/draft-to-video`) | `video` (or `draft_job_id`, a 480p/768p H3 Flex job of the account); `resolution` `1080p`, `2k` (default), `4k` | short side at most 768 px; 512 MiB, 1,441 frames; keeps the frames, frame rate and sound | the price per second of the final resolution, less the native 480p/768p price |

`crates/flikk-cloud/src/pika/upscale.rs` offers these operations:

- Topaz Proteus (the default), Starlight Precise 2.6 and Starlight
  Fast 2. Each one has `h264_output` on. Each one uses the smallest factor
  that reaches the requested resolution.
- The `draft-to-video` operation of H3 Flex, with the requested resolution
  itself.

The editor does not use Proteus tuning, frame interpolation, or
Starlight Mini, HQ and Sharp. It does not use Topaz Image Upscale
(`topaz/topaz-image-upscale/image-upscale`), because it does not upscale
pictures yet.

## Verified

- A script verified every field, enum and limit above against the live
  catalog (`/catalog/apis/…?expand=inputs`) and `openapi.json`. For each
  model, it checked these items:
  - the lengths;
  - the shapes;
  - the resolutions, with their letter case and default;
  - the reference limits;
  - the fields for its frames, references and sound.
- Pika validates the body of a submit before its key. Thus a made-up key
  checks the fields of a request at no cost.
  - We sent the text-to-video request of each model. We used each
    resolution with its shortest and longest length, each shape, and no
    shape. Pika accepted each request (401).
  - This test found the 6 s limit of Hailuo 2.3 at 1080p. No schema
    shows this limit.
- With a made-up key, `/billing/balance`, submit, uploads and job reads
  answer `401 {"message":"Invalid API key"}`. Without a key, they answer
  `"missing API key"`. A submit with `duration: 30` answers
  `422 {"message":"duration: Input should be less than or equal to 15"}`
  before Pika checks the key.

- On 2026-11-11, we sent with a made-up key each audio request that the
  editor writes. These requests used each model above, with and without
  a length, sections, lyrics and loop. Pika accepted them (401). Pika
  refused these requests (422):
  - a length out of range;
  - a fractional length to Sonilo SFX;
  - a length to Lyria.

- On 2026-11-15, we sent with a made-up key each voiceover request that
  the editor writes, a clone's id to ElevenLabs v4, and a clone to
  `/v1/voices`. Pika accepted them (401). It refused an unknown stock
  voice, an unknown preset and an unknown MiniMax `format` (422), with
  enums that match the lists in `speech.rs`. `/v1/voices` reads
  `X-API-Key` as the media endpoints do.

We did not verify these items, because we have no paid key:

- a real job end to end;
- the exact `headers` that an upload gives;
- if `output.video.url` downloads without the key;
- the failed-job bodies for balance, rate limits and moderation (they
  come from the specification).

## Sources

- <https://dev.pika.art/llms.txt>
- <https://dev.pika.art/openapi.json>
- <https://api.dev.pika.art/catalog/apis>
- <https://dev.pika.art/llms/pika/pika-h3-flex/text-to-video>,
  [image-to-video](https://dev.pika.art/llms/pika/pika-h3-flex/image-to-video),
  [reference-to-video](https://dev.pika.art/llms/pika/pika-h3-flex/reference-to-video)
- <https://dev.pika.art/llms/pika/pika-2.5/text-to-video>,
  [image-to-video](https://dev.pika.art/llms/pika/pika-2.5/image-to-video)
- <https://dev.pika.art/llms/pika/pikaframes/image-to-video>
- <https://dev.pika.art/llms/openai/gpt-image-2/text-to-image> (the image
  job's output)
- <https://dev.pika.art/llms/pika/pika-audio/pika-music>,
  [pika-sfx](https://dev.pika.art/llms/pika/pika-audio/pika-sfx),
  [Sonilo text-to-music](https://dev.pika.art/llms/sonilo/sonilo-v1.1-music/text-to-music)
- <https://dev.pika.art/llms/elevenlabs/eleven-v4/text-to-speech>,
  [MiniMax Speech 2.8 HD](https://dev.pika.art/llms/minimax/minimax-speech-2.8-hd/text-to-speech),
  [Pika Speech](https://dev.pika.art/llms/pika/pika-audio/pika-speech)
- <https://dev.pika.art/llms/voice-cloning>
- <https://dev.pika.art/models>, <https://dev.pika.art/pricing>
