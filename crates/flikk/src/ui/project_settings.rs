//! The sequence a project is cut in: its frame shape, resolution, frame
//! rate and audio rate. New project asks for them together with a name and
//! a folder; Project settings changes them later. Both keep them in the
//! project's `.flikk` file.

use crate::projects;
use crate::state::EditorState;
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, DialogExt as _, Icon, IconName, Input, Kbd, Segmented, Tooltip, h_flex, v_flex,
};
use crate::ui::media_pool::form;
use flikk_core::command::Command;
use flikk_core::model::{Format, SequenceSettings};
use flikk_core::time::Rate;
use flikk_store::project_file::{EXTENSION, sanitize};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};

/// Width and height the preview frame is fitted in.
const PREVIEW: (f32, f32) = (196., 132.);

/// Side of the box a format's outline is drawn in.
const GLYPH: f32 = 20.;

/// A new project's name and folder.
struct Place {
    name: Entity<InputState>,
    dir: PathBuf,
    _name: Subscription,
}

/// The form inside both dialogs.
pub struct ProjectForm {
    state: Entity<EditorState>,
    /// `None` when the form edits the open project.
    place: Option<Place>,
    settings: SequenceSettings,
}

impl ProjectForm {
    /// The file a new project would be saved as; `None` while unnamed.
    fn path(&self, cx: &App) -> Option<PathBuf> {
        let place = self.place.as_ref()?;
        let name = place.name.read(cx).value().trim().to_owned();
        (!name.is_empty()).then(|| place.dir.join(format!("{}.{EXTENSION}", sanitize(&name))))
    }

    /// Why Create can't go ahead.
    fn problem(&self, cx: &App) -> Option<String> {
        let path = self.path(cx)?;
        path.exists().then(|| {
            format!(
                "“{}” is already in this folder",
                path.file_name().unwrap_or_default().to_string_lossy()
            )
        })
    }

    fn ready(&self, cx: &App) -> bool {
        match &self.place {
            Some(_) => self.path(cx).is_some() && self.problem(cx).is_none(),
            None => self.settings != self.state.read(cx).bus.project().timeline.settings,
        }
    }

    fn set_size(&mut self, (width, height): (u32, u32), cx: &mut Context<Self>) {
        self.settings.width = width;
        self.settings.height = height;
        cx.notify();
    }

