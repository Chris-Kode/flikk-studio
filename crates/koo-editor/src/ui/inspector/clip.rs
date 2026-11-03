//! The selected clip's properties by kind. The header names the clip
//! (double-click renames) and its menu labels and enables it. Picture clips
//! (video, overlays, generators) page through Video, Audio, Speed, Color,
//! Effects and Masks; audio clips get their volume, effects, fades, track
//! mix and speed; captions and titles get their text, timing, transform and
//! style.

use super::param::Key;
use super::speed::SpeedRamp;
use super::{InspectorPanel, View, group, label, row, wide};
use crate::ui::caption_style::Target;
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, DropdownMenu as _, IconName, Input, NumberField, PanelHeader, PopupMenuItem,
    ScrubLabel, Switch, Tab, Tabs, Textarea, h_flex, v_flex,
};
use crate::ui::mixer::clip::ClipAudioFx;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::command::{Command, Edge};
use koo_core::model::{Clip, ClipColor, ClipContent, Project, Timeline, Track, TrackKind};
use koo_core::{ClipId, Time, TimeRange, TrackId};

/// Speeds one click away under the rate.
const SPEEDS: [(f64, &str); 4] = [(0.5, "0.5×"), (1., "1×"), (1.5, "1.5×"), (2., "2×")];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ClipKind {
    Video,
    Audio,
    Caption,
    Title,
    Overlay,
    Generator,
}

impl ClipKind {
    pub(super) fn of(track: &Track, clip: &Clip) -> Self {
        match &clip.content {
            ClipContent::Caption { .. } => Self::Caption,
            ClipContent::Title { .. } => Self::Title,
            ClipContent::Overlay { .. } => Self::Overlay,
            ClipContent::Generator { .. } => Self::Generator,
            ClipContent::Media { .. } if track.kind == TrackKind::Audio => Self::Audio,
            ClipContent::Media { .. } => Self::Video,
        }
    }
}

/// What the clip view needs of the clip's track.
#[derive(Clone)]
pub(super) struct ClipTrack {
    pub id: TrackId,
    pub kind: ClipKind,
}

/// The pages of a picture clip; Speed and Color for media only, Audio only
/// when it has linked audio.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PictureTab {
    #[default]
    Video,
    Audio,
    Speed,
    Color,
    Effects,
    Masks,
}

impl InspectorPanel {
    /// The first selected clip and its track.
    pub(super) fn primary_on_track(&self, cx: &App) -> Option<(ClipTrack, Clip)> {
        let state = self.state.read(cx);
        let tl = &state.bus.project().timeline;
        state.selection.clips.iter().find_map(|id| {
            let (track, clip) = tl.clip(*id)?;
            let info = ClipTrack {
                id: track.id,
                kind: ClipKind::of(track, clip),
            };
            Some((info, clip.clone()))
        })
    }

