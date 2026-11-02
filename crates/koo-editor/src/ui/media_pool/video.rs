//! Video: a shot from a description, made by the provider set in Settings →
//! Integrations, landing in the Video Generated bin. It can start on a
//! picture in the pool or the frame at the playhead (and end on another),
//! or follow references from the pool; a reference's chip puts its tag in
//! the prompt. Lengths, shapes and resolutions are the picked model's, and
//! the video runs as long as In/Out when they are set.

use super::{MediaPoolPanel, item_name};
use crate::state::EditorState;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::searchable_list::SearchableListItem;
use gpui_kit::component::select::{SearchableVec, Select, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, IndexPath, Sizable as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_agent::tools::video::FrameGrab;
use koo_core::cloud::{Capability, GenerateRequest, VideoModel, VideoReference, VideoSound};
use koo_core::model::{MediaItem, MediaKind};
use koo_core::{MediaId, Time};

/// The start or end frame left out.
const NONE: &str = "";
/// The program's frame at the playhead, grabbed when it is made.
const PLAYHEAD: &str = "playhead";
/// The model picks the shape.
const AUTO: &str = "auto";

/// One choice in a picker: what it shows, and its key.
#[derive(Clone)]
struct Choice {
    key: String,
    title: SharedString,
}

impl Choice {
    fn new(key: impl Into<String>, title: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
        }
    }
}

impl SearchableListItem for Choice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &String {
        &self.key
    }
}

type Picker = Entity<SelectState<SearchableVec<Choice>>>;

pub struct VideoForm {
    state: Entity<EditorState>,
    model: VideoModel,
    prompt: Entity<TextareaState>,
    start: Picker,
    end: Picker,
    references: Vec<MediaId>,
    length: Entity<SliderState>,
    aspect: Picker,
    resolution: Picker,
    audio: bool,
    _subscriptions: Vec<Subscription>,
}

