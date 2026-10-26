//! Color (PRD K1 and up): wheels, primaries, curves and a LUT for the clip
//! under the playhead (or the selection) or the whole timeline in the Color
//! workspace, for the selected clips in a clip's Color tab. "Match to…"
//! grades the clip toward a reference clip through `scopes::match_frames`.

mod curves;
mod editor;
mod fields;
mod wheels;

use super::param::Key;
use super::{InspectorPanel, empty, is_media};
use editor::{GradeUi, Target};
use fields::{PRIMARY, TONE};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::*;
use koo_core::command::Command;
use koo_core::model::{Clip, ClipContent, Grade, MediaKind, Project, TrackKind};
use koo_core::{ClipId, Time};
use std::path::PathBuf;

/// Source width of the frames shot matching measures.
const MATCH_WIDTH: u32 = 480;

impl InspectorPanel {
    fn grade_ui(&self, window: &mut Window, cx: &mut Context<Self>) -> Entity<GradeUi> {
        let state = self.state.clone();
        window.use_keyed_state("grade-ui", cx, move |_, _| GradeUi::new(state))
    }

    /// Color workspace: the clip under the playhead (or the selected media
    /// clips), or the timeline grade over every clip.
    pub(super) fn timeline_color_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = self.grade_ui(window, cx);
        let timeline = ui.read(cx).timeline_tab;
        let tab_ui = ui.clone();
        let tabs = TabBar::new("grade-target")
            .segmented()
            .xsmall()
            .selected_index(timeline as usize)
            .child(Tab::new().label("Clip"))
            .child(Tab::new().label("Timeline"))
            .on_click(move |ix, _, cx| {
                tab_ui.update(cx, |ui, cx| {
                    ui.timeline_tab = *ix == 1;
                    cx.notify();
                })
            });
        if timeline {
            let g = self.project(cx).timeline.grade.clone();
            ui.update(cx, |ui, _| ui.sync(Target::Timeline, g.clone()));
            let reset_ui = ui.clone();
            let actions = h_flex().gap_1().child(
                Button::new("timeline-grade-reset")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Undo2)
                    .tooltip("Reset")
                    .disabled(g == Grade::default())
                    .on_click(move |_, _, cx| {
                        reset_ui.update(cx, |ui, cx| ui.commit_grade(Grade::default(), cx))
                    }),
            );
            return self.grade_controls(ui, tabs, actions, window, cx);
        }
        let mut clips = self.media_selection(cx);
        if clips.is_empty() {
            let state = self.state.read(cx);
            clips.extend(clip_at(state.bus.project(), state.playhead));
        }
        if clips.is_empty() {
            return v_flex()
                .gap_2()
                .child(tabs)
                .child(empty("No clip", cx))
                .into_any_element();
        }
        self.clip_controls(ui, clips, tabs.into_any_element(), window, cx)
    }

    /// A clip's Color tab: the selected media clips.
    pub(super) fn color_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let clips = self.media_selection(cx);
        if clips.is_empty() {
            return empty("No clip selected", cx);
        }
        let ui = self.grade_ui(window, cx);
        self.clip_controls(ui, clips, div().into_any_element(), window, cx)
    }

    fn media_selection(&self, cx: &App) -> Vec<Clip> {
        self.selected_clips(cx)
            .into_iter()
            .filter(is_media)
            .collect()
    }

    fn clip_controls(
        &mut self,
        ui: Entity<GradeUi>,
        clips: Vec<Clip>,
        title: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let clip = clips[0].clone();
        let g = clip.grade.clone();
        let ids: Vec<ClipId> = clips.iter().map(|c| c.id).collect();
        ui.update(cx, |ui, _| ui.sync(Target::Clips(ids.clone()), g.clone()));
        let source = self
            .copied_grade
            .filter(|from| ids.iter().any(|id| id != from));
        let references = self.references(&ids, cx);
        let can_match = !references.is_empty()
            && self.state.read(cx).services.media.is_some()
            && !ui.read(cx).matching;
        let (reset_ui, match_ui, match_targets) = (ui.clone(), ui.clone(), ids.clone());
        let this = cx.entity();
        let actions = h_flex()
            .gap_1()
            .child(
                Button::new("grade-match")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Palette)
                    .tooltip("Match to…")
                    .disabled(!can_match)
                    .dropdown_menu(move |mut menu, _, _| {
                        for (id, name) in &references {
                            let (id, this) = (*id, this.clone());
                            let (ui, targets) = (match_ui.clone(), match_targets.clone());
                            menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                                move |_, _, cx| {
                                    let (ui, targets) = (ui.clone(), targets.clone());
                                    this.update(cx, |p, cx| p.match_to(id, targets, ui, cx));
                                },
                            ));
                        }
                        menu
                    }),
            )
            .child(
                Button::new("grade-copy")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Copy)
                    .tooltip("Copy grade")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.copied_grade = Some(clip.id);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("grade-paste")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Replace)
                    .tooltip("Paste grade")
                    .disabled(source.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(from) = source else { return };
                        let to: Vec<_> = ids.iter().copied().filter(|id| *id != from).collect();
                        this.run(vec![Command::CopyGrade { from, to }], "copy_grade", cx);
                    })),
            )
            .child(
                Button::new("grade-reset")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Undo2)
                    .tooltip("Reset")
                    .disabled(g == Grade::default())
                    .on_click(move |_, _, cx| {
                        reset_ui.update(cx, |ui, cx| ui.commit_grade(Grade::default(), cx))
                    }),
            );
        self.grade_controls(ui, title, actions, window, cx)
    }

    /// Wheels, sliders, LUT and curves for `ui`'s target.
    fn grade_controls(
        &mut self,
        ui: Entity<GradeUi>,
        title: impl IntoElement,
        actions: impl IntoElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sliders: Vec<Div> = ui.update(cx, |ui, cx| {
            PRIMARY
                .iter()
                .chain(&TONE)
                .map(|f| ui.slider_row(*f, window, cx))
                .collect()
        });
        let shown = ui.read(cx).shown().clone();
        let channel = ui.read(cx).channel;
        let (channel_ui, reset_ui) = (ui.clone(), ui.clone());
        let curve_tabs = h_flex()
            .justify_between()
            .child(
                TabBar::new("curve-channel")
                    .segmented()
                    .xsmall()
                    .selected_index(channel)
                    .children(curves::CHANNELS.iter().map(|c| Tab::new().label(*c)))
                    .on_click(move |ix, _, cx| {
                        channel_ui.update(cx, |ui, cx| {
                            ui.channel = *ix;
                            cx.notify();
                        })
                    }),
            )
            .child(
                Button::new("curve-reset")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Undo2)
                    .tooltip("Reset curve")
                    .disabled(curves::points(&shown, channel).is_empty())
                    .on_click(move |_, _, cx| {
                        reset_ui.update(cx, |ui, cx| {
                            let ch = ui.channel;
                            ui.commit(move |g| curves::set_points(g, ch, &[]), cx)
                        })
                    }),
            );
        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .h(px(24.))
                    .justify_between()
                    .child(title)
                    .child(actions),
            )
            .child(wheels::wheels(ui.clone(), cx))
            .child(
                v_flex()
                    .gap_0p5()
                    .children(sliders)
                    .child(self.lut_row(ui.clone(), &shown, cx)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(curve_tabs)
                    .child(curves::editor(ui, cx)),
            )
            .into_any_element()
    }

    /// The LUT file, picked from `.cube` files in `<home>/luts`.
    fn lut_row(&self, ui: Entity<GradeUi>, g: &Grade, cx: &App) -> Div {
        let current = g.lut.clone();
        let label = current
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "None".into());
        h_flex()
            .h(px(28.))
            .gap_2()
            .child(
                div()
                    .w(px(64.))
                    .flex_none()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("LUT"),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Button::new("grade-lut")
                        .outline()
                        .xsmall()
                        .w_full()
                        .label(label)
                        .dropdown_menu(move |menu, _, _| {
                            let pick = |lut: Option<PathBuf>, ui: Entity<GradeUi>| {
                                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                                    let lut = lut.clone();
                                    ui.update(cx, |ui, cx| {
                                        ui.commit(move |g| g.lut = lut.clone(), cx)
                                    })
                                }
                            };
                            let mut menu = menu.item(
                                PopupMenuItem::new("None")
                                    .checked(current.is_none())
                                    .on_click(pick(None, ui.clone())),
                            );
                            let files = koo_core::grade::list_luts();
                            if !files.is_empty() {
                                menu = menu.item(PopupMenuItem::separator());
                            }
                            for path in files {
                                let name = path
                                    .file_stem()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                menu = menu.item(
                                    PopupMenuItem::new(name)
                                        .checked(current.as_ref() == Some(&path))
                                        .on_click(pick(Some(path), ui.clone())),
                                );
                            }
                            menu
                        }),
                ),
            )
    }

    /// Media clips that can be matched to, named by file and start.
    fn references(&self, exclude: &[ClipId], cx: &App) -> Vec<(ClipId, SharedString)> {
        let project = self.project(cx);
        let mut out: Vec<(Time, ClipId, SharedString)> = Vec::new();
        for track in project
            .timeline
            .tracks
            .iter()
            .filter(|t| is_picture(t.kind))
        {
            for clip in track.clips.iter().filter(|c| !exclude.contains(&c.id)) {
                let Some(item) = media_item(project, clip) else {
                    continue;
                };
                let name = item
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let s = clip.start.as_secs_f64() as u64;
                let label = format!("{name}  {}:{:02}", s / 60, s % 60);
                out.push((clip.start, clip.id, label.into()));
            }
        }
        out.sort_by_key(|(t, _, _)| *t);
        out.into_iter().map(|(_, id, l)| (id, l)).collect()
    }

    /// Grade `targets` toward `reference`, measured on one frame of each
    /// (the playhead's when it is over the clip, else the middle).
    fn match_to(
        &mut self,
        reference: ClipId,
        targets: Vec<ClipId>,
        ui: Entity<GradeUi>,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.read(cx);
        let Some(media) = state.services.media.clone() else {
            return;
        };
        let project = state.bus.project().clone();
        let playhead = state.playhead;
        ui.update(cx, |ui, cx| {
            ui.matching = true;
            cx.notify();
        });
        let task = cx.background_executor().spawn(async move {
            let frame_of = |id: ClipId| {
                let (_, clip) = project.timeline.clip(id)?;
                let item = media_item(&project, clip)?;
                let at = if clip.range().contains(playhead) {
                    playhead
                } else {
                    clip.start + Time(clip.duration.0 / 2)
                };
                let frame = media.frame(item, clip.to_source(at)?, MATCH_WIDTH).ok()?;
                Some((clip.grade.clone(), frame))
            };
            let Some((ref_grade, ref_frame)) = frame_of(reference) else {
                return Vec::new();
            };
            targets
                .into_iter()
                .filter_map(|id| {
                    let (current, frame) = frame_of(id)?;
                    let grade =
                        koo_core::scopes::match_frames(&ref_frame, &ref_grade, &frame, &current);
                    Some(Command::SetGrade {
                        clips: vec![id],
                        grade,
                    })
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let commands = task.await;
            let _ = this.update(cx, |this, cx| {
                ui.update(cx, |ui, cx| {
                    ui.matching = false;
                    cx.notify();
                });
                this.run(commands, "match_grade", cx);
            });
        })
        .detach();
    }
}

fn is_picture(kind: TrackKind) -> bool {
    matches!(kind, TrackKind::Video | TrackKind::Overlay)
}

fn media_item<'a>(project: &'a Project, clip: &Clip) -> Option<&'a koo_core::model::MediaItem> {
    let ClipContent::Media { media, .. } = &clip.content else {
        return None;
    };
    project.media(*media).filter(|m| m.kind != MediaKind::Audio)
}

/// The topmost visible media clip at `t`.
fn clip_at(project: &Project, t: Time) -> Option<Clip> {
    project
        .timeline
        .tracks
        .iter()
        .filter(|tr| is_picture(tr.kind) && !tr.mix.mute)
        .filter_map(|tr| tr.clip_at(t))
        .find(|c| media_item(project, c).is_some())
        .cloned()
}

/// Set the grade field `key` names to `v`.
pub(super) fn set_grade_value(g: &mut Grade, key: Key, v: f32) {
    match key {
        Key::Exposure => g.exposure = v,
        Key::Contrast => g.contrast = v,
        Key::Saturation => g.saturation = v,
        Key::Gamma => g.gamma = v,
        Key::Temperature => g.temperature = v,
        Key::Tint => g.tint = v,
        Key::Lift(i) => g.lift[i] = v,
        Key::GammaRgb(i) => g.gamma_rgb[i] = v,
        Key::Gain(i) => g.gain[i] = v,
        _ => {}
    }
}
