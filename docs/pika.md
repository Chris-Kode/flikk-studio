# Pika

Pika's developer API (the "Pika API Club", <https://dev.pika.art/>) is now a
paid, prepaid aggregator: one key reaches Pika's own models and about 170
operations from other vendors (Seedance, Kling, Veo, Wan, MiniMax, ElevenLabs
and more). The editor uses it as a video provider for **Pika's own models**
only; the other vendors' models are on fal already.

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

- `pika-h3-flex/draft-to-video` (finish a 480p/768p draft at 1080p–4K),
  `pika-h3-flex/prompt-rewriter`, `pika-h3-flex/audio-stems`;
- Pikadditions and Pikaswaps (`video-to-video`: add or swap something in a
  clip), Pikaffects (`image-to-video`, `video-to-video`: named effects);
- Pika Dynamic Captions, Video Compose, Video Merge, and the Pika audio
  models (music, SFX, speech, soundtrack);
- every other vendor's model on the same API;
- webhooks, idempotency keys, quotes, spend reports.

These either edit media the editor's video request has no field for, or
duplicate what fal and ElevenLabs already do here.

## What the editor uses

`crates/koo-cloud/src/pika/`:

| Model (catalog id) | Text | First frame | First + last | References | Lengths | Shapes | Resolutions | Sound |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Pika H3 Flex (`pika/pika-h3-flex`, default) | text-to-video | image-to-video | image-to-video | reference-to-video (9 pictures, 3 clips, 3 sounds, 12 in all, 15 s per kind) | 5–15 s | the 11 ratios above | 768p, 480p, 1080p, 2k, 4k | always |
| Pika 2.5 (`pika/pika-2.5`) | text-to-video | image-to-video (the picture sets the shape) | Pikaframes, one transition | — | 5 or 10 s | `5:2`, `16:9`, `4:3`, `1:1`, `4:5`, `9:16` (text) | 1080p, 720p, 480p (720p or 1080p between two frames) | none |

- The prompt is sent as written; `rewrite`, `seed` and `output_mode` keep
  Pika's defaults. No length or shape asked for leaves the field out.
- Before anything is uploaded or spent, `VideoModel::check` and
  `pika/video.rs` refuse what Pika would: references that are only sounds,
  clips or sounds under 2 s, 480p between two frames on Pika 2.5.
- Files are uploaded with `POST /v1/media/uploads` and a signed `PUT`.
- The job is polled every 3 s, as queued or running; it can't be stopped, so
  the media pool shows no stop button for it. It is followed for at most
  two hours. The video is downloaded from `output.video.url` without the
  key; the job's charge is not recorded.

## Verified

- Every field, enum and limit above against the live catalog
  (`/catalog/apis/pika/…?expand=inputs`) and `openapi.json`.
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
- <https://dev.pika.art/models>, <https://dev.pika.art/pricing>