impl VideoForm {
    fn new(
        state: Entity<EditorState>,
        model: VideoModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (min, max) = (model.shortest(), model.longest());
        let editor = state.read(cx);
        let in_out = editor
            .in_out
            .map(|r| r.duration().as_secs_f64().round() as f32);
        let settings = &editor.bus.project().timeline.settings;
        let shape = shape_of(settings.width, settings.height);
        let pictures: Vec<Choice> = editor
            .bus
            .project()
            .media
            .iter()
            .filter(|m| m.kind == MediaKind::Image)
            .map(|m| Choice::new(m.id.0.to_string(), item_name(m)))
            .collect();
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 8)
                .placeholder("Slow dolly in on a lighthouse at dusk, waves breaking below…")
        });
        let mut frames = |cx: &mut Context<Self>| {
            let mut rows = vec![
                Choice::new(NONE, "None"),
                Choice::new(PLAYHEAD, "Frame at playhead"),
            ];
            rows.extend(pictures.iter().cloned());
            picker(rows, 0, window, cx)
        };
        let start = frames(cx);
        let end = frames(cx);
        let length = cx.new(|_| {
            SliderState::new()
                .min(min as f32)
                .max(max as f32)
                .step(1.)
                .default_value(in_out.unwrap_or(5.).clamp(min as f32, max as f32))
        });
        let mut aspects = vec![Choice::new(AUTO, "Auto")];
        aspects.extend(model.aspects.iter().map(|a| Choice::new(*a, *a)));
        let aspect_ix = aspects.iter().position(|c| Some(c.key.as_str()) == shape);
        let aspect = picker(aspects, aspect_ix.unwrap_or(0), window, cx);
        let resolutions = model
            .resolutions
            .iter()
            .map(|r| Choice::new(*r, *r))
            .collect();
        let resolution = picker(resolutions, 0, window, cx);
        let _subscriptions = vec![
            cx.subscribe(&prompt, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&length, |_, _, _: &SliderEvent, cx| cx.notify()),
            cx.observe(&start, |_, _, cx| cx.notify()),
            cx.observe(&end, |_, _, cx| cx.notify()),
            cx.observe(&aspect, |_, _, cx| cx.notify()),
            cx.observe(&resolution, |_, _, cx| cx.notify()),
        ];
        let sound = state.read(cx).video_sound();
        Self {
            state,
            prompt,
            start,
            end,
            references: Vec::new(),
            length,
            aspect,
            resolution,
            audio: sound,
            model,
            _subscriptions,
        }
    }

    fn picked(picker: &Picker, cx: &App) -> String {
        picker
            .read(cx)
            .selected_value()
            .cloned()
            .unwrap_or_default()
    }

    /// The start frame, when there is one.
    fn starts(&self, cx: &App) -> bool {
        !Self::picked(&self.start, cx).is_empty()
    }

    /// The pool items taken as references, each with its tag, in order.
    fn reference_items(&self, cx: &App) -> Vec<(String, MediaItem)> {
        let project = self.state.read(cx).bus.project();
        let mut counts = [0usize; 3];
        self.references
            .iter()
            .filter_map(|id| project.media(*id))
            .map(|item| {
                let n = &mut counts[kind_index(item.kind)];
                *n += 1;
                (VideoReference::tag(item.kind, *n), item.clone())
            })
            .collect()
    }

    /// A picked frame as a path, with the grab that makes it.
    fn frame(&self, key: &str, cx: &App) -> (Option<std::path::PathBuf>, Option<FrameGrab>) {
        let state = self.state.read(cx);
        match key {
            NONE => (None, None),
            PLAYHEAD => {
                let grab = FrameGrab::new(state.playhead);
                (Some(grab.path.clone()), Some(grab))
            }
            id => {
                let path = state
                    .bus
                    .project()
                    .media
                    .iter()
                    .find(|m| m.id.0.to_string() == id)
                    .map(|m| m.path.clone());
                (path, None)
            }
        }
    }

    /// The request, and the frames to grab first; `None` without a prompt.
    fn request(&self, cx: &App) -> Option<(GenerateRequest, Vec<FrameGrab>)> {
        let prompt = self.prompt.read(cx).value().trim().to_owned();
        if prompt.is_empty() {
            return None;
        }
        let mut grabs = Vec::new();
        let starts = self.starts(cx);
        let (first_frame, grab) = self.frame(&Self::picked(&self.start, cx), cx);
        grabs.extend(grab);
        let (last_frame, grab) = if starts && self.model.last_frame {
            self.frame(&Self::picked(&self.end, cx), cx)
        } else {
            (None, None)
        };
        grabs.extend(grab);
        let references = if starts {
            Vec::new()
        } else {
            self.reference_items(cx)
                .into_iter()
                .map(|(_, item)| VideoReference {
                    kind: item.kind,
                    path: item.path.clone(),
                    length: item.info.as_ref().map(|i| i.duration),
                })
                .collect()
        };
        let aspect = Self::picked(&self.aspect, cx);
        let resolution = Self::picked(&self.resolution, cx);
        let request = GenerateRequest::Video {
            prompt,
            length: Some(Time::from_secs_f64(self.seconds(cx) as f64)),
            aspect: (aspect != AUTO && !aspect.is_empty()).then_some(aspect),
            resolution: (!resolution.is_empty()).then_some(resolution),
            first_frame,
            last_frame,
            references,
            audio: self.model.makes_sound(self.audio),
        };
        Some((request, grabs))
    }

    /// The length the slider is at, as the model makes it.
    fn seconds(&self, cx: &App) -> u32 {
        let at = self.length.read(cx).value().start() as f64;
        self.model.nearest_length(at)
    }

    /// Why the model can't make what the form asks, when it can't.
    fn problem(&self, cx: &App) -> Option<String> {
        let (request, _) = self.request(cx)?;
        self.model.check(&request).err().map(|e| e.to_string())
    }

    fn ready(&self, cx: &App) -> bool {
        self.request(cx).is_some() && self.problem(cx).is_none()
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            return;
        }
        if let Some((request, grabs)) = self.request(cx) {
            self.state
                .update(cx, |s, cx| s.generate_video(request, grabs, cx));
            window.close_dialog(cx);
        }
    }

    /// Put `tag` in the prompt at the cursor, set off from the word before.
    fn insert_tag(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt.update(cx, |prompt, cx| {
            let value = prompt.value();
            let before = value.get(..prompt.cursor()).unwrap_or(&value);
            let lead = if before.chars().last().is_some_and(|c| !c.is_whitespace()) {
                " "
            } else {
                ""
            };
            prompt.insert(format!("{lead}{tag} "), window, cx);
            prompt.focus(window, cx);
        });
        cx.notify();
    }

    /// The pool items that can still be added as references.
    fn addable(&self, cx: &App) -> Vec<MediaItem> {
        let Some(limits) = self.model.references else {
            return Vec::new();
        };
        let taken = self.reference_items(cx);
        let room = |kind| {
            let most = match kind {
                MediaKind::Image => limits.images,
                MediaKind::Video => limits.videos,
                MediaKind::Audio => limits.sounds,
            };
            taken.iter().filter(|(_, m)| m.kind == kind).count() < most
        };
        if taken.len() >= limits.files {
            return Vec::new();
        }
        self.state
            .read(cx)
            .bus
            .project()
            .media
            .iter()
            .filter(|m| !self.references.contains(&m.id) && room(m.kind))
            .cloned()
            .collect()
    }

    fn references_block(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let form = cx.entity();
        let addable = self.addable(cx);
        let chips: Vec<AnyElement> = self
            .reference_items(cx)
            .into_iter()
            .enumerate()
            .map(|(ix, (tag, item))| {
                let id = item.id;
                let insert: SharedString = tag.clone().into();
                h_flex()
                    .gap_1()
                    .pl_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .text_xs()
                    .child(
                        div()
                            .id(("reference-tag", ix))
                            .cursor_pointer()
                            .child(format!("{tag} · {}", item_name(&item)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.insert_tag(&insert, window, cx)
                            })),
                    )
                    .child(
                        Button::new(("reference-remove", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.references.retain(|r| *r != id);
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .justify_between()
                    .text_sm()
                    .child("References")
                    .child(
                        Button::new("reference-add")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .tooltip("Add")
                            .disabled(addable.is_empty())
                            .dropdown_menu(move |mut menu, _, _| {
                                for item in &addable {
                                    let (form, id) = (form.clone(), item.id);
                                    menu = menu.item(PopupMenuItem::new(item_name(item)).on_click(
                                        move |_, _, cx| {
                                            form.update(cx, |this, cx| {
                                                this.references.push(id);
                                                cx.notify();
                                            })
                                        },
                                    ));
                                }
                                menu
                            }),
                    ),
            )
            .when(!chips.is_empty(), |el| {
                el.child(h_flex().flex_wrap().gap_1().children(chips))
            })
    }
}

impl Render for VideoForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let seconds = self.seconds(cx);
        let starts = self.starts(cx);
        let row = |name: &'static str, picker: &Picker| {
            h_flex().justify_between().text_sm().child(name).child(
                div()
                    .w(px(220.))
                    .flex_none()
                    .child(Select::new(picker).small()),
            )
        };
        v_flex()
            .gap_4()
            .child(Textarea::new(&self.prompt).text_sm())
            .child(row("Start frame", &self.start))
            .when(starts && self.model.last_frame, |el| {
                el.child(row("End frame", &self.end))
            })
            .when(!starts && self.model.references.is_some(), |el| {
                el.child(self.references_block(cx))
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex().justify_between().text_sm().child("Length").child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("{seconds} s")),
                        ),
                    )
                    .child(Slider::new(&self.length)),
            )
            .when(!(starts && self.model.frame_sets_aspect), |el| {
                el.child(row("Aspect ratio", &self.aspect))
            })
            .when(!self.model.resolutions.is_empty(), |el| {
                el.child(row("Resolution", &self.resolution))
            })
            .when(self.model.audio == VideoSound::Switch, |el| {
                el.child(
                    h_flex().justify_between().text_sm().child("Sound").child(
                        Switch::new("video-audio")
                            .small()
                            .checked(self.audio)
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                this.audio = *on;
                                cx.notify();
                            })),
                    ),
                )
            })
            .when_some(self.problem(cx), |el, problem| {
                el.child(div().text_xs().text_color(theme.red).child(problem))
            })
    }
}

