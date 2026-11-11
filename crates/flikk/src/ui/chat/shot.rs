//! The card of a video the agent asks to make: everything it will be made
//! from, to check and change before the provider is paid. The blockout
//! plays; the first frame and the references are tiles, each replaced by a
//! picture made for it, the frame at the playhead or a pool item; the
//! prompt is the text that is sent, and can be typed over. Every change
//! goes into the waiting request at once, so Approve makes what the card
//! shows.

use crate::state::EditorState;
use crate::ui::blockout::{self, BlockoutPlayer};
use crate::ui::format::item_name;
use crate::ui::kit::{
    Button, DialogExt as _, DropdownMenu as _, Icon, IconName, PopupMenu, PopupMenuItem, Spinner,
    Textarea, Tooltip, h_flex, theme as t, v_flex,
};
use crate::ui::media_pool::{form, thumbs};
use flikk_agent::reblock::{Reblock, Reblocked};
use flikk_agent::tools::video::{self, Blockout, FrameGrab, Shot};
use flikk_core::MediaId;
use flikk_core::cloud::{Capability, GenerateRequest, VideoReference};
use flikk_core::model::{MediaItem, MediaKind};
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// How tall a picture's tile is, and the blockout's: two tiles and the gap
/// between them.
const TILE: Pixels = px(44.);
const GAP: Pixels = px(4.);
const STAGE: Pixels = px(92.);
/// A tile is this many times as wide as it is tall, whatever the video's
/// shape: an upright video's picture is cropped to it, and its blockout
/// shown whole inside it, so a caption always has room.
const TILE_SHAPE: f32 = 16. / 9.;
/// How wide the tile that adds a reference is: it sits beside a picture's
/// tile in the narrowest chat.
const ADD: Pixels = px(24.);
/// The widest the blockout is drawn on the card.
const STAGE_WIDTH: u32 = 480;
/// How wide a clip's poster is read.
const POSTER_WIDTH: u32 = 320;

/// A place on the card a file can go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    First,
    Last,
    /// One of the references, by its place among them.
    Reference(usize),
    /// A reference more.
    Added,
}

/// What a slot waits on.
#[derive(Clone, Debug, PartialEq)]
enum Busy {
    /// The program's frame is being grabbed for it.
    Grabbing,
    /// A picture is being made for it, as this generation.
    Making(u64),
    /// Making or grabbing its picture failed, with why.
    Failed(String),
}

/// When a tile's menu button shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Corner {
    Always,
    OnHover,
}

/// A blockout being written for a shot that has none, at the user's ask.
enum Draft {
    None,
    Working(Box<Reblock>),
    /// Why none came of it: the backend failed, or said there is nothing
    /// to block.
    Failed(String),
}

/// How often a blockout being written is looked at.
const DRAFT_POLL: Duration = Duration::from_millis(150);
/// What an image model is told of a shot's prompt, which runs past the
/// frame it is asked for.
const OPENING_ONLY: &str = "Show only the opening instant of what follows: none of its later \
    shots, camera moves or sound. Nothing blurred by motion, no text.";

/// A clip's poster frame.
enum Poster {
    Reading,
    Read(Arc<Image>),
    Missing,
}

pub struct ShotCard {
    state: Entity<EditorState>,
    /// The request it shows and changes.
    id: u64,
    shot: Shot,
    prompt: Entity<TextareaState>,
    /// The shot's prompt changed here, not in its field, which shows it on
    /// the next paint.
    prompt_stale: bool,
    /// The blockout playing; made on the next paint after it changed.
    player: Option<Entity<BlockoutPlayer>>,
    draft: Draft,
    draft_poll: Option<Task<()>>,
    /// The picture's shape: width to height.
    shape: (u32, u32),
    busy: Vec<(Slot, Busy)>,
    /// The program's frames the request named are still being grabbed.
    grabbing: bool,
    posters: HashMap<PathBuf, Poster>,
    _subscriptions: Vec<Subscription>,
}

