//! Image: a picture from a description, made by the provider set in
//! Settings → Integrations, landing in the Images Generated bin. It can
//! edit or follow pictures in the pool and the frame at the playhead; a
//! picture's chip puts its tag in the prompt. Shapes, resolutions and how
//! many pictures it follows are the picked model's.

use super::video::{AUTO, Choice, Picker, picked, picker, shape_of};
use super::{MediaPoolPanel, form};
use crate::state::EditorState;
use crate::ui::format::item_name;
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, DialogExt as _, DropdownMenu as _, IconName, PopupMenuItem, Select, SelectEvent,
    Switch, Textarea, h_flex, v_flex,
};
use flikk_agent::tools::video::FrameGrab;
use flikk_core::MediaId;
use flikk_core::cloud::{Capability, GenerateRequest, ImageModel};
use flikk_core::model::MediaKind;
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// A picture the new one edits or follows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Reference {
    Media(MediaId),
    /// The program's frame at the playhead, grabbed when it is made.
    Playhead,
}

pub struct ImageForm {
    state: Entity<EditorState>,
    model: ImageModel,
    prompt: Entity<TextareaState>,
    references: Vec<Reference>,
    aspect: Picker,
    resolution: Picker,
    transparent: bool,
    _subscriptions: Vec<Subscription>,
}

