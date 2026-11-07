//! Audio workspace inspector: the selected clip's volume automation and
//! effects ([`ClipAudioFx`]), loudness with normalize, and ducking (PRD
//! A1–A3). The track mixer is its own panel (`ui/mixer.rs`).

use super::param::{Key, format_value};
use super::{
    InspectorPanel, View, audio_tracks, clip, group, is_media, label, panel_header, row,
    sound_clips, wide,
};
use crate::ui::kit::theme as t;
use crate::ui::kit::{Button, IconName, NumberField, Segmented, Switch, h_flex, v_flex};
use crate::ui::mixer::clip::ClipAudioFx;
use flikk_core::analysis::Loudness;
use flikk_core::command::Command;
use flikk_core::model::{Clip, Ducking, Project};
use flikk_core::{ClipId, Time, TimeRange};
use gpui_kit::*;
use std::collections::HashMap;
use std::path::PathBuf;

impl InspectorPanel {
    /// The Audio workspace: the selected clip's volume (with keyframes) and
    /// effects, then whole-timeline loudness and ducking. Track faders, pan
    /// and inserts are in the mixer.
    pub(super) fn timeline_audio_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> View {
        let clips = self.selected_clips(cx);
        let project = self.project(cx);
        let sound = sound_clips(&project.timeline, &clips)
            .first()
            .map(|c| (c.id, clip::clip_name(c, project)));
        let context = sound
            .as_ref()
            .map_or_else(|| project.name.clone(), |(_, name)| name.clone());
        let mut body = v_flex();
        if let Some((id, _)) = sound {
            let state = self.state.clone();
            let fx = window.use_keyed_state("clip-audio-fx", cx, move |_, cx| {
                ClipAudioFx::new(state, cx)
            });
            fx.update(cx, |f, cx| f.set_clip(Some(id), cx));
            body = body.child(fx);
        }
        let body = body
            .child(self.loudness_view(cx))
            .child(self.ducking_view(window, cx));
        View::new(panel_header(context), body)
    }

    /// Clips the loudness readout covers: every media clip on an audio track.
    fn loudness_scope(&self, cx: &App) -> Vec<Clip> {
        audio_tracks(self.project(cx))
            .into_iter()
            .flat_map(|t| t.clips)
            .filter(is_media)
            .collect()
    }

