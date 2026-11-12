//! `generate_video`: a video made by the provider the user set for it (the
//! Generate category), from a description, from a first frame (a picture
//! in the pool, or the program at a time), or after references (pictures,
//! clips and sounds in the pool). It spends the user's money, so each call
//! waits for their approval. The file goes to the pool's
//! Video Generated bin; with `at` (or `range`) it is also proposed on a
//! video track, its sound on an audio track beside it. Placing and adding
//! to the pool are `generate`'s, as for audio.
//!
//! A [`Shot`] is the video with what it is made from: the user sees it,
//! and may change it, before approving. It can follow a blockout, a rough
//! 3D scene drawn here for nothing, so staging, action, camera and timing
//! are settled before the provider is paid. `preview_blockout` shows the
//! agent a scene it wrote, drawn and in words, before it asks. A blockout
//! is kept under an id ([`drafts`]) and changed there, so no one writes
//! a scene twice.

use super::drafts;
use super::generate::Placement;
use super::preview::{collage, columns};
use super::view::{image_content, save_png};
use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolError, ToolResultOf, fail, media_id, secs, track_id};
use crate::bridge::{Content, ToolResult};
use flikk_core::blockout::{Detail, Outside, Scene};
use flikk_core::cloud::{
    CloudError, GenerateRequest, GenerationService, VideoModel, VideoReference,
};
use flikk_core::compose::{self, Stills};
use flikk_core::model::{MediaKind, Project, TrackKind};
use flikk_core::services::Services;
use flikk_core::{Time, paths};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Widest frame sent as a first or last frame.
const FRAME_WIDTH: u32 = 1920;
/// The tool that shows a blockout before its video is asked for.
pub const PREVIEW: &str = "preview_blockout";
/// How many frames its collage has, and the longer side of each.
const PREVIEW_FRAMES: usize = 8;
const PREVIEW_SIDE: f64 = 384.0;

/// A frame of the program to grab before the video is made, into `path`
/// (which the request already names).
#[derive(Clone, Debug, PartialEq)]
pub struct FrameGrab {
    pub at: Time,
    pub path: PathBuf,
}

impl FrameGrab {
    /// A grab of the program at `at`, into a new file under the cache.
    pub fn new(at: Time) -> Self {
        let name = format!("{}.png", uuid::Uuid::new_v4().simple());
        Self {
            at,
            path: paths::cache().join("frames").join(name),
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({ "at": secs(self.at), "path": self.path })
    }

    pub(crate) fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            at: Time::from_secs_f64(value.get("at")?.as_f64()?),
            path: PathBuf::from(value.get("path")?.as_str()?),
        })
    }
}

/// Write the program at `at` as a PNG at `path`: the sequence's picture,
/// at most [`FRAME_WIDTH`] wide, as the viewer shows it.
pub(crate) fn grab_frame(
    project: &Project,
    services: &Services,
    at: Time,
    path: &Path,
) -> Result<(), String> {
    let settings = &project.timeline.settings;
    let width = settings.width.clamp(2, FRAME_WIDTH);
    let size = compose::size_for_width(project, width);
    let mut stills = Stills::new(services.media.clone(), width);
    let frame = compose::program(project, at, size, services, &mut stills);
    let fail = |e: String| format!("could not grab the frame at {:.2} s: {e}", at.as_secs_f64());
    super::view::write_png(path, &frame).map_err(|e| fail(e.to_string()))
}

/// Grab each of `grabs`, so the request's frames are on disk.
pub fn grab_all(project: &Project, services: &Services, grabs: &[FrameGrab]) -> Result<(), String> {
    grabs
        .iter()
        .try_for_each(|g| grab_frame(project, services, g.at, &g.path))
}

/// A blockout a video follows: the scene, and the clip of it the provider
/// is sent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Blockout {
    pub scene: Scene,
    /// Where its clip is written just before the video is made.
    pub clip: PathBuf,
    /// The clip's size in pixels: the video's shape.
    pub size: (u32, u32),
    /// What it is kept as ([`drafts`]), so it is changed where it is, by
    /// the agent and on the card, and outlives the request.
    #[serde(default)]
    pub id: String,
}

