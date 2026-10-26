//! Audio workspace: clip gain and fades, track mixer, loudness with
//! normalize, and ducking (PRD A1–A3).

use super::param::{Key, format_value};
use super::{InspectorPanel, audio_tracks, is_media, section, section_with};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::analysis::Loudness;
use koo_core::command::Command;
use koo_core::model::{Clip, Ducking};
use koo_core::{ClipId, Time, TimeRange};
use std::path::PathBuf;

impl InspectorPanel {
    pub(super) fn audio_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let selected: Vec<Clip> = self
            .selected_clips(cx)
            .into_iter()
            .filter(is_media)
            .collect();
        let mut body = v_flex().gap_4();
        let tl = &self.project(cx).timeline;
        let sound = selected
            .first()
            .map(|c| super::clip::sound_clip(tl, c).clone());
        if let Some(clip) = &sound {
            let rows = [
                (Key::ClipGain, clip.audio.gain_db),
                (Key::FadeIn, clip.audio.fade_in.as_secs_f64() as f32),
                (Key::FadeOut, clip.audio.fade_out.as_secs_f64() as f32),
            ]
            .map(|(key, v)| self.param(key, v, window, cx).row(cx));
            body = body.child(section("Clip", cx).children(rows));
        }
        body.child(self.mixer(window, cx))
            .child(self.loudness_view(&selected, cx))
            .child(self.ducking_view(window, cx))
            .into_any_element()
    }

    fn mixer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let tracks = audio_tracks(self.project(cx));
        let mut rows = Vec::new();
        for (i, track) in tracks.into_iter().enumerate() {
            let id = track.id;
            let volume = self
                .param(Key::Volume(id), track.mix.volume_db, window, cx)
                .slider(None);
            let pan = self
                .param(Key::Pan(id), track.mix.pan, window, cx)
                .slider(None);
            let muted = cx.theme().muted_foreground;
            let (mute_mix, solo_mix) = (track.mix.clone(), track.mix.clone());
            rows.push(
                h_flex()
                    .h(px(28.))
                    .gap_2()
                    .child(
                        div()
                            .w(px(24.))
                            .flex_none()
                            .text_xs()
                            .child(track.name.clone()),
                    )
                    .child(volume)
                    .child(
                        div()
                            .w(px(34.))
                            .flex_none()
                            .text_xs()
                            .text_right()
                            .text_color(muted)
                            .child(format_value(track.mix.volume_db, 1)),
                    )
                    .child(div().w(px(52.)).flex_none().flex().child(pan))
                    .child(
                        Button::new(("mute", i))
                            .ghost()
                            .xsmall()
                            .label("M")
                            .tooltip("Mute")
                            .toggled(track.mix.mute)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let mut mix = mute_mix.clone();
                                mix.mute = !mix.mute;
                                this.run(vec![Command::SetTrackMix { track: id, mix }], "mix", cx);
                            })),
                    )
                    .child(
                        Button::new(("solo", i))
                            .ghost()
                            .xsmall()
                            .label("S")
                            .tooltip("Solo")
                            .toggled(track.mix.solo)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let mut mix = solo_mix.clone();
                                mix.solo = !mix.solo;
                                this.run(vec![Command::SetTrackMix { track: id, mix }], "mix", cx);
                            })),
                    ),
            );
        }
        section("Mixer", cx).children(rows)
    }

    /// Clips the loudness readout covers: the selection, else every media
    /// clip on an audio track.
    fn loudness_scope(&self, selected: &[Clip], cx: &App) -> Vec<Clip> {
        if !selected.is_empty() {
            return selected.to_vec();
        }
        audio_tracks(self.project(cx))
            .into_iter()
            .flat_map(|t| t.clips)
            .filter(is_media)
            .collect()
    }

    fn loudness_view(&self, selected: &[Clip], cx: &mut Context<Self>) -> Div {
        let scope = self.loudness_scope(selected, cx);
        let reading = combined(&scope, &self.loudness);
        let has_service = self.state.read(cx).services.analysis.is_some();
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let measure = Button::new("measure")
            .ghost()
            .xsmall()
            .icon(IconName::RefreshCw)
            .tooltip("Measure")
            .loading(self.measuring)
            .disabled(!has_service || scope.is_empty())
            .on_click(cx.listener(|this, _, _, cx| this.measure(cx)));
        let figure = |value: Option<f32>, unit: &'static str| {
            h_flex()
                .items_end()
                .gap_1()
                .child(
                    div()
                        .text_lg()
                        .when(value.is_none(), |d| d.text_color(muted))
                        .child(value.map(|v| format_value(v, 1)).unwrap_or("—".into())),
                )
                .child(div().text_xs().text_color(muted).child(unit))
        };
        section_with("Loudness", measure, cx)
            .child(
                h_flex()
                    .gap_4()
                    .child(figure(reading.map(|r| r.integrated_lufs), "LUFS"))
                    .child(figure(reading.map(|r| r.true_peak_db), "dBTP")),
            )
            .child(
                h_flex()
                    .pt_1()
                    .gap_2()
                    .child(
                        div()
                            .w(px(84.))
                            .child(Input::new(&self.lufs_target).xsmall().suffix("LUFS")),
                    )
                    .child(
                        div()
                            .w(px(84.))
                            .child(Input::new(&self.peak_target).xsmall().suffix("dBTP")),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("normalize")
                            .small()
                            .label("Normalize")
                            .disabled(reading.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.normalize(cx))),
                    ),
            )
    }

    fn measure(&mut self, cx: &mut Context<Self>) {
        let selected: Vec<Clip> = self
            .selected_clips(cx)
            .into_iter()
            .filter(is_media)
            .collect();
        let scope = self.loudness_scope(&selected, cx);
        let state = self.state.read(cx);
        let Some(analysis) = state.services.analysis.clone() else {
            return;
        };
        let project = state.bus.project();
        let jobs: Vec<(ClipId, PathBuf, TimeRange)> = scope
            .iter()
            .filter_map(|c| {
                let path = project.media(c.media()?)?.path.clone();
                let start = c.to_source(c.start)?;
                Some((c.id, path, TimeRange::new(start, start + c.duration)))
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
                            analysis.loudness(&path, Some(range)).ok().map(|l| (id, l))
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

    /// One batch of `SetGain`: a selection is normalized clip by clip, the
    /// timeline as a whole.
    fn normalize(&mut self, cx: &mut Context<Self>) {
        let target = self.lufs_target.read(cx).value().trim().parse::<f32>();
        let peak = self.peak_target.read(cx).value().trim().parse::<f32>();
        let presets = &self.state.read(cx).presets;
        let target = target.unwrap_or(presets.loudness_lufs);
        let peak = peak.unwrap_or(presets.true_peak_db);
        let selected: Vec<Clip> = self
            .selected_clips(cx)
            .into_iter()
            .filter(is_media)
            .collect();
        let measured = |c: &Clip| self.loudness.get(&c.id).copied();
        let mut commands = Vec::new();
        if selected.is_empty() {
            let scope = self.loudness_scope(&selected, cx);
            let Some(all) = combined(&scope, &self.loudness) else {
                return;
            };
            let delta = (target - all.integrated_lufs).min(peak - all.true_peak_db);
            for c in scope.iter().filter(|c| measured(c).is_some()) {
                commands.push(Command::SetGain {
                    clip: c.id,
                    db: c.audio.gain_db + delta,
                });
            }
        } else {
            for c in &selected {
                if let Some(l) = measured(c) {
                    let db = (target - l.integrated_lufs).min(peak - l.true_peak_db);
                    commands.push(Command::SetGain { clip: c.id, db });
                }
            }
        }
        self.run(commands, "normalize", cx);
    }

    fn ducking_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let project = self.project(cx);
        let tracks = audio_tracks(project);
        let ducking = project.timeline.ducking.clone();
        let default_music = tracks.last().map(|t| t.id);
        let reduction = self.state.read(cx).presets.ducking_db;
        let toggle = Switch::new("ducking")
            .small()
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
        let mut view = section_with("Ducking", toggle, cx);
        let Some(d) = ducking else {
            return view;
        };
        let selected = tracks
            .iter()
            .position(|t| t.id == d.music_track)
            .unwrap_or(0);
        let ids: Vec<_> = tracks.iter().map(|t| t.id).collect();
        let current = d.clone();
        view = view.child(
            h_flex().h(px(28.)).child(
                TabBar::new("music-track")
                    .segmented()
                    .xsmall()
                    .selected_index(selected)
                    .children(tracks.iter().map(|t| Tab::new().label(t.name.clone())))
                    .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                        if let Some(track) = ids.get(*ix) {
                            let d = Ducking {
                                music_track: *track,
                                ..current.clone()
                            };
                            this.set_ducking(Some(d), cx);
                        }
                    })),
            ),
        );
        for (key, v) in [
            (Key::DuckReduction, d.reduction_db),
            (Key::DuckAttack, d.attack.as_secs_f64() as f32),
            (Key::DuckRelease, d.release.as_secs_f64() as f32),
        ] {
            view = view.child(self.param(key, v, window, cx).row(cx));
        }
        view
    }
}

/// Duration-weighted power mean of the clips' loudness plus their gain, and
/// the highest true peak. `None` until every clip is measured.
fn combined(
    clips: &[Clip],
    measured: &std::collections::HashMap<ClipId, Loudness>,
) -> Option<Loudness> {
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
