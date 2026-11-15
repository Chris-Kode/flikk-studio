# Editing model

* **Project** (`flikk-core/src/model.rs`): media, bins, one `Timeline` (tracks
  of kind Video, Audio, Title, Caption, Overlay; markers; ducking; a timeline
  grade; `SequenceSettings`) and the project's FX programs.
* **Clips** carry speed, enabled, name, color, a transform, a grade, an
  effect stack, audio effects, volume automation, `transition_in` /
  `transition_out` (resolved by `fx::transitions`), masks and a `retime`.
* **Speed ramps** (`flikk-core/src/speed.rs`, `Clip::retime`): speed keys
  on clip time; between them the speed changes straight, before the first
  and after the last it holds. Source time is the integral
  (`Clip::source_offset`, inverse `Clip::timeline_offset`), behind
  `Clip::to_source` / `to_timeline` and so behind the viewer, compositor,
  render, transcript and caption mapping, handles, waveforms and silence
  bands. `SetSpeedRamp` takes points over the clip (0..1, speed) and keeps
  the source it plays: the clip's length follows the ramp, or with
  `keep_length` the speeds are scaled to fit; presets (`SpeedPreset`: Ramp
  in, Ramp out, Hero, Bullet, Montage) are shapes in multiples of the
  clip's speed. Trims and splits slice the keys (one added at the cut), so
  each frame keeps its speed; `SetSpeed` on a ramped clip scales the ramp.
  `SetRetime` picks how slowed frames are drawn (`Interpolation`: repeat,
  blend, optical flow) and whether ramped audio is stretched (pitch kept)
  or muted (`RampAudio`). Without keys a clip plays at `speed` as before,
  and old projects load unchanged.
* **Transform:** position, scale, rotation, opacity, per-axis stretch, skew,
  anchor, crop, flip, blend mode, `fit` (`Contain` with bars, or `Cover`,
  filling the frame and panned by position) and a corner pin; all keyframable.
  A pinned picture is placed through a homography (`Placement::homography`),
  others through one affine matrix shared by the GPU and CPU compositors.
  `compose::GroupMove` moves, scales and turns several clips about a common
  pivot and turns that back into each clip's transform.
* **Masks** (`flikk-core/src/mask.rs`): rectangles, ellipses and closed paths
  (straight or smooth) in the picture's own uv, with feather, expansion,
  opacity and invert, combined in order by add, subtract or intersect, each
  keyframable as a whole. Captions and titles take no masks or corner pins:
  renders burn them in with ASS, which cannot draw them.
* **Stabilization** (`Clip::stabilization`, `flikk-core/src/stabilize.rs`):
  on, a smoothness (0..1), Smooth or Locked, and a crop (Auto zoom or
  None); off by default and left out of `.flikk` files until set, so older
  files open unchanged. `Command::SetStabilization` sets it on video
  clips (see [Analysis and models](analysis-and-models.md), "Stabilization").
* **Subject masks** (`MaskShape::Subject`, `flikk-core/src/subject.rs`): the
  mask keeps the object its `Subject`'s prompts pick: on some source
  frames, clicks on the object (or off it) and a box, in picture uv, so
  trims, splits and speed changes keep them on their frames. With `track`
  the object is followed over every frame the clip shows; otherwise each
  frame shows the nearest prompted frame's matte. `refine` eases the edge
  onto the picture's (hair). Feather, expansion, opacity, invert and mode
  work as for shapes; the prompts are not keyframed.
* **Person masks** (`MaskShape::Person { edge }`, `flikk-core/src/person.rs`):
  the mask keeps every person a matting model finds in each frame, with no
  prompt; `edge` (0..1, keyframable with the rest of the state) eases the
  edge onto the picture's. Invert keeps the background instead. Feather,
  expansion, opacity and mode work as for shapes, so it combines with
  shape and subject masks. Old projects open unchanged (a new shape kind).
* **Cuts in speech** (`flikk-core/src/cuts.rs`) go in the pause between two
  words, clear of both (40 ms after the earlier, 30 ms before the later), on
  a frame boundary when the pause holds one, with a 10 ms audio fade at the
  join. The transcript panel's word delete and the agent's `cut_words` use
  it; the agent's `blade` and `ripple_delete` move a cut that falls on a word
  into the nearest pause and say so (`snap: false` turns it off).