    pub(super) fn clip_view(
        &mut self,
        track: ClipTrack,
        clip: Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> View {
        let header = self.clip_header(&clip, cx);
        match track.kind {
            ClipKind::Video | ClipKind::Overlay | ClipKind::Generator => {
                let (tabs, body) = self.picture_view(&clip, window, cx);
                View {
                    header: Some(header),
                    tabs: Some(tabs),
                    body,
                }
            }
            ClipKind::Audio => {
                let body = self.clip_audio_view(&track, &clip, window, cx);
                View::new(header, body)
            }
            ClipKind::Caption | ClipKind::Title => {
                let body = self.text_view(&track, &clip, window, cx);
                View::new(header, body)
            }
        }
    }

    /// "Inspector", the clip's name (double-click renames it) and its menu.
    fn clip_header(&mut self, clip: &Clip, cx: &mut Context<Self>) -> PanelHeader {
        let project = self.project(cx);
        let name = clip
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| clip_name(clip, project));
        if self.renaming.is_some_and(|id| id != clip.id) {
            self.renaming = None;
        }
        let id = clip.id;
        let title: AnyElement = if self.renaming == Some(id) {
            div()
                .flex_1()
                .min_w_0()
                .child(Input::new(&self.name_field).small())
                .into_any_element()
        } else {
            div()
                .id("clip-name")
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(t::T3)
                .when(!clip.enabled, |d| d.line_through())
                .child(name.clone())
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    if ev.click_count() >= 2 {
                        this.start_rename(id, window, cx);
                    }
                }))
                .into_any_element()
        };
        PanelHeader::new()
            .leading(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(6.))
                    .child(div().flex_none().text_color(t::T1).child("Inspector"))
                    .child(title),
            )
            .child(self.clip_menu(clip, cx))
    }

    fn start_rename(&mut self, clip: ClipId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, c)) = self.project(cx).timeline.clip(clip) else {
            return;
        };
        let shown = c
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| clip_name(c, self.project(cx)));
        self.renaming = Some(clip);
        self.name_field.update(cx, |f, cx| {
            f.set_value(shown, window, cx);
            f.focus(window, cx);
        });
        cx.notify();
    }

    /// Rename, enable or disable, and the colour label.
    fn clip_menu(&self, clip: &Clip, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (id, enabled, current) = (clip.id, clip.enabled, clip.color);
        let panel = cx.entity();
        Button::new("clip-menu")
            .ghost()
            .xsmall()
            .icon(IconName::Ellipsis)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                let p = panel.clone();
                menu = menu
                    .item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
                        p.update(cx, |this, cx| this.start_rename(id, window, cx))
                    }))
                    .item({
                        let p = panel.clone();
                        PopupMenuItem::new(if enabled { "Disable" } else { "Enable" }).on_click(
                            move |_, _, cx| {
                                p.update(cx, |this, cx| {
                                    this.run(
                                        vec![Command::SetClipsEnabled {
                                            clips: vec![id],
                                            enabled: !enabled,
                                        }],
                                        "set_clips_enabled",
                                        cx,
                                    )
                                })
                            },
                        )
                    })
                    .separator()
                    .item(PopupMenuItem::label("Color"));
                let set = |color: Option<ClipColor>| {
                    let panel = panel.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        panel.update(cx, |this, cx| {
                            this.run(
                                vec![Command::SetClipColor {
                                    clips: vec![id],
                                    color,
                                }],
                                "set_clip_color",
                                cx,
                            )
                        })
                    }
                };
                menu = menu.item(
                    PopupMenuItem::new("None")
                        .checked(current.is_none())
                        .on_click(set(None)),
                );
                for c in ClipColor::ALL {
                    menu = menu.item(
                        PopupMenuItem::element(move |_, _| {
                            h_flex()
                                .gap(px(8.))
                                .child(div().size(px(10.)).rounded_full().bg(rgb(c.rgb())))
                                .child(c.label())
                        })
                        .checked(current == Some(c))
                        .on_click(set(Some(c))),
                    );
                }
                menu
            })
    }

    pub(super) fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(clip) = self.renaming.take() else {
            return;
        };
        let typed = self.name_field.read(cx).value().trim().to_owned();
        let Some((_, c)) = self.project(cx).timeline.clip(clip) else {
            return;
        };
        let source = clip_name(c, self.project(cx));
        let name = (!typed.is_empty() && typed != source).then_some(typed);
        if name != c.name {
            self.run(vec![Command::RenameClip { clip, name }], "rename_clip", cx);
        } else {
            cx.notify();
        }
    }

    /// The clip's rate with shortcuts, and whether changing it ripples
    /// later clips.
    fn speed_group(&mut self, clip: &Clip, cx: &mut Context<Self>) -> Div {
        self.speed_clip = Some(clip.id);
        let speed = clip.speed;
        self.speed_field
            .update(cx, |f, cx| f.set_value(speed * 100., cx));
        let rows = if self.is_open("speed") {
            let chips = SPEEDS.map(|(s, name)| {
                Button::new(name)
                    .xsmall()
                    .label(name)
                    .font_family(t::mono())
                    .selected((speed - s).abs() < 1e-6)
                    .accent((speed - s).abs() < 1e-6)
                    .on_click(cx.listener(move |this, _, _, cx| this.commit_speed(s, cx)))
            });
            vec![
                row(
                    ScrubLabel::new(&self.speed_field, "Rate"),
                    NumberField::new(&self.speed_field).unit("%"),
                    None,
                ),
                h_flex()
                    .flex_none()
                    .gap(px(4.))
                    .pl(px(104.))
                    .pb(px(4.))
                    .children(chips),
                row(
                    label("Ripple"),
                    Switch::new("speed-ripple")
                        .checked(self.speed_ripple)
                        .tooltip("Move later clips with the change")
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.speed_ripple = *on;
                            cx.notify();
                        })),
                    None,
                ),
            ]
        } else {
            Vec::new()
        };
        group(self.header("speed", "Speed", cx), rows)
    }

    /// Set the shown clip's speed (a fraction, 1 = normal).
    pub(super) fn commit_speed(&mut self, speed: f64, cx: &mut Context<Self>) {
        let Some(clip) = self.speed_clip else {
            return;
        };
        let current = self.project(cx).timeline.clip(clip).map(|(_, c)| c.speed);
        if current.is_some_and(|c| (c - speed).abs() > 1e-6) {
            let ripple = self.speed_ripple;
            self.run(
                vec![Command::SetSpeed {
                    clip,
                    speed,
                    ripple,
                }],
                "set_speed",
                cx,
            );
        }
        // Show the stored (clamped or refused) speed again.
        cx.notify();
    }

    // ---------------------------------------------------------- picture

    /// The page tabs and the selected page.
    fn picture_view(
        &mut self,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (AnyElement, AnyElement) {
        let media = matches!(clip.content, ClipContent::Media { .. });
        let sound = linked_audio(&self.project(cx).timeline, clip).map(|(track, audio)| {
            let info = ClipTrack {
                id: track.id,
                kind: ClipKind::Audio,
            };
            (info, audio.clone())
        });
        let mut tabs = vec![(PictureTab::Video, "Video", 0)];
        if sound.is_some() {
            tabs.push((PictureTab::Audio, "Audio", 0));
        }
        if media {
            tabs.push((PictureTab::Speed, "Speed", 0));
            tabs.push((PictureTab::Color, "Color", 0));
        }
        tabs.push((PictureTab::Effects, "Effects", clip.effects.len()));
        tabs.push((PictureTab::Masks, "Masks", clip.masks.len()));
        let tab = Some(self.picture_tab)
            .filter(|t| tabs.iter().any(|(tab, _, _)| tab == t))
            .unwrap_or(PictureTab::Video);
        let linked = sound.as_ref().map(|(_, a)| a.id);
        let body = match (tab, sound) {
            (PictureTab::Color, _) => div()
                .p(px(12.))
                .child(self.color_view(window, cx))
                .into_any_element(),
            (PictureTab::Speed, _) => {
                let speed = self.speed_group(clip, cx);
                let state = self.state.clone();
                let ramp = window.use_keyed_state("clip-speed-ramp", cx, move |_, cx| {
                    SpeedRamp::new(state, cx)
                });
                let (id, ripple) = (clip.id, self.speed_ripple);
                ramp.update(cx, |r, _| r.set_clip(id, linked, ripple));
                v_flex().child(speed).child(ramp).into_any_element()
            }
            (PictureTab::Audio, Some((track, audio))) => self
                .clip_audio_view(&track, &audio, window, cx)
                .into_any_element(),
            (PictureTab::Effects, _) => self.effects_view(clip, window, cx).into_any_element(),
            (PictureTab::Masks, _) => self.masks_view(clip, window, cx).into_any_element(),
            _ => {
                let program = self.generator_view(clip, window, cx);
                let transform = self.transform_view(clip, false, window, cx);
                let stabilize = self.stabilize_view(clip, window, cx);
                let effects = self.effects_summary(clip, cx);
                v_flex()
                    .children(program)
                    .children(transform)
                    .children(stabilize)
                    .child(effects)
                    .into_any_element()
            }
        };
        let picked: Vec<PictureTab> = tabs.iter().map(|(t, _, _)| *t).collect();
        let tabs = Tabs::new("clip-tabs")
            .selected(picked.iter().position(|t| *t == tab).unwrap_or(0))
            .children(
                tabs.into_iter()
                    .map(|(_, label, count)| Tab::new(label).count(count)),
            )
            .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                this.picture_tab = picked.get(*ix).copied().unwrap_or_default();
                cx.notify();
            }));
        (tabs.into_any_element(), body)
    }

    // ------------------------------------------------------------ audio

    /// Volume (with keyframes) and the effect stack, as in the Audio
    /// workspace; then fades, the track's mix and, for an audio clip, its
    /// speed.
    fn clip_audio_view(
        &mut self,
        track: &ClipTrack,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = self.state.clone();
        let fx = window.use_keyed_state("edit-clip-audio-fx", cx, move |_, cx| {
            ClipAudioFx::new(state, cx)
        });
        let id = clip.id;
        fx.update(cx, |f, cx| f.set_clip(Some(id), cx));
        let fades = if self.is_open("fades") {
            [
                (Key::FadeIn, clip.audio.fade_in.as_secs_f64() as f32),
                (Key::FadeOut, clip.audio.fade_out.as_secs_f64() as f32),
            ]
            .map(|(key, v)| self.param_row(key, v, false, None, window, cx))
            .into()
        } else {
            Vec::new()
        };
        let (mix, name) = self
            .project(cx)
            .timeline
            .track(track.id)
            .map(|t| (t.mix.clone(), t.name.clone()))
            .unwrap_or_default();
        let track_rows = if self.is_open("track") {
            [
                (Key::Volume(track.id), mix.volume_db),
                (Key::Pan(track.id), mix.pan),
            ]
            .map(|(key, v)| self.param_row(key, v, false, None, window, cx))
            .into()
        } else {
            Vec::new()
        };
        let track_header = self.header("track", "Track", cx).child(
            div()
                .font_weight(FontWeight::NORMAL)
                .text_size(px(11.))
                .text_color(t::T3)
                .child(name),
        );
        let speed = (track.kind == ClipKind::Audio && clip.media().is_some())
            .then(|| self.speed_group(clip, cx));
        v_flex()
            .child(fx)
            .child(group(self.header("fades", "Fades", cx), fades))
            .child(group(track_header, track_rows))
            .children(speed)
    }

    // ------------------------------------------------- captions, titles

    fn text_view(
        &mut self,
        track: &ClipTrack,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let text = match &clip.content {
            ClipContent::Caption { text, .. } | ClipContent::Title { text, .. } => text.clone(),
            _ => String::new(),
        };
        // Another caption was picked before the field's blur committed it.
        if let Some(old) = self.text_clip.filter(|old| *old != clip.id)
            && let Some(typed) = self.typed_text(old, cx)
        {
            self.run_later(
                Command::EditCaption {
                    clip: old,
                    text: Some(typed),
                    range: None,
                },
                cx,
            );
        }
        let editing = self.text.read(cx).focus_handle(cx).is_focused(window);
        if self.text_clip != Some(clip.id) || (!editing && self.text.read(cx).value() != text) {
            self.text_clip = Some(clip.id);
            self.text.update(cx, |t, cx| t.set_value(text, window, cx));
        }
        let text_rows = if self.is_open("text") {
            vec![wide(Textarea::new(&self.text).small())]
        } else {
            Vec::new()
        };
        let timing = self.timing_group(clip, window, cx);
        let transform = self.transform_view(clip, true, window, cx);
        let mut body = v_flex()
            .child(group(self.header("text", "Text", cx), text_rows))
            .child(timing)
            .children(transform);
        if track.kind == ClipKind::Caption {
            let target = Target::Track(track.id);
            self.caption_style
                .update(cx, |p, cx| p.set_target(target, cx));
            body = body.child(self.caption_style.clone());
        }
        body
    }

    /// Commit the text field to the clip it was loaded from.
    pub(super) fn commit_text(&mut self, cx: &mut Context<Self>) {
        let Some(clip) = self.text_clip else {
            return;
        };
        if let Some(text) = self.typed_text(clip, cx) {
            self.run(
                vec![Command::EditCaption {
                    clip,
                    text: Some(text),
                    range: None,
                }],
                "edit_caption",
                cx,
            );
        }
    }

    /// The text field, if it holds a change to `clip`'s text.
    fn typed_text(&self, clip: ClipId, cx: &App) -> Option<String> {
        let text = self.text.read(cx).value().trim().to_owned();
        let current = clip_text(self.project(cx), clip)?;
        (!text.is_empty() && current != text).then_some(text)
    }

    /// Apply an edit after this render, for one the selection overtook.
    fn run_later(&self, command: Command, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = this.update(cx, |this, cx| this.run(vec![command], "edit_caption", cx));
        });
    }

    /// Start and end as `M:SS.mmm` fields.
    fn timing_group(&mut self, clip: &Clip, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if let Some(old) = self.timing_clip.filter(|old| *old != clip.id) {
            for edge in [Edge::Start, Edge::End] {
                if let Some(range) = self.typed_range(old, edge, cx) {
                    self.run_later(
                        Command::EditCaption {
                            clip: old,
                            text: None,
                            range: Some(range),
                        },
                        cx,
                    );
                }
            }
        }
        for (field, t) in [
            (&self.start_field, clip.start),
            (&self.end_field, clip.end()),
        ] {
            let shown = t.to_string();
            let editing = field.read(cx).focus_handle(cx).is_focused(window);
            if self.timing_clip != Some(clip.id) || (!editing && field.read(cx).value() != shown) {
                field.update(cx, |f, cx| f.set_value(shown, window, cx));
            }
        }
        self.timing_clip = Some(clip.id);
        let rows = if self.is_open("timing") {
            vec![
                row(
                    label("Start"),
                    Input::new(&self.start_field).small().font_family(t::mono()),
                    None,
                ),
                row(
                    label("End"),
                    Input::new(&self.end_field).small().font_family(t::mono()),
                    None,
                ),
            ]
        } else {
            Vec::new()
        };
        group(self.header("timing", "Timing", cx), rows)
    }

    /// Commit an edited start or end; anything unparsable or inverted is
    /// put back on the next render.
    pub(super) fn commit_timing(&mut self, edge: Edge, cx: &mut Context<Self>) {
        let Some(clip) = self.timing_clip else {
            return;
        };
        match self.typed_range(clip, edge, cx) {
            Some(range) => self.run(
                vec![Command::EditCaption {
                    clip,
                    text: None,
                    range: Some(range),
                }],
                "edit_caption",
                cx,
            ),
            None => cx.notify(),
        }
    }

    /// `clip`'s range with the `edge` field applied, if that is a valid
    /// change.
    fn typed_range(&self, clip: ClipId, edge: Edge, cx: &App) -> Option<TimeRange> {
        let range = self.project(cx).timeline.clip(clip)?.1.range();
        let field = match edge {
            Edge::Start => &self.start_field,
            Edge::End => &self.end_field,
        };
        let typed = field.read(cx).value();
        let shown = match edge {
            Edge::Start => range.start,
            Edge::End => range.end,
        };
        // The field shows milliseconds; untouched, it must not round the clip.
        if typed.trim() == shown.to_string() {
            return None;
        }
        let t = Time::parse_clock(&typed)?;
        let next = match edge {
            Edge::Start => TimeRange::new(t.max(Time::ZERO), range.end),
            Edge::End => TimeRange::new(range.start, t),
        };
        (next != range && next.end > next.start).then_some(next)
    }
}

