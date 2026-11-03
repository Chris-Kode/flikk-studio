//! Color (PRD K1 and up): wheels, primaries, curves and a LUT for the clip
//! under the playhead (or the selection) or the whole timeline in the Color
//! workspace, for the selected clips in a clip's Color tab. "Match to…"
//! grades the clip toward a reference clip through `scopes::match_frames`.

mod curves;
mod editor;
mod fields;
mod wheels;

use super::{InspectorPanel, View, empty, group, is_media, label, select_field};
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, DropdownMenu as _, Icon, IconName, PanelHeader, PopupMenuItem, Segmented, h_flex,
    v_flex,
};
use crate::ui::library::shared;
use editor::{GradeUi, Layout, Target};
use fields::{PRIMARY, TONE, WHEELS};
use flikk_core::command::Command;
use flikk_core::library::Preset;
use flikk_core::model::{Clip, ClipContent, Grade, MediaKind, Project, TrackKind};
use flikk_core::{ClipId, LibraryId, Time};
use gpui_kit::*;
use std::path::PathBuf;

/// Source width of the frames shot matching measures.
const MATCH_WIDTH: u32 = 480;
/// Height of the curve editor.
const CURVE_HEIGHT: f32 = 120.;

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
    ) -> View {
        let ui = self.grade_ui(window, cx);
        let timeline = ui.read(cx).timeline_tab;
        let tab_ui = ui.clone();
        let target = h_flex()
            .gap(px(8.))
            .child(div().text_color(t::T1).child("Color"))
            .child(
                Segmented::new("grade-target")
                    .compact()
                    .items(["Clip", "Timeline"])
                    .selected(timeline as usize)
                    .on_click(move |ix, _, cx| {
                        tab_ui.update(cx, |ui, cx| {
                            ui.timeline_tab = *ix == 1;
                            cx.notify();
                        })
                    }),
            );
        let header = PanelHeader::new().leading(target);
        if timeline {
            let project = self.project(cx);
            let g = project.timeline.grade.clone();
            let name = project.name.clone();
            let count = project
                .timeline
                .tracks
                .iter()
                .filter(|t| is_picture(t.kind))
                .flat_map(|t| &t.clips)
                .filter(|c| media_item(project, c).is_some())
                .count();
            ui.update(cx, |ui, cx| ui.sync(Target::Timeline, g.clone(), cx));
            let count = div()
                .flex_none()
                .text_size(px(11.))
                .text_color(t::T3)
                .child(format!(
                    "{count} {}",
                    if count == 1 { "clip" } else { "clips" }
                ));
            return View {
                header: Some(header.child(reset_button("timeline-grade-reset", &ui, &g))),
                tabs: Some(subject(IconName::Layers, name, count)),
                body: self.grade_controls(ui, Layout::Workspace, window, cx),
            };
        }
        let mut clips = self.media_selection(cx);
        if clips.is_empty() {
            let state = self.state.read(cx);
            clips.extend(clip_at(state.bus.project(), state.playhead));
        }
        if clips.is_empty() {
            return View {
                header: Some(header),
                tabs: None,
                body: empty("No clip", cx),
            };
        }
        let name = match clips.len() {
            1 => super::clip::clip_name(&clips[0], self.project(cx)),
            n => format!("{n} clips"),
        };
        let (actions, matcher) = self.clip_actions(ui.clone(), &clips, cx);
        View {
            header: Some(header.children(actions)),
            tabs: Some(subject(IconName::Film, name, matcher)),
            body: self.clip_body(ui, clips, Layout::Workspace, window, cx),
        }
    }

    /// A clip's Color tab: the selected media clips.
    pub(super) fn color_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let clips = self.media_selection(cx);
        if clips.is_empty() {
            return empty("No clip selected", cx);
        }
        let ui = self.grade_ui(window, cx);
        let (actions, matcher) = self.clip_actions(ui.clone(), &clips, cx);
        let bar = h_flex()
            .flex_none()
            .h(px(36.))
            .gap(px(2.))
            .pl(px(10.))
            .pr(px(8.))
            .border_b_1()
            .border_color(t::LN)
            .child(matcher)
            .child(div().flex_1())
            .children(actions);
        v_flex()
            .child(bar)
            .child(self.clip_body(ui, clips, Layout::Tab, window, cx))
            .into_any_element()
    }

    fn media_selection(&self, cx: &App) -> Vec<Clip> {
        self.selected_clips(cx)
            .into_iter()
            .filter(is_media)
            .collect()
    }

    /// The HDR tone map, when the clips' media is HDR, over the grade.
    fn clip_body(
        &mut self,
        ui: Entity<GradeUi>,
        clips: Vec<Clip>,
        layout: Layout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ids: Vec<ClipId> = clips.iter().map(|c| c.id).collect();
        let g = clips[0].grade.clone();
        ui.update(cx, |ui, cx| ui.sync(Target::Clips(ids), g, cx));
        let tone_map = self.tone_map_section(&clips, window, cx);
        v_flex()
            .children(tone_map)
            .child(self.grade_controls(ui, layout, window, cx))
            .into_any_element()
    }

    /// Copy, paste, presets and reset for the clips' grade, and the
    /// "Match to…" button.
    fn clip_actions(
        &self,
        ui: Entity<GradeUi>,
        clips: &[Clip],
        cx: &mut Context<Self>,
    ) -> (Vec<AnyElement>, AnyElement) {
        let clip = clips[0].clone();
        let g = clip.grade.clone();
        let ids: Vec<ClipId> = clips.iter().map(|c| c.id).collect();
        let source = self
            .copied_grade
            .filter(|from| ids.iter().any(|id| id != from));
        let references = self.references(&ids, cx);
        let matching = ui.read(cx).matching;
        let can_match =
            !references.is_empty() && self.state.read(cx).services.media.is_some() && !matching;
        let presets = self.grade_presets(clip.id, ids.clone(), g.clone(), cx);
        let (match_ui, match_targets) = (ui.clone(), ids.clone());
        let this = cx.entity();
        let matcher = Button::new("grade-match")
            .small()
            .icon(IconName::Pipette)
            .label("Match to…")
            .loading(matching)
            .disabled(!can_match)
            .dropdown_menu(move |mut menu, _, _| {
                for (id, name) in &references {
                    let (id, this) = (*id, this.clone());
                    let (ui, targets) = (match_ui.clone(), match_targets.clone());
                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                        let (ui, targets) = (ui.clone(), targets.clone());
                        this.update(cx, |p, cx| p.match_to(id, targets, ui, cx));
                    }));
                }
                menu
            })
            .into_any_element();
        let actions = vec![
            icon_button("grade-copy", IconName::Copy, "Copy grade")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.copied_grade = Some(clip.id);
                    cx.notify();
                }))
                .into_any_element(),
            icon_button("grade-paste", IconName::ClipboardPaste, "Paste grade")
                .disabled(source.is_none())
                .on_click(cx.listener(move |this, _, _, cx| {
                    let Some(from) = source else { return };
                    let to: Vec<_> = ids.iter().copied().filter(|id| *id != from).collect();
                    this.run(vec![Command::CopyGrade { from, to }], "copy_grade", cx);
                }))
                .into_any_element(),
            presets.into_any_element(),
            reset_button("grade-reset", &ui, &g).into_any_element(),
        ];
        (actions, matcher)
    }

    /// Grade presets from the library to apply to `clips`, and saving
    /// `grade` as one.
    fn grade_presets(
        &self,
        clip: ClipId,
        clips: Vec<ClipId>,
        grade: Grade,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let presets: Vec<(LibraryId, String)> = self
            .state
            .read(cx)
            .library
            .presets
            .iter()
            .filter(|p| p.grade.is_some())
            .map(|p| (p.id, p.name.clone()))
            .collect();
        let state = self.state.clone();
        let neutral = grade == Grade::default();
        icon_button("grade-presets", IconName::Star, "Presets").dropdown_menu_with_anchor(
            Anchor::TopRight,
            move |mut menu, _, _| {
                for (id, name) in &presets {
                    let (state, id, clips) = (state.clone(), *id, clips.clone());
                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                        state.update(cx, |s, cx| {
                            if let Some(preset) = s.library.preset(id).cloned() {
                                shared::apply_preset_to(s, &preset, &clips, &[], cx);
                            }
                        })
                    }));
                }
                if !presets.is_empty() {
                    menu = menu.separator();
                }
                let (state, grade) = (state.clone(), grade.clone());
                menu.item(
                    PopupMenuItem::new("Save as Preset")
                        .disabled(neutral)
                        .on_click(move |_, _, cx| {
                            state.update(cx, |s, cx| {
                                let base = s
                                    .bus
                                    .project()
                                    .timeline
                                    .clip(clip)
                                    .and_then(|(_, c)| c.name.clone())
                                    .unwrap_or_else(|| "Grade".into());
                                let name = s.library.free_preset_name(&base);
                                let preset = Preset::grade(&name, grade.clone());
                                shared::save_preset(s, preset, cx);
                            })
                        }),
                )
            },
        )
    }

    /// Wheels, sliders, LUT and curves for `ui`'s target: titled, folding
    /// sections in the workspace, plain groups in a clip's tab.
    fn grade_controls(
        &mut self,
        ui: Entity<GradeUi>,
        layout: Layout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let shown = ui.read(cx).shown().clone();
        let sliders: Vec<Div> = ui.update(cx, |ui, cx| {
            PRIMARY
                .iter()
                .chain(&TONE)
                .map(|f| ui.slider_row(*f, layout, window, cx))
                .collect()
        });
        let wheels = wheels::wheels(ui.clone(), layout, cx);
        let channel = ui.read(cx).channel;
        let channel_ui = ui.clone();
        let channels = Segmented::new("curve-channel")
            .compact()
            .items(curves::CHANNELS)
            .selected(channel)
            .on_click(move |ix, _, cx| {
                channel_ui.update(cx, |ui, cx| {
                    ui.channel = *ix;
                    cx.notify();
                })
            });
        let curve_ui = ui.clone();
        let curve_reset = Button::new("curve-reset")
            .ghost()
            .xsmall()
            .icon(IconName::RotateCcw)
            .tooltip("Reset curve")
            .disabled(curves::points(&shown, channel).is_empty())
            .on_click(move |_, _, cx| {
                curve_ui.update(cx, |ui, cx| {
                    let ch = ui.channel;
                    ui.commit(move |g| curves::set_points(g, ch, &[]), cx)
                })
            });
        let curve = div()
            .flex_none()
            .pl(layout.inset())
            .pr(px(10.))
            .child(curves::editor(ui.clone(), CURVE_HEIGHT));
        let lut = lut_select(ui.clone(), &shown);

        if layout == Layout::Tab {
            let block = || v_flex().flex_none().border_b_1().border_color(t::LN);
            return v_flex()
                .child(block().child(wheels))
                .child(
                    block().py(px(6.)).children(sliders).child(
                        h_flex()
                            .h(px(28.))
                            .gap(px(8.))
                            .px(px(10.))
                            .child(label("LUT").w(px(64.)).flex_none())
                            .child(div().flex_1().min_w_0().child(lut)),
                    ),
                )
                .child(
                    v_flex()
                        .flex_none()
                        .gap(px(6.))
                        .pt(px(6.))
                        .pb(px(10.))
                        .child(
                            h_flex()
                                .h(px(22.))
                                .px(px(10.))
                                .child(channels)
                                .child(div().flex_1())
                                .child(curve_reset),
                        )
                        .child(curve),
                )
                .into_any_element();
        }

        let neutral_wheels = WHEELS.iter().all(|w| w.get(&shown) == [w.neutral(); 3]);
        let wheels_ui = ui.clone();
        let wheels_reset = Button::new("wheels-reset")
            .ghost()
            .xsmall()
            .icon(IconName::RotateCcw)
            .tooltip("Reset wheels")
            .disabled(neutral_wheels)
            .on_click(move |_, _, cx| {
                wheels_ui.update(cx, |ui, cx| {
                    ui.commit(
                        |g| {
                            for w in WHEELS {
                                w.set(g, [w.neutral(); 3]);
                            }
                        },
                        cx,
                    )
                })
            });
        let wheels = self.section(
            "grade-wheels",
            "Wheels",
            vec![wheels_reset.into_any_element()],
            vec![wheels.into_any_element()],
            cx,
        );
        let primaries = self.section(
            "grade-primaries",
            "Primaries",
            Vec::new(),
            sliders
                .into_iter()
                .map(IntoElement::into_any_element)
                .collect(),
            cx,
        );
        let curves = self.section(
            "grade-curves",
            "Curves",
            vec![channels.into_any_element(), curve_reset.into_any_element()],
            vec![curve.into_any_element()],
            cx,
        );
        let lut = div()
            .flex_none()
            .pl(layout.inset())
            .pr(px(10.))
            .child(lut)
            .into_any_element();
        let lut = self.section("grade-lut", "LUT", Vec::new(), vec![lut], cx);
        v_flex()
            .child(wheels)
            .child(primaries)
            .child(curves)
            .child(lut.border_b_0())
            .into_any_element()
    }

    /// A folding section of the workspace's grade: `actions` on its header,
    /// `rows` while it is open.
    fn section(
        &self,
        key: &'static str,
        title: &'static str,
        actions: Vec<AnyElement>,
        rows: Vec<AnyElement>,
        cx: &Context<Self>,
    ) -> Div {
        let rows = if self.is_open(key) { rows } else { Vec::new() };
        group(self.header(key, title, cx).children(actions), rows)
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
                        flikk_core::scopes::match_frames(&ref_frame, &ref_grade, &frame, &current);
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

/// A 22 px ghost icon button with its tooltip.
fn icon_button(id: &'static str, icon: IconName, tooltip: &'static str) -> Button {
    Button::new(id).ghost().small().icon(icon).tooltip(tooltip)
}

/// Back to the neutral grade.
fn reset_button(id: &'static str, ui: &Entity<GradeUi>, g: &Grade) -> Button {
    let ui = ui.clone();
    icon_button(id, IconName::RotateCcw, "Reset")
        .disabled(*g == Grade::default())
        .on_click(move |_, _, cx| ui.update(cx, |ui, cx| ui.commit_grade(Grade::default(), cx)))
}

/// What the workspace grades, under its header: an icon, the clip or
/// timeline name, and what goes on the right.
fn subject(
    icon: IconName,
    name: impl Into<SharedString>,
    trailing: impl IntoElement,
) -> AnyElement {
    h_flex()
        .flex_none()
        .h(px(36.))
        .gap(px(6.))
        .pl(px(10.))
        .pr(px(8.))
        .border_b_1()
        .border_color(t::LN)
        .child(Icon::new(icon).size(px(12.)).color(t::T3))
        .child(div().flex_1().min_w_0().truncate().child(name.into()))
        .child(trailing)
        .into_any_element()
}

/// The LUT file, picked from `.cube` files in `<home>/luts`.
fn lut_select(ui: Entity<GradeUi>, g: &Grade) -> impl IntoElement + use<> {
    let current = g.lut.clone();
    let label = current
        .as_ref()
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "None".into());
    select_field("grade-lut", label).dropdown_menu(move |menu, _, _| {
        let pick = |lut: Option<PathBuf>, ui: Entity<GradeUi>| {
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let lut = lut.clone();
                ui.update(cx, |ui, cx| ui.commit(move |g| g.lut = lut.clone(), cx))
            }
        };
        let mut menu = menu.item(
            PopupMenuItem::new("None")
                .checked(current.is_none())
                .on_click(pick(None, ui.clone())),
        );
        let files = flikk_core::grade::list_luts();
        if !files.is_empty() {
            menu = menu.separator();
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
    })
}

fn is_picture(kind: TrackKind) -> bool {
    matches!(kind, TrackKind::Video | TrackKind::Overlay)
}

fn media_item<'a>(project: &'a Project, clip: &Clip) -> Option<&'a flikk_core::model::MediaItem> {
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