* **Depth masks** (`MaskShape::Depth`): keep what lies between `near` and
  `far` from the camera (relative distance, 0 nearest to 1 farthest; see
  "Depth maps"), fading over `softness` past each, from the picture's
  depth map at the source time; the background, the foreground or a slice
  between. They combine, invert, fade and keyframe (`near`, `far`,
  `softness` move) like shapes; feather and expansion do not apply. To
  grade or add an effect to one depth only, a copy of the clip on the track
  above takes the mask and the change. Without its map yet the mask is
  left out.
* **Fixing words** (`flikk-core/src/respeak*`): words typed over others in
  the transcript are said in the speaker's voice and cut in where the old
  ones were. `respeak::fix` takes the cuts either side of them from
  `Speech::around` (as a word delete places them), the rest of their
  sentence (back to the end of the one before, on to its own end, in that
  voice or in words fixed before) and up to 50 words either side as
  context, and the quietest pauses between words within 20 s for room
  tone. The provider reads the whole sentence with the new words in it,
  the context as `previous_text`/`next_text`, so the delivery fits;
  `respeak::make` times the take's words (`AnalysisService::time_script`:
  as a voiceover's, the aligner or the edges measured on the levels), cuts
  the new words out with up to 80 ms before and 150 ms after (half the
  pause to the next word at most), brings them to the speaker's speech
  level in the original sentence (20 ms frames within 25 dB of the
  loudest) and its tilt (a one-pole split at 1.5 kHz, the high band moved
  by the ratio of high to low energy, at most ×4), lays the longest pause
  found under them looped with 50 ms equal-power crossfades, and pads them
  to the pauses the old words had before and after. The result is a WAV in
  `paths::generated()` (with its `.json`), a voiceover item whose script is
  the new words, so analysis times them and the transcript shows them, in
  italics as every voiceover's words. The take goes to `cache/respeak/`
  and is deleted once used. `Command::ReplaceSpeech` cuts it in: on the
  speech's track the range goes and the new file plays there with a 10 ms
  fade at both joins (in the pauses), keeping the replaced clip's gain and
  audio effects; shorter, the rest ripples as a delete of the range's end
  would; longer, everything after moves on every unlocked track, the
  picture over the range slows to fill it, other audio leaves a gap and
  titles and captions over its end last longer; the same source heard on
  another audio track is cut out of the range. One undo step; a `Fix`
  finds its words again by clip and source time when the timeline moved
  while the provider read. Words in a sped-up, slowed or ramped clip, or
  on a picture track only, are refused; a ramped picture held over longer
  words keeps its ramp's shape, slower. A source with clips on audio tracks is now read
  for words there only (`query::words_in_range`), so the old words don't
  come back from the picture over them.
* **Queries** (`flikk-core/src/query.rs`): topmost clip at a time, sample
  points, transcript↔timeline word mapping, typed timestamp and chat
  reference resolution, effective style rules.

## Change sets

`Author::Agent` and `Author::External` commands go into the author's
open `ChangeSet` (`flikk-core/src/changeset.rs`, `bus.rs`), planned against that
set's *draft* (committed project + its own pending items), so the agent sees
the effect of its earlier calls.

* **One growing set per chat.** Agent sets are stamped with the bus's current
  chat; the timeline overlay shows only the open chat's sets. At the start of
  each turn the host calls `reopen_change_set`: the chat's pending set opens
  again and the turn adds to it. If the user edited since, the draft is
  rebuilt on the current project by replaying each item's ops (re-planning
  those that fail, dropping those that still fail).
* **Review waits for the turn.** At the end of a turn the set becomes
  `Pending`, or `Resolved` if empty; the card is posted on the newest message
  only if the set grew. While the set is `Open`, the card's Accept and Reject
  are disabled; Preview and Review still work.
* **Accept** takes all pending items or the ticked subset. An item becomes
  `Conflicted` if a clip its ops touch was changed by the user after the
  version it was planned against; otherwise its ops replay, or its command
  is re-planned if they no longer apply. The whole accept is one history
  entry (AG5).
