# Pika

Pika's developer API (the "Pika API Club", <https://dev.pika.art/>) is now a
paid, prepaid aggregator: one key reaches Pika's own models and about 170
operations from other vendors (Seedance, Kling, Veo, Wan, MiniMax, ElevenLabs
and more). The editor uses it as a video provider for Pika's own models and
the other vendors' video models that fit its video request (see "What the
editor uses"), and as an image provider for the other vendors' image models
(see "Image operations").

Read on 2026-11-02 from `llms.txt`, the per-operation specifications, the
public catalog and `openapi.json` (links at the end). The catalog's
`input_schema` is Pika's stated source of truth when prose and schema differ.

## Access

| | |
| --- | --- |
| Base URL | `https://api.dev.pika.art` |
| Auth | `X-API-Key: <key>` (or `Authorization: Bearer <key>` on `/v1/media`) |
| Keys | made, rotated and revoked at <https://dev.pika.art/keys> |
| Env var | `PIKA_API_KEY` (the name Pika's docs suggest) |
| Billing | prepaid in micro-USD (1,000,000 = $1); a $10/month membership comes back as $10 of credits; no free tier, no sandbox; only successful generations are charged |
| Free calls | `GET /catalog/apis`, `GET /catalog/apis/{api_id}?expand=inputs` (no key), `POST /catalog/apis/{api_id}/quote`, `GET /billing/balance` |

## Job flow

1. **Upload** (local files): `POST /v1/media/uploads` with
   `{"content_type": "image/png", "size_bytes": 12345}` → `upload_url`
   (valid 5 minutes), `headers`, `url`, `expires_in`, `max_bytes`. `PUT` the
   raw bytes to `upload_url` with `headers` sent verbatim (`Content-Type`
   and `Content-Length` are signed; storage answers a bare 403 otherwise).
   The request then names `url`. Caps: 20 MB image or audio, 50 MB video;
   `413` too big, `415` unsupported type. A public URL can be passed as is.
   `DELETE /v1/media/uploads?url=…` erases one.
2. **Submit**: `POST /v1/media/{vendor}/{model}/{function}` with the JSON
   body → the job, `{"id": "media_…", "status": "queued"}`. An optional
   `Idempotency-Key` header, and an optional `webhook_url`.
3. **Poll**: `GET /v1/media/jobs/{id}`. `status` goes `queued` → `running` →
   `completed` or `failed` (it may skip `running`). A completed job has
   `output.video.url` (and `content_type`); a failed one `error.code` and
   `error.message`. A terminal job carries `usage` and `billing`
   (`state`: `settled` with `charge_micro_usd`, `pending`, `unavailable`).
   `GET /v1/media/jobs/{id}/content` gives the same URL as `{"url"}`, `409`
   before it is done.
4. **Cancel**: there is none. `DELETE /v1/media/jobs/{id}` erases a finished
   job's media and request; on a running job it answers `409`.

## Errors

Two body shapes: request errors before a job exists are
`{"message": "..."}`; a submit refused after the job row exists comes back as
the failed job, `{"id", "status": "failed", "error": {"code", "message"}}`.

| HTTP | Meaning |
| --- | --- |
| 401 | Key missing or invalid (`{"message":"Invalid API key"}`; `"missing API key"` without one) |
| 403 | Inactive key (`{"message"}`), or the balance / postpaid cycle can't cover it (failed job, `insufficient_balance` / `cycle_limit_exceeded`) |
| 404 | Unknown job, or unknown vendor/model/function |
| 409 | Result asked for too early, or an idempotency key reused with another body |
| 413 / 415 | Upload too big, or of a type it won't take |
| 422 | Validation: `{"message": "duration: Input should be less than or equal to 15"}`; an unpriceable combination is a failed job with `invalid_input` |
| 429 | Per-minute or per-day limit, or the org's queue is full (failed job, `rate_limited`; honour `Retry-After`) |
| 503 | A Pika dependency is down (failed job, `provider_unavailable`) |

`error.code` is one of `invalid_input`, `content_moderation`,
`provider_error`, `provider_timeout`, `provider_unavailable`, `rate_limited`,
`insufficient_balance`, `membership_required`, `cycle_limit_exceeded`,
`admission_suspended`, `usage_limit_exceeded`, `timed_out`, `internal`.

The editor maps them (`pika/mod.rs`, `failure`): `insufficient_balance` and
`cycle_limit_exceeded` → out of credits; `membership_required` and
`usage_limit_exceeded` → not allowed; `invalid_input` and
`content_moderation` → refused prompt; `rate_limited` and
`provider_unavailable` → busy; other codes → failed with Pika's message.
Without a code: 401 → bad key, 403 → not allowed, 422 → refused prompt,
429 / 503 → busy.

## Balance

`GET /billing/balance` (same key, free): `unit` (`micro_usd`), `currency`,
`org_id`, `balance_micro_usd` (prepaid plus comp credits),
`prepaid_micro_usd`, `credit_micro_usd`, and `postpaid` (`null` for a
prepaid org; when `active`, `cycle.remaining_micro_usd` is what can still be
spent). Settings → Integrations shows it as dollars left.

## Pika's video operations

All async, all `POST /v1/media/pika/…`.

### Pika H3 Flex — `pika-h3-flex/{text,image,reference}-to-video`

Video with native, synchronized sound (no switch to turn it off).

| Field | Type | Values / default |
| --- | --- | --- |
| `prompt` | string, required | 1–8000 chars |
| `duration` | integer | 5–15, default 5 |
| `aspect_ratio` | enum or null | `auto`, `21:9`, `16:9`, `3:2`, `4:3`, `5:4`, `1:1`, `4:5`, `3:4`, `2:3`, `9:16`, `9:21`; left out, `auto` while `rewrite` is on, else the first keyframe's, else 16:9. A fixed ratio wins over the frames (they are cropped) |
| `resolution` | enum | `480p`, `768p` (default) native; `1080p`, `2k`, `4k` are 768p upscaled |
| `seed` | integer | default 0 |
| `rewrite` | boolean | default true: Pika rewrites the prompt |
| `output_mode` | enum | `video` (default), `audio-stems` (adds dialogue, music and effects WAVs) |
| `channel_mode` | enum | `mono` (default), `stereo`; stems only |
| `image` | URL | image-to-video: first frame (JPEG, PNG, WebP; 24 MiB, 40 MP) |
| `last_frame_image` | URL | image-to-video: last frame; `image`, this, or both |
| `references` | `[{type: image\|video\|audio, url}]`, required | reference-to-video: 1–12, at most 9 images, 3 videos, 3 audio; at least one image or video; each 24 MiB, 64 MiB base64 together; clips and sounds 2–15 s, 15 s together per kind |

References are cited in the prompt as `@Image1`, `@Video1`, `@Audio1`,
counted per kind in order. (The prose specification also mentions flat
`image_urls` / `video_urls` / `audio_urls` lists; the schema, which wins,
takes `references`.)

Price per output second: $0.01 at 480p to $0.08–0.10 at 4K
(reference-to-video costs more and adds reference video seconds).

### Pika 2.5 — `pika-2.5/{text,image}-to-video`

No sound.

| Field | Type | Values / default |
| --- | --- | --- |
| `prompt` | string | text: required, 1–4000 chars; image: optional |
| `negative_prompt` | string | optional |
| `duration_s` | integer | `5` (default) or `10` |
| `resolution` | enum | `480p`, `720p`, `1080p` (default) |
| `aspect_ratio` | enum or null | text only: `5:2`, `16:9`, `4:3`, `1:1`, `4:5`, `9:16`; left out, the model picks |
| `seed` | integer or null | random by default |
| `image` | URL, required | image-to-video: the picture to animate (its shape is the video's) |

$0.02–0.09 per second by resolution.

### Pikaframes (Pika 2.5 Keyframe) — `pikaframes/image-to-video`

| Field | Type | Values / default |
| --- | --- | --- |
| `images` | URL[], required | 1–5 ordered keyframes; one needs `loop` |
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
- Pikadditions and Pikaswaps (`video-to-video`: add or swap something in a
  clip), Pikaffects (`image-to-video`, `video-to-video`: named effects);
- Pika Dynamic Captions, Video Compose, Video Merge, and the Pika audio
  models (music, SFX, speech, soundtrack);
- webhooks, idempotency keys, quotes, spend reports.

These edit media the editor's video request has no field for, or duplicate
what ElevenLabs already does here.

## Other vendors' video models

Each is at `POST /v1/media/{vendor}/{model}/{function}`, with its own fields
(the catalog's `input_schema` is the reference; `pika/models.rs` records
how each takes a request).

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

Not offered, because the editor's video request needs both text and image to
video and always sends a length:

- Veo 3.1 and Veo 3.1 Fast: text-to-video only (with up to 3 reference
  images at 8 s), plus video extension;
- Veo 3.1 Lite, Hailuo 2.3 Fast, Grok Imagine Video 1.5: image-to-video only;
- Gemini Omni 1.1 Flash: no length field;
- the other video operations: video-to-video, video extension, motion
  control, avatars and lip sync. Upscaling is offered on its own (see
  "Upscaling").

Kling's omni-video references are left out: Pika states no limits for them.
Most models' references are cited `@Image1`, `@Video1`, `@Audio1`, as Pika's
specifications say for every reference model; MiniMax H3's fields say
`Image 1`, so the editor's tags are rewritten for it.

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

- The prompt is sent as written (for MiniMax H3, with its reference tags
  rewritten); `rewrite`, `seed`, `draft`, `watermark` and the like keep
  their defaults. No shape asked for leaves the field out; no length leaves
  it out too, or sends `"auto"` where the model takes it (Seedance requires
  one). The resolution is always sent, in the case the model spells it.
- Before anything is uploaded or spent, `VideoModel::check` and
  `pika/video.rs` refuse what Pika would: references that are only sounds,
  clips or sounds under 2 s (H3 Flex, Seedance), 480p between two frames on
  Pika 2.5, over 6 s at 1080p on Hailuo 2.3.
- Files are uploaded with `POST /v1/media/uploads` and a signed `PUT`.
- The job is polled every 3 s, as queued or running; it can't be stopped, so
  the media pool shows no stop button for it. It is followed for at most
  two hours. The video is downloaded from `output.video.url` without the
  key; the job's charge is not recorded.

## Image operations

Each image model is two operations at `POST /v1/media/{vendor}/{model}/…`:
`text-to-image`, and `image-to-image` (editing, or following, 1 to its
limit of pictures as `image_urls`). Jobs, uploads and errors are as for
video; a completed job has `output.images[].url` (and `content_type`). Read
on 2026-11-07 from the catalog's `input_schema`.

`crates/flikk-cloud/src/pika/` (`models::IMAGE`, `image.rs`):

| Model (catalog id) | Shape | Resolution | Pictures | Transparent |
| --- | --- | --- | --- | --- |
| Nano Banana 2.1 (`google/gemini-nano-banana-2.1`, default) | `aspect_ratio`, `auto` | `resolution` 1K, 2K, 4K | 16 | — |
| Nano Banana Pro (`google/gemini-3-pro-image`) | as above | as above | 16 | — |
| GPT Image 2, 2.6 Flare, 2.6 Sunburst (`openai/gpt-image-2`, `-2.6-flare`, `-2.6-sunburst`) | `aspect_ratio`, no `auto` (1:1 left out) | `resolution` 1K, 2K, 4K | 16 | `background` |
| Seedream 5.0 Pro (`bytedance/seedream-5.0-pro`) | `aspect_ratio` (left out: picked from the prompt) | `size` 2K, 1.5K, 1K | 10 | — |
| Seedream 5.0 Lite (`bytedance/seedream-5.0-lite`) | as above | `size` 2K, 3K, 4K | 14 | — |
| Ideogram 4.5 (`ideogram/ideogram-4.5`) | `size` in pixels: the larger size its text-to-image enum has for each ratio (editing takes any `WxH` in multiples of 32 up to 4,194,304 pixels, which these are) | — | 5 | — |
| Grok Imagine Image 2.0 (`x-ai/grok-imagine-image-2.0`) | `aspect_ratio`, `auto` | `resolution` 1K, 2K | 5 | — |
| Muse Image (`meta/muse-image-1.0`) | none (`size` left at `auto`) | — | 10 | — |
| Recraft 4.1 (`recraft/recraft-4.1`) | none: its `size` answers 422 "not enabled on this environment yet" | — | text only | — |

`num_images`, `seed`, `quality`, `output_format` and the like keep their
defaults; one picture is made per request. Not used: Seedream 4.0, 4.5 and
the 5.0 Pro layer decomposition, Gemini 3.1 Flash Image and Flash Lite,
Ideogram 4.0 (and Transparent) and P-Image, GPT Image 2.5, Grok Imagine
Image and Image Quality, and Topaz Image Upscale.

Pika validates an image request's body before its key too: with a made-up
key, every model's text-to-image body (no shape, each shape, each
resolution, transparent where offered) and image-to-image body (one
picture, its most pictures, pictures with a shape) as `image.rs` writes it
answers 401, which is how Recraft 4.1's refused `size` was found.

## Upscaling

Two operations make a video larger; both are jobs as above, take the video
as an uploaded URL (50 MB at most) and answer `output.video.url`. Read on
2026-11-10 from the catalog's `input_schema`.

| Operation | Fields | Limits | Price |
| --- | --- | --- | --- |
| Topaz Video Upscale (`topaz/topaz-video-upscale/video-upscale`) | `video_url`; `upscale_factor` `x2` (default), `x3`, `x4` (`x1` only interpolates frames); `model` `proteus` (default), `starlight-mini`, `starlight-hq`, `starlight-sharp`, `starlight-fast-2`, `starlight-precise-2.6`; `h264_output` (default H.265); Proteus tuning (`compression`, `noise`, `halo`, `grain`, `recover_detail`), `target_fps`, `slowdown_factor` | output at most 4096 × 2160 pixels in all; Starlight takes sources up to 30 fps and, except Precise 2.6, never makes less than 720p | Proteus $0.19 per 1,000 output megapixels; Starlight $1.92 (Fast 2) or $3.84 per 1,000 1080p frames, $4.20 or $8.39 per 1,000 4K frames |
| Pika H3 Flex Draft to Video (`pika/pika-h3-flex/draft-to-video`) | `video` (or `draft_job_id`, a 480p/768p H3 Flex job of the account's); `resolution` `1080p`, `2k` (default), `4k` | short side at most 768 px; 512 MiB, 1,441 frames; keeps the frames, frame rate and sound | the final resolution's price per second less the native 480p/768p price |

`crates/flikk-cloud/src/pika/upscale.rs` offers Topaz Proteus (the
default), Starlight Precise 2.6 and Starlight Fast 2, each with
`h264_output` on and the smallest factor reaching the resolution asked
for, and H3 Flex's `draft-to-video` with the resolution itself. Proteus
tuning, frame interpolation and Starlight Mini, HQ and Sharp are not used.
Topaz Image Upscale (`topaz/topaz-image-upscale/image-upscale`) is not
used: pictures are not upscaled yet.

## Verified

- Every field, enum and limit above against the live catalog
  (`/catalog/apis/…?expand=inputs`) and `openapi.json`, with a script: each
  model's lengths, shapes, resolutions (and their case and default),
  reference limits and the fields its frames, references and sound go in.
- Pika validates a submit's body before its key, so a made-up key checks a
  request's fields for free: every model's text-to-video request, at each
  resolution with its shortest and longest length, with each shape and with
  none, is accepted (401). That is how Hailuo 2.3's 6 s limit at 1080p was
  found; it is in no schema.
- With a made-up key: `401 {"message":"Invalid API key"}` on
  `/billing/balance`, submit, uploads and job reads (`"missing API key"` with
  none); a submit with `duration: 30` answers `422 {"message":"duration:
  Input should be less than or equal to 15"}` before the key is checked.

Not verified without a paid key: a real job end to end, the exact
`headers` an upload hands out, whether `output.video.url` downloads without
the key, and the failed-job bodies for balance, rate limits and moderation
(taken from the specification).

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
- <https://dev.pika.art/models>, <https://dev.pika.art/pricing>