impl ShotCard {
    pub fn new(
        state: Entity<EditorState>,
        id: u64,
        shot: Shot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = &state.read(cx).bus.project().timeline.settings;
        let sequence = (settings.width.max(1), settings.height.max(1));
        let shape = match (&shot.blockout, &shot.request) {
            (Some(blockout), _) => blockout.size,
            (None, GenerateRequest::Video { aspect, .. }) => {
                (aspect.as_deref().and_then(shape_of)).unwrap_or(sequence)
            }
            _ => sequence,
        };
        let text = shot.prompt().to_owned();
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 5)
                .default_value(text)
        });
        form::mark_tags(&prompt, form::Tags::References, window, cx);
        let _subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &prompt,
                window,
                |this, prompt, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        form::mark_tags(prompt, form::Tags::References, window, cx);
                        let text = prompt.read(cx).value().to_string();
                        if text != this.shot.prompt() {
                            this.shot.set_prompt(text);
                            this.save(cx);
                        }
                    }
                },
            ),
        ];
        let mut this = Self {
            state,
            id,
            shot,
            prompt,
            prompt_stale: false,
            player: None,
            draft: Draft::None,
            draft_poll: None,
            shape,
            busy: Vec::new(),
            grabbing: false,
            posters: HashMap::new(),
            _subscriptions,
        };
        this.grab_named(cx);
        this
    }

    /// Bring what is painted in step with the shot: its blockout playing,
    /// and its prompt in the field. Both need the window, which a change
    /// that lands from a background job doesn't have.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.player.is_none()
            && let Some(blockout) = &self.shot.blockout
        {
            let (scene, shape) = (blockout.scene.clone(), self.shape);
            self.player =
                Some(cx.new(|cx| BlockoutPlayer::new(scene, shape, STAGE_WIDTH, window, cx)));
        }
        if std::mem::take(&mut self.prompt_stale) {
            let text = self.shot.prompt().to_owned();
            self.prompt
                .update(cx, |prompt, cx| prompt.set_value(text, window, cx));
            form::mark_tags(&self.prompt, form::Tags::References, window, cx);
        }
    }

    /// The shot follows `blockout` now, or none: the prompt says so, and
    /// the next paint plays it.
    fn set_blockout(&mut self, blockout: Option<Blockout>, cx: &mut Context<Self>) {
        self.shot.set_blockout(blockout);
        self.player = None;
        self.prompt_stale = true;
        self.save(cx);
    }

    /// Put the card's shot into the waiting request.
    fn save(&mut self, cx: &mut Context<Self>) {
        let (id, shot) = (self.id, self.shot.clone());
        self.state.update(cx, |s, cx| s.revise_shot(id, &shot, cx));
        cx.notify();
    }

    /// Grab the program's frames the request names now, so their tiles
    /// show what is sent. Left for the approval to grab if it fails.
    fn grab_named(&mut self, cx: &mut Context<Self>) {
        if self.shot.grabs.is_empty() {
            return;
        }
        self.grabbing = true;
        let grabs = self.shot.grabs.clone();
        let state = self.state.read(cx);
        let (project, services) = (state.bus.project().clone(), state.services.clone());
        let grabbed = cx
            .background_executor()
            .spawn(async move { video::grab_all(&project, &services, &grabs) });
        cx.spawn(async move |this, cx| {
            let grabbed = grabbed.await;
            let _ = this.update(cx, |this, cx| {
                this.grabbing = false;
                if grabbed.is_ok() {
                    this.shot.grabs.clear();
                    this.save(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ the shot

    fn request_parts(&self) -> (Option<&PathBuf>, Option<&PathBuf>, &[VideoReference]) {
        match &self.shot.request {
            GenerateRequest::Video {
                first_frame,
                last_frame,
                references,
                ..
            } => (first_frame.as_ref(), last_frame.as_ref(), references),
            _ => (None, None, &[]),
        }
    }

    /// What the prompt calls reference `ix`: "@Image2". The blockout is
    /// the first clip, and a first frame beside it the first picture.
    fn tag(&self, ix: usize) -> String {
        let (first, _, references) = self.request_parts();
        let kind = references[ix].kind;
        let before = references[..ix].iter().filter(|r| r.kind == kind).count();
        let blockout = self.shot.blockout.is_some();
        let ahead = match kind {
            MediaKind::Video => blockout as usize,
            MediaKind::Image => (blockout && first.is_some()) as usize,
            MediaKind::Audio => 0,
        };
        VideoReference::tag(kind, before + ahead + 1)
    }

    /// Put `reference` in `slot` and save.
    fn put(&mut self, slot: Slot, reference: VideoReference, cx: &mut Context<Self>) {
        self.busy.retain(|(s, _)| *s != slot);
        if let GenerateRequest::Video {
            first_frame,
            last_frame,
            references,
            ..
        } = &mut self.shot.request
        {
            match slot {
                Slot::First => *first_frame = Some(reference.path),
                Slot::Last => *last_frame = Some(reference.path),
                Slot::Reference(ix) if ix < references.len() => references[ix] = reference,
                Slot::Reference(_) | Slot::Added => references.push(reference),
            }
        }
        self.frame_changed(slot);
        self.save(cx);
    }

    /// `slot` was filled or emptied: beside a blockout the prompt says
    /// what the first frame is, or stops saying it.
    fn frame_changed(&mut self, slot: Slot) {
        if slot == Slot::First && self.shot.blockout.is_some() {
            self.shot.settle_roles();
            self.prompt_stale = true;
        }
    }

    fn put_picture(&mut self, slot: Slot, path: PathBuf, cx: &mut Context<Self>) {
        let reference = VideoReference {
            kind: MediaKind::Image,
            path,
            length: None,
        };
        self.put(slot, reference, cx);
    }

    fn put_media(&mut self, slot: Slot, media: MediaId, cx: &mut Context<Self>) {
        let Some(item) = self.state.read(cx).bus.project().media(media) else {
            return;
        };
        let reference = VideoReference {
            kind: item.kind,
            path: item.path.clone(),
            length: item.info.as_ref().map(|i| i.duration),
        };
        self.put(slot, reference, cx);
    }

    fn clear(&mut self, slot: Slot, cx: &mut Context<Self>) {
        self.busy.retain(|(s, _)| *s != slot);
        if let GenerateRequest::Video {
            first_frame,
            last_frame,
            references,
            ..
        } = &mut self.shot.request
        {
            match slot {
                Slot::First => *first_frame = None,
                Slot::Last => *last_frame = None,
                Slot::Reference(ix) if ix < references.len() => {
                    references.remove(ix);
                }
                Slot::Reference(_) | Slot::Added => {}
            }
        }
        self.frame_changed(slot);
        self.save(cx);
    }

    fn set_busy(&mut self, slot: Slot, busy: Busy, cx: &mut Context<Self>) {
        self.busy.retain(|(s, _)| *s != slot);
        self.busy.push((slot, busy));
        cx.notify();
    }

    /// Fill `slot` with the program's frame at the playhead, grabbed now.
    fn grab_playhead(&mut self, slot: Slot, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let grab = FrameGrab::new(state.playhead);
        let (project, services) = (state.bus.project().clone(), state.services.clone());
        let path = grab.path.clone();
        let grabbed = cx
            .background_executor()
            .spawn(async move { video::grab_all(&project, &services, &[grab]) });
        self.set_busy(slot, Busy::Grabbing, cx);
        cx.spawn(async move |this, cx| {
            let grabbed = grabbed.await;
            let _ = this.update(cx, |this, cx| {
                // Changed meanwhile, the slot keeps what it was given.
                if !this.busy.contains(&(slot, Busy::Grabbing)) {
                    return;
                }
                match grabbed {
                    Ok(()) => this.put_picture(slot, path, cx),
                    Err(e) => this.set_busy(slot, Busy::Failed(e), cx),
                }
            });
        })
        .detach();
    }

    /// Why the model picked can't make the shot as it stands.
    fn problem(&self, cx: &App) -> Option<String> {
        let generation = self.state.read(cx).services.generation.clone();
        let model = generation.and_then(|g| g.video_model());
        self.shot.check(model.as_ref()).err()
    }

    /// Whether something is still on its way into the shot. A picture
    /// whose generation was stopped is no longer on its way.
    fn waiting(&self, cx: &App) -> bool {
        let on_its_way = |(slot, _): &(Slot, Busy)| {
            matches!(self.busy(*slot, cx), Some(Busy::Grabbing | Busy::Making(_)))
        };
        self.grabbing || self.busy.iter().any(on_its_way) || matches!(self.draft, Draft::Working(_))
    }

    // -------------------------------------------------------- the blockout

    pub(super) fn edit_blockout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(blockout) = &self.shot.blockout else {
            return;
        };
        let card = cx.entity().downgrade();
        let on_save: blockout::editor::OnSave = Rc::new(move |scene, _, cx| {
            let _ = card.update(cx, |card, cx| card.set_scene(scene, cx));
        });
        blockout::editor::open(
            self.state.clone(),
            blockout.scene.clone(),
            blockout.size,
            on_save,
            window,
            cx,
        );
    }

    /// The shot follows `scene` from now on: in place of the blockout it
    /// had, where that is kept, or as a new one. Its clip is its own.
    fn set_scene(&mut self, scene: flikk_core::blockout::Scene, cx: &mut Context<Self>) {
        let blockout = match &self.shot.blockout {
            Some(blockout) => blockout.with_scene(scene),
            None => Blockout::new(scene, self.shape.0 as f64 / self.shape.1 as f64),
        };
        self.set_blockout(Some(blockout), cx);
    }

    /// Whether the model picked can follow a blockout; one not known yet
    /// is taken to.
    fn follows_clips(&self, cx: &App) -> bool {
        let generation = self.state.read(cx).services.generation.clone();
        let model = generation.and_then(|g| g.video_model());
        model.is_none_or(|m| m.references.is_some_and(|limits| limits.videos > 0))
    }

    /// Have the agent's backend write a blockout for the shot from its
    /// prompt, at the user's ask: one run without tools, which spends
    /// nothing with the video provider.
    fn draft_blockout(&mut self, cx: &mut Context<Self>) {
        if matches!(self.draft, Draft::Working(_)) || self.shot.blockout.is_some() {
            return;
        }
        let seconds = match &self.shot.request {
            GenerateRequest::Video {
                length: Some(length),
                ..
            } => length.as_secs_f64(),
            _ => 5.0,
        };
        let settings = self.state.read(cx).settings.clone();
        self.draft = match Reblock::draft(self.shot.look(), seconds, self.shape, &settings) {
            Ok(drafting) => Draft::Working(Box::new(drafting)),
            Err(e) => Draft::Failed(e.to_string()),
        };
        self.draft_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(DRAFT_POLL).await;
                let working = this.update(cx, |this, cx| this.look_at_draft(cx));
                if !matches!(working, Ok(true)) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// Take what the blockout being written came to; whether it is still
    /// being written.
    fn look_at_draft(&mut self, cx: &mut Context<Self>) -> bool {
        let Draft::Working(drafting) = &mut self.draft else {
            return false;
        };
        match drafting.poll() {
            Reblocked::Working => return true,
            Reblocked::Done {
                scene: Some(scene), ..
            } => {
                self.draft = Draft::None;
                self.set_scene(*scene, cx);
            }
            Reblocked::Done {
                scene: None,
                answer,
            } => self.draft = Draft::Failed(answer),
            Reblocked::Failed(e) => self.draft = Draft::Failed(e),
        }
        cx.notify();
        false
    }

    /// Stop the blockout being written; stopped, it didn't fail.
    fn stop_draft(&mut self, cx: &mut Context<Self>) {
        self.draft = Draft::None;
        self.draft_poll = None;
        cx.notify();
    }

    // ------------------------------------------------------- making pictures

    /// Open the form that makes a picture for `slot`: after the picture in
    /// it when there is one, else, for the first frame, after the
    /// blockout's opening frame.
    fn make_picture(&mut self, slot: Slot, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let follows = self.picture_in(slot).cloned();
        let from_blockout = follows.is_none() && slot == Slot::First && self.player.is_some();
        // The shot's prompt is written for a video model: an image model
        // is told which part of it is its own.
        let start = match (&follows, from_blockout, slot) {
            (Some(_), _, _) => String::new(),
            (None, true, _) => format!(
                "The finished first frame of this shot, as one sharp still. @Image1 is a rough \
                 3D blockout: keep its camera and composition exactly, and make the grey \
                 stand-ins what they stand for. {OPENING_ONLY}\n\n{}",
                self.shot.look()
            ),
            (None, false, Slot::First) => format!(
                "The first frame of this shot, as one sharp still. {OPENING_ONLY}\n\n{}",
                self.shot.look()
            ),
            (None, false, _) => self.shot.look().to_owned(),
        };
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 8)
                .default_value(start)
                .placeholder(match follows {
                    Some(_) => "What changes in @Image1…",
                    None => "Neon-lit ramen bar on a rainy night, shot on 35 mm…",
                })
        });
        form::mark_tags(&prompt, form::Tags::References, window, cx);
        let card = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let (card, typed) = (card.clone(), prompt.clone());
            let empty = prompt.read(cx).value().trim().is_empty();
            dialog
                .title(Capability::Image.label())
                .w(px(520.))
                .child(Textarea::new(&prompt).token(form::tag_token))
                .confirm("Generate", move |_, cx| {
                    let text = typed.read(cx).value().trim().to_owned();
                    card.update(cx, |card, cx| card.start_picture(slot, text, cx));
                    true
                })
                .confirm_disabled(empty)
        });
    }

    /// The picture in `slot`, when it holds one.
    fn picture_in(&self, slot: Slot) -> Option<&PathBuf> {
        let (first, last, references) = self.request_parts();
        match slot {
            Slot::First => first,
            Slot::Last => last,
            Slot::Reference(ix) => references
                .get(ix)
                .filter(|r| r.kind == MediaKind::Image)
                .map(|r| &r.path),
            Slot::Added => None,
        }
    }

    /// Have the image provider make `prompt` for `slot`, as the user's own
    /// generation: it shows in the media pool, lands there, and fills the
    /// slot.
    fn start_picture(&mut self, slot: Slot, prompt: String, cx: &mut Context<Self>) {
        let mut references = Vec::new();
        if let Some(picture) = self.picture_in(slot) {
            references.push(picture.clone());
        } else if let (Slot::First, Some(blockout)) = (slot, &self.shot.blockout) {
            match blockout.still(0.0) {
                Ok(still) => references.push(still),
                Err(e) => return self.set_busy(slot, Busy::Failed(e), cx),
            }
        }
        let generation = self.state.read(cx).services.generation.clone();
        let model = generation.and_then(|g| g.image_model());
        let shape = format!("{}:{}", self.shape.0, self.shape.1);
        let aspect = match &self.shot.request {
            GenerateRequest::Video {
                aspect: Some(aspect),
                ..
            } => Some(aspect.clone()),
            _ => crate::ui::media_pool::shape_of(self.shape.0, self.shape.1).map(str::to_owned),
        };
        let mut request = GenerateRequest::Image {
            prompt,
            aspect,
            resolution: None,
            references,
            transparent: false,
        };
        // A shape the model doesn't make is left to it rather than refused.
        if let (Some(model), GenerateRequest::Image { aspect, .. }) = (&model, &mut request)
            && aspect
                .as_deref()
                .is_some_and(|a| !model.aspects.contains(&a))
        {
            *aspect = None;
        }
        if let Some(Err(e)) = model.as_ref().map(|m| m.check(&request)) {
            return self.set_busy(slot, Busy::Failed(format!("{e} ({shape})")), cx);
        }
        let card = cx.entity().downgrade();
        let making = self.state.update(cx, |s, cx| {
            s.generate_then(
                request,
                Vec::new(),
                move |s, made, cx| {
                    let path = s.bus.project().media(made).map(|m| m.path.clone());
                    // Once the editor state is free again.
                    cx.defer(move |cx| {
                        let _ = card.update(cx, |card, cx| card.picture_made(slot, path, cx));
                    });
                },
                cx,
            )
        });
        self.set_busy(slot, Busy::Making(making), cx);
    }

    fn picture_made(&mut self, slot: Slot, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if !matches!(
            self.busy.iter().find(|(s, _)| *s == slot),
            Some((_, Busy::Making(_)))
        ) {
            return;
        }
        match path {
            Some(path) => self.put_picture(slot, path, cx),
            None => self.clear_busy(slot, cx),
        }
    }

    fn clear_busy(&mut self, slot: Slot, cx: &mut Context<Self>) {
        self.busy.retain(|(s, _)| *s != slot);
        cx.notify();
    }

    /// What `slot` waits on, as it stands now: a generation that failed or
    /// went shows as that.
    fn busy(&self, slot: Slot, cx: &App) -> Option<Busy> {
        let (_, busy) = self.busy.iter().find(|(s, _)| *s == slot)?;
        let Busy::Making(id) = busy else {
            return Some(busy.clone());
        };
        let state = self.state.read(cx);
        match state.generating.iter().find(|g| g.id == *id) {
            Some(g) => match &g.failed {
                Some(e) => Some(Busy::Failed(e.to_string())),
                None => Some(busy.clone()),
            },
            // Stopped, or made and on its way in.
            None => None,
        }
    }

    // ---------------------------------------------------------------- tiles

    /// A clip's poster, read off the UI thread the first time.
    fn poster(&mut self, item: &MediaItem, cx: &mut Context<Self>) -> Option<Arc<Image>> {
        match self.posters.get(&item.path) {
            Some(Poster::Read(image)) => return Some(image.clone()),
            Some(_) => return None,
            None => {}
        }
        let media = self.state.read(cx).services.media.clone()?;
        self.posters.insert(item.path.clone(), Poster::Reading);
        let (item, path) = (item.clone(), item.path.clone());
        let at = (item.info.as_ref()).map_or(flikk_core::Time::ZERO, |i| {
            flikk_core::Time(i.duration.0 / 10)
        });
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move { media.frame(&item, at, POSTER_WIDTH).ok() })
                .await;
            let _ = this.update(cx, |this, cx| {
                let poster = match read {
                    Some(frame) => Poster::Read(Arc::new(Image::from_bytes(
                        ImageFormat::Bmp,
                        thumbs::bmp(&frame),
                    ))),
                    None => Poster::Missing,
                };
                this.posters.insert(path, poster);
                cx.notify();
            });
        })
        .detach();
        None
    }

    /// The menu of a slot a picture goes in: make one, take the frame at
    /// the playhead or a pool picture, or empty it.
    fn picture_menu(
        &self,
        slot: Slot,
        filled: bool,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let card = cx.entity();
        let state = self.state.read(cx);
        let can_make = (state.services.generation.as_ref())
            .is_some_and(|g| g.provider_for(Capability::Image).is_some());
        // The "+" tile takes anything in the pool; the others, pictures.
        let any = slot == Slot::Added;
        let pool: Vec<(MediaId, String)> = (state.bus.project().media.iter())
            .filter(|m| any || m.kind == MediaKind::Image)
            .map(|m| (m.id, item_name(m)))
            .collect();
        move |menu, window, cx| {
            let on = |card: &Entity<Self>, act: fn(&mut Self, Slot, &mut Context<Self>)| {
                let card = card.clone();
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    card.update(cx, |card, cx| act(card, slot, cx))
                }
            };
            let make = card.clone();
            let mut menu = menu
                .item(
                    PopupMenuItem::new("Generate…")
                        .disabled(!can_make)
                        .on_click(move |_, window, cx| {
                            make.update(cx, |card, cx| card.make_picture(slot, window, cx))
                        }),
                )
                .item(
                    PopupMenuItem::new("Frame at playhead")
                        .on_click(on(&card, Self::grab_playhead)),
                );
            if !pool.is_empty() {
                let (card, pool) = (card.clone(), pool.clone());
                menu = menu.submenu("From pool", window, cx, move |mut menu, _, _| {
                    for (media, name) in &pool {
                        let (card, media) = (card.clone(), *media);
                        menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                            move |_, _, cx| {
                                card.update(cx, |card, cx| card.put_media(slot, media, cx))
                            },
                        ));
                    }
                    menu.max_h(px(280.))
                });
            }
            if filled {
                menu = menu
                    .separator()
                    .item(PopupMenuItem::new("Remove").on_click(on(&card, Self::clear)));
            }
            menu
        }
    }

    /// One tile: `content` under its caption, with the button that opens
    /// its menu in the corner. On a filled picture tile the button shows
    /// while the pointer is over the tile, so the picture is seen whole.
    fn tile(
        &self,
        key: usize,
        height: Pixels,
        content: AnyElement,
        caption: Option<String>,
        corner: Option<(AnyElement, Corner)>,
    ) -> Stateful<Div> {
        div()
            .id(("shot-tile", key))
            .group("shot-tile")
            .relative()
            .flex_none()
            .w(height * TILE_SHAPE)
            .h(height)
            .overflow_hidden()
            .rounded(t::RADIUS_SM)
            .border_1()
            .border_color(t::LN)
            .bg(t::SCOPE)
            .child(content)
            .children(caption.map(|caption| {
                // A tag is what the prompt types; a slot's name is a label.
                let tag = caption.starts_with('@');
                div()
                    .absolute()
                    .left(px(2.))
                    .bottom(px(2.))
                    .max_w(height * TILE_SHAPE - px(6.))
                    .truncate()
                    .px(px(4.))
                    .rounded(t::RADIUS_XS)
                    .bg(t::alpha(t::G, 0.72))
                    .when(tag, |el| el.font_family(t::mono()))
                    .text_size(px(10.))
                    .text_color(if tag { t::T1 } else { t::T2 })
                    .child(caption)
            }))
            .children(corner.map(|(button, shown)| {
                div()
                    .absolute()
                    .top(px(2.))
                    .right(px(2.))
                    .rounded(t::RADIUS)
                    .bg(t::alpha(t::G, 0.72))
                    .when(shown == Corner::OnHover, |el| {
                        el.invisible().group_hover("shot-tile", |el| el.visible())
                    })
                    .child(button)
            }))
    }

    /// What fills a tile: the picture or poster at `path`, a glyph for a
    /// sound or a file not read yet, or how its slot stands.
    fn tile_content(
        &mut self,
        slot: Slot,
        file: Option<(MediaKind, PathBuf)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let centred = || h_flex().size_full().justify_center();
        match self.busy(slot, cx) {
            Some(Busy::Grabbing | Busy::Making(_)) => {
                return centred()
                    .child(Spinner::new().color(t::T2))
                    .into_any_element();
            }
            Some(Busy::Failed(why)) => {
                return centred()
                    .id(("shot-failed", slot_key(slot)))
                    .text_color(t::ERR)
                    .child(Icon::new(IconName::CircleAlert))
                    .tooltip(Tooltip::text(why))
                    .into_any_element();
            }
            None => {}
        }
        let picture = |image: ImageSource| {
            img(image)
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element()
        };
        let glyph = |icon: IconName| {
            centred()
                .child(Icon::new(icon).color(t::T3))
                .into_any_element()
        };
        match file {
            // Empty: its name and the button that fills it say enough.
            None => div().into_any_element(),
            // Named by the request, not on disk until it is grabbed.
            Some((MediaKind::Image, path)) if !path.exists() => match self.grabbing {
                true => centred()
                    .child(Spinner::new().color(t::T2))
                    .into_any_element(),
                false => glyph(IconName::ImageOff),
            },
            Some((MediaKind::Image, path)) => picture(path.into()),
            Some((MediaKind::Audio, _)) => glyph(IconName::AudioLines),
            Some((MediaKind::Video, path)) => {
                let item = self.pool_item(&path, cx);
                match item.and_then(|item| self.poster(&item, cx)) {
                    Some(poster) => picture(poster.into()),
                    None => glyph(IconName::Film),
                }
            }
        }
    }

    fn pool_item(&self, path: &Path, cx: &App) -> Option<MediaItem> {
        let project = self.state.read(cx).bus.project();
        project.media.iter().find(|m| m.path == path).cloned()
    }

    fn menu_button(key: usize, icon: IconName, tooltip: &'static str) -> Button {
        Button::new(("shot-menu", key))
            .ghost()
            .xsmall()
            .icon(icon)
            .tooltip(tooltip)
    }

    /// The blockout playing; without one, the tile that has one written,
    /// where the model picked can follow it.
    fn render_blockout(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if self.shot.blockout.is_none() {
            return self.follows_clips(cx).then(|| self.render_no_blockout(cx));
        }
        let player = self.player.clone()?;
        let card = cx.entity();
        let menu = Self::menu_button(usize::MAX, IconName::Ellipsis, "Blockout").dropdown_menu(
            move |menu, _, _| {
                let (edit, remove) = (card.clone(), card.clone());
                menu.item(PopupMenuItem::new("Edit…").on_click(move |_, window, cx| {
                    edit.update(cx, |card, cx| card.edit_blockout(window, cx))
                }))
                .separator()
                .item(PopupMenuItem::new("Remove").on_click(move |_, _, cx| {
                    remove.update(cx, |card, cx| card.set_blockout(None, cx))
                }))
            },
        );
        let tile = self.tile(
            usize::MAX,
            STAGE,
            player.into_any_element(),
            Some(VideoReference::tag(MediaKind::Video, 1)),
            Some((menu.into_any_element(), Corner::OnHover)),
        );
        Some(
            tile.cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| this.edit_blockout(window, cx))),
        )
    }

    /// The blockout's tile while the shot has none: the button that has one
    /// written, or how the writing stands.
    fn render_no_blockout(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let centred = || h_flex().size_full().justify_center();
        let write = |tooltip| {
            Self::menu_button(usize::MAX, IconName::Plus, tooltip)
                .on_click(cx.listener(|this, _, _, cx| this.draft_blockout(cx)))
        };
        let (content, button) = match &self.draft {
            Draft::None => (div().into_any_element(), write("Generate blockout")),
            Draft::Working(_) => (
                centred()
                    .child(Spinner::new().color(t::T2))
                    .into_any_element(),
                Self::menu_button(usize::MAX, IconName::Square, "Stop")
                    .on_click(cx.listener(|this, _, _, cx| this.stop_draft(cx))),
            ),
            Draft::Failed(why) => (
                centred()
                    .id("shot-draft-failed")
                    .text_color(t::ERR)
                    .child(Icon::new(IconName::CircleAlert))
                    .tooltip(Tooltip::text(why.clone()))
                    .into_any_element(),
                write("Generate blockout"),
            ),
        };
        self.tile(
            usize::MAX,
            TILE,
            content,
            Some("Blockout".into()),
            Some((button.into_any_element(), Corner::Always)),
        )
    }

    /// The first frame, the last when there is one, each reference, and
    /// the tile that adds one.
    fn render_tiles(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (first, last, references) = self.request_parts();
        let (first, last) = (first.cloned(), last.cloned());
        let references: Vec<VideoReference> = references.to_vec();
        let beside_blockout = self.shot.blockout.is_some();
        let mut tiles = Vec::new();
        let frames = [
            (Slot::First, first.clone(), "First frame", true),
            (Slot::Last, last.clone(), "Last frame", last.is_some()),
        ];
        for (slot, path, name, shown) in frames {
            if !shown {
                continue;
            }
            let filled = path.is_some();
            // Beside a blockout the first frame is a picture the prompt names.
            let caption = match (slot, beside_blockout && filled) {
                (Slot::First, true) => VideoReference::tag(MediaKind::Image, 1),
                _ => name.to_owned(),
            };
            let content = self.tile_content(slot, path.map(|p| (MediaKind::Image, p)), cx);
            let icon = if filled {
                IconName::Ellipsis
            } else {
                IconName::Plus
            };
            let menu = Self::menu_button(slot_key(slot), icon, name)
                .dropdown_menu(self.picture_menu(slot, filled, cx));
            let shown = if filled {
                Corner::OnHover
            } else {
                Corner::Always
            };
            let tile = self.tile(
                slot_key(slot),
                TILE,
                content,
                Some(caption),
                Some((menu.into_any_element(), shown)),
            );
            tiles.push(tile.into_any_element());
        }
        for (ix, reference) in references.iter().enumerate() {
            let slot = Slot::Reference(ix);
            let content =
                self.tile_content(slot, Some((reference.kind, reference.path.clone())), cx);
            let name = self
                .pool_item(&reference.path, cx)
                .map(|item| item_name(&item));
            let menu = Self::menu_button(slot_key(slot), IconName::Ellipsis, "Reference");
            let menu = match reference.kind {
                MediaKind::Image => menu.dropdown_menu(self.picture_menu(slot, true, cx)),
                _ => {
                    let card = cx.entity();
                    menu.dropdown_menu(move |menu, _, _| {
                        let card = card.clone();
                        menu.item(PopupMenuItem::new("Remove").on_click(move |_, _, cx| {
                            card.update(cx, |card, cx| card.clear(slot, cx))
                        }))
                    })
                }
            };
            let tile = self
                .tile(
                    slot_key(slot),
                    TILE,
                    content,
                    Some(self.tag(ix)),
                    Some((menu.into_any_element(), Corner::OnHover)),
                )
                .when_some(name, |el, name| el.tooltip(Tooltip::text(name)));
            tiles.push(tile.into_any_element());
        }
        let adds = Self::menu_button(slot_key(Slot::Added), IconName::Plus, "Add reference")
            .dropdown_menu(self.picture_menu(Slot::Added, false, cx));
        // While a reference more is on its way, how it stands shows here.
        let adding = match self.busy(Slot::Added, cx) {
            Some(_) => self.tile_content(Slot::Added, None, cx),
            None => adds.into_any_element(),
        };
        tiles.push(
            h_flex()
                .flex_none()
                .w(ADD)
                .h(TILE)
                .justify_center()
                .child(adding)
                .into_any_element(),
        );
        tiles
    }

    /// "Seedance 2.5 · 6 s · 16:9", as far as each is known.
    fn meta(&self, cx: &App) -> String {
        let generation = self.state.read(cx).services.generation.clone();
        let model = generation.and_then(|g| g.video_model()).map(|m| m.name);
        let GenerateRequest::Video { length, aspect, .. } = &self.shot.request else {
            return String::new();
        };
        let length = length.map(|l| format!("{:.0} s", l.as_secs_f64()));
        let parts = [model.map(str::to_owned), length, aspect.clone()];
        parts.into_iter().flatten().collect::<Vec<_>>().join(" · ")
    }
}