fn picker(rows: Vec<Choice>, selected: usize, window: &mut Window, cx: &mut App) -> Picker {
    let many = rows.len() > 8;
    cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(rows),
            Some(IndexPath::new(selected)),
            window,
            cx,
        )
        .searchable(many)
    })
}

/// The aspect ratio a `width` × `height` sequence has, when it is a common
/// one.
fn shape_of(width: u32, height: u32) -> Option<&'static str> {
    let ratio = width as f64 / height.max(1) as f64;
    [
        ("16:9", 16. / 9.),
        ("9:16", 9. / 16.),
        ("1:1", 1.),
        ("4:3", 4. / 3.),
        ("3:4", 3. / 4.),
        ("21:9", 21. / 9.),
    ]
    .into_iter()
    .find(|(_, r)| (ratio - r).abs() < 0.02)
    .map(|(name, _)| name)
}

fn kind_index(kind: MediaKind) -> usize {
    match kind {
        MediaKind::Image => 0,
        MediaKind::Video => 1,
        MediaKind::Audio => 2,
    }
}

impl MediaPoolPanel {
    /// What the model picked for video can make, when a provider is ready
    /// for it.
    pub(super) fn video_model(&self, cx: &App) -> Option<VideoModel> {
        let generation = self.state.read(cx).services.generation.clone()?;
        generation.provider_for(Capability::Video)?;
        generation.video_model()
    }

    pub(super) fn open_video(
        &mut self,
        model: VideoModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let form = cx.new(|cx| VideoForm::new(self.state.clone(), model, window, cx));
        window.open_dialog(cx, move |dialog, _, cx| {
            let submit = form.clone();
            let ready = form.read(cx).ready(cx);
            dialog
                .title(Capability::Video.label())
                .w(px(520.))
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("video-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("video-ok")
                                .primary()
                                .label("Generate")
                                .disabled(!ready)
                                .on_click(move |_, window, cx| {
                                    submit.update(cx, |f, cx| f.submit(window, cx))
                                }),
                        ),
                )
        });
    }
}
