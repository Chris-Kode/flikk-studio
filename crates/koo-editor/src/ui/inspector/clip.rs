//! The selected clip's properties by kind. The header renames, labels,
//! enables and retimes it. Video, overlays and generators get transform and
//! effects (generators their program, video its color and linked audio),
//! audio gets gain, fades and its track's mix, captions and titles get their
//! text, timing and style.

use super::param::{Key, format_value};
use super::{InspectorPanel, section};
use crate::ui::caption_style::Target;
use crate::ui::library::{FxGlyph, fx_glyph};
use crate::ui::mixer::clip::ClipAudioFx;
use crate::ui::timeline::glyph::{Glyph, glyph};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::command::{Command, Edge};
use koo_core::model::{Clip, ClipColor, ClipContent, Project, Timeline, Track, TrackKind};
use koo_core::{ClipId, Time, TimeRange, TrackId};

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

    fn label(self) -> &'static str {
        match self {
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Caption => "Caption",
            Self::Title => "Title",
            Self::Overlay => "Overlay",
            Self::Generator => "Generator",
        }
    }
}

/// What the clip view needs of the clip's track.
#[derive(Clone)]
pub(super) struct ClipTrack {
    pub id: TrackId,
    pub kind: ClipKind,
}

/// The tabs of a picture clip; Color for media only, Audio only when it has
/// linked audio.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PictureTab {
    #[default]
    Video,
    Effects,
    Color,
    Audio,
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
    ) -> AnyElement {
        let header = self.clip_header(&track, &clip, window, cx);
        let body = match track.kind {
            ClipKind::Video | ClipKind::Overlay | ClipKind::Generator => {
                self.picture_view(&clip, window, cx)
            }
            ClipKind::Audio => self.clip_audio_view(&track, &clip, window, cx),
            ClipKind::Caption | ClipKind::Title => self.text_view(&track, &clip, window, cx),
        };
        v_flex()
            .gap_4()
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// Kind, name (double-click renames), color label and enable; then the
    /// clip's place on the timeline and, for media, its speed. Captions and
    /// titles edit their timing in the text view instead.
    fn clip_header(
        &mut self,
        track: &ClipTrack,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let project = self.project(cx);
        let name = clip
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| clip_name(clip, project));
        if self.renaming.is_some_and(|id| id != clip.id) {
            self.renaming = None;
        }
        let renaming = self.renaming == Some(clip.id);
        let theme = cx.theme();
        let (muted, secondary) = (theme.muted_foreground, theme.secondary);
        let id = clip.id;
        let title: AnyElement = if renaming {
            div()
                .flex_1()
                .min_w_0()
                .child(Input::new(&self.name_field).xsmall())
                .into_any_element()
        } else {
            let shown = name.clone();
            div()
                .id("clip-name")
                .flex_1()
                .min_w_0()
                .text_sm()
                .truncate()
                .when(!clip.enabled, |d| d.text_color(muted))
                .child(name)
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    if ev.click_count() >= 2 {
                        this.renaming = Some(id);
                        let shown = shown.clone();
                        this.name_field.update(cx, |f, cx| {
                            f.set_value(shown, window, cx);
                            f.focus(window, cx);
                        });
                        cx.notify();
                    }
                }))
                .into_any_element()
        };
        let enabled = clip.enabled;
        let head = h_flex()
            .gap_1()
            .child(
                div()
                    .flex_none()
                    .mr_1()
                    .px_1p5()
                    .rounded_sm()
                    .bg(secondary)
                    .text_xs()
                    .child(track.kind.label()),
            )
            .child(title)
            .child(self.color_label(clip, cx))
            .child(
                Button::new("clip-enabled")
                    .ghost()
                    .xsmall()
                    .tooltip(if enabled { "Disable" } else { "Enable" })
                    .child(glyph(
                        if enabled { Glyph::Eye } else { Glyph::EyeOff },
                        px(13.),
                        muted,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.run(
                            vec![Command::SetClipsEnabled {
                                clips: vec![id],
                                enabled: !enabled,
                            }],
                            "set_clips_enabled",
                            cx,
                        )
                    })),
            );
        let timing = match track.kind {
            ClipKind::Caption | ClipKind::Title => self.timing_fields(clip, window, cx),
            _ => h_flex()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(muted)
                        .child(format!(
                            "{} · {}",
                            clip.range(),
                            crate::ui::media_pool::clock(clip.duration)
                        )),
                )
                .children(
                    clip.media()
                        .is_some()
                        .then(|| self.speed_fields(clip, window, cx)),
                )
                .into_any_element(),
        };
        v_flex().gap_1p5().child(head).child(timing)
    }

    /// Clip color label: a dot that opens the palette.
    fn color_label(&self, clip: &Clip, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let current = clip.color;
        let id = clip.id;
        let panel = cx.entity();
        let dot = |c: Option<ClipColor>| {
            div().size(px(10.)).rounded_full().map(|d| match c {
                Some(c) => d.bg(rgb(c.rgb())),
                None => d.border_1().border_color(muted),
            })
        };
        Button::new("clip-color")
            .ghost()
            .xsmall()
            .tooltip("Color")
            .child(dot(current))
            .dropdown_menu(move |mut menu, _, _| {
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
                                .gap_2()
                                .child(
                                    div()
                                        .size(px(10.))
                                        .rounded_full()
                                        .border_1()
                                        .border_color(border)
                                        .bg(rgb(c.rgb())),
                                )
                                .child(c.label())
                        })
                        .checked(current == Some(c))
                        .on_click(set(Some(c))),
                    );
                }
                menu
            })
            .into_any_element()
    }

    /// Speed in percent, and whether changing it ripples later clips.
    fn speed_fields(&mut self, clip: &Clip, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let shown = format!("{}%", format_value((clip.speed * 100.0) as f32, 0));
        let editing = self
            .speed_field
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        if self.speed_clip != Some(clip.id)
            || (!editing && self.speed_field.read(cx).value() != shown)
        {
            self.speed_field
                .update(cx, |f, cx| f.set_value(shown, window, cx));
        }
        self.speed_clip = Some(clip.id);
        let muted = cx.theme().muted_foreground;
        h_flex()
            .flex_none()
            .gap_0p5()
            .child(
                div()
                    .w(px(56.))
                    .child(Input::new(&self.speed_field).xsmall()),
            )
            .child(
                Button::new("speed-ripple")
                    .ghost()
                    .xsmall()
                    .toggled(self.speed_ripple)
                    .tooltip("Ripple")
                    .child(fx_glyph(FxGlyph::Ripple, px(13.), muted))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.speed_ripple = !this.speed_ripple;
                        cx.notify();
                    })),
            )
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

    pub(super) fn commit_speed(&mut self, cx: &mut Context<Self>) {
        let Some(clip) = self.speed_clip else {
            return;
        };
        let typed = self.speed_field.read(cx).value();
        let Ok(percent) = typed.trim().trim_end_matches('%').trim().parse::<f64>() else {
            cx.notify();
            return;
        };
        let speed = percent / 100.0;
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
        self.speed_clip = None;
        cx.notify();
    }

    // ---------------------------------------------------------- picture

    fn picture_view(
        &mut self,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let media = matches!(clip.content, ClipContent::Media { .. });
        let sound = linked_audio(&self.project(cx).timeline, clip).map(|(track, audio)| {
            let info = ClipTrack {
                id: track.id,
                kind: ClipKind::Audio,
            };
            (info, audio.clone())
        });
        let mut tabs = vec![
            (PictureTab::Video, "Video"),
            (PictureTab::Effects, "Effects"),
        ];
        if media {
            tabs.push((PictureTab::Color, "Color"));
        }
        if sound.is_some() {
            tabs.push((PictureTab::Audio, "Audio"));
        }
        let tab = Some(self.picture_tab)
            .filter(|t| tabs.iter().any(|(tab, _)| tab == t))
            .unwrap_or(PictureTab::Video);
        let body = match (tab, sound) {
            (PictureTab::Color, _) => self.color_view(window, cx),
            (PictureTab::Audio, Some((track, audio))) => {
                self.clip_audio_view(&track, &audio, window, cx)
            }
            (PictureTab::Effects, _) => self.effects_view(clip, window, cx),
            _ => {
                let program = self.generator_view(clip, window, cx);
                let transform = self.transform_view(clip, false, window, cx);
                let stabilize = self.stabilize_view(clip, window, cx);
                v_flex()
                    .gap_4()
                    .children(program)
                    .children(stabilize)
                    .child(transform)
                    .into_any_element()
            }
        };
        let picked: Vec<PictureTab> = tabs.iter().map(|(t, _)| *t).collect();
        let tabs = TabBar::new("clip-tabs")
            .segmented()
            .xsmall()
            .selected_index(picked.iter().position(|t| *t == tab).unwrap_or(0))
            .children(tabs.into_iter().map(|(t, label)| {
                let count = clip.effects.len();
                match t {
                    PictureTab::Effects if count > 0 => {
                        Tab::new().label(format!("{label} {count}"))
                    }
                    _ => Tab::new().label(label),
                }
            }))
            .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                this.picture_tab = picked.get(*ix).copied().unwrap_or_default();
                cx.notify();
            }));
        v_flex().gap_3().child(tabs).child(body).into_any_element()
    }

    // ------------------------------------------------------------ audio

    fn clip_audio_view(
        &mut self,
        track: &ClipTrack,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Volume (with keyframes) and the effect stack, as in the Audio
        // workspace.
        let state = self.state.clone();
        let fx = window.use_keyed_state("edit-clip-audio-fx", cx, move |_, cx| {
            ClipAudioFx::new(state, cx)
        });
        let id = clip.id;
        fx.update(cx, |f, cx| f.set_clip(Some(id), cx));
        let rows = [
            (Key::FadeIn, clip.audio.fade_in.as_secs_f64() as f32),
            (Key::FadeOut, clip.audio.fade_out.as_secs_f64() as f32),
        ]
        .map(|(key, v)| self.param(key, v, window, cx).row(cx));
        let mix = self
            .project(cx)
            .timeline
            .track(track.id)
            .map(|t| t.mix.clone())
            .unwrap_or_default();
        let track_rows = [
            (Key::Volume(track.id), mix.volume_db),
            (Key::Pan(track.id), mix.pan),
        ]
        .map(|(key, v)| self.param(key, v, window, cx).row(cx));
        v_flex()
            .gap_4()
            .child(fx)
            .child(section("Fades", cx).children(rows))
            .child(section("Track", cx).children(track_rows))
            .into_any_element()
    }

    // ------------------------------------------------- captions, titles

    fn text_view(
        &mut self,
        track: &ClipTrack,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let transform = self.transform_view(clip, true, window, cx);
        let mut body = v_flex()
            .gap_4()
            .child(section("Text", cx).child(Textarea::new(&self.text)))
            .child(transform);
        if track.kind == ClipKind::Caption {
            let target = Target::Track(track.id);
            self.caption_style
                .update(cx, |p, cx| p.set_target(target, cx));
            body = body.child(self.caption_style.clone());
        }
        body.into_any_element()
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
    fn timing_fields(
        &mut self,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let muted = cx.theme().muted_foreground;
        h_flex()
            .gap_2()
            .child(div().flex_1().child(Input::new(&self.start_field).xsmall()))
            .child(div().text_xs().text_color(muted).child("–"))
            .child(div().flex_1().child(Input::new(&self.end_field).xsmall()))
            .into_any_element()
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

fn clip_name(clip: &Clip, project: &Project) -> String {
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