    /// Integrated loudness and true peak of the timeline, measured on
    /// request, and normalizing it to the targets.
    fn loudness_view(&self, cx: &mut Context<Self>) -> Div {
        let scope = self.loudness_scope(cx);
        let reading = combined(&scope, &self.measured(&scope, cx));
        let has_service = self.state.read(cx).services.analysis.is_some();
        let measure = Button::new("measure")
            .ghost()
            .xsmall()
            .icon(IconName::Gauge)
            .tooltip("Measure")
            .loading(self.measuring)
            .disabled(!has_service || scope.is_empty())
            .on_click(cx.listener(|this, _, _, cx| this.measure(cx)));
        let header = self.header("loudness", "Loudness", cx).child(measure);
        if !self.is_open("loudness") {
            return group(header, Vec::<Div>::new());
        }
        let figure = |value: Option<f32>, unit: &'static str| {
            h_flex()
                .items_end()
                .gap(px(4.))
                .child(
                    div()
                        .font_family(t::mono())
                        .text_size(px(18.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if value.is_some() { t::T1 } else { t::T3 })
                        .child(value.map_or("\u{2014}".into(), |v| format_value(v, 1))),
                )
                .child(
                    div()
                        .pb(px(3.))
                        .text_size(px(10.))
                        .text_color(t::T3)
                        .child(unit),
                )
        };
        let readout = h_flex()
            .gap(px(16.))
            .child(figure(reading.map(|r| r.integrated_lufs), "LUFS"))
            .child(figure(reading.map(|r| r.true_peak_db), "dBTP"));
        let targets = h_flex()
            .gap(px(6.))
            .child(
                div()
                    .w(px(76.))
                    .child(NumberField::new(&self.lufs_target).unit("LUFS")),
            )
            .child(
                div()
                    .w(px(76.))
                    .child(NumberField::new(&self.peak_target).unit("dBTP")),
            )
            .child(div().flex_1())
            .child(
                Button::new("normalize")
                    .small()
                    .label("Normalize")
                    .disabled(reading.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.normalize(cx))),
            );
        group(header, [wide(readout), wide(targets)])
    }

    fn measure(&mut self, cx: &mut Context<Self>) {
        let scope = self.loudness_scope(cx);
        let state = self.state.read(cx);
        let Some(analysis) = state.services.analysis.clone() else {
            return;
        };
        let project = state.bus.project();
        let jobs: Vec<(ClipId, PathBuf, TimeRange)> = scope
            .iter()
            .filter_map(|c| {
                let (path, range) = span(project, c)?;
                Some((c.id, path, range))
            })
            .collect();
        self.measuring = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    jobs.into_iter()
                        .filter_map(|(id, path, range)| {
                            let l = analysis.loudness(&path, Some(range)).ok()?;
                            Some((id, (path, range, l)))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                this.loudness.extend(results);
                this.measuring = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The readings still true of `clips`: measured on the span each
    /// covers now.
    fn measured(&self, clips: &[Clip], cx: &App) -> HashMap<ClipId, Loudness> {
        let project = self.project(cx);
        clips
            .iter()
            .filter_map(|c| {
                let (path, range, l) = self.loudness.get(&c.id)?;
                (span(project, c)? == (path.clone(), *range)).then_some((c.id, *l))
            })
            .collect()
    }

    /// One batch of `SetGain` that moves the timeline as a whole.
    fn normalize(&mut self, cx: &mut Context<Self>) {
        let target = self.lufs_target.read(cx).value() as f32;
        let peak = self.peak_target.read(cx).value() as f32;
        let scope = self.loudness_scope(cx);
        let measured = self.measured(&scope, cx);
        let Some(all) = combined(&scope, &measured) else {
            return;
        };
        let delta = (target - all.integrated_lufs).min(peak - all.true_peak_db);
        let commands = scope
            .iter()
            .filter(|c| measured.contains_key(&c.id))
            .map(|c| Command::SetGain {
                clip: c.id,
                db: c.audio.gain_db + delta,
            })
            .collect();
        self.run(commands, "normalize", cx);
    }

    /// Lower the music under speech: which track is the music, by how
    /// much, and how fast.
    fn ducking_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let project = self.project(cx);
        let tracks = audio_tracks(project);
        let ducking = project.timeline.ducking.clone();
        let default_music = tracks.last().map(|t| t.id);
        let reduction = self.state.read(cx).presets.ducking_db;
        let toggle = Switch::new("ducking")
            .checked(ducking.is_some())
            .disabled(default_music.is_none())
            .on_click(cx.listener(move |this, on: &bool, _, cx| {
                let ducking = match (on, default_music) {
                    (true, Some(music_track)) => Some(Ducking {
                        music_track,
                        reduction_db: reduction,
                        attack: Time::from_millis(200),
                        release: Time::from_millis(600),
                    }),
                    _ => None,
                };
                this.set_ducking(ducking, cx);
            }));
        let header = self.header("ducking", "Ducking", cx).child(toggle);
        let Some(d) = ducking.filter(|_| self.is_open("ducking")) else {
            return group(header, Vec::<Div>::new());
        };
        let selected = tracks
            .iter()
            .position(|t| t.id == d.music_track)
            .unwrap_or(0);
        let ids: Vec<_> = tracks.iter().map(|t| t.id).collect();
        let current = d.clone();
        let mut rows = vec![row(
            label("Music"),
            Segmented::new("music-track")
                .compact()
                .fill()
                .items(tracks.iter().map(|t| t.name.clone()))
                .selected(selected)
                .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                    if let Some(track) = ids.get(*ix) {
                        let d = Ducking {
                            music_track: *track,
                            ..current.clone()
                        };
                        this.set_ducking(Some(d), cx);
                    }
                })),
            None,
        )];
        for (key, v) in [
            (Key::DuckReduction, d.reduction_db),
            (Key::DuckAttack, d.attack.as_secs_f64() as f32),
            (Key::DuckRelease, d.release.as_secs_f64() as f32),
        ] {
            rows.push(self.param_row(key, v, false, None, window, cx));
        }
        group(header, rows)
    }
}

/// The source file and range a clip plays, which its loudness is measured on.
fn span(project: &Project, c: &Clip) -> Option<(PathBuf, TimeRange)> {
    let path = project.media(c.media()?)?.path.clone();
    let start = c.to_source(c.start)?;
    Some((path, TimeRange::new(start, start + c.duration)))
}

/// Duration-weighted power mean of the clips' loudness plus their gain, and
/// the highest true peak. `None` until every clip is measured.
fn combined(clips: &[Clip], measured: &HashMap<ClipId, Loudness>) -> Option<Loudness> {
    if clips.is_empty() {
        return None;
    }
    let (mut energy, mut total, mut peak) = (0.0f64, 0.0f64, f32::NEG_INFINITY);
    for c in clips {
        let l = measured.get(&c.id)?;
        let d = c.duration.as_secs_f64();
        energy += d * 10f64.powf((l.integrated_lufs + c.audio.gain_db) as f64 / 10.0);
        total += d;
        peak = peak.max(l.true_peak_db + c.audio.gain_db);
    }
    (total > 0.0).then(|| Loudness {
        integrated_lufs: (10.0 * (energy / total).log10()) as f32,
        true_peak_db: peak,
        range_lu: 0.0,
    })
}