    fn choose_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            if let Some(dir) = paths.into_iter().next() {
                let _ = this.update(cx, |this, cx| {
                    if let Some(place) = &mut this.place {
                        place.dir = dir;
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    /// The frame at its shape, with its size and rates under it.
    fn preview(&self) -> impl IntoElement {
        let s = &self.settings;
        let (w, h) = (s.width.max(1) as f32, s.height.max(1) as f32);
        let k = (PREVIEW.0 / w).min(PREVIEW.1 / h);
        let (w, h) = ((w * k).round(), (h * k).round());
        let third = |vertical: bool, at: f32| {
            let line = div().absolute().bg(t::LN);
            if vertical {
                line.top_0().bottom_0().w(px(1.)).left(px((w * at).round()))
            } else {
                line.left_0().right_0().h(px(1.)).top(px((h * at).round()))
            }
        };
        v_flex()
            .flex_none()
            .w(px(PREVIEW.0 + 32.))
            .items_center()
            .justify_center()
            .gap(px(12.))
            .py(px(16.))
            .rounded(t::RADIUS_LG)
            .border_1()
            .border_color(t::LN)
            .bg(t::G)
            .child(
                h_flex().h(px(PREVIEW.1)).items_center().child(
                    div()
                        .relative()
                        .w(px(w))
                        .h(px(h))
                        .rounded(px(2.))
                        .border_1()
                        .border_color(t::T2)
                        .bg(t::P)
                        .child(third(true, 1. / 3.))
                        .child(third(true, 2. / 3.))
                        .child(third(false, 1. / 3.))
                        .child(third(false, 2. / 3.)),
                ),
            )
            .child(
                v_flex()
                    .items_center()
                    .gap(px(2.))
                    .font_family(t::mono())
                    .child(
                        div()
                            .text_size(px(12.))
                            .child(format!("{} × {}", s.width, s.height)),
                    )
                    .child(div().text_size(px(10.)).text_color(t::T3).child(format!(
                        "{} fps · {}",
                        fps(s.rate),
                        khz(s.sample_rate)
                    ))),
            )
    }

    fn format_tiles(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.settings.format();
        let short = self.settings.short_side();
        let tiles = Format::ALL.into_iter().enumerate().map(|(i, f)| {
            let on = current == Some(f);
            let (w, h) = glyph(f);
            h_flex()
                .id(("project-format", i))
                .h(px(40.))
                .gap(px(8.))
                .pl(px(6.))
                .pr(px(8.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(on, |d| d.bg(t::HV))
                .when(!on, |d| d.hover(|s| s.bg(t::alpha(t::HV, 0.6))))
                .child(
                    div()
                        .size(px(GLYPH + 4.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .w(px(w))
                                .h(px(h))
                                .rounded(px(2.))
                                .border_1()
                                .border_color(if on { t::T1 } else { t::T3 }),
                        ),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if on { t::T1 } else { t::T2 })
                                .child(f.name()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(10.))
                                .text_color(t::T3)
                                .child(f.plays),
                        ),
                )
                .tooltip(Tooltip::text(f.uses))
                .on_click(cx.listener(move |this, _, _, cx| this.set_size(f.size(short), cx)))
        });
        div()
            .grid()
            .grid_cols(3)
            .gap(px(2.))
            .p(px(2.))
            .rounded(t::RADIUS_LG)
            .border_1()
            .border_color(t::LN)
            .bg(t::G)
            .children(tiles)
    }

    fn place_rows(&self, cx: &mut Context<Self>) -> Option<Div> {
        let place = self.place.as_ref()?;
        let name = Input::new(&place.name).suffix(
            div()
                .font_family(t::mono())
                .text_size(px(11.))
                .child(format!(".{EXTENSION}")),
        );
        let location = Button::new("project-location")
            .w_full()
            .justify_start()
            // A child, not `.icon()`: an icon with no label makes a square
            // button, without side padding.
            .child(Icon::new(IconName::Folder).size(px(13.)))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(t::mono())
                    .text_size(px(11.))
                    .font_weight(FontWeight::NORMAL)
                    .child(tilde(&place.dir)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(t::T2)
                    .child("Change…"),
            )
            .on_click(cx.listener(|this, _, window, cx| this.choose_dir(window, cx)));
        Some(
            v_flex()
                .gap(px(8.))
                .pb(px(12.))
                .mb(px(4.))
                .border_b_1()
                .border_color(t::LN)
                .child(form::row("Name", name))
                .child(form::row("Location", location)),
        )
    }
}

impl Render for ProjectForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.settings.clone();
        let short = s.short_side();
        let resolutions = Format::RESOLUTIONS;
        let resolution = Segmented::new("project-resolution")
            .compact()
            .fill()
            .items(resolutions.map(|(_, name)| name))
            .selected(
                resolutions
                    .iter()
                    .position(|(p, _)| *p == short)
                    .unwrap_or(usize::MAX),
            )
            .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                if let Some((p, _)) = resolutions.get(*ix) {
                    let size = this.settings.scaled(*p);
                    this.set_size(size, cx);
                }
            }));
        let rates = Rate::SEQUENCE;
        let rate = Segmented::new("project-rate")
            .compact()
            .fill()
            .font_family(t::mono())
            .items(rates.map(|(_, name)| name))
            .selected(
                rates
                    .iter()
                    .position(|(r, _)| *r == s.rate)
                    .unwrap_or(usize::MAX),
            )
            .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                if let Some((r, _)) = rates.get(*ix) {
                    this.settings.rate = *r;
                    cx.notify();
                }
            }));
        let sample_rates = SequenceSettings::SAMPLE_RATES;
        let audio = Segmented::new("project-audio")
            .compact()
            .font_family(t::mono())
            .items(sample_rates.map(|(_, name)| name))
            .selected(
                sample_rates
                    .iter()
                    .position(|(r, _)| *r == s.sample_rate)
                    .unwrap_or(usize::MAX),
            )
            .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                if let Some((r, _)) = sample_rates.get(*ix) {
                    this.settings.sample_rate = *r;
                    cx.notify();
                }
            }));

        h_flex()
            .items_start()
            .gap(px(20.))
            .child(self.preview())
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(8.))
                    .children(self.place_rows(cx))
                    .child(
                        h_flex()
                            .items_start()
                            .gap(px(10.))
                            .child(
                                div()
                                    .w(px(96.))
                                    .flex_none()
                                    .pt(px(4.))
                                    .text_color(t::T2)
                                    .child("Format"),
                            )
                            .child(div().flex_1().min_w_0().child(self.format_tiles(cx))),
                    )
                    .child(form::row("Resolution", resolution))
                    .child(form::row("Frame rate", rate))
                    .child(form::row("Audio", h_flex().child(audio)))
                    .children(self.problem(cx).map(form::problem)),
            )
    }
}