impl Blockout {
    /// `scene` for a video `aspect` wide to one high, kept from now on,
    /// its clip a new file among what providers are handed.
    pub fn new(scene: Scene, aspect: f64) -> Self {
        let id = drafts::keep(&scene, aspect);
        Self::kept(id, scene, aspect)
    }

    /// `scene` as the blockout kept as `id`.
    fn kept(id: String, scene: Scene, aspect: f64) -> Self {
        let name = format!("blockout-{}.mp4", uuid::Uuid::new_v4().simple());
        Self {
            scene,
            clip: paths::cache().join("upload").join(name),
            size: Scene::clip_size(aspect),
            id,
        }
    }

    /// The same blockout with `scene` in its place, there and where it is
    /// kept, and a clip of its own.
    pub fn with_scene(&self, scene: Scene) -> Self {
        let aspect = self.size.0 as f64 / self.size.1.max(1) as f64;
        if self.id.is_empty() {
            return Self::new(scene, aspect);
        }
        drafts::put(&self.id, &scene, aspect);
        Self::kept(self.id.clone(), scene, aspect)
    }

    fn length(&self) -> Time {
        Time::from_secs_f64(self.scene.duration)
    }

    /// The scene at `at` seconds as a new PNG among the grabbed frames,
    /// for a picture model to turn into a finished frame.
    pub fn still(&self, at: f64) -> Result<PathBuf, String> {
        let name = format!("{}.png", uuid::Uuid::new_v4().simple());
        let path = paths::cache().join("frames").join(name);
        let frame = (self.scene).render(at, self.size.0, self.size.1, Detail::Full);
        super::view::write_png(&path, &frame)
            .map(|()| path)
            .map_err(|e| format!("could not write the blockout's frame: {e}"))
    }
}

/// A video to make, with what it is made from, as the user sees it before
/// approving: the request, the frames of the program to grab for it, where
/// it goes on the timeline, and the blockout it follows.
#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    /// A [`GenerateRequest::Video`] as the user reads it: its first frame
    /// stays a first frame beside a blockout. [`Shot::sent`] is what the
    /// provider gets.
    pub request: GenerateRequest,
    pub grabs: Vec<FrameGrab>,
    pub placement: Option<Placement>,
    pub blockout: Option<Blockout>,
}

impl Shot {
    /// The request as the provider gets it. A model follows references or
    /// starts on a frame, not both, so beside a blockout the first frame
    /// goes as the first picture it follows: the prompt's `@Video1` is the
    /// blockout and `@Image1` the first frame.
    pub fn sent(&self) -> GenerateRequest {
        let (
            Some(blockout),
            GenerateRequest::Video {
                first_frame,
                references,
                ..
            },
        ) = (&self.blockout, &self.request)
        else {
            return self.request.clone();
        };
        let picture = |path: &PathBuf| VideoReference {
            kind: MediaKind::Image,
            path: path.clone(),
            length: None,
        };
        let mut follows = vec![VideoReference {
            kind: MediaKind::Video,
            path: blockout.clip.clone(),
            length: Some(blockout.length()),
        }];
        follows.extend(first_frame.as_ref().map(picture));
        follows.extend(references.iter().cloned());
        let mut sent = self.request.clone();
        if let GenerateRequest::Video {
            first_frame,
            references,
            ..
        } = &mut sent
        {
            *first_frame = None;
            *references = follows;
        }
        sent
    }

    /// Why `model` can't make it, before any money is spent on it.
    pub fn check(&self, model: Option<&VideoModel>) -> Result<(), String> {
        let ends_on_frame = matches!(
            &self.request,
            GenerateRequest::Video {
                last_frame: Some(_),
                ..
            }
        );
        if self.blockout.is_some() && ends_on_frame {
            return Err("A video that follows a blockout can't end on a given frame".into());
        }
        let Some(model) = model else {
            return Ok(());
        };
        if self.blockout.is_some() && model.references.is_none_or(|limits| limits.videos == 0) {
            return Err(format!(
                "{} can't follow a blockout: it takes no clips to follow. Make the video \
                 without one, or tell the user a model that follows clips does (Seedance, \
                 Kling, Wan).",
                model.name
            ));
        }
        model.check(&self.sent()).map_err(|e| error_text(&e))
    }

