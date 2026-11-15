# PRD → code map

| PRD | Where |
| --- | --- |
| M1 import, bins, search | `ui/media_pool*`, `Command::ImportMedia` |
| M2 inspector, M4 proxies | `flikk-media`, `ui/media_pool*` |
| M3 analysis | `flikk-analysis`, `flikk-models` |
| E1, E2 timeline and tools | `ui/timeline*`, `flikk-core/src/edit/` |
| E3 transcript editing | `ui/transcript*`, `flikk-core/src/query.rs`, `cuts.rs`; fixing words: `flikk-core/src/respeak*`, `state/respeak.rs`, `tools/respeak.rs` |
| E4 silences | `AnalysisService::silences`, `ui/timeline/silence.rs` |
| E5 transform, keyframes | `model::Transform`, `mask.rs`, `ui/inspector*`, `ui/viewer/gizmo.rs`, `ui/viewer/masks.rs` |
| Subject masks | `flikk-core/src/subject.rs`, `mask.rs`, `compose.rs`, `flikk-models/src/segmentation/`, `flikk-analysis/src/subjects*`, `ui/viewer/masks.rs`, `ui/inspector/masks.rs`, `tools/masks.rs` |
| Stabilization | `flikk-core/src/stabilize.rs`, `compose.rs`, `flikk-analysis/src/motion.rs`, `ui/inspector/stabilize.rs`, `tools/stabilize.rs`, `flikk-deliver/src/composed.rs` |
| Person masks | `flikk-core/src/person.rs`, `mask.rs`, `compose.rs`, `flikk-models/src/matting/`, `flikk-analysis/src/people*`, `ui/inspector/masks.rs`, `ui/viewer.rs`, `tools/masks.rs` |
| Depth maps, depth effects and masks | `flikk-core/src/depth.rs`, `fx/depth.rs`, `mask.rs`, `compose.rs`, `flikk-gpu` (`wrap.rs`, `batch.rs`), `flikk-models/src/depth/`, `flikk-analysis/src/depth*`, `flikk-deliver/src/composed.rs`, `ui/inspector/effects.rs`, `ui/inspector/masks.rs`, `ui/viewer/focus.rs`, `tools/depth.rs` |
| E8 timecode readout | `Clip::to_source`, viewer |
| Speed ramps, slow motion | `flikk-core/src/speed.rs`, `edit/retime.rs` (`set_speed_ramp`), `compose.rs`, `compose/pictures.rs`, `flikk-models/src/interpolation/`, `flikk-analysis/src/interpolation.rs`, `audiofx/stretch.rs`, `flikk-deliver/src/graph.rs`, `ui/inspector/speed.rs`, `tools/speed.rs` |
| C1–C3, C6 | `flikk-core/src/captions.rs`, caption tracks, `CaptionStyle`, `ui/caption_style*`, `flikk-deliver` SRT/ASS, `ui/lettering.rs` |
| K1 grade | `flikk-core/src/grade.rs`, `scopes.rs`, `ui/inspector/color*`, `ui/scopes.rs` |
| A1–A3 mixer, loudness, ducking | `ui/mixer*`, `ui/inspector/audio.rs`, `audiofx.rs`, `AnalysisService::loudness`, render graph |
| Fit music to length | `flikk-core/src/music_fit*`, `Command::FitMusic`, `state/music_fit.rs`, `ui/timeline/menu.rs`, `tools/music.rs` |
| D1 render | `flikk-deliver`, `ui/deliver.rs`; hardware encoders `flikk-deliver/src/hw.rs` |
| Render cache | `flikk/src/render_cache*`, `ui/viewer/player.rs`, `ui/timeline/paint.rs` (render bar), `ui/editor_settings/storage.rs`, `flikk-store/src/render_cache.rs` |
| AG1–AG10 | `flikk-agent`, `agent_link*`, `ui/chat*`, `ui/settings.rs` |
| Online providers | `flikk-cloud`, `flikk-core/src/cloud/`, `pool.rs`, `state/cloud.rs`, `ui/editor_settings/integrations.rs`, `ui/media_pool/generate.rs`, `tools/generate.rs` |
| HDR tone mapping | `flikk-core/src/hdr.rs`, `flikk-media` probe, decode, proxy, `flikk-deliver` graph, `ui/inspector/tone_map.rs`, `ui/media_pool/hdr.rs`, `set_tone_map` |
| V1–V7 collages | `MediaService::collage`, `flikk-agent` view tools |
| S1, S3, S4, F1, F2, F4 | `flikk-core::memory`, `flikk-store`, cut planners |
| Sync by sound (podcasts, multicam) | `flikk-core/src/sync/`, `Command::SyncClips` / `PlaceSynced`, `state/sync.rs`, `ui/timeline/menu.rs`, `ui/media_pool.rs`, `ui/viewer/audio.rs`, `flikk-deliver/src/varispeed.rs`, `tools/sync.rs` |