impl ImageForm {
    fn new(
        state: Entity<EditorState>,
        model: ImageModel,
        first: Option<MediaId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = &state.read(cx).bus.project().timeline.settings;
        let shape = shape_of(settings.width, settings.height);
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 8)
                .placeholder("Neon-lit ramen bar on a rainy night, shot on 35 mm…")
        });
        let mut aspects = Vec::new();
        if model.auto_aspect {
            aspects.push(Choice::new(AUTO, "Auto"));
        }
        aspects.extend(model.aspects.iter().map(|a| Choice::new(*a, *a)));
        // The sequence's shape, else the model's choice, else its first.
        let aspect_ix = aspects
            .iter()
            .position(|c| Some(c.key.as_str()) == shape)
            .unwrap_or(0);
        let aspect = picker(aspects, aspect_ix, window, cx);
        let resolutions = model
            .resolutions
            .iter()
            .map(|r| Choice::new(*r, *r))
            .collect();
        let resolution = picker(resolutions, 0, window, cx);
        let _subscriptions = vec![
            cx.subscribe_in(&prompt, window, |_, prompt, _: &InputEvent, window, cx| {
                form::mark_tags(prompt, form::Tags::References, window, cx);
                cx.notify();
            }),
            cx.subscribe(&aspect, |_, _, _: &SelectEvent<Choice>, cx| cx.notify()),
            cx.subscribe(&resolution, |_, _, _: &SelectEvent<Choice>, cx| cx.notify()),
        ];
        let references = first
            .filter(|_| model.references > 0)
            .map(Reference::Media)
            .into_iter()
            .collect();
        Self {
            state,
            model,
            prompt,
            references,
            aspect,
            resolution,
            transparent: false,
            _subscriptions,
        }
    }

    /// The name a reference goes by in the form.
    fn reference_name(&self, reference: Reference, cx: &App) -> Option<String> {
        match reference {
            Reference::Playhead => Some("Frame at playhead".into()),
            Reference::Media(id) => self.state.read(cx).bus.project().media(id).map(item_name),
        }
    }

    /// The request, and the frames to grab first; `None` without a prompt.
    fn request(&self, cx: &App) -> Option<(GenerateRequest, Vec<FrameGrab>)> {
        let prompt = self.prompt.read(cx).value().trim().to_owned();
        if prompt.is_empty() {
            return None;
        }
        let state = self.state.read(cx);
        let mut grabs = Vec::new();
        let mut references = Vec::new();
        for reference in &self.references {
            match reference {
                Reference::Media(id) => {
                    if let Some(item) = state.bus.project().media(*id) {
                        references.push(item.path.clone());
                    }
                }
                Reference::Playhead => {
                    let grab = FrameGrab::new(state.playhead);
                    references.push(grab.path.clone());
                    grabs.push(grab);
                }
            }
        }
        let aspect = picked(&self.aspect, cx);
        let resolution = picked(&self.resolution, cx);
        let request = GenerateRequest::Image {
            prompt,
            aspect: (aspect != AUTO && !aspect.is_empty()).then_some(aspect),
            resolution: (!resolution.is_empty()).then_some(resolution),
            references,
            transparent: self.model.transparent && self.transparent,
        };
        Some((request, grabs))
    }

    /// Why the model can't make what the form asks, when it can't.
    fn problem(&self, cx: &App) -> Option<String> {
        let (request, _) = self.request(cx)?;
        self.model.check(&request).err().map(|e| e.to_string())
    }

    fn ready(&self, cx: &App) -> bool {
        self.request(cx).is_some() && self.problem(cx).is_none()
    }

    /// Start it; `true` once it is on its way.
    fn submit(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.ready(cx) {
            return false;
        }
        let Some((request, grabs)) = self.request(cx) else {
            return false;
        };
        self.state
            .update(cx, |s, cx| s.generate_with_frames(request, grabs, cx));
        true
    }

    /// Put `tag` in the prompt at the cursor, set off from the word before.
    fn insert_tag(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        form::insert_tag(&self.prompt, tag, true, window, cx);
        cx.notify();
    }

    /// What can still be added: pictures in the pool not taken yet, and the
    /// frame at the playhead; nothing once the model follows no more.
    fn addable(&self, cx: &App) -> Vec<(Reference, String)> {
        if self.references.len() >= self.model.references {
            return Vec::new();
        }
        let mut rows = Vec::new();
        if !self.references.contains(&Reference::Playhead) {
            rows.push((Reference::Playhead, "Frame at playhead".to_owned()));
        }
        let project = self.state.read(cx).bus.project();
        rows.extend(
            project
                .media
                .iter()
                .filter(|m| m.kind == MediaKind::Image)
                .filter(|m| !self.references.contains(&Reference::Media(m.id)))
                .map(|m| (Reference::Media(m.id), item_name(m))),
        );
        rows
    }

    /// The pictures taken, each a chip that puts its tag in the prompt, and
    /// the button adding another.
    fn references(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let form = cx.entity();
        let addable = self.addable(cx);
        let chips: Vec<AnyElement> = self
            .references
            .iter()
            // Numbered as `request` sends them: without pictures gone from
            // the pool.
            .filter_map(|r| Some((*r, self.reference_name(*r, cx)?)))
            .enumerate()
            .map(|(ix, (reference, name))| {
                let tag = format!("@Image{}", ix + 1);
                let insert: SharedString = tag.clone().into();
                let remove = tag.clone();
                h_flex()
                    .h(px(20.))
                    .pl(px(6.))
                    .rounded(t::RADIUS_SM)
                    .border_1()
                    .border_color(t::LN)
                    .bg(t::R1)
                    .text_size(px(11.))
                    .child(
                        div()
                            .id(("image-reference-tag", ix))
                            .cursor_pointer()
                            .hover(|s| s.text_color(t::T1))
                            .child(format!("{tag} · {name}"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.insert_tag(&insert, window, cx)
                            })),
                    )
                    .child(
                        Button::new(("image-reference-remove", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.references.retain(|r| *r != reference);
                                form::drop_tag(&this.prompt, &remove, window, cx);
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        h_flex()
            .gap(px(6.))
            .flex_wrap()
            .when(chips.is_empty(), |el| {
                el.child(div().text_color(t::T3).child("None"))
            })
            .children(chips)
            .child(
                Button::new("image-reference-add")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .tooltip("Add")
                    .disabled(addable.is_empty())
                    .dropdown_menu(move |mut menu, _, _| {
                        for (reference, name) in &addable {
                            let (form, reference) = (form.clone(), *reference);
                            menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                                move |_, _, cx| {
                                    form.update(cx, |this, cx| {
                                        this.references.push(reference);
                                        cx.notify();
                                    })
                                },
                            ));
                        }
                        menu
                    }),
            )
    }
}

impl Render for ImageForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pick = |name: &'static str, picker: &Picker| form::row(name, Select::new(picker));
        v_flex()
            .gap(px(10.))
            .child(Textarea::new(&self.prompt).token(form::tag_token))
            .when(self.model.references > 0, |el| {
                el.child(form::row("References", self.references(cx)))
            })
            .when(!self.model.aspects.is_empty(), |el| {
                el.child(pick("Aspect ratio", &self.aspect))
            })
            .when(!self.model.resolutions.is_empty(), |el| {
                el.child(pick("Resolution", &self.resolution))
            })
            .when(self.model.transparent, |el| {
                el.child(form::row(
                    "Transparent",
                    Switch::new("image-transparent")
                        .checked(self.transparent)
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.transparent = *on;
                            cx.notify();
                        })),
                ))
            })
            .when_some(self.problem(cx), |el, problem| {
                el.child(form::problem(problem))
            })
    }
}

impl MediaPoolPanel {
    /// What the model picked for pictures can make, when a provider is
    /// ready for it.
    pub(super) fn image_model(&self, cx: &App) -> Option<ImageModel> {
        let generation = self.state.read(cx).services.generation.clone()?;
        generation.provider_for(Capability::Image)?;
        generation.image_model()
    }

    /// The form, editing or following `first` when given.
    pub(super) fn open_image(
        &mut self,
        model: ImageModel,
        first: Option<MediaId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let form = cx.new(|cx| ImageForm::new(self.state.clone(), model, first, window, cx));
        window.open_dialog(cx, move |dialog, _, cx| {
            let submit = form.clone();
            let ready = form.read(cx).ready(cx);
            dialog
                .title(Capability::Image.label())
                .w(px(520.))
                .child(form.clone())
                .confirm("Generate", move |_, cx| {
                    submit.update(cx, |f, cx| f.submit(cx))
                })
                .confirm_disabled(!ready)
        });
    }
}