/// "9:16" as a shape, width to height; `None` when it isn't two numbers.
fn shape_of(aspect: &str) -> Option<(u32, u32)> {
    let (wide, high) = aspect.split_once(':')?;
    let (wide, high): (f64, f64) = (wide.trim().parse().ok()?, high.trim().parse().ok()?);
    // Whole numbers that keep a ratio such as 2.39:1.
    (wide > 0.0 && high > 0.0).then_some(((wide * 100.0) as u32, (high * 100.0) as u32))
}

/// A key for the elements of `slot`'s tile.
fn slot_key(slot: Slot) -> usize {
    match slot {
        Slot::First => 0,
        Slot::Last => 1,
        Slot::Added => 2,
        Slot::Reference(ix) => 3 + ix,
    }
}

impl Render for ShotCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(window, cx);
        let id = self.id;
        let problem = self.problem(cx);
        let blocked =
            problem.is_some() || self.waiting(cx) || self.prompt.read(cx).value().trim().is_empty();
        let blockout = self.render_blockout(cx);
        let tiles = self.render_tiles(cx);
        let meta = self.meta(cx);
        let state = self.state.clone();
        let deny = Button::new(("shot-deny", id as usize))
            .label("Deny")
            .small()
            .on_click({
                let state = state.clone();
                move |_, _, cx| state.update(cx, |s, cx| s.deny_approval(id, cx))
            });
        let approve = Button::new(("shot-approve", id as usize))
            .label("Approve")
            .primary()
            .small()
            .disabled(blocked)
            .on_click(move |_, _, cx| state.update(cx, |s, cx| s.approve(id, cx)));
        v_flex()
            .id(("shot", id as usize))
            .flex_none()
            .w_full()
            .overflow_hidden()
            .rounded(t::RADIUS)
            .border_1()
            .border_color(t::LN)
            .bg(t::R1)
            .child(
                h_flex()
                    .items_start()
                    .p(px(8.))
                    .gap(GAP)
                    .children(blockout)
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .flex_wrap()
                            .gap(GAP)
                            .children(tiles),
                    ),
            )
            .child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(t::LN)
                    .bg(t::G)
                    .child(Textarea::new(&self.prompt).bare().token(form::tag_token)),
            )
            .child(
                h_flex()
                    .pl(px(10.))
                    .pr(px(8.))
                    .py(px(6.))
                    .gap(px(8.))
                    .border_t_1()
                    .border_color(t::LN)
                    .child(Icon::new(IconName::Video).color(t::T2))
                    .child(
                        div()
                            .id(("shot-problem", id as usize))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .map(|el| match problem {
                                Some(problem) => el
                                    .text_color(t::ERR)
                                    .child(problem.clone())
                                    .tooltip(Tooltip::text(problem)),
                                None => el.font_family(t::mono()).text_color(t::T3).child(meta),
                            }),
                    )
                    .child(deny)
                    .child(approve),
            )
    }
}
