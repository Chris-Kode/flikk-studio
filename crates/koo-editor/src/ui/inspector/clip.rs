//! Edit workspace: the selected clip's properties by kind. Video and
//! overlays get transform and color, audio gets gain, fades and its track's
//! mix, captions and titles get their text, timing and style.

use super::param::Key;
use super::{InspectorPanel, section};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use koo_core::command::{Command, Edge};
use koo_core::model::{Clip, ClipContent, Project, Track, TrackKind};
use koo_core::{ClipId, Time, TimeRange, TrackId};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ClipKind {
    Video,
    Audio,
    Caption,
    Title,
    Overlay,
}

impl ClipKind {
    pub(super) fn of(track: &Track, clip: &Clip) -> Self {
        match &clip.content {
            ClipContent::Caption { .. } => Self::Caption,
            ClipContent::Title { .. } => Self::Title,
            ClipContent::Overlay { .. } => Self::Overlay,
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
        }
    }
}

/// What the clip view needs of the clip's track.
#[derive(Clone)]
pub(super) struct ClipTrack {
    pub id: TrackId,
    pub kind: ClipKind,
    pub caption_style: Option<String>,
}

/// The two tabs of a picture clip.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum PictureTab {
    #[default]
    Video,
    Color,
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
                caption_style: track.caption_style.clone(),
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
            ClipKind::Video | ClipKind::Overlay => self.picture_view(&clip, window, cx),
            ClipKind::Audio => self.clip_audio_view(&track, &clip, window, cx),
            ClipKind::Caption | ClipKind::Title => self.text_view(&track, &clip, window, cx),
        };
        v_flex()
            .gap_4()
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// Name and kind, with the clip's place on the timeline. Captions and
    /// titles edit their timing in the text view instead.
    fn clip_header(
        &mut self,
        track: &ClipTrack,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let name = clip_name(clip, self.project(cx));
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let head = h_flex()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .px_1p5()
                    .rounded_sm()
                    .bg(theme.secondary)
                    .text_xs()
                    .child(track.kind.label()),
            )
            .child(div().flex_1().min_w_0().text_sm().truncate().child(name));
        let timing = match track.kind {
            ClipKind::Caption | ClipKind::Title => self.timing_fields(clip, window, cx),
            _ => div()
                .text_xs()
                .text_color(muted)
                .child(format!(
                    "{} · {}",
                    clip.range(),
                    crate::ui::media_pool::clock(clip.duration)
                ))
                .into_any_element(),
        };
        v_flex().gap_1p5().child(head).child(timing)
    }

    // ---------------------------------------------------------- picture

    fn picture_view(
        &mut self,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let media = matches!(clip.content, ClipContent::Media { .. });
        let tab = if media {
            self.picture_tab
        } else {
            PictureTab::Video
        };
        let body = match tab {
            PictureTab::Video => self.transform_view(clip, window, cx),
            PictureTab::Color => self.color_view(window, cx),
        };
        if !media {
            return body;
        }
        let tabs = TabBar::new("clip-tabs")
            .segmented()
            .xsmall()
            .selected_index(tab as usize)
            .child(Tab::new().label("Video"))
            .child(Tab::new().label("Color"))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.picture_tab = if *ix == 1 {
                    PictureTab::Color
                } else {
                    PictureTab::Video
                };
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
        let rows = [
            (Key::ClipGain, clip.audio.gain_db),
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
            .child(section("Clip", cx).children(rows))
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
        let mut body = v_flex()
            .gap_4()
            .child(section("Text", cx).child(Textarea::new(&self.text)));
        if track.kind == ClipKind::Caption {
            body = body.child(self.caption_style_picker(track, cx));
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

    /// The caption track's style preset.
    fn caption_style_picker(&self, track: &ClipTrack, cx: &mut Context<Self>) -> Div {
        let state = self.state.clone();
        let styles: Vec<String> = self
            .state
            .read(cx)
            .presets
            .caption_styles
            .iter()
            .map(|s| s.name.clone())
            .collect();
        let current = track
            .caption_style
            .clone()
            .or_else(|| styles.first().cloned())
            .unwrap_or_default();
        let track_id = track.id;
        let picked = current.clone();
        section("Style", cx).child(
            Button::new("caption-style")
                .outline()
                .small()
                .w_full()
                .label(current)
                .dropdown_caret(true)
                .dropdown_menu(move |mut menu, _, _| {
                    for (i, name) in styles.iter().enumerate() {
                        let state = state.clone();
                        let style = name.clone();
                        menu = menu.item(
                            PopupMenuItem::new(name.clone())
                                .checked(*name == picked)
                                .on_click(move |_, _, cx| {
                                    state.update(cx, |s, cx| {
                                        set_caption_style(s, track_id, i, style.clone(), cx)
                                    })
                                }),
                        );
                    }
                    menu
                }),
        )
    }
}

/// Style the caption track. The first caption track's style is also the
/// one new captions use.
fn set_caption_style(
    s: &mut crate::state::EditorState,
    track: TrackId,
    index: usize,
    style: String,
    cx: &mut Context<crate::state::EditorState>,
) {
    let first = s
        .bus
        .project()
        .timeline
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Caption)
        .map(|t| t.id);
    if first == Some(track) {
        s.pick_caption_style(index, cx);
    } else {
        s.execute(
            Command::SetCaptionStyle {
                track,
                style: Some(style),
            },
            cx,
        );
    }
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
    }
}