/// New project: name, folder and sequence; Create settles the open
/// project's unsaved changes, then makes and saves the new one.
pub fn open_new(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) {
        return;
    }
    let dir = state
        .read(cx)
        .file
        .as_ref()
        .and_then(|f| f.parent())
        .map(PathBuf::from)
        .unwrap_or_else(crate::ui::deliver::default_dir);
    let name = untitled(&dir);
    let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
    input.update(cx, |i, cx| {
        i.focus(window, cx);
        i.select_all(window, cx);
    });
    let form = cx.new(|cx| ProjectForm {
        state: state.clone(),
        place: Some(Place {
            _name: cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()),
            name: input,
            dir,
        }),
        settings: SequenceSettings::default(),
    });
    let state = state.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let (form, state) = (form.clone(), state.clone());
        let ready = form.read(cx).ready(cx);
        dialog
            .title("New project")
            .w(px(760.))
            .pt(px(16.))
            .child(form.clone())
            .note(settings_note())
            .confirm("Create", move |window, cx| {
                let f = form.read(cx);
                let Some(path) = f.path(cx).filter(|_| f.ready(cx)) else {
                    return false;
                };
                let (state, settings) = (state.clone(), f.settings.clone());
                window.defer(cx, move |window, cx| {
                    projects::create(&state, path, settings, window, cx)
                });
                true
            })
            .confirm_disabled(!ready)
    });
}

/// Project settings: the open project's sequence, applied as one undoable
/// edit.
pub fn open_settings(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) {
        return;
    }
    let settings = state.read(cx).bus.project().timeline.settings.clone();
    let form = cx.new(|_| ProjectForm {
        state: state.clone(),
        place: None,
        settings,
    });
    let state = state.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let (form, state) = (form.clone(), state.clone());
        let ready = form.read(cx).ready(cx);
        dialog
            .title("Project settings")
            .w(px(760.))
            .pt(px(16.))
            .child(form.clone())
            .confirm("Apply", move |_, cx| {
                let after = form.read(cx).settings.clone();
                state.update(cx, |s, cx| {
                    let before = s.bus.project().timeline.settings.clone();
                    let mut commands = Vec::new();
                    if (before.width, before.height) != (after.width, after.height) {
                        commands.push(Command::SetFormat {
                            width: after.width,
                            height: after.height,
                        });
                    }
                    if (before.rate, before.sample_rate) != (after.rate, after.sample_rate) {
                        commands.push(Command::SetRates {
                            rate: after.rate,
                            sample_rate: after.sample_rate,
                        });
                    }
                    match commands.len() {
                        0 => {}
                        1 => {
                            s.execute(commands.remove(0), cx);
                        }
                        _ => {
                            s.execute_batch(commands, "set_sequence", cx);
                        }
                    }
                });
                true
            })
            .confirm_disabled(!ready)
    });
}

/// Where these settings live afterwards.
fn settings_note() -> impl IntoElement {
    h_flex()
        .gap(px(6.))
        .child(Kbd::new(format!("{} J", crate::app::MOD)))
        .child("Project settings")
}

/// "Untitled", or "Untitled 2" and on when `dir` already has one.
fn untitled(dir: &Path) -> String {
    (1..)
        .map(|n| match n {
            1 => "Untitled".to_owned(),
            n => format!("Untitled {n}"),
        })
        .find(|name| !dir.join(format!("{name}.{EXTENSION}")).exists())
        .unwrap_or_default()
}

/// `dir` with the home folder as `~`.
fn tilde(dir: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.as_deref().and_then(|h| dir.strip_prefix(h).ok()) {
        Some(rest) => Path::new("~").join(rest).display().to_string(),
        None => dir.display().to_string(),
    }
}

/// `23.976`, `25`.
fn fps(rate: Rate) -> String {
    Rate::SEQUENCE
        .iter()
        .find(|(r, _)| *r == rate)
        .map(|(_, name)| (*name).to_owned())
        .unwrap_or_else(|| {
            let fps = format!("{:.3}", rate.fps());
            fps.trim_end_matches('0').trim_end_matches('.').to_owned()
        })
}

/// `48 kHz`, `44.1 kHz`.
fn khz(rate: u32) -> String {
    let khz = format!("{:.1}", rate as f32 / 1000.);
    format!("{} kHz", khz.trim_end_matches(".0"))
}

/// A format's outline size: its shape, fitted in [`GLYPH`].
fn glyph(f: Format) -> (f32, f32) {
    let (a, b) = (f.aspect.0 as f32, f.aspect.1 as f32);
    if a >= b {
        (GLYPH, (GLYPH * b / a).max(4.))
    } else {
        ((GLYPH * a / b).max(4.), GLYPH)
    }
}