    /// Put what the request names on disk (the program's frames, grabbed
    /// from `project`, and the blockout's clip) and hand back the request
    /// to send. `cancelled` stops the clip between frames.
    pub fn prepare(
        &self,
        project: &Project,
        services: &Services,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<GenerateRequest, String> {
        grab_all(project, services, &self.grabs)?;
        if let Some(blockout) = &self.blockout {
            (blockout.scene).write_clip(blockout.size, &blockout.clip, cancelled)?;
        }
        Ok(self.sent())
    }

    /// The shot's prompt.
    pub fn prompt(&self) -> &str {
        self.request.prompt()
    }

    /// The prompt without what [`parse`] added about the blockout: what
    /// the shot shows, to describe its first frame by.
    pub fn look(&self) -> &str {
        let prompt = self.prompt();
        let end = prompt.find(BLOCKOUT_ROLE).unwrap_or(prompt.len());
        prompt[..end].trim()
    }

    /// Follow `blockout`, or none, and have the prompt say so
    /// ([`Self::settle_roles`]).
    pub fn set_blockout(&mut self, blockout: Option<Blockout>) {
        self.blockout = blockout;
        self.settle_roles();
    }

    /// Make the prompt say what the blockout, and a first frame beside it,
    /// are for, and stop saying it once they are gone. Call it when either
    /// came or went. Only the lines put in here are taken out, and a prompt
    /// that names `@Video1` or `@Image1` its own way is left as it is.
    pub fn settle_roles(&mut self) {
        let first_frame = matches!(
            &self.request,
            GenerateRequest::Video {
                first_frame: Some(_),
                ..
            }
        );
        let mut prompt = self.prompt().to_owned();
        if self.blockout.is_none() || !first_frame {
            prompt = without_line(&prompt, FIRST_FRAME_LINE);
        }
        if self.blockout.is_none() {
            prompt = without_line(&prompt, BLOCKOUT_LINE);
        } else {
            prompt = with_blockout_roles(prompt, first_frame);
        }
        self.set_prompt(prompt);
    }

    /// Replace the shot's prompt.
    pub fn set_prompt(&mut self, text: String) {
        if let GenerateRequest::Video { prompt, .. } = &mut self.request {
            *prompt = text;
        }
    }
}

/// How a prompt is told what its blockout is, when it doesn't say, and
/// what a first frame beside it is.
const BLOCKOUT_ROLE: &str = "@Video1 is a rough 3D blockout";
const BLOCKOUT_LINE: &str = "@Video1 is a rough 3D blockout: follow its camera move, staging, \
    action and timing. Its grey shapes are stand-ins, not the look.";
const FIRST_FRAME_LINE: &str = "Start on @Image1 as the first frame.";

/// `prompt` saying what the blockout, and a first frame sent with it, are
/// for, unless it names them itself: models read a clip as something to
/// copy unless told it is a stand-in.
fn with_blockout_roles(prompt: String, first_frame: bool) -> String {
    let mut prompt = prompt;
    if !prompt.contains("@Video1") {
        prompt.push_str("\n\n");
        prompt.push_str(BLOCKOUT_LINE);
    }
    if first_frame && !prompt.contains("@Image1") {
        prompt.push('\n');
        prompt.push_str(FIRST_FRAME_LINE);
    }
    prompt
}

/// `prompt` without `line` as [`with_blockout_roles`] added it.
fn without_line(prompt: &str, line: &str) -> String {
    prompt.replace(line, "").trim_end().to_owned()
}

/// What the user set for video in Settings → Integrations: the model
/// picked, and the sound and resolution videos are made with.
#[derive(Clone, Debug)]
pub(crate) struct VideoSetup {
    pub model: Option<VideoModel>,
    /// Off: videos are made silent, whatever a request says.
    pub sound: bool,
    /// The resolution a request without one is made at, fitted to the
    /// model; `None` takes the model's default.
    pub resolution: Option<String>,
}

impl VideoSetup {
    pub(crate) fn of(generation: Option<&dyn GenerationService>) -> Self {
        Self {
            model: generation.and_then(|g| g.video_model()),
            sound: generation.is_none_or(|g| g.video_sound()),
            resolution: generation.and_then(|g| g.video_resolution()),
        }
    }
}

/// The shot the tool's arguments ask for; checked against the model
/// `setup` picked, and made at its resolution and, without its sound,
/// silent, where the arguments don't say. A blockout without a length runs
/// as long as the video, and a video without one as long as its blockout.
pub(crate) fn parse(project: &Project, setup: &VideoSetup, args: &Args) -> ToolResultOf<Shot> {
    let (model, sound) = (setup.model.as_ref(), setup.sound);
    let prompt = args.str("prompt")?.trim().to_owned();
    if prompt.is_empty() {
        return fail("`prompt` describes what to make");
    }
    let range = args.opt_range("range")?;
    let asked = match (args.opt_f64("duration")?, range) {
        (Some(secs), _) if secs > 0.0 => Some(secs.round()),
        (Some(_), _) => return fail("`duration` is in seconds, more than 0"),
        (None, Some(r)) => Some(r.duration().as_secs_f64().round()),
        (None, None) => None,
    };
    let aspect = args.opt_str("aspect")?.map(str::to_owned);
    let written = args.value("blockout").filter(|json| !json.is_null());
    let blockout = match (written, args.opt_str("blockout_id")?) {
        (Some(_), Some(_)) => return fail("give `blockout` or `blockout_id`, not both"),
        (Some(json), None) => {
            let scene = scene_of(json, asked)?;
            Some(Blockout::new(scene, shape_of(project, aspect.as_deref())))
        }
        (None, Some(id)) => {
            let draft = drafts::find(id).map_err(ToolError)?;
            let shape = (aspect.as_deref())
                .and_then(aspect_ratio)
                .unwrap_or(draft.shape);
            Some(Blockout::kept(id.to_owned(), draft.scene, shape))
        }
        (None, None) => None,
    };
    let seconds = asked.or_else(|| {
        let secs = blockout.as_ref()?.scene.duration;
        Some(model.map_or(secs.round().max(1.0), |m| m.nearest_length(secs) as f64))
    });
    let length = seconds.map(Time::from_secs_f64);
    let mut grabs = Vec::new();
    let mut frame = |media: &str, at: &str| -> ToolResultOf<Option<PathBuf>> {
        match (args.opt_str(media)?, args.opt_time(at)?) {
            (Some(_), Some(_)) => fail(format!("give `{media}` or `{at}`, not both")),
            (Some(id), None) => {
                let item = project
                    .media(media_id(project, id)?)
                    .filter(|m| m.kind == MediaKind::Image);
                match item {
                    Some(item) => Ok(Some(item.path.clone())),
                    None => fail(format!("`{media}` must be a picture in the pool")),
                }
            }
            (None, Some(t)) => {
                let grab = FrameGrab::new(t);
                let path = grab.path.clone();
                grabs.push(grab);
                Ok(Some(path))
            }
            (None, None) => Ok(None),
        }
    };
    let first_frame = frame("first_frame", "first_frame_at")?;
    let last_frame = frame("last_frame", "last_frame_at")?;
    let mut references = Vec::new();
    for id in args.strings("references")? {
        let media = media_id(project, &id)?;
        let Some(item) = project.media(media) else {
            return fail(format!("no media {id}"));
        };
        references.push(VideoReference {
            kind: item.kind,
            path: item.path.clone(),
            length: item.info.as_ref().map(|i| i.duration),
        });
    }
    let prompt = match &blockout {
        Some(_) => with_blockout_roles(prompt, first_frame.is_some()),
        None => prompt,
    };
    let request = GenerateRequest::Video {
        prompt,
        length,
        aspect,
        resolution: match args.opt_str("resolution")? {
            Some(asked) => Some(asked.to_owned()),
            None => setup
                .resolution
                .as_deref()
                .zip(model)
                .and_then(|(preferred, m)| m.resolution_for(preferred))
                .map(str::to_owned),
        },
        first_frame,
        last_frame,
        references,
        // As the model will make it: some always have sound, some never.
        audio: {
            let wanted = sound && args.opt_bool("audio")?.unwrap_or(true);
            model.map_or(wanted, |m| m.makes_sound(wanted))
        },
    };
    let at = args.opt_time("at")?.or(range.map(|r| r.start));
    let track = args
        .opt_str("track")?
        .map(|t| track_id(project, t))
        .transpose()?;
    if let Some(track) = track
        && project
            .timeline
            .track(track)
            .is_some_and(|t| !matches!(t.kind, TrackKind::Video | TrackKind::Overlay))
    {
        return fail("`track` must be a video track");
    }
    let shot = Shot {
        request,
        grabs,
        placement: at.map(|at| Placement { at, track }),
        blockout,
    };
    shot.check(model).map_err(ToolError)?;
    Ok(shot)
}

/// The scene `json` describes, `seconds` long when it gives no length.
fn scene_of(json: &Value, seconds: Option<f64>) -> ToolResultOf<Scene> {
    let mut json = json.clone();
    if let (Some(scene), Some(secs)) = (json.as_object_mut(), seconds)
        && !scene.contains_key("duration")
    {
        scene.insert("duration".into(), json!(secs));
    }
    Scene::parse(&json).map_err(ToolError)
}

/// The shape of a video asked for as `aspect`, width over height: the
/// sequence's when it gives none.
fn shape_of(project: &Project, aspect: Option<&str>) -> f64 {
    let settings = &project.timeline.settings;
    aspect
        .and_then(aspect_ratio)
        .unwrap_or(settings.width as f64 / settings.height.max(1) as f64)
}

/// `preview_blockout`: a blockout drawn at a few moments and said in
/// words, so whoever wrote it can see it before the user does. The scene
/// is kept, and a later call changes the one kept instead of writing it
/// again.
pub(crate) fn preview(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let written = args.value("blockout").filter(|json| !json.is_null());
    let changes = args.value("changes").filter(|json| !json.is_null());
    let asked = args.opt_str("aspect")?;
    let (id, scene, shape) = match (written, args.opt_str("id")?) {
        (Some(_), Some(_)) => {
            return fail("give `blockout`, a scene to keep, or `id`, one kept: not both");
        }
        (Some(_), None) if changes.is_some() => {
            return fail("`changes` are made to a blockout kept earlier: give its `id`");
        }
        (Some(json), None) => {
            let seconds = args.opt_f64("duration")?.filter(|secs| *secs > 0.0);
            let scene = scene_of(json, seconds)?;
            let shape = shape_of(ctx.project(), asked);
            (drafts::keep(&scene, shape), scene, shape)
        }
        (None, Some(id)) => {
            let draft = drafts::find(id).map_err(ToolError)?;
            let shape = asked.and_then(aspect_ratio).unwrap_or(draft.shape);
            let scene = match changes {
                Some(changes) => draft.scene.changed(changes).map_err(ToolError)?,
                None => draft.scene,
            };
            drafts::put(id, &scene, shape);
            (id.to_owned(), scene, shape)
        }
        (None, None) => {
            return fail("give `blockout`, the scene to look at, or the `id` of one kept");
        }
    };
    let job: Job = Box::new(move || JobDone::Done(drawn(&id, &scene, shape)));
    Ok(Step::Job(job))
}

/// `scene`, kept as `id`, in a picture `shape` wide to one high: as a
/// collage of frames across it and as what its camera sees.
fn drawn(id: &str, scene: &Scene, shape: f64) -> Done {
    let (wide, high) = Scene::clip_size(shape);
    let scale = PREVIEW_SIDE / wide.max(high) as f64;
    let size = [wide, high].map(|side| ((side as f64 * scale) as u32).max(8));
    let times: Vec<f64> = (0..PREVIEW_FRAMES)
        .map(|i| scene.duration * i as f64 / (PREVIEW_FRAMES - 1) as f64)
        .collect();
    let mut tiles: Vec<_> = (times.iter())
        .map(|t| {
            let frame = scene.render(*t, size[0], size[1], Detail::Full);
            (frame, format!("{t:.1}s"))
        })
        .collect();
    let moments = tiles.len();
    // And the set from straight above, where what the shot's own camera
    // hides (who is where, how far the camera is) shows.
    let above = Outside {
        around: 0.0,
        above: 90.0,
        closer: 1.0,
    };
    let middle = scene.duration * 0.5;
    let plan = scene.render_outside(middle, (size[0], size[1]), &above, Detail::Full);
    tiles.push((plan, "plan".into()));
    let image = save_png(&collage(&tiles, (size[0], size[1])))
        .and_then(|path| Ok((image_content(&path)?, path)));
    let (image, path) = match image {
        Ok(image) => image,
        Err(ToolError(e)) => return Done::error(e),
    };
    let cols = columns(tiles.len());
    let review = scene.review((wide, high)).text();
    let text = format!(
        "Blockout `{id}`, {:.1} s, at {moments} moments: {cols} columns × {} rows, left to right \
         then top to bottom, each stamped with its time. The last, `plan`, is the set from \
         straight above at {middle:.1} s (+X to the right, +Z toward the bottom): the camera in \
         white with the way it goes, and in black the way each thing goes, a tick a second. \
         What its camera sees:\n{review}\n\nIt is kept \
         as `{id}`. To fix what is not as the shot needs, call this again with `id` and `changes` \
         (only what differs), not the whole scene; then ask for the video with `blockout_id`.",
        scene.duration,
        tiles.len().div_ceil(cols),
    );
    Done {
        result: ToolResult {
            content: vec![Content::Text { text }, image],
            is_error: false,
        },
        summary: format!("Blockout, {:.0} s: {moments} frames", scene.duration),
        images: vec![path],
        ..Default::default()
    }
}

/// "16:9" as 1.78; `None` when it isn't two numbers.
fn aspect_ratio(aspect: &str) -> Option<f64> {
    let (wide, high) = aspect.split_once(':')?;
    let (wide, high): (f64, f64) = (wide.trim().parse().ok()?, high.trim().parse().ok()?);
    (wide > 0.0 && high > 0.0).then_some(wide / high)
}

/// A provider's refusal as the model should read it.
pub fn error_text(e: &CloudError) -> String {
    match e {
        CloudError::BadPrompt { message, .. } => format!(
            "Refused: {message}. Change the request to fit (the model's limits are in the tool's description), or tell the user."
        ),
        e => super::generate::error_text(e),
    }
}

/// "Video, 5 s: …", then what it starts on or follows, and where it goes.
pub fn summary(shot: &Shot) -> String {
    let mut text = shot.request.summary();
    if let GenerateRequest::Video {
        first_frame,
        references,
        audio,
        ..
    } = &shot.request
    {
        if let Some(blockout) = &shot.blockout {
            // Its id, so the agent can go on from it after the user decides.
            match blockout.id.as_str() {
                "" => text.push_str(" (after a blockout)"),
                id => text.push_str(&format!(" (after blockout {id})")),
            }
        } else if first_frame.is_some() {
            text.push_str(" (from a frame)");
        } else if !references.is_empty() {
            text.push_str(&format!(" (after {} references)", references.len()));
        }
        if !audio {
            text.push_str(", no sound");
        }
    }
    match &shot.placement {
        Some(p) => format!("{text} → {:.1}s", p.at.as_secs_f64()),
        None => text,
    }
}

/// The shot as an approval carries it.
pub fn request_json(shot: &Shot) -> Value {
    json!({
        "request": shot.request,
        "frames": shot.grabs.iter().map(FrameGrab::to_json).collect::<Vec<_>>(),
        "place": shot.placement.as_ref().map(Placement::to_json),
        "blockout": shot.blockout,
    })
}

/// What [`request_json`] made.
pub fn from_json(value: &Value) -> Option<Shot> {
    let request = serde_json::from_value(value.get("request")?.clone()).ok()?;
    let grabs = value
        .get("frames")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(FrameGrab::from_json).collect())
        .unwrap_or_default();
    let blockout = value
        .get("blockout")
        .and_then(|b| serde_json::from_value(b.clone()).ok());
    Some(Shot {
        request,
        grabs,
        placement: value.get("place").and_then(Placement::from_json),
        blockout,
    })
}
