# Editing model

* **Project** (`flikk-core/src/model.rs`) contains:
  * media
  * bins
  * one `Timeline`: tracks (of kind Video, Audio, Title, Caption, Overlay),
    markers, ducking, a timeline grade and `SequenceSettings`
  * the FX programs of the project.
* **Clips** have speed, enabled, name, color, a transform, a grade, an
  effect stack, audio effects, volume automation, `transition_in` /
  `transition_out`, masks and a `retime`. `fx::transitions` resolves
  `transition_in` and `transition_out`.
* **Speed ramps** (`flikk-core/src/speed.rs`, `Clip::retime`):
  * Speed keys are on clip time. Between two keys, the speed changes in a
    straight line. Before the first key and after the last key, the speed
    holds.
  * Source time is the integral (`Clip::source_offset`, inverse
    `Clip::timeline_offset`). It is behind `Clip::to_source` /
    `to_timeline`.
  * Thus source time is also behind the viewer, compositor, render,
    transcript and caption mapping, handles, waveforms and silence bands.
  * `SetSpeedRamp` takes points over the clip (0..1, speed). It keeps the
    source that the clip plays. The length of the clip follows the ramp.
    With `keep_length`, the speeds are scaled to fit.
  * Presets (`SpeedPreset`: Ramp in, Ramp out, Hero, Bullet, Montage) are
    shapes in multiples of the speed of the clip.
  * Trims and splits slice the keys, and add one key at the cut. Thus each
    frame keeps its speed.
  * `SetSpeed` on a ramped clip scales the ramp.
  * `SetRetime` selects how slowed frames are drawn (`Interpolation`:
    repeat, blend, optical flow). It also selects whether ramped audio is
    stretched with the pitch kept, or muted (`RampAudio`).
  * Without keys, a clip plays at `speed` as before. Old projects load
    with no change.
* **Transform:** position, scale, rotation, opacity, per-axis stretch,
  skew, anchor, crop, flip, blend mode, `fit` and a corner pin. All are
  keyframable.
  * `fit` is `Contain` with bars, or `Cover`. `Cover` fills the frame, and
    the position pans it.
  * A pinned picture is placed through a homography
    (`Placement::homography`). Other pictures are placed through one
    affine matrix. The GPU and CPU compositors share this matrix.
  * `compose::GroupMove` moves, scales and turns more than one clip about
    a common pivot. Then it changes that back into the transform of each
    clip.
* **Masks** (`flikk-core/src/mask.rs`):
  * The shapes are rectangles, ellipses and closed paths (straight or
    smooth), in the own uv of the picture.
  * Each mask has feather, expansion, opacity and invert.
  * Masks combine in sequence by add, subtract or intersect. Each mask is
    keyframable as a whole.
  * Captions and titles take no masks or corner pins. Renders burn them in
    with ASS, and ASS cannot draw masks or corner pins.
* **Stabilization** (`Clip::stabilization`, `flikk-core/src/stabilize.rs`):
  * The settings are on, a smoothness (0..1), Smooth or Locked, and a crop
    (Auto zoom or None).
  * It is off by default. `.flikk` files do not contain it until it is
    set. Thus older files open with no change.
  * `Command::SetStabilization` sets it on video clips (see
    [Analysis and models](analysis-and-models.md), "Stabilization").
* **Subject masks** (`MaskShape::Subject`, `flikk-core/src/subject.rs`):
  * The mask keeps the object that the prompts of its `Subject` pick.
  * The prompts are on some source frames. They are clicks on the object
    (or off it) and a box, in picture uv. Thus trims, splits and speed
    changes keep the prompts on their frames.
  * With `track`, the object is followed over every frame that the clip
    shows. Without `track`, each frame shows the matte of the nearest
    prompted frame.
  * `refine` eases the edge onto the edge of the picture (hair).
  * Feather, expansion, opacity, invert and mode work as for shapes. The
    prompts are not keyframed.
* **Person masks** (`MaskShape::Person { edge }`,
  `flikk-core/src/person.rs`):
  * The mask keeps every person that a matting model finds in each frame,
    with no prompt.
  * `edge` (0..1) eases the edge onto the edge of the picture. It is
    keyframable with the rest of the state.
  * Invert keeps the background.
  * Feather, expansion, opacity and mode work as for shapes. Thus a person
    mask combines with shape and subject masks.
  * Old projects open with no change, because this is a new shape kind.
* **Cuts in speech** (`flikk-core/src/cuts.rs`):
  * A cut goes in the pause between two words, clear of the two words. It
    is 40 ms after the earlier word and 30 ms before the later word.
  * If the pause holds a frame boundary, the cut goes on it.
  * The join has a 10 ms audio fade.
  * The word delete of the transcript panel and the agent's `cut_words`
    use this module.
  * The agent's `blade` and `ripple_delete` can put a cut on a word. Then
    they move the cut into the nearest pause and say so. `snap: false`
    disables this.
