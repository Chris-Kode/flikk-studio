# Whisper small: no more words over silence (a weight edit)

A weight-space edit of Whisper small, following the weight-space programming
idea: the model is changed by writing weights directly, without fine-tuning,
gradient descent or new data. Decoding and the runtime stay as they are.

## What was wrong

Over silence, hum, fan noise, room tone or clicks, the original Whisper small
writes text, and in auto-language mode it loops:

- Spanish: "Subtítulos por la comunidad de Amara.org", "¡Suscríbete!", "y"
- English: "Thank you.", "Thank you for watching."
- German: "Danke fürs Zusehen."
- Auto mode: language detected as Sinhala, Georgian or Welsh, then a loop
  such as "වවවවවවවව…", "ლლლლლლ…" or "Tek Tok Tok Tok…".

## What the model already knew

A linear readout of the decoder's MLP input, at the step that picks the first
word, tells silence from speech perfectly. It does so in every prompt language
and at every layer (AUC 1.0). Whisper "knows" there is no speech; it just doesn't
act on it. At that step, end-of-text sits at about −8.6 nats.

## The edit

16 of the least-used MLP neurons in **decoder layer 5** are rewritten. That's
24,592 of 241,734,912 values (0.010%).

- `fc1` row: reads the no-speech direction. This is a shrunk-LDA direction
  fitted on the original model's own activations.
- `fc1` bias: a threshold placed midway between the most silence-like speech
  and the least speech-like silence in the fitting data. On speech the neuron
  sits at z ≤ −135, fully off.
- `fc2` column: writes toward end-of-text (the end-of-text unembedding, taken
  through the final layer norm).

Each neuron is in its own 32-value q8_0 block, and its output column stays
inside that block's existing range. So in `ggml-small-q8_0.bin` no other value
or scale changes: 25,272 bytes differ, out of 264 MB.

## Results (held-out test, never used for fitting or choosing)

Original vs edited. Greedy decoding with OpenAI's rules, no fallback, no
no-speech gate:

| Set | Original | Edited |
|---|---|---|
| Noise/silence, `es` prompt (63 clips, 9 kinds, 2 never seen) | 100% write text | 1.6% |
| Noise/silence, `en` prompt | 95.2% | 0% |
| Noise/silence, auto language | 96.8%, **36.5% loop** | 0%, 0% loop |
| Noise/silence, `de` / `ja` prompt | 98.4% / 100% | 0% / 0% |
| Speech WER: EN · ES · FR · DE · IT · PT | 7.65 · 7.95 · 16.4 · 14.0 · 26.4 · 15.8 | identical |
| Speech with noise after / before / late in the window (EN, ES) | — | identical WER |
| Short utterances inside long silence (6 languages) | — | no real speech lost |

600 of 613 speech transcripts are byte-identical. The 13 that change are
punctuation, capitalisation (German "sie" → "Sie") or one letter, plus one
"Danke fürs Zusehen." over a short German clip that disappears.

The same file in **whisper.cpp** with its stock decoding (beam 5, temperature
fallback, its own thresholds, conditioned on previous text), q8_0:

| Set | Original | Edited |
|---|---|---|
| Noise/silence, `es` | 46% write text | 0% |
| Noise/silence, auto | 38%, **31% loop** | 0%, 0% loop |
| 4.7 min Spanish with 2–12 s noisy pauses (WER) | 8.15% | 7.74% |
| 2.7 min English with pauses (WER) | 4.66% | 4.66% |
| ES / EN clips (WER) | 6.65% / 9.0% | 5.78% / 9.5% |

## Use it

`edit.npz` holds the edit itself (the 16 neurons' new values); `apply.py` reads
it from its own folder.

```sh
python scripts/whisper-small-optimized/apply.py ggml \
  ~/.flikk/models/ggml-small-q8_0.bin ~/.flikk/models/ggml-small-q8_0-silence-fix.bin
python scripts/whisper-small-optimized/apply.py hf openai/whisper-small ./whisper-small-silence-fix
```

`ggml-small-q8_0-silence-fix.bin` made from whisper.cpp's
`ggml-small-q8_0.bin` (sha256 `49c8fb02…779f`, the file Flikk downloads) has
sha256 `c439c7fc…a248`. Any whisper.cpp program can load it in place of the
original. Flikk picks it up without code changes:

```sh
FLIKK_WHISPER_MODEL=ggml-small-q8_0-silence-fix.bin cargo run -p flikk
```

## How it was found (`method/`)

Everything the scripts download or write goes to `$WHISPER_EDIT_WORK`, by
default `$FLIKK_HOME/whisper-edit` (or `~/.flikk/whisper-edit`).

1. Data. Generated non-speech in 9 kinds: white, pink, brown, low-pass room
   tone, hum, dithered silence, clicks, plus fan and breath, which only appear
   in test. Speech from LibriSpeech (EN) and MLS (ES, FR, DE, IT, PT). Train,
   dev and test are disjoint by seed and by clip. `fetch_data.py`, then
   `export_sets.py`, `export_sets.py head`, `hard.py sets`, `multi.py sets` and
   `longform.py` write the dev and test sets.
2. Encoder outputs are cached once (`fasteval.py <set>...`), since the edit only
   touches the decoder. `greedy.py` decodes with OpenAI's rules on them.
3. Activations of the original model: `pool.py` then `feats.py pool`,
   `hard.py pool` then `pool_hard.py`, `pool_auto.py`, `multi.py pool`.
4. Reader (`tune2.py`, `tune3.py`): no-speech examples at the first-word step,
   prompted in many languages. Speech examples at every step while words are
   still to come, including hard cases (short utterances in long silence,
   speech starting late), in six languages.
5. Chosen on dev (`tune3.py 16 1280 save`), evaluated once on test
   (`eval_final.py`; the original with `evalmodel.py openai/whisper-small test
   out.json`), carried into ggml (`ggml_apply.py`), and checked in whisper.cpp
   (`wcli/`, `wcpp2.sh`, `score_wcpp.py`).

## Limits

- Non-speech here is synthetic plus real pauses in read speech. Music, crowd
  noise and real room recordings weren't tested.
- The test sets are small: 63 noise clips per prompt, 20–73 speech clips per
  language, 2 long recordings in whisper.cpp.
- The loops seen were all over non-speech, which the edit removes. No loop over
  real speech came up in these sets, so no separate loop edit was made or measured.
- Two earlier versions were dropped. A first one, at layer 10, deleted short
  or late speech. A single-neuron one had a big output column that coarsened
  its 31 neighbouring neurons in q8_0.
