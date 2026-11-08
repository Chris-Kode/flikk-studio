//! What the canvas paints this frame: the visible tracks, clips, tiles
//! and overlays gathered into a [`Scene`].

use super::layout::{self, RULER_HEIGHT, Row, SNAP_PX};
use super::overlay::{self, OverlayItem};
use super::paint::{ClipScene, Ghost, Marquee, Scene, TrackScene, TransitionScene, Warp};
use super::{Drag, TimelinePanel, Tool, fx, thumbs};
use crate::ui::kit::IconName;
use flikk_core::command::Edge;
use flikk_core::model::{ClipContent, MediaKind, TrackKind};
use flikk_core::{ClipId, Rate, Time, TimeRange};
use gpui_kit::*;

/// A signed offset as `+S:FF` (seconds and frames).
fn signed_timecode(t: Time, rate: Rate) -> String {
    let fps = rate.rounded_fps() as i64;
    let sign = if t.0 < 0 { '-' } else { '+' };
    let frames = Time(t.0.abs()).to_frame(rate);
    format!("{sign}{}:{:02}", frames / fps.max(1), frames % fps.max(1))
}

impl TimelinePanel {
    pub(super) fn scene(&mut self, cx: &mut Context<Self>) -> Scene {
        let hover_edge = self
            .hovered_edge(self.hover, cx)
            .filter(|_| self.drag.is_none());
        let state = self.state.read(cx);
        let project = state.bus.project();
        let media = state.services.media.clone();
        let mut selection = state.selection.clips.clone();
        let selected_transition = state.selected_transition;
        let playhead = state.playhead;
        let in_out = state.in_out;
        let focused = state.focused_change().map(|i| i.id);
        let overlay: Vec<OverlayItem> =
            overlay::collect(&project.timeline, state.bus.chat_change_sets());
        let tl = &project.timeline;
        let rate = tl.settings.rate;

        let mut marquee = None;
        if let Some(Drag::Marquee {
            from,
            to,
            base,
            moved: true,
        }) = &self.drag
        {
            selection = base.clone();
            selection.extend(self.marquee_hits(*from, *to, tl));
            marquee = Some(Marquee {
                from: *from,
                to: *to,
            });
        }

        let lifted: Vec<ClipId> = match &self.drag {
            Some(Drag::Move {
                clips, moved: true, ..
            }) => clips.clone(),
            _ => vec![],
        };
        let mut ghosts = Vec::new();
        match &self.drag {
            Some(Drag::Move {
                clips,
                to_track,
                from_track,
                delta,
                moved: true,
                ..
            }) => {
                for id in clips {
                    if let Some((track, c)) = tl.clip(*id) {
                        let track = if track.id == *from_track {
                            *to_track
                        } else {
                            track.id
                        };
                        ghosts.push(Ghost {
                            track,
                            range: c.range().shift(*delta),
                            label: None,
                            edge: None,
                        });
                    }
                }
            }
            Some(Drag::Trim {
                clip, edge, delta, ..
            }) => {
                if let Some((track, c)) = tl.clip(*clip) {
                    let r = c.range();
                    let (range, moved) = match edge {
                        Edge::Start => (TimeRange::new(r.start + *delta, r.end), r.start + *delta),
                        Edge::End => (TimeRange::new(r.start, r.end + *delta), r.end + *delta),
                    };
                    ghosts.push(Ghost {
                        track: track.id,
                        range,
                        label: Some(signed_timecode(*delta, rate).into()),
                        edge: Some(moved),
                    });
                }
            }
            Some(Drag::Roll { left, right, delta }) => {
                if let (Some((track, l)), Some((_, r))) = (tl.clip(*left), tl.clip(*right)) {
                    let cut = l.end() + *delta;
                    for (range, label) in [
                        (
                            TimeRange::new(l.start, cut),
                            Some(signed_timecode(*delta, rate)),
                        ),
                        (TimeRange::new(cut, r.end()), None),
                    ] {
                        ghosts.push(Ghost {
                            track: track.id,
                            range,
                            label: label.map(Into::into),
                            edge: Some(cut),
                        });
                    }
                }
            }
            Some(Drag::Slip { clip, delta, .. }) => {
                if let Some((track, c)) = tl.clip(*clip) {
                    ghosts.push(Ghost {
                        track: track.id,
                        range: c.range(),
                        label: Some(signed_timecode(*delta, rate).into()),
                        edge: None,
                    });
                }
            }
            _ => {}
        }

        let rows = self.rows(tl);
        let width = f32::from(self.bounds.get().size.width);
        // Only clips in view are built and painted: a long transcript makes
        // thousands of captions.
        let visible = TimeRange::new(
            self.viewport.time(-SNAP_PX).max(Time::ZERO),
            self.viewport.time(width + SNAP_PX),
        );
        let mut requests = Vec::new();
        let tracks = rows
            .iter()
            .zip(&tl.tracks)
            .map(|(row, track)| {
                let clips = track
                    .clips
                    .iter()
                    .filter(|clip| clip.end() >= visible.start && clip.start <= visible.end)
                    .map(|clip| {
                        let peaks = if track.kind == TrackKind::Audio && media.is_some() {
                            let (peaks, request) = self.waveforms.lookup(clip, project);
                            requests.extend(request);
                            peaks
                        } else {
                            None
                        };
                        let tiles = match (&media, clip.media().and_then(|m| project.media(m))) {
                            (Some(_), Some(item))
                                if matches!(track.kind, TrackKind::Video | TrackKind::Overlay)
                                    && item.kind != MediaKind::Audio =>
                            {
                                self.tiles(clip, item, row, width)
                            }
                            _ => vec![],
                        };
                        let icon = if !clip.enabled {
                            Some(IconName::EyeOff)
                        } else if matches!(clip.content, ClipContent::Generator { .. }) {
                            Some(IconName::PaintBucket)
                        } else if track.kind == TrackKind::Title {
                            Some(IconName::Type)
                        } else {
                            None
                        };
                        ClipScene {
                            tiles,
                            range: clip.range(),
                            label: clip.display_name(project).into(),
                            color: clip.color.map(|c| rgb(c.rgb())),
                            enabled: clip.enabled,
                            offline: clip.media().is_some_and(|m| state.is_offline(m)),
                            icon,
                            effects: !clip.effects.is_empty(),
                            speed: if clip.is_ramped() {
                                Some("ramp".into())
                            } else {
                                ((clip.speed - 1.).abs() > 1e-6)
                                    .then(|| format!("{} %", (clip.speed * 100.).round()).into())
                            },
                            warp: (clip.media().is_some() && clip.is_retimed()).then(|| {
                                let clip = clip.clone();
                                std::sync::Arc::new(move |t: Time| clip.source_offset(t)) as Warp
                            }),
                            selected: selection.contains(&clip.id),
                            lifted: lifted.contains(&clip.id),
                            hover_edge: hover_edge
                                .filter(|(id, _)| *id == clip.id)
                                .map(|(_, edge)| edge),
                            peaks,
                            fade_in: clip.audio.fade_in,
                            fade_out: clip.audio.fade_out,
                        }
                    })
                    .collect();
                let audio = track.kind == TrackKind::Audio;
                let hidden = !audio && track.mix.mute;
                let silent = audio && !tl.audible(track);
                let fade = if hidden {
                    0.3
                } else if silent {
                    0.45
                } else {
                    1.
                };
                TrackScene {
                    row: *row,
                    clips,
                    fade: if track.locked { fade * 0.55 } else { fade },
                    locked: track.locked,
                    hidden,
                }
            })
            .collect();

        let resizing = match &self.drag {
            Some(Drag::TransitionEdge { owner, duration }) => Some((*owner, *duration)),
            _ => None,
        };
        let mut transitions = Vec::new();
        for track in &tl.tracks {
            for t in fx::placed(track) {
                let range = match resizing {
                    Some((owner, d)) if owner == t.owner => t.range_with(d),
                    _ => t.range,
                };
                if range.end < visible.start || range.start > visible.end {
                    continue;
                }
                transitions.push(TransitionScene {
                    track: track.id,
                    range,
                    label: format!("{:.1}s", range.duration().as_secs_f64()).into(),
                    selected: selected_transition == Some(t.owner),
                });
            }
        }

        let silences = if self.silence_open {
            let (min, pad) = self.silence_params(cx);
            self.silences.timeline_silences(project, min, pad)
        } else {
            vec![]
        };

        let blade = match (self.tool, self.hover, &self.drag) {
            (Tool::Blade, Some(p), None) => {
                let (x, y) = self.local(p);
                let row = layout::row_at(&rows, y).map(|r| r.track);
                let candidates = vec![playhead];
                let t = self.viewport.time(x);
                let t = layout::snap(t, &candidates, self.viewport.span(SNAP_PX))
                    .unwrap_or(t)
                    .floor_to_frame(tl.settings.rate);
                (p.y > self.bounds.get().origin.y + px(RULER_HEIGHT)).then_some((t, row))
            }
            _ => None,
        };

        let scene = Scene {
            viewport: self.viewport,
            rate: tl.settings.rate,
            tracks,
            markers: tl.markers.clone(),
            in_out,
            playhead,
            scrubbing: matches!(self.drag, Some(Drag::Scrub)),
            overlay,
            focused,
            hovered: self.hovered_item.map(|(_, i)| i),
            silences,
            transitions,
            ghosts,
            snap: self.snap,
            blade,
            drop: self.drop,
            marquee,
            render: self.state.read(cx).render.status(),
        };

        if let Some(media) = media {
            for request in requests {
                let media = media.clone();
                cx.spawn(async move |this, cx| {
                    let key = request.key;
                    let peaks = cx
                        .background_executor()
                        .spawn(async move { request.run(media.as_ref()) })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        this.waveforms.insert(key, peaks);
                        cx.notify();
                    });
                })
                .detach();
            }
        }

        self.thumbs.trim_queue(160);
        self.pump_thumbs(cx);

        scene
    }

    /// Visible filmstrip tiles of a video clip.
    fn tiles(
        &mut self,
        clip: &flikk_core::model::Clip,
        item: &flikk_core::model::MediaItem,
        row: &Row,
        width: f32,
    ) -> Vec<(f32, f32, std::sync::Arc<RenderImage>)> {
        let vp = self.viewport;
        let (_, strip) = layout::filmstrip(row.height - layout::CLIP_INSET * 2.);
        let tile_w = (strip * 16. / 9.).max(8.);
        let (x0, x1) = (vp.x(clip.start), vp.x(clip.end()));
        let (v0, v1) = (x0.max(0.), x1.min(width));
        if v1 <= v0 {
            return vec![];
        }
        let step = thumbs::step((tile_w / vp.px_per_sec) as f64);
        let first = ((v0 - x0) / tile_w).floor() as i64;
        let last = ((v1 - x0) / tile_w).ceil() as i64;
        let mut out = Vec::new();
        for i in first.max(0)..last.min(first + 200) {
            let offset = i as f32 * tile_w;
            let Some(source) = clip.to_source(clip.start + vp.span(offset)) else {
                continue;
            };
            if let Some(image) = self.thumbs.get(item, source, step) {
                out.push((offset, tile_w, image));
            }
        }
        out
    }

    fn pump_thumbs(&mut self, cx: &mut Context<Self>) {
        let Some(media) = self.state.read(cx).services.media.clone() else {
            return;
        };
        for (key, item, t) in self.thumbs.next_requests() {
            let media = media.clone();
            cx.spawn(async move |this, cx| {
                let image = cx
                    .background_executor()
                    .spawn(async move { thumbs::fetch(media.as_ref(), &item, t) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.thumbs.finish(key, image);
                    this.pump_thumbs(cx);
                    cx.notify();
                });
            })
            .detach();
        }
    }
}