* **Depth masks** (`MaskShape::Depth`):
  * The mask keeps what lies between `near` and `far` from the camera. The
    distance is relative: 0 is nearest and 1 is farthest (see "Depth
    maps").
  * The mask fades over `softness` past each limit.
  * The mask uses the depth map of the picture at the source time. Thus it
    keeps the background, the foreground or a slice between them.
  * Depth masks combine, invert, fade and keyframe like shapes (`near`,
    `far` and `softness` move). Feather and expansion do not apply.
  * A grade or an effect for one depth only uses a copy of the clip on
    the track above. The copy takes the mask and the change.
  * If its map does not exist yet, the mask is left out.
* **Word fixes** (`flikk-core/src/respeak*`). The user types words over
  other words in the transcript. The new words are said in the voice of
  the speaker, and cut in where the old words were.
  * **Context.** `respeak::fix` takes these items:
    * the cuts on each side of the words, from `Speech::around` (as a word
      delete places them)
    * the rest of their sentence, in that voice or in words fixed before.
      It goes back to the end of the sentence before, and on to its own
      end.
    * a maximum of 50 words on each side, as context
    * the quietest pauses between words within 20 s, for room tone.
  * **Provider.** The provider reads the whole sentence with the new words
    in it. It gets the context as `previous_text`/`next_text`, so the
    delivery fits.
  * **Take.** `respeak::make` does these steps:
    1. It times the words of the take (`AnalysisService::time_script`). It
       times them as the words of a voiceover: with the aligner, or with
       the edges measured on the levels.
    2. It cuts the new words out, with a maximum of 80 ms before and
       150 ms after. This margin is at most half the pause to the next
       word.
    3. It brings them to the speech level of the speaker in the original
       sentence. The level uses 20 ms frames within 25 dB of the loudest.
    4. It brings them to the tilt of the original sentence. A one-pole
       split at 1.5 kHz makes two bands. The ratio of high to low energy
       moves the high band, at most ×4.
    5. It puts the longest pause that it found under them. The pause is
       looped with 50 ms equal-power crossfades.
    6. It pads them to the pauses that the old words had before and after.
  * **Result.** The result is a WAV in `paths::generated()`, with its
    `.json`.
    * It is a voiceover item whose script is the new words. Thus analysis
      times them, and the transcript shows them.
    * The transcript shows them in italics, as it shows the words of all
      voiceovers.
    * The take goes to `cache/respeak/`. It is deleted after it is used.
  * **Replacement.** `Command::ReplaceSpeech` cuts the result in.
    * On the track of the speech, the range goes. The new file plays
      there, with a 10 ms fade at the two joins (in the pauses).
    * The new file keeps the gain and audio effects of the replaced clip.
    * If the new file is shorter, the rest ripples as a delete of the end
      of the range does.
    * If the new file is longer, everything after it moves on every
      unlocked track. The picture over the range slows to fill it. Other
      audio leaves a gap. Titles and captions over its end last longer.
    * If another audio track has the same source, the command cuts that
      source out of the range.
    * The replacement is one undo step.
    * If the timeline moved while the provider read, a `Fix` finds its
      words again by clip and source time.
  * **Limits.** Words in a sped-up, slowed or ramped clip are refused.
    Words only on a picture track are also refused.
    * A ramped picture held over longer words keeps the shape of its ramp,
      but slower.
    * If a source has clips on audio tracks, its words are now read only
      there (`query::words_in_range`). Thus the old words do not come back
      from the picture over them.
* **Queries** (`flikk-core/src/query.rs`):
  * the topmost clip at a time
  * sample points
  * the transcript↔timeline mapping of words
  * the resolution of typed timestamps and chat references
  * effective style rules.

## Change sets

`Author::Agent` and `Author::External` commands go into the open
`ChangeSet` of the author (`flikk-core/src/changeset.rs`, `bus.rs`). They
are planned against the *draft* of that set. The draft is the committed
project plus the pending items of the set. Thus the agent sees the effect
of its earlier calls.

* **One set for each chat, and the set grows.**
  * Agent sets are stamped with the current chat of the bus. The timeline
    overlay shows only the sets of the open chat.
  * At the start of each turn, the host calls `reopen_change_set`. The
    pending set of the chat opens again, and the turn adds to it.
  * If the user edited after that, the draft is built again on the current
    project. Each item replays its ops. Items that fail are planned again.
    Items that fail again are dropped.
* **Review waits for the turn.**
  * At the end of a turn, the set becomes `Pending`, or `Resolved` if it
    is empty.
  * The card is posted on the newest message only if the set grew.
  * While the set is `Open`, Accept and Reject on the card are disabled.
    Preview and Review continue to work.
* **Accept** takes all pending items or the ticked subset.
  * An item becomes `Conflicted` if the user changed a clip that its ops
    touch. This applies to changes after the version that the item was
    planned against.
  * If not, the ops of the item replay. If they do not apply any more, its
    command is planned again.
  * The whole accept is one history entry (AG5).
* Change sets are in the bus only. Saved chats keep the change-set id of a
  message, not the set.

## Sync by sound

A second camera or a separate audio recorder is aligned with the main clip
by what they both heard (`flikk-core/src/sync/`). This is for podcasts and
multicam.

* **Measurement** (`sync/measure.rs`, pure Rust with its own radix-2 FFT
  in `sync/fft.rs`):
  * **Decode.** The stream of each recording is decoded through
    `MediaService::audio` to mono at 8 kHz. The decode is whole, up to
    6 hours.
  * **Coarse pass.**
    * The pass changes each stream into an onset envelope. The envelope is
      the spectral flux of 16 log-spaced band energies, 100 each second.
    * The pass removes the local mean of the envelope and caps its peaks.
    * The pass cross-correlates the two envelopes over all lags at the
      same time. It normalizes each lag by the overlap that the lag leaves.
      This overlap is at least 3 s, or 30% of the shorter recording.
    * Thus offsets of hours cost the same. Recordings that start late or
      stop early also cost the same.
  * **Fine pass.**
    * The pass reads 3–12 windows of 8 s from the two recordings. The
      windows spread over the overlap.
    * It finds the lag of each window with band-limited (150–3,800 Hz)
      GCC-PHAT (β 0.8). A parabola gives the lag to a fraction of a sample.
  * **Fit.** A flat fit comes first: the median offset of the windows, and
    the windows within 1.5 ms of it. A robust line (Theil–Sen, then least
    squares on the windows within 1.5 ms) wins on two conditions:
    * more windows agree with it
    * the drift that it shows adds up to 2 ms or more over the overlap.
  * **Result.** The result is the source time of the follower,
    `offset + r · (1 + drift)`, for the source time `r` of the reference.
    It has a confidence:
    * 0.35 comes from how far the coarse peak stands above the best lag
      elsewhere
    * 0.65 comes from the share of fine windows that agree.

    If the confidence is less than 0.5, the result is not used. Then the
    follower stays where it was.
  * A follower can fail to match the reference. Then it is tried through
    another follower that did match (`sync::measure_group`).
  * The measurement runs off the UI thread.
* **Placement** (`sync/plan.rs`):
  * `Command::SyncClips` and `Command::PlaceSynced` carry the
    measurements, not positions. Thus a proposal that is planned again
    after the user moved the reference still lands in sync.
  * A recording on the timeline is the clips that play one media item from
    the same point. An example is a video and the sound that landed with
    it. These clips move together.
  * The reference is the recording on the lowest video track (the earliest
    there). If there is none, it is the first selected recording. The
    agent can name it.
  * If a follower will overlap another clip, it goes to a free track of
    its kind. If there is none, it goes to a new track.
  * If a follower will start before zero, it loses its head.
  * A speed-ramped reference or follower is refused. Its sound does not
    keep one offset. Also, a drift fix is a constant speed, and that speed
    would replace the ramp.
  * `PlaceSynced` lays pool items down whole from the playhead, each on a
    free or new track. If one item would start before it, the group moves
    later together.
  * "Recorder sound" disables the sound clips of the cameras when the
    group holds a sound-only recording.
* **Drift** is corrected by the speed of the follower
  (`speed × (1 + drift)`).
  * Speeds within 0.1% of 1 (`sync::varispeed`, under two cents of pitch)
    play by resampling, as a fast clock does. They do not play by
    time-stretching.
  * Output frame `n` is the input at `n · speed`, with Catmull-Rom
    interpolation.
  * The preview mixer uses this method in place of WSOLA.
  * The render decodes the span of such a clip as the preview does. It
    runs the span through the same code. It reads the result as FLAC from
    the work folder of the job (`flikk-deliver/src/varispeed.rs`), instead
    of `atempo`.
  * Thus preview and export put every sample at the same time.
  * WSOLA and `atempo` were not used for this. Each one wanders up to
    ±10 ms around the nominal time.
  * A follower video at such a speed is composited in renders, as all
    retimed clips are.
* **UI:**
  * The clip menu of the timeline shows "Sync by Audio" when the selection
    holds two or more recordings with sound.
  * It shows "Sync by Audio, Recorder Sound" when the selection holds a
    sound-only recording and a camera.
  * The media pool accepts cmd/ctrl- or shift-click to add items to its
    selection. Then the menu of an item shows "Sync" (and "Sync, Recorder
    Sound").
  * The edit is one undo step. The status line says only what did not
    match, and the drift that was corrected.
  * The agent's `sync_audio` (`tools/sync.rs`) takes `clips` (or pool
    `media` with `at`), `reference` and `replace_camera_audio`.
  * `sync_audio` runs as a job and proposes the edit. It reports the
    offset, drift and confidence of each follower, or why the follower was
    not synced.