* Change sets live in the bus only; saved chats keep a message's change-set
  id, not the set.

## Sync by sound

A second camera or a separate audio recorder is lined up with the main clip
by what they both heard (`flikk-core/src/sync/`), for podcasts and multicam.

* **Measuring** (`sync/measure.rs`, pure Rust with its own radix-2 FFT in
  `sync/fft.rs`): each recording's stream is decoded through
  `MediaService::audio` to mono at 8 kHz, whole (up to 6 hours). A coarse
  pass turns each into an onset envelope (spectral flux of 16 log-spaced
  band energies, 100 a second, its local mean removed and its peaks capped)
  and cross-correlates the two over every lag at once, each lag normalized
  by the overlap it leaves (at least 3 s, or 30% of the shorter), so
  offsets of hours and recordings that start late or stop early cost the
  same. A fine pass reads 3–12 windows of 8 s spread over the overlap from
  both and finds each window's lag with band-limited (150–3,800 Hz)
  GCC-PHAT (β 0.8), to a fraction of a sample by a parabola. Flat first:
  the windows' median offset and those within 1.5 ms of it; a robust line
  (Theil–Sen, then least squares on the windows within 1.5 ms) wins when
  more windows agree with it and the drift it shows adds up to 2 ms or more
  over the overlap. The result is the follower's source time
  `offset + r · (1 + drift)` for the reference's source time `r`, with a
  confidence (0.35 from how far the coarse peak stands above the best lag
  elsewhere, 0.65 from the share of fine windows that agree); under 0.5 it
  is not used and the follower is left where it was. A follower that does
  not match the reference is tried through another follower that did
  (`sync::measure_group`). The measuring runs off the UI thread.
* **Placing** (`sync/plan.rs`): `Command::SyncClips` and
  `Command::PlaceSynced` carry what was measured, not positions, so a
  proposal planned again after the user moved the reference still lands in
  sync. A recording on the timeline is the clips that play one media item
  from the same point (a video and the sound that landed with it); they
  move together. The reference is the one on the lowest video track (the
  earliest there), else the first selected; the agent can name it. A
  follower that would overlap another clip goes to a free track of its kind,
  else a new one; one that would start before zero loses its head. A
  speed-ramped reference or follower is refused: its sound does not keep
  one offset, and a drift fix is a constant speed that would replace the
  ramp.
  `PlaceSynced` lays pool items down whole from the playhead, each on a
  free or new track, the group moved later together when one would start
  before it. "Recorder sound" disables the cameras' sound clips when the
  group holds a sound-only recording.
* **Drift** is corrected by the follower's speed (`speed × (1 + drift)`).
  Speeds within 0.1% of 1 (`sync::varispeed`, under two cents of pitch)
  play by resampling, as a fast clock does, not by time-stretching: output
  frame `n` is the input at `n · speed`, Catmull-Rom interpolated. The
  preview mixer uses it in place of WSOLA, and the render decodes such a
  clip's span the way the preview does, runs it through the same code and
  reads the result as FLAC from the job's work folder
  (`flikk-deliver/src/varispeed.rs`) instead of `atempo`, so preview and
  export put every sample at the same time. WSOLA and `atempo` were not
  used for this: each wanders up to ±10 ms around the nominal time. A
  follower video at such a speed is composited in renders, as any retimed
  clip is.
* **UI:** the timeline's clip menu offers "Sync by Audio" when the
  selection holds two or more recordings with sound, and "Sync by Audio,
  Recorder Sound" when it holds a sound-only one and a camera. The media
  pool takes cmd/ctrl- or shift-click to add items to its selection; an
  item's menu then offers "Sync" (and "Sync, Recorder Sound"). The edit is
  one undo step; the status line says only what did not match and drift
  that was corrected. The agent's `sync_audio` (`tools/sync.rs`) takes
  `clips` (or pool `media` with `at`), `reference` and
  `replace_camera_audio`, runs as a job and proposes the edit, reporting
  each follower's offset, drift and confidence, or why it was not synced.