/// The audio clip dropped with a video clip: same source, same source
/// offset, on an audio track; the one overlapping it most. Survives blades
/// and trims, since both halves keep their offset.
pub(super) fn linked_audio<'a>(tl: &'a Timeline, clip: &Clip) -> Option<(&'a Track, &'a Clip)> {
    let ClipContent::Media {
        media, source_in, ..
    } = &clip.content
    else {
        return None;
    };
    let offset = *source_in - clip.start;
    let range = clip.range();
    tl.tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
        .filter(|(_, c)| match &c.content {
            ClipContent::Media {
                media: m,
                source_in: s,
                ..
            } => m == media && *s - c.start == offset && c.range().overlaps(&range),
            _ => false,
        })
        .max_by_key(|(_, c)| c.end().min(range.end) - c.start.max(range.start))
}

/// Where a media clip's sound lives: itself on an audio track, else its
/// linked audio clip when it has one.
pub(crate) fn sound_clip<'a>(tl: &'a Timeline, clip: &'a Clip) -> &'a Clip {
    let on_audio = tl
        .clip(clip.id)
        .is_some_and(|(t, _)| t.kind == TrackKind::Audio);
    if on_audio {
        return clip;
    }
    linked_audio(tl, clip).map_or(clip, |(_, c)| c)
}

fn clip_text(project: &Project, clip: ClipId) -> Option<String> {
    match &project.timeline.clip(clip)?.1.content {
        ClipContent::Caption { text, .. } | ClipContent::Title { text, .. } => Some(text.clone()),
        _ => None,
    }
}

pub(super) fn clip_name(clip: &Clip, project: &Project) -> String {
    match &clip.content {
        ClipContent::Media { media, .. } => project
            .media(*media)
            .map(crate::ui::media_pool::item_name)
            .unwrap_or_default(),
        ClipContent::Title { template, .. } => template.clone(),
        ClipContent::Caption { text, .. } => text.lines().next().unwrap_or_default().to_owned(),
        ClipContent::Overlay { asset, .. } => asset
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        ClipContent::Generator { .. } => clip.display_name(project),
    }
}
