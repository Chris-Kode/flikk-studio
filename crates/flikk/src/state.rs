//! Shared editor state. One entity per window; panels observe it and call
//! its methods instead of touching the bus directly, so every change goes
//! through the same path whether it came from a button or the agent.

mod cloud;
mod music_fit;
mod respeak;
mod source;
mod sync;
mod update;
pub mod voices;

pub use cloud::{Check, Generating, ProviderStatus};
pub use source::Source;

use crate::agent_link::{AgentLink, approve};
use crate::notices::{Level, Notice, NoticeAction, Notices};
use crate::persistence;
use flikk_agent::PendingApproval;
use flikk_core::agent::*;
use flikk_core::bus::{AcceptReport, Author, CommandBus, Outcome};
use flikk_core::changeset::ChangeItem;
use flikk_core::command::{Command, Warning};
use flikk_core::model::Project;
use flikk_core::services::{AnalysisService, MediaService, Services, SubjectService};
use flikk_core::{ChangeItemId, ChangeSetId, Time, TimeRange};
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often agent output and tool calls are drained on the UI thread.
const AGENT_PUMP: Duration = Duration::from_millis(33);
/// How often the model files are looked for on disk.
const MODELS_POLL: Duration = Duration::from_secs(1);
/// Key of the notice listing the models not downloaded.
const MODELS_NOTICE: &str = "models-missing";
/// How often subject mattes are looked for, so views draw them as they
/// are made.
const SUBJECTS_POLL: Duration = Duration::from_millis(250);
/// Key of the notice that subject masks need their model.
const SUBJECTS_NOTICE: &str = "subjects-model";
/// Key of the notice that optical flow needs its model.
const FLOW_NOTICE: &str = "optical-flow-model";
/// Key of the notice that person masks need their model.
const PEOPLE_NOTICE: &str = "people-model";

pub struct EditorState {
    pub bus: CommandBus,
    /// The open project's file; `None` until first saved.
    pub file: Option<PathBuf>,
    /// Undo position (see `undo_position`) as last saved or opened; `None` when
    /// the project opened with unsaved changes (recovered).
    saved: Option<u64>,
    /// What the autosaver was last told about the unsaved changes.
    autosaved: Autosaved,
    pub services: Services,
    pub workspace: Workspace,
    pub playhead: Time,
    pub in_out: Option<TimeRange>,
    pub selection: Selection,
    pub playing: bool,
    pub settings: AgentSettings,
    /// Messages of the active chat session.
    pub chat: Vec<ChatMessage>,
    /// Every chat session of the project. The active one's messages are
    /// edited in `chat` and copied back on save.
    chats: persistence::Chats,
    /// Change set whose draft the viewer plays instead of the committed project.
    pub preview: Option<ChangeSetId>,
    /// Uncommitted edit the viewer shows while a control is dragged.
    live: Option<Project>,
    /// Counts changes to `live`, so the viewer knows to redraw.
    pub live_version: u64,
    /// Last error or notice for the status line; cleared on the next action.
    pub status: Option<String>,
    // --- panels
    /// Pool item loaded in the viewer as source (double-click in the pool).
    /// The item playing on its own, or showing in the viewer.
    pub source: Option<Source>,
    /// In and out marked on pool items in the viewer, for the next edit
    /// with them; for the session.
    pub source_marks: std::collections::HashMap<flikk_core::MediaId, TimeRange>,
    /// Caption styles, loudness and ducking defaults of the selected style
    /// profile; edit through [`EditorState::edit_presets`] so they are saved.
    pub presets: flikk_core::memory::Presets,
    /// Index into `presets.caption_styles` used for new captions.
    pub caption_style: usize,
    /// Unsaved caption style the viewer shows while a control is dragged,
    /// with its index.
    caption_live: Option<(usize, flikk_core::memory::CaptionStyle)>,
    /// The caption styles `services.lettering` letters in.
    lettering_styles: Vec<flikk_core::memory::CaptionStyle>,
    /// Counts changes to `services.lettering`, so the viewer knows to redraw.
    pub lettering_version: u64,
    /// `services.subjects`' version when last looked at: mattes made since
    /// show on the next draw.
    pub subjects_version: u64,
    /// A depth effect whose focus the next click in the viewer picks.
    pub depth_pick: Option<(flikk_core::ClipId, flikk_core::EffectId)>,
    // --- timeline/viewer
    /// Shuttle speed while playing: 1.0 is real time, negative plays backwards (JKL).
    pub play_speed: f64,
    /// Change item picked on the timeline overlay; the chat panel reveals it.
    pub focused_change: Option<(ChangeSetId, ChangeItemId)>,
    /// Transition picked on the timeline: the clip and edge it sits on. The
    /// inspector shows it.
    pub selected_transition: Option<(flikk_core::ClipId, flikk_core::command::Edge)>,
    /// `@` references waiting for the chat composer ("Add to chat" from the
    /// timeline, inspector, transcript…): token text and chip label.
    pending_mentions: Vec<(String, SharedString)>,
    clock: Option<Task<()>>,
    agent: AgentLink,
    _pump: Task<()>,
    // --- chat
    /// Requests the user settled since their last message, and what came of them.
    pub settled: Vec<(PendingApproval, ApprovalOutcome)>,
    /// Renders approved in the chat. Clones share the queue.
    pub render_queue: flikk_deliver::RenderQueue,
    // --- library (effects library, inspector, FX code editor)
    /// FX program open in the code editor; built-ins open read-only.
    pub editing_fx: Option<flikk_core::fx::FxRef>,
    /// The user's programs and presets, as last read from disk.
    pub library: flikk_core::library::Library,
    // --- models
    /// The tasks analysis runs on a model, in the order it runs them.
    pub model_tasks: Vec<flikk_models::Task>,
    /// Model downloads, started only by the user. Clones share them.
    pub models: flikk_models::Downloads,
    /// Warnings and errors under the top bar's notice button.
    pub notices: Notices,
    /// Media whose source file is missing.
    pub offline: HashSet<flikk_core::MediaId>,
    /// The media paths `offline` was worked out for.
    media_paths: Vec<(flikk_core::MediaId, PathBuf)>,
    /// Which of `model_tasks` had their model, and `models.finished()`, when
    /// the notices were last brought up to date.
    models_seen: (Vec<bool>, u64),
    _models_poll: Task<()>,
    _subjects_poll: Task<()>,
    /// Models each backend's CLI offers: the list it gave last time until
    /// it answers again.
    pub agent_models: HashMap<Backend, Vec<AgentModel>>,
    /// Backends whose CLI was asked for its models this run, or is being.
    models_asked: HashSet<Backend>,
    // --- online providers
    /// Transcription and generation on the providers picked in Settings →
    /// Integrations; also `services.generation`.
    pub cloud: Arc<flikk_cloud::Cloud>,
    /// Each provider's key and account, by provider id.
    providers: HashMap<&'static str, ProviderStatus>,
    /// Music and sound effects being made from the media pool.
    pub generating: Vec<Generating>,
    next_generation: u64,
    /// What was made for a project closed before it was done, added to its
    /// pool when it is opened again.
    kept: Vec<(flikk_core::ProjectId, flikk_core::model::MediaItem)>,
    /// Generated items in the pool already looked at for analysis.
    generated_seen: HashSet<flikk_core::MediaId>,
    /// The voices of the provider doing voiceovers, as last read.
    pub account_voices: voices::AccountVoices,
    /// Samples fetched for approval cards, by voice id; `None` while on
    /// their way.
    pub heard_voices: HashMap<String, Option<flikk_core::model::MediaItem>>,
    /// Heavy ranges composed onto disk while the user is idle.
    pub render: crate::render_cache::Background,
    // --- updates
    /// A newer version of Flikk, from being found to being installed.
    updater: update::Updater,
    _update_watch: Task<()>,
}

impl EditorState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let persistence::Loaded {
            project,
            settings,
            file,
            recovered,
        } = persistence::load_session();
        // Before analysis is wired, so it runs on the models the user picked.
        flikk_models::restore_picks(
            &persistence::with_store(|s| s.model_picks()).unwrap_or_default(),
        );
        let chats = persistence::load_chats(project.id);
        let chat = active_messages(&chats);
        let mut bus = CommandBus::new(project);
        bus.set_permissions(settings.permissions.clone());
        let position = undo_position(&bus);
        let pump = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AGENT_PUMP).await;
                if this.update(cx, |this, cx| this.pump_agent(cx)).is_err() {
                    break;
                }
            }
        });
        let models_poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(MODELS_POLL).await;
                if this.update(cx, |this, cx| this.poll_models(cx)).is_err() {
                    break;
                }
            }
        });
        let subjects_poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SUBJECTS_POLL).await;
                if this.update(cx, |this, cx| this.poll_subjects(cx)).is_err() {
                    break;
                }
            }
        });
        let cloud = Arc::new(flikk_cloud::Cloud::new(
            persistence::with_store(|s| s.cloud_settings()).unwrap_or_default(),
        ));
        let model_tasks = flikk_analysis::LocalAnalysis::model_tasks();
        let downloaded = model_tasks
            .iter()
            .map(|t| t.model().is_downloaded())
            .collect();
        let mut this = Self {
            bus,
            file,
            saved: (!recovered).then_some(position),
            autosaved: Autosaved::opened(recovered, position),
            services: services(&cloud),
            workspace: Workspace::default(),
            playhead: Time::ZERO,
            in_out: None,
            selection: Selection::default(),
            playing: false,
            settings,
            chat,
            chats,
            preview: None,
            live: None,
            live_version: 0,
            status: persistence::take_notice(),
            source: None,
            source_marks: Default::default(),
            presets: flikk_core::memory::Presets::default(),
            caption_style: 0,
            caption_live: None,
            lettering_styles: Vec::new(),
            lettering_version: 0,
            subjects_version: 0,
            depth_pick: None,
            play_speed: 1.0,
            focused_change: None,
            selected_transition: None,
            pending_mentions: Vec::new(),
            editing_fx: None,
            library: flikk_core::library::Library::load(),
            clock: None,
            agent: AgentLink::new(),
            _pump: pump,
            settled: Vec::new(),
            render_queue: flikk_deliver::RenderQueue::new(),
            models_seen: (downloaded, 0),
            model_tasks,
            models: flikk_models::Downloads::default(),
            notices: Notices::default(),
            offline: HashSet::new(),
            media_paths: Vec::new(),
            _models_poll: models_poll,
            _subjects_poll: subjects_poll,
            agent_models: HashMap::new(),
            models_asked: HashSet::new(),
            cloud,
            providers: HashMap::new(),
            generating: Vec::new(),
            next_generation: 1,
            kept: Vec::new(),
            generated_seen: HashSet::new(),
            account_voices: Default::default(),
            heard_voices: HashMap::new(),
            render: crate::render_cache::Background::new(cx),
            updater: update::Updater::default(),
            _update_watch: Self::watch_for_updates(cx),
        };
        this.mark_generated_seen();
        this.configure_analysis();
        this.analyze_sounds(cx);
        this.load_keys(cx);
        this.refresh_model_notices();
        this.check_offline();
        this.load_agent_memory();
        this.load_presets();
        this.load_agent_models(cx);
        // Quitting mid-render stops the renders and removes their part
        // files, rather than leaving ffmpeg to finish a cut file.
        cx.on_app_quit(|this, _| {
            this.render_queue.shutdown();
            async {}
        })
        .detach();
        this
    }

    /// Ask the backend's CLI which models it offers, once per run. Until it
    /// answers the list from last time stands; a failed ask is tried again
    /// on the next call.
    pub fn load_agent_models(&mut self, cx: &mut Context<Self>) {
        let backend = self.settings.backend;
        if !self.models_asked.insert(backend) {
            return;
        }
        self.agent_models
            .entry(backend)
            .or_insert_with(|| flikk_agent::backend::cached_models(backend));
        let task = cx
            .background_executor()
            .spawn(async move { flikk_agent::backend::refresh_models(backend) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(models) => {
                        this.agent_models.insert(backend, models);
                    }
                    Err(_) => {
                        this.models_asked.remove(&backend);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Hand the agent its role, its recipes, the approved rules of the
    /// selected style profile and the facts about this project's sources.
    fn load_agent_memory(&mut self) {
        self.agent.set_role(self.settings.role);
        self.agent.set_chat(&self.chats.active);
        self.bus.set_chat(&self.chats.active);
        use flikk_core::memory::{FactStatus, RuleStatus};
        use flikk_store::{FactFilter, RuleFilter};
        let recipes = persistence::with_store(|s| s.list_recipes()).unwrap_or_default();
        let rules = self
            .settings
            .style_profile
            .and_then(|profile| {
                let filter = RuleFilter {
                    status: Some(RuleStatus::Approved),
                    ..Default::default()
                };
                persistence::with_store(|s| s.rules(profile, &filter))
            })
            .unwrap_or_default();
        let fingerprints: Vec<_> = self
            .bus
            .project()
            .media
            .iter()
            .filter_map(|m| m.fingerprint.clone())
            .collect();
        let facts = fingerprints
            .into_iter()
            .flat_map(|fingerprint| {
                let filter = FactFilter {
                    fingerprint: Some(fingerprint),
                    ..Default::default()
                };
                persistence::with_store(|s| s.facts(&filter)).unwrap_or_default()
            })
            .filter(|f| f.status != FactStatus::Suspended)
            .collect();
        self.agent.set_recipes(recipes);
        self.agent.set_memory(rules, facts);
    }

    pub fn context(&self) -> EditorContext {
        EditorContext {
            workspace: self.workspace,
            playhead: self.playhead,
            in_out: self.in_out,
            selection: self.selection.clone(),
        }
    }

    /// What the viewer shows: a control being dragged, else the previewed
    /// change set's draft, else the committed project.
    pub fn display_project(&self) -> &Project {
        if let Some(live) = &self.live {
            return live;
        }
        self.preview
            .and_then(|id| self.bus.change_set(id))
            .and_then(|set| set.draft.as_ref())
            .unwrap_or(self.bus.project())
    }

    /// Show `commands` applied in the viewer without committing them; none
    /// clears it. Any committed edit clears it too.
    pub fn show_live(&mut self, commands: &[Command], cx: &mut Context<Self>) {
        let last = self.live.take();
        if commands.is_empty() {
            if last.is_some() {
                self.live_version += 1;
                cx.notify();
            }
            return;
        }
        let mut project = self.display_project().clone();
        let applied = commands.iter().all(|c| {
            c.plan(&project)
                .and_then(|plan| flikk_core::ops::apply_all(&mut project, &plan.ops))
                .is_ok()
        });
        // Not applicable: keep the last good preview.
        self.live = if applied { Some(project) } else { last };
        self.live_version += 1;
        cx.notify();
    }

    pub fn set_preview(&mut self, preview: Option<ChangeSetId>, cx: &mut Context<Self>) {
        self.preview = preview;
        cx.notify();
    }

    // ------------------------------------------------------------ hand edits

    pub fn execute(&mut self, command: Command, cx: &mut Context<Self>) -> Option<Outcome> {
        let result = self.bus.execute(command, Author::User, "");
        self.finish(result, cx)
    }

    pub fn execute_batch(
        &mut self,
        commands: Vec<Command>,
        label: &str,
        cx: &mut Context<Self>,
    ) -> Option<Outcome> {
        let result = self.bus.execute_batch(commands, label);
        self.finish(result, cx)
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let result = self.bus.undo();
        self.finish(result, cx);
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let result = self.bus.redo();
        self.finish(result, cx);
    }

    pub fn accept(
        &mut self,
        set: ChangeSetId,
        items: Option<&[ChangeItemId]>,
        cx: &mut Context<Self>,
    ) -> Option<AcceptReport> {
        let result = self.bus.accept(set, items);
        self.finish(result, cx)
    }

    pub fn reject(
        &mut self,
        set: ChangeSetId,
        items: Option<&[ChangeItemId]>,
        cx: &mut Context<Self>,
    ) {
        self.bus.reject(set, items);
        cx.notify();
    }

    fn finish<T>(&mut self, result: flikk_core::Result<T>, cx: &mut Context<Self>) -> Option<T> {
        if self.live.take().is_some() {
            self.live_version += 1;
        }
        self.status = result.as_ref().err().map(|e| e.to_string());
        if result.is_ok() {
            self.autosave();
            self.check_offline();
        }
        cx.notify();
        result.ok()
    }

    // ------------------------------------------------------------- navigation

    pub fn set_workspace(&mut self, workspace: Workspace, cx: &mut Context<Self>) {
        self.workspace = workspace;
        cx.notify();
    }

    pub fn seek(&mut self, t: Time, cx: &mut Context<Self>) {
        self.playhead = t.max(Time::ZERO);
        if self.source_picture().is_some() {
            self.source = None;
        }
        cx.notify();
    }

    /// Select clips. A clip selection and a transition selection exclude
    /// each other, whichever panel sets them.
    pub fn select_clips(&mut self, clips: Vec<flikk_core::ClipId>, cx: &mut Context<Self>) {
        if !clips.is_empty() {
            self.selected_transition = None;
        }
        self.selection.clips = clips;
        cx.notify();
    }

    /// Select a transition, which deselects the clips.
    pub fn select_transition(
        &mut self,
        transition: Option<(flikk_core::ClipId, flikk_core::command::Edge)>,
        cx: &mut Context<Self>,
    ) {
        if transition.is_some() {
            self.selection.clips.clear();
        }
        self.selected_transition = transition;
        cx.notify();
    }

    /// Queue `@` references (token text as the agent reads it, chip label)
    /// for the chat composer, which inserts them as chips.
    pub fn mention(&mut self, refs: Vec<(String, SharedString)>, cx: &mut Context<Self>) {
        self.pending_mentions.extend(refs);
        cx.notify();
    }

    /// References queued by [`Self::mention`], oldest first.
    pub fn take_mentions(&mut self) -> Vec<(String, SharedString)> {
        std::mem::take(&mut self.pending_mentions)
    }

    // --- panels

    /// Clips on the timeline that use `media`.
    pub fn clips_using(&self, media: flikk_core::MediaId) -> Vec<flikk_core::ClipId> {
        self.bus
            .project()
            .timeline
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| c.media() == Some(media))
            .map(|c| c.id)
            .collect()
    }

    /// Remove pool items and their clips, as one undo step.
    pub fn remove_media(&mut self, media: &[flikk_core::MediaId], cx: &mut Context<Self>) {
        self.remove_from_pool(media, None, cx);
    }

    /// Remove pool items and their clips and, with `bins`, set the pool's
    /// bins to those: one undo step.
    pub fn remove_from_pool(
        &mut self,
        media: &[flikk_core::MediaId],
        bins: Option<Vec<flikk_core::model::Bin>>,
        cx: &mut Context<Self>,
    ) {
        let clips: Vec<_> = media.iter().flat_map(|m| self.clips_using(*m)).collect();
        let mut commands = Vec::new();
        if !clips.is_empty() {
            commands.push(Command::DeleteClips {
                clips: clips.clone(),
                ripple: false,
            });
        }
        commands.extend(media.iter().map(|&media| Command::RemoveMedia { media }));
        let label = if bins.is_some() {
            "delete_bin"
        } else {
            "remove_media"
        };
        commands.extend(bins.map(|bins| Command::SetBins { bins }));
        if self.execute_batch(commands, label, cx).is_some() {
            self.selection.clips.retain(|c| !clips.contains(c));
            if self
                .source
                .as_ref()
                .is_some_and(|s| media.contains(&s.item().id))
            {
                self.source = None;
            }
        }
    }

    pub fn select_words(
        &mut self,
        words: Option<(flikk_core::MediaId, TimeRange)>,
        cx: &mut Context<Self>,
    ) {
        self.selection.words = words;
        cx.notify();
    }

    /// The selected style profile's presets, or the defaults without one.
    fn load_presets(&mut self) {
        let mut presets = self
            .settings
            .style_profile
            .and_then(|id| persistence::with_store(|s| s.profile_header(id)).flatten())
            .map(|profile| profile.presets)
            .unwrap_or_default();
        if presets.caption_styles.is_empty() {
            presets.caption_styles = flikk_core::memory::Presets::default().caption_styles;
        }
        self.presets = presets;
        self.caption_style = 0;
        self.sync_caption_style();
        self.refresh_lettering();
    }

    /// Letter captions in the styles the viewer shows, when they changed.
    fn refresh_lettering(&mut self) {
        let styles = self.caption_styles().into_owned();
        if self.services.lettering.is_some() && styles == self.lettering_styles {
            return;
        }
        self.services.lettering = Some(Arc::new(flikk_deliver::Letterer::new(styles.clone())));
        self.lettering_styles = styles;
        self.lettering_version += 1;
    }

    /// Change the presets and save them to the selected style profile,
    /// creating a "Default" profile if there is none yet.
    pub fn edit_presets(
        &mut self,
        f: impl FnOnce(&mut flikk_core::memory::Presets),
        cx: &mut Context<Self>,
    ) {
        let before = self.presets.clone();
        f(&mut self.presets);
        self.refresh_lettering();
        if self.presets == before {
            return;
        }
        let profile = self
            .settings
            .style_profile
            .filter(|id| {
                persistence::with_store(|s| s.profile_header(*id))
                    .flatten()
                    .is_some()
            })
            .or_else(|| persistence::with_store(|s| s.create_profile("Default")).map(|p| p.id));
        if let Some(id) = profile {
            persistence::with_store(|s| s.set_presets(id, &self.presets));
            if self.settings.style_profile != Some(id) {
                let mut settings = self.settings.clone();
                settings.style_profile = Some(id);
                // The profile already holds these presets.
                persistence::save_settings(&settings);
                self.settings = settings;
                self.load_agent_memory();
            }
        }
        if let Some(notice) = persistence::take_notice() {
            self.status = Some(notice);
        }
        cx.notify();
    }

    /// Make preset `index` the active caption style and save its name on
    /// the first caption track, so the choice travels with the project.
    pub fn pick_caption_style(&mut self, index: usize, cx: &mut Context<Self>) {
        self.caption_style = index;
        let name = self
            .presets
            .caption_styles
            .get(index)
            .map(|s| s.name.clone());
        let track = self
            .bus
            .project()
            .timeline
            .tracks
            .iter()
            .find(|t| t.kind == flikk_core::model::TrackKind::Caption);
        match track {
            Some(t) if name.is_some() && t.caption_style != name => {
                let track = t.id;
                self.execute(Command::SetCaptionStyle { track, style: name }, cx);
            }
            _ => cx.notify(),
        }
    }

    /// Follow the caption track's saved style, e.g. after a project loads.
    pub fn sync_caption_style(&mut self) {
        let saved = self
            .bus
            .project()
            .timeline
            .tracks
            .iter()
            .find(|t| t.kind == flikk_core::model::TrackKind::Caption)
            .and_then(|t| t.caption_style.as_deref());
        if let Some(i) = saved.and_then(|name| {
            self.presets
                .caption_styles
                .iter()
                .position(|s| s.name == name)
        }) {
            self.caption_style = i;
        }
    }

    /// Caption styles as the viewer shows them: with the one being dragged.
    pub fn caption_styles(&self) -> std::borrow::Cow<'_, [flikk_core::memory::CaptionStyle]> {
        let styles = &self.presets.caption_styles;
        match &self.caption_live {
            Some((i, live)) if *i < styles.len() => {
                let mut styles = styles.clone();
                styles[*i] = live.clone();
                std::borrow::Cow::Owned(styles)
            }
            _ => std::borrow::Cow::Borrowed(styles),
        }
    }

    /// Show caption style `index` as `style` without saving it.
    pub fn show_caption_style(
        &mut self,
        index: usize,
        style: flikk_core::memory::CaptionStyle,
        cx: &mut Context<Self>,
    ) {
        self.caption_live = Some((index, style));
        self.refresh_lettering();
        cx.notify();
    }

    /// Change caption style `index` and save it.
    pub fn edit_caption_style(
        &mut self,
        index: usize,
        f: impl FnOnce(&mut flikk_core::memory::CaptionStyle),
        cx: &mut Context<Self>,
    ) {
        self.caption_live = None;
        self.edit_presets(
            |p| {
                if let Some(style) = p.caption_styles.get_mut(index) {
                    f(style);
                }
            },
            cx,
        );
        self.refresh_lettering();
        cx.notify();
    }

    /// The preset index caption `track` is styled with: its named preset,
    /// else the first.
    pub fn track_caption_style(&self, track: flikk_core::TrackId) -> usize {
        self.bus
            .project()
            .timeline
            .track(track)
            .and_then(|t| t.caption_style.as_deref())
            .and_then(|name| {
                self.presets
                    .caption_styles
                    .iter()
                    .position(|s| s.name == name)
            })
            .unwrap_or(0)
    }

    /// Style caption `track` with preset `index`. The first caption track's
    /// style is also the one new captions use.
    pub fn set_track_caption_style(
        &mut self,
        track: flikk_core::TrackId,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let first = self
            .bus
            .project()
            .timeline
            .tracks
            .iter()
            .find(|t| t.kind == flikk_core::model::TrackKind::Caption)
            .map(|t| t.id);
        if first == Some(track) {
            self.pick_caption_style(index, cx);
        } else if let Some(style) = self.presets.caption_styles.get(index) {
            let style = Some(style.name.clone());
            self.execute(Command::SetCaptionStyle { track, style }, cx);
        }
    }

    /// The caption style new captions use.
    pub fn active_caption_style(&self) -> flikk_core::memory::CaptionStyle {
        self.presets
            .caption_styles
            .get(self.caption_style)
            .cloned()
            .unwrap_or_default()
    }

    // ------------------------------------------------------------------ agent

    /// A message is queued or its turn under way: another is refused.
    pub fn agent_busy(&self) -> bool {
        self.agent.status().is_busy()
    }

    /// Send `text` as the user's message. Does nothing while the agent is
    /// busy: it would refuse the message, which must then not show, nor
    /// clear the settled cards.
    pub fn send_chat(&mut self, text: String, cx: &mut Context<Self>) {
        if self.agent_busy() {
            return;
        }
        self.chat.push(ChatMessage {
            role: ChatRole::User,
            text: text.clone(),
            choices: vec![],
            change_set: None,
            tools: vec![],
            at: now(),
        });
        let context = self.context();
        self.agent.send(text, context, &self.settings);
        self.clear_settled();
        self.save_chat();
        cx.notify();
    }

    fn save_chat(&mut self) {
        let id = self.chats.active.clone();
        let title = self
            .chat
            .iter()
            .find(|m| m.role == ChatRole::User)
            .map(|m| chat_title(&crate::ui::chat::plain_text(&m.text, self.bus.project())));
        let Some(session) = self.chats.sessions.iter_mut().find(|s| s.id == id) else {
            return;
        };
        if session.title.is_empty()
            && let Some(title) = title
        {
            session.title = title;
        }
        if let Some(last) = self.chat.last() {
            session.at = last.at;
        }
        session.messages = self.chat.clone();
        persistence::save_chats(self.bus.project().id, &self.chats);
    }

    // ---------------------------------------------------------- chat sessions

    /// Chat sessions, most recent first, as `(id, title)`.
    pub fn chat_sessions(&self) -> Vec<(String, String)> {
        let mut sessions: Vec<_> = self.chats.sessions.iter().collect();
        sessions.sort_by_key(|s| std::cmp::Reverse(s.at));
        sessions
            .into_iter()
            .map(|s| {
                // The active chat may have messages not saved into it yet.
                let first = (s.id == self.chats.active)
                    .then(|| self.chat.iter().find(|m| m.role == ChatRole::User))
                    .flatten();
                let title = match first {
                    _ if !s.title.is_empty() => s.title.clone(),
                    Some(m) => {
                        chat_title(&crate::ui::chat::plain_text(&m.text, self.bus.project()))
                    }
                    None => "Untitled".to_owned(),
                };
                (s.id.clone(), title)
            })
            .collect()
    }

    pub fn active_chat(&self) -> &str {
        &self.chats.active
    }

    /// Start a chat with a fresh agent conversation. An empty active chat
    /// is reused instead of piling up blank ones.
    pub fn new_chat(&mut self, cx: &mut Context<Self>) {
        if self.agent_busy() || self.chat.is_empty() {
            return;
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.chats
            .sessions
            .push(persistence::ChatSession::new(id.clone(), now()));
        self.switch_chat(&id, cx);
    }

    pub fn switch_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.agent_busy()
            || id == self.chats.active
            || !self.chats.sessions.iter().any(|s| s.id == id)
        {
            return;
        }
        self.save_chat();
        self.chats.active = id.to_owned();
        // A chat left without a message has nothing to come back to.
        self.chats
            .sessions
            .retain(|s| s.id == id || !s.messages.is_empty());
        self.chat = active_messages(&self.chats);
        self.settled.clear();
        self.focused_change = None;
        self.preview = None;
        self.agent.set_chat(id);
        self.bus.set_chat(id);
        persistence::save_chats(self.bus.project().id, &self.chats);
        cx.notify();
    }

    /// Delete a chat and forget its agent conversation. Deleting the active
    /// chat opens the most recent other one, or a new empty chat.
    pub fn delete_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.agent_busy() {
            return;
        }
        let project = self.bus.project().id;
        if id == self.chats.active {
            let next = self
                .chat_sessions()
                .into_iter()
                .map(|(id, _)| id)
                .find(|other| other != id);
            match next {
                Some(next) => self.switch_chat(&next, cx),
                None => {
                    let fresh = uuid::Uuid::new_v4().simple().to_string();
                    self.chats
                        .sessions
                        .push(persistence::ChatSession::new(fresh.clone(), now()));
                    self.chat.clear();
                    self.chats.active = fresh.clone();
                    self.agent.set_chat(&fresh);
                    self.bus.set_chat(&fresh);
                }
            }
        }
        self.chats.sessions.retain(|s| s.id != id);
        flikk_agent::session::forget_chat(project, id);
        persistence::save_chats(project, &self.chats);
        cx.notify();
    }

    // --------------------------------------------------------------- projects

    /// Changes not saved to the project's file (or never saved at all).
    /// Undoing back to the saved state makes it clean again.
    pub fn is_dirty(&self) -> bool {
        self.saved != Some(undo_position(&self.bus))
    }

    /// Recently opened project files that still exist, newest first.
    pub fn recent_projects(&self) -> Vec<PathBuf> {
        persistence::recent()
            .into_iter()
            .filter(|p| p.exists())
            .collect()
    }

    /// Hand unsaved changes to the autosaver, for crash recovery.
    fn autosave(&mut self) {
        if !self.is_dirty() {
            // Undone back to the saved state: what was kept is stale.
            if matches!(self.autosaved, Autosaved::Kept(_)) {
                persistence::discard(self.bus.project().id);
                self.autosaved = Autosaved::Nothing;
            }
            return;
        }
        let position = undo_position(&self.bus);
        if self.autosaved.covers(position) {
            return;
        }
        self.autosaved = Autosaved::Kept(position);
        persistence::autosave(self.bus.project(), self.file.as_deref());
        if let Some(e) = persistence::autosave_failure() {
            self.status = Some(format!(
                "Unsaved changes could not be kept for recovery: {e}"
            ));
        }
    }

    /// Replace the open project with a new one saved at `path`, which names
    /// it, with a sequence of `settings`. Unsaved changes must be settled
    /// first (see `crate::projects`). Whether it saved.
    pub fn create_project(
        &mut self,
        path: PathBuf,
        settings: flikk_core::model::SequenceSettings,
        cx: &mut Context<Self>,
    ) -> bool {
        let name = flikk_store::project_file::name_of(&path);
        self.switch_to(persistence::new_project(&name, settings), cx);
        self.save_project(path, cx)
    }

    /// Open a project file in place of the open project; unsaved changes
    /// must be settled first. Whether it opened.
    pub fn open_project(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        if self.file.as_deref() == Some(path) {
            return true;
        }
        match persistence::open(path) {
            Some(loaded) => {
                self.switch_to(loaded, cx);
                true
            }
            None => {
                if !path.exists() {
                    persistence::forget_recent(path);
                }
                self.status = persistence::take_notice();
                cx.notify();
                false
            }
        }
    }

    /// Save the project to `path`, which becomes its file (Save As when it
    /// differs). The project takes the file's name. Whether it saved.
    pub fn save_project(&mut self, path: PathBuf, cx: &mut Context<Self>) -> bool {
        let name = flikk_store::project_file::name_of(&path);
        if self.bus.project().name != name {
            self.bus.rename_project(&name);
        }
        let saved = persistence::save(&path, self.bus.project());
        if saved {
            self.file = Some(path);
            self.saved = Some(undo_position(&self.bus));
            self.autosaved = Autosaved::Nothing;
            self.status = None;
        } else {
            self.status = persistence::take_notice();
        }
        cx.notify();
        saved
    }

    /// The user chose not to save: forget the unsaved changes kept for
    /// crash recovery, and keep them out of later autosaves (and the one
    /// written on quit). Edits made after this are kept as usual.
    pub fn discard_changes(&mut self) {
        persistence::discard(self.bus.project().id);
        self.autosaved = Autosaved::Discarded(undo_position(&self.bus));
    }

    /// Replace everything tied to the open project; engines stay. Undo
    /// history does not carry over.
    fn switch_to(&mut self, loaded: persistence::Loaded, cx: &mut Context<Self>) {
        self.set_playing(false, cx);
        self.agent.cancel();
        self.agent.close_project(self.bus.project().id);
        self.save_chat();
        self.chats = persistence::load_chats(loaded.project.id);
        self.chat = active_messages(&self.chats);
        self.bus = CommandBus::new(loaded.project);
        self.bus
            .set_permissions(loaded.settings.permissions.clone());
        self.mark_generated_seen();
        let position = undo_position(&self.bus);
        self.file = loaded.file;
        self.saved = (!loaded.recovered).then_some(position);
        self.autosaved = Autosaved::opened(loaded.recovered, position);
        self.settings = loaded.settings;
        self.playhead = Time::ZERO;
        self.in_out = None;
        self.selection = Selection::default();
        self.selected_transition = None;
        self.source = None;
        self.preview = None;
        self.focused_change = None;
        self.editing_fx = None;
        self.settled.clear();
        self.status = persistence::take_notice();
        self.check_offline();
        self.load_agent_memory();
        self.load_presets();
        self.add_kept(cx);
        cx.notify();
    }

    /// Work out which sources are missing when the media list changed, and
    /// keep the notice about them current.
    fn check_offline(&mut self) {
        let paths: Vec<_> = self
            .bus
            .project()
            .media
            .iter()
            .map(|m| (m.id, m.path.clone()))
            .collect();
        if paths == self.media_paths {
            return;
        }
        self.offline = paths
            .iter()
            .filter(|(_, p)| !p.exists())
            .map(|(id, _)| *id)
            .collect();
        self.media_paths = paths;
        self.offline_notice();
    }

    /// Missing sources that came back, e.g. a drive plugged in again.
    /// Whether any did.
    fn recheck_offline(&mut self) -> bool {
        let project = self.bus.project();
        let back: Vec<_> = self
            .offline
            .iter()
            .copied()
            .filter(|id| project.media(*id).is_some_and(|m| m.path.exists()))
            .collect();
        if back.is_empty() {
            return false;
        }
        for id in back {
            self.offline.remove(&id);
        }
        self.offline_notice();
        true
    }

    fn offline_notice(&mut self) {
        const KEY: &str = "offline-media";
        if self.offline.is_empty() {
            self.notices.clear(KEY);
            return;
        }
        let items: Vec<String> = self
            .bus
            .project()
            .media
            .iter()
            .filter(|m| self.offline.contains(&m.id))
            .map(crate::ui::media_pool::item_name)
            .collect();
        let title = match items.len() {
            1 => "1 media file is offline".to_owned(),
            n => format!("{n} media files are offline"),
        };
        self.notices.set(Notice {
            key: KEY.into(),
            level: Level::Warning,
            title,
            items,
            detail: None,
            action: Some(NoticeAction::Relink),
        });
    }

    pub fn cancel_agent(&mut self, cx: &mut Context<Self>) {
        self.agent.cancel();
        cx.notify();
    }

    pub fn set_settings(&mut self, settings: AgentSettings, cx: &mut Context<Self>) {
        self.bus.set_permissions(settings.permissions.clone());
        persistence::save_settings(&settings);
        let profile_changed = settings.style_profile != self.settings.style_profile;
        let backend_changed = settings.backend != self.settings.backend;
        self.settings = settings;
        self.load_agent_memory();
        if profile_changed {
            self.load_presets();
        }
        if backend_changed {
            self.load_agent_models(cx);
        }
        cx.notify();
    }

    fn pump_agent(&mut self, cx: &mut Context<Self>) {
        let events = self.agent.pump(&mut self.bus, &self.services);
        for (project, item) in self.agent.take_orphans() {
            self.keep_for(project, item);
        }
        // A job the agent was allowed may have added what a provider made.
        self.analyze_generated(cx);
        if events.is_empty() {
            return;
        }
        for event in events {
            match event {
                AgentEvent::TextDelta { text } => match self.chat.last_mut() {
                    Some(m)
                        if m.role == ChatRole::Agent
                            && m.change_set.is_none()
                            && m.choices.is_empty()
                            && m.tools.is_empty() =>
                    {
                        m.text.push_str(&text)
                    }
                    _ => self.push_agent(text, vec![]),
                },
                AgentEvent::Question { text, choices } => self.push_agent(text, choices),
                AgentEvent::ChangeSetReady { change_set } => self.post_change_set(change_set),
                AgentEvent::Activity { entry } => {
                    self.tool_done(&entry, cx);
                    self.push_tool(entry)
                }
                AgentEvent::ToolStarted { entry, .. } => self.push_tool(entry),
                AgentEvent::ToolFinished { call, entry } => {
                    self.tool_done(&entry, cx);
                    self.finish_tool(call, entry)
                }
                AgentEvent::TurnDone => {
                    self.stop_tools();
                    self.save_chat()
                }
                AgentEvent::Progress { .. } => {}
                AgentEvent::Error { message } => self.status = Some(message),
            }
        }
        // Generated media the agent was allowed to make went into the pool.
        self.autosave();
        cx.notify();
    }

    /// Close `change_set` for review. Its card sits above the composer
    /// rather than in the thread, so a set the chat grew over several turns
    /// shows once.
    fn post_change_set(&mut self, change_set: ChangeSetId) {
        self.bus.close_change_set(change_set);
        self.chat.retain(|m| m.change_set != Some(change_set));
    }

    fn push_agent(&mut self, text: String, choices: Vec<String>) {
        self.chat.push(ChatMessage {
            role: ChatRole::Agent,
            text,
            choices,
            change_set: None,
            tools: vec![],
            at: now(),
        });
    }

    fn tool_done(&mut self, entry: &ActivityEntry, cx: &mut Context<Self>) {
        // A voice the agent was allowed to add or design went to the
        // library.
        if entry.ok && matches!(entry.tool.as_str(), "add_voice" | "design_voice") {
            self.reload_library(cx);
            self.refresh_voices(cx);
        }
    }

    /// Put `entry` where call `call` was shown running. A call no longer
    /// shown (its turn was stopped, or another chat is open) is dropped.
    fn finish_tool(&mut self, call: u64, entry: ActivityEntry) {
        let row = self
            .chat
            .iter_mut()
            .rev()
            .flat_map(|m| m.tools.iter_mut())
            .find(|t| t.running == Some(call));
        if let Some(row) = row {
            *row = entry;
        }
    }

    /// Calls still shown running when the turn ends were stopped with it.
    fn stop_tools(&mut self) {
        for row in self.chat.iter_mut().flat_map(|m| m.tools.iter_mut()) {
            if row.running.take().is_some() {
                row.ok = false;
                row.summary = "Stopped".into();
            }
        }
    }

    /// Calls in a row stack into one message; anything else in between
    /// starts a new stack.
    fn push_tool(&mut self, entry: ActivityEntry) {
        match self.chat.last_mut() {
            Some(m) if m.role == ChatRole::Agent && !m.tools.is_empty() => m.tools.push(entry),
            _ => self.chat.push(ChatMessage {
                role: ChatRole::Agent,
                text: String::new(),
                choices: vec![],
                change_set: None,
                tools: vec![entry],
                at: now(),
            }),
        }
    }
}

/// The engines, wired once per window. Media is optional: without FFmpeg
/// the editor still opens, and panels degrade quietly.
fn services(cloud: &Arc<flikk_cloud::Cloud>) -> Services {
    let media = match flikk_media::FfmpegMedia::new() {
        Ok(m) => Some(Arc::new(m) as Arc<dyn MediaService>),
        Err(e) => {
            eprintln!("media engine unavailable: {e}");
            None
        }
    };
    let fx = match flikk_gpu::GpuFx::new() {
        Ok(gpu) => Some(Arc::new(gpu) as Arc<dyn flikk_core::services::FxService>),
        Err(e) => {
            eprintln!("GPU effects unavailable: {e}");
            None
        }
    };
    let html = match flikk_html::HtmlEngine::new() {
        Ok(h) => Some(Arc::new(h) as Arc<dyn flikk_core::services::HtmlService>),
        Err(e) => {
            eprintln!("HTML engine unavailable: {e}");
            None
        }
    };
    let subjects = media
        .clone()
        .map(|m| Arc::new(flikk_analysis::LocalSubjects::new(m)) as Arc<dyn SubjectService>);
    let stabilizer = media.clone().map(|m| {
        Arc::new(flikk_analysis::LocalStabilizer::new(m))
            as Arc<dyn flikk_core::stabilize::Stabilizer>
    });
    let depth = media.clone().map(|m| {
        Arc::new(flikk_analysis::LocalDepth::new(m)) as Arc<dyn flikk_core::depth::DepthService>
    });
    Services {
        media,
        analysis: Some(
            Arc::new(flikk_analysis::LocalAnalysis::default()) as Arc<dyn AnalysisService>
        ),
        fx,
        html,
        subjects,
        stabilizer,
        interpolation: Some(Arc::new(flikk_analysis::LocalInterpolation::new())
            as Arc<dyn flikk_core::services::InterpolationService>),
        depth,
        // Made with the caption styles, see `EditorState::refresh_lettering`.
        lettering: None,
        generation: Some(cloud.clone() as Arc<dyn flikk_core::cloud::GenerationService>),
    }
}

// --- models
impl EditorState {
    /// Download `model`: the user asked for it.
    pub fn download_model(&mut self, model: &'static flikk_models::Model, cx: &mut Context<Self>) {
        self.notices.clear(&download_notice(model));
        self.models.start(model);
        cx.notify();
    }

    pub fn cancel_download(&mut self, model: &flikk_models::Model, cx: &mut Context<Self>) {
        self.models.cancel(model);
        cx.notify();
    }

    /// Delete `model`'s files: the user asked. Analysis that needs it waits
    /// until it is downloaded again.
    pub fn delete_model(&mut self, model: &flikk_models::Model, cx: &mut Context<Self>) {
        if let Err(e) = model.delete() {
            self.status = Some(format!("{e:#}"));
        }
        self.poll_models(cx);
        cx.notify();
    }

    /// Run `task` on `model` from now on: the user picked it. Media
    /// analyzed already keeps its results.
    pub fn pick_model(
        &mut self,
        task: flikk_models::Task,
        model: &'static flikk_models::Model,
        cx: &mut Context<Self>,
    ) {
        if task.model().id == model.id {
            return;
        }
        task.pick(model);
        persistence::with_store(|s| s.save_model_picks(&flikk_models::picked()));
        self.poll_models(cx);
        self.refresh_model_notices();
        cx.notify();
    }

    /// Media can be analyzed: every analysis task has a model downloaded
    /// or the provider set to transcribe doing it, with its key.
    pub fn models_ready(&self) -> bool {
        let online = self
            .online_transcription()
            .is_none_or(|p| self.provider_status(p).has_key());
        online
            && self
                .required_tasks()
                .iter()
                .all(|t| t.model().is_downloaded())
    }

    /// Analyze `media` (again, replacing its results, when `again`): the
    /// user asked. Waits its turn behind the items queued before it.
    pub fn analyze(&mut self, media: flikk_core::MediaId, again: bool, cx: &mut Context<Self>) {
        let (Some(analysis), Some(item)) =
            (&self.services.analysis, self.bus.project().media(media))
        else {
            return;
        };
        if again {
            analysis.analyze_again(item);
        } else {
            analysis.analyze(item);
        }
        cx.notify();
    }

    /// Whether subject masks can be made; when not (their model is
    /// missing), a notice that leads to Settings → Integrations.
    pub fn subjects_ready(&mut self, cx: &mut Context<Self>) -> bool {
        let ready = match &self.services.subjects {
            Some(s) => s.available(),
            None => Err("Subject masks need the media engine (FFmpeg).".into()),
        };
        let changed = match &ready {
            Ok(()) => self.notices.clear(SUBJECTS_NOTICE),
            Err(e) => {
                // Asked for just now: shown even if closed before.
                self.notices.clear(SUBJECTS_NOTICE);
                self.notices.set(Notice {
                    key: SUBJECTS_NOTICE.into(),
                    level: Level::Warning,
                    title: "Subject masks need their model".into(),
                    items: Vec::new(),
                    detail: Some(e.clone()),
                    action: Some(NoticeAction::OpenModels),
                })
            }
        };
        if changed {
            cx.notify();
        }
        ready.is_ok()
    }

    /// When optical flow is picked without its model: a notice that leads
    /// to Settings → Integrations (slowed frames are blended meanwhile).
    pub fn optical_flow_picked(&mut self, cx: &mut Context<Self>) {
        let Some(Err(e)) = self.services.interpolation.as_ref().map(|s| s.available()) else {
            return;
        };
        self.notices.clear(FLOW_NOTICE);
        self.notices.set(Notice {
            key: FLOW_NOTICE.into(),
            level: Level::Warning,
            title: "Motion smoothing needs its model".into(),
            items: Vec::new(),
            detail: Some(e),
            action: Some(NoticeAction::OpenModels),
        });
        cx.notify();
    }

    /// Whether person masks can be made; when not (their model is
    /// missing), a notice that leads to Settings → Integrations.
    pub fn people_ready(&mut self, cx: &mut Context<Self>) -> bool {
        let ready = match &self.services.subjects {
            Some(s) => s.people_available(),
            None => Err("Person masks need the media engine (FFmpeg).".into()),
        };
        let changed = match &ready {
            Ok(()) => self.notices.clear(PEOPLE_NOTICE),
            Err(e) => {
                self.notices.clear(PEOPLE_NOTICE);
                self.notices.set(Notice {
                    key: PEOPLE_NOTICE.into(),
                    level: Level::Warning,
                    title: "Person masks need their model".into(),
                    items: Vec::new(),
                    detail: Some(e.clone()),
                    action: Some(NoticeAction::OpenModels),
                })
            }
        };
        if changed {
            cx.notify();
        }
        ready.is_ok()
    }

    /// Mattes were made, frames made between, or camera paths measured:
    /// draw again.
    fn poll_subjects(&mut self, cx: &mut Context<Self>) {
        let version = self.services.subjects.as_ref().map_or(0, |s| s.version())
            + self
                .services
                .interpolation
                .as_ref()
                .map_or(0, |s| s.version())
            + self.services.stabilizer.as_ref().map_or(0, |s| s.version())
            + self.services.depth.as_ref().map_or(0, |d| d.version());
        if version != self.subjects_version {
            self.subjects_version = version;
            cx.notify();
        }
    }

    pub fn dismiss_notice(&mut self, key: &str, cx: &mut Context<Self>) {
        self.notices.dismiss(key);
        cx.notify();
    }

    /// When a download ends or a model file appears or goes, bring the
    /// notices up to date.
    fn poll_models(&mut self, cx: &mut Context<Self>) {
        // The same clock looks for offline sources coming back.
        if self.recheck_offline() {
            cx.notify();
        }
        let seen = (
            self.model_tasks
                .iter()
                .map(|t| t.model().is_downloaded())
                .collect::<Vec<_>>(),
            self.models.finished(),
        );
        if seen == self.models_seen {
            return;
        }
        self.models_seen = seen;
        self.analyze_sounds(cx);
        if self.refresh_model_notices() {
            cx.notify();
        }
    }

    /// One notice listing the models analysis needs that are not
    /// downloaded, one for each download that failed, and one for a
    /// provider without its key. Whether any changed.
    fn refresh_model_notices(&mut self) -> bool {
        let mut changed = self.refresh_key_notice();
        let mut missing = Vec::new();
        let required = self.required_tasks();
        for task in &self.model_tasks {
            let model = task.model();
            let key = download_notice(model);
            match self.models.status(model) {
                flikk_models::Status::Downloaded => {
                    changed |= self.notices.clear(&key);
                    continue;
                }
                flikk_models::Status::Failed(message) => {
                    changed |= self.notices.set(Notice {
                        key,
                        level: Level::Error,
                        title: format!("Could not download {}", task.label().to_lowercase()),
                        items: Vec::new(),
                        detail: Some(message),
                        action: Some(NoticeAction::OpenModels),
                    });
                }
                flikk_models::Status::Missing | flikk_models::Status::Downloading(_) => {}
            }
            if required.contains(task) {
                missing.push(task.label().to_owned());
            }
        }
        changed |= if missing.is_empty() {
            self.notices.clear(MODELS_NOTICE)
        } else {
            self.notices.set(Notice {
                key: MODELS_NOTICE.into(),
                level: Level::Warning,
                title: "Some models are not downloaded".into(),
                items: missing,
                detail: Some("Features that use them may not be available.".into()),
                action: Some(NoticeAction::OpenModels),
            })
        };
        changed
    }
}

fn download_notice(model: &flikk_models::Model) -> String {
    format!("download-failed:{}", model.id)
}

// --- timeline/viewer
impl EditorState {
    /// Like [`Self::execute`], but reports plan warnings (protected ranges)
    /// in the status line.
    pub fn edit(&mut self, command: Command, cx: &mut Context<Self>) -> Option<Outcome> {
        let outcome = self.execute(command, cx);
        self.report_warnings(outcome.as_ref());
        outcome
    }

    /// Several commands as one undo step, reporting warnings like [`Self::edit`].
    pub fn edit_batch(
        &mut self,
        commands: Vec<Command>,
        label: &str,
        cx: &mut Context<Self>,
    ) -> Option<Outcome> {
        let outcome = self.execute_batch(commands, label, cx);
        self.report_warnings(outcome.as_ref());
        outcome
    }

    fn report_warnings(&mut self, outcome: Option<&Outcome>) {
        let warnings = match outcome {
            Some(Outcome::Applied { warnings }) | Some(Outcome::Proposed { warnings, .. }) => {
                warnings
            }
            None => return,
        };
        let text: Vec<String> = warnings.iter().map(warning_text).collect();
        if !text.is_empty() {
            self.status = Some(text.join(" · "));
        }
    }

    /// The change item picked on the timeline, as `(set, item)`.
    pub fn focused_change_item(&self) -> Option<(ChangeSetId, ChangeItemId)> {
        self.focused_change
    }

    /// The picked change item itself, if it still exists.
    pub fn focused_change(&self) -> Option<&ChangeItem> {
        let (set, item) = self.focused_change?;
        self.bus.change_set(set)?.item(item)
    }

    pub fn focus_change_item(
        &mut self,
        item: Option<(ChangeSetId, ChangeItemId)>,
        cx: &mut Context<Self>,
    ) {
        if self.focused_change != item {
            self.focused_change = item;
            cx.notify();
        }
    }

    pub fn set_in_out(&mut self, in_out: Option<TimeRange>, cx: &mut Context<Self>) {
        self.in_out = in_out;
        cx.notify();
    }

    /// Clear the marks of the source in the viewer, else the program's.
    pub fn clear_marks(&mut self, cx: &mut Context<Self>) {
        match self.source_picture().map(|s| s.item().id) {
            Some(id) => {
                self.source_marks.remove(&id);
                cx.notify();
            }
            None => self.set_in_out(None, cx),
        }
    }

    /// Mark in at the playhead, keeping the out point when it is still after it.
    /// With a source in the viewer, marks the source.
    pub fn mark_in(&mut self, cx: &mut Context<Self>) {
        if self.mark_source(false, cx) {
            return;
        }
        let t = self.playhead;
        let end = match self.in_out {
            Some(r) if r.end > t => r.end,
            _ => self.bus.project().timeline.duration().max(t),
        };
        self.set_in_out(Some(TimeRange::new(t, end)), cx);
    }

    /// Mark out at the playhead, keeping the in point when it is still before it.
    /// With a source in the viewer, marks the source.
    pub fn mark_out(&mut self, cx: &mut Context<Self>) {
        if self.mark_source(true, cx) {
            return;
        }
        let t = self.playhead;
        let start = match self.in_out {
            Some(r) if r.start < t => r.start,
            _ => Time::ZERO,
        };
        self.set_in_out(Some(TimeRange::new(start, t)), cx);
    }

    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if let Some(item) = self.source_picture().map(|s| s.item().clone()) {
            self.toggle_source(&item, cx);
            return;
        }
        let playing = !self.playing;
        self.play_speed = 1.0;
        self.set_playing(playing, cx);
    }

    /// JKL: `speed` 0 stops; otherwise play at that rate (negative = reverse).
    pub fn shuttle(&mut self, speed: f64, cx: &mut Context<Self>) {
        if speed == 0.0 {
            self.set_playing(false, cx);
        } else {
            self.play_speed = speed;
            self.clock = None;
            self.set_playing(true, cx);
        }
    }

    pub fn set_playing(&mut self, playing: bool, cx: &mut Context<Self>) {
        self.playing = playing;
        if !playing {
            self.clock = None;
            cx.notify();
            return;
        }
        self.close_source(cx);
        let end = self.display_project().timeline.duration();
        if self.play_speed > 0.0 && self.playhead >= end {
            self.playhead = Time::ZERO;
        }
        if self.clock.is_none() {
            self.clock = Some(cx.spawn(async move |this, cx| {
                let mut last = (Instant::now(), None::<Time>);
                loop {
                    cx.background_executor().timer(PLAY_TICK).await;
                    let alive = this.update(cx, |this, cx| this.tick(&mut last, cx));
                    if !matches!(alive, Ok(true)) {
                        break;
                    }
                }
            }));
        }
        cx.notify();
    }

    /// Advance the playhead by the wall time since the last tick. A seek in
    /// between (the playhead moved elsewhere) restarts from the new position.
    /// Returns false once playback stopped.
    fn tick(&mut self, last: &mut (Instant, Option<Time>), cx: &mut Context<Self>) -> bool {
        if !self.playing {
            return false;
        }
        let now = Instant::now();
        let elapsed = now.duration_since(last.0).as_secs_f64() * self.play_speed;
        *last = (now, Some(self.playhead));
        let Some(from) = last.1.filter(|_| elapsed != 0.0) else {
            return true;
        };
        let end = self.display_project().timeline.duration();
        let t = from + Time::from_secs_f64(elapsed);
        let done =
            (self.play_speed > 0.0 && t >= end) || (self.play_speed < 0.0 && t <= Time::ZERO);
        self.playhead = t.max(Time::ZERO).min(end);
        last.1 = Some(self.playhead);
        if done {
            self.playing = false;
            self.clock = None;
        }
        cx.notify();
        !done
    }
}

fn warning_text(warning: &Warning) -> String {
    match warning {
        Warning::TouchesProtected { note, .. } if note.is_empty() => {
            "Touches a protected range".to_string()
        }
        Warning::TouchesProtected { note, .. } => format!("Touches protected range “{note}”"),
        Warning::NeedsConfirmation { message } => message.clone(),
    }
}

/// Playback clock resolution.
const PLAY_TICK: Duration = Duration::from_millis(16);

/// Where the project is in its undo history: the version of the newest
/// undo step, 0 before any. Unlike the bus version, which every undo bumps,
/// it comes back to the same value when undo or redo returns to a state.
fn undo_position(bus: &CommandBus) -> u64 {
    bus.history().undo.last().map_or(0, |e| e.version)
}

/// What the autosaver was last told about the open project's unsaved
/// changes, by undo position.
#[derive(Clone, Copy)]
enum Autosaved {
    /// Nothing since the last save.
    Nothing,
    /// It holds the changes at this position.
    Kept(u64),
    /// The user threw away the changes at this position: they are not
    /// autosaved again.
    Discarded(u64),
}

impl Autosaved {
    /// A recovered project's autosave already holds its changes.
    fn opened(recovered: bool, position: u64) -> Self {
        if recovered {
            Self::Kept(position)
        } else {
            Self::Nothing
        }
    }

    /// Nothing needs to be handed over while the project is at `position`.
    fn covers(self, position: u64) -> bool {
        match self {
            Self::Nothing => false,
            Self::Kept(at) | Self::Discarded(at) => at == position,
        }
    }
}

/// The active session's messages as last saved.
fn active_messages(chats: &persistence::Chats) -> Vec<ChatMessage> {
    chats
        .sessions
        .iter()
        .find(|s| s.id == chats.active)
        .map(|s| s.messages.clone())
        .unwrap_or_default()
}

/// A session title from its first message: the first line, shortened.
fn chat_title(text: &str) -> String {
    const MAX: usize = 28;
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= MAX {
        line.to_owned()
    } else {
        let short: String = line.chars().take(MAX - 1).collect();
        format!("{}…", short.trim_end())
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// --- chat

impl EditorState {
    /// Memory and deliver requests waiting for the user (PRD "Agent
    /// configuration": these categories need approval per call).
    pub fn pending_approvals(&self) -> Vec<flikk_agent::PendingApproval> {
        self.agent.pending_approvals()
    }

    pub fn deny_approval(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(pending) = self.agent.resolve_approval(id) else {
            return;
        };
        self.agent.report_failure(
            id,
            format!("denied by the user: {}", pending.request.summary),
        );
        self.settled.push((pending, ApprovalOutcome::Denied));
        cx.notify();
    }

    /// Carry out an approved request (store, render queue, export), take it
    /// off the list and tell the agent on its next turn.
    pub fn approve(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(pending) = self.agent.resolve_approval(id) else {
            return;
        };
        let summary = pending.request.summary.clone();
        let owner = self.bus.project().id;
        let mut settings = self.settings.clone();
        let started = approve::start(
            &pending.request,
            approve::Inputs {
                project: self.bus.project(),
                caption_styles: &self.presets.caption_styles,
                queue: &self.render_queue,
                services: &self.services,
                settings: &mut settings,
            },
        );
        if settings != self.settings {
            self.set_settings(settings, cx);
        } else {
            self.load_agent_memory();
        }
        let outcome = match started {
            Ok(approve::Started::Done(line)) => {
                self.agent
                    .report_outcome(id, format!("approved: {summary}"));
                if pending.request.tool == "save_fx_to_library" {
                    self.reload_library(cx);
                    if let Some(fx) = pending.request.args.get("fx").and_then(|v| v.as_str()) {
                        crate::ui::library::shared::link_to_library(self, fx, cx);
                    }
                }
                if pending.request.tool == "save_sound_to_library" {
                    self.reload_library(cx);
                }
                ApprovalOutcome::Done(line)
            }
            Ok(approve::Started::Render(job, path)) => {
                self.agent.report_outcome(
                    id,
                    format!("approved: {summary}; rendering to {}", path.display()),
                );
                ApprovalOutcome::Rendering {
                    job,
                    file: approve::file_name(&path),
                }
            }
            Ok(approve::Started::Background(work)) => {
                let task = cx.background_executor().spawn(async move { work() });
                cx.spawn(async move |this, cx| {
                    let result = task.await;
                    let _ = this.update(cx, |this, cx| {
                        let outcome = match result {
                            Ok(file) => {
                                this.agent.report_outcome(
                                    id,
                                    format!("approved: {summary}; wrote {file}"),
                                );
                                ApprovalOutcome::Done(file)
                            }
                            Err(e) => {
                                this.agent.report_failure(
                                    id,
                                    format!("approved but failed: {summary}: {e:#}"),
                                );
                                ApprovalOutcome::Failed(format!("{e:#}"))
                            }
                        };
                        if let Some(entry) = this.settled.iter_mut().find(|(p, _)| p.id == id) {
                            entry.1 = outcome;
                        }
                        cx.notify();
                    });
                })
                .detach();
                ApprovalOutcome::Working
            }
            Ok(approve::Started::Voice(work)) => {
                let task = cx.background_executor().spawn(async move { work() });
                cx.spawn(async move |this, cx| {
                    let result = task.await;
                    let _ = this.update(cx, |this, cx| {
                        let outcome = match result {
                            Ok(line) => {
                                this.agent
                                    .report_outcome(id, format!("approved: {summary}; {line}"));
                                this.reload_library(cx);
                                this.refresh_voices(cx);
                                ApprovalOutcome::Done("Voice kept in the library".into())
                            }
                            Err(e) => {
                                this.agent.report_failure(
                                    id,
                                    format!("approved but failed: {summary}: {e:#}"),
                                );
                                ApprovalOutcome::Failed(format!("{e:#}"))
                            }
                        };
                        if let Some(entry) = this.settled.iter_mut().find(|(p, _)| p.id == id) {
                            entry.1 = outcome;
                        }
                        cx.notify();
                    });
                })
                .detach();
                ApprovalOutcome::Working
            }
            Ok(approve::Started::Respeak(work)) => {
                let task = cx.background_executor().spawn(async move { work() });
                cx.spawn(async move |this, cx| {
                    let result = task.await;
                    let _ = this.update(cx, |this, cx| {
                        let outcome = match result {
                            Ok(made) if this.bus.project().id != owner => {
                                this.keep_approved(id, owner, made.item, &summary)
                            }
                            Ok(made) => this.place_respoken(id, made, cx),
                            Err(e) => {
                                this.agent.report_failure(
                                    id,
                                    format!("approved but failed: {summary}: {e:#}"),
                                );
                                ApprovalOutcome::Failed(format!("{e:#}"))
                            }
                        };
                        if let Some(entry) = this.settled.iter_mut().find(|(p, _)| p.id == id) {
                            entry.1 = outcome;
                        }
                        cx.notify();
                    });
                })
                .detach();
                ApprovalOutcome::Working
            }
            Ok(approve::Started::Generate(work)) => {
                let task = cx.background_executor().spawn(async move { work() });
                cx.spawn(async move |this, cx| {
                    let result = task.await;
                    let _ = this.update(cx, |this, cx| {
                        let outcome = match result {
                            Ok(made) if this.bus.project().id != owner => {
                                this.keep_approved(id, owner, made.item, &summary)
                            }
                            Ok(made) => this.place_generated(id, made, cx),
                            Err(e) => {
                                this.agent.report_failure(
                                    id,
                                    format!("approved but failed: {summary}: {e:#}"),
                                );
                                ApprovalOutcome::Failed(format!("{e:#}"))
                            }
                        };
                        if let Some(entry) = this.settled.iter_mut().find(|(p, _)| p.id == id) {
                            entry.1 = outcome;
                        }
                        cx.notify();
                    });
                })
                .detach();
                ApprovalOutcome::Working
            }
            Err(e) => {
                self.agent
                    .report_failure(id, format!("approved but failed: {summary}: {e:#}"));
                ApprovalOutcome::Failed(format!("{e:#}"))
            }
        };
        self.settled.push((pending, outcome));
        cx.notify();
    }

    /// A render started from the chat, as the queue sees it now.
    pub fn render_job(&self, job: flikk_core::JobId) -> Option<flikk_deliver::RenderJob> {
        self.render_queue.jobs().into_iter().find(|j| j.id == job)
    }

    /// The user's recipes, for `/` completion and the quick-command bar.
    pub fn recipes(&self) -> Vec<flikk_core::memory::Recipe> {
        persistence::with_store(|s| s.list_recipes()).unwrap_or_default()
    }
}

// --- library (effects library, inspector, FX code editor)
impl EditorState {
    /// Read the library again after it changed.
    pub fn reload_library(&mut self, cx: &mut Context<Self>) {
        self.library = flikk_core::library::Library::load();
        self.analyze_sounds(cx);
        cx.notify();
    }

    /// Analyze the library's sounds that have no results, so they can be
    /// found by how they sound. Without speech: it costs nothing.
    fn analyze_sounds(&self, cx: &mut Context<Self>) {
        let Some(analysis) = self.services.analysis.clone() else {
            return;
        };
        let items: Vec<_> = self.library.sounds.iter().map(|s| s.item()).collect();
        cx.background_executor()
            .spawn(async move { items.iter().for_each(|i| analysis.analyze_sound(i)) })
            .detach();
    }

    /// Open `fx` in the code editor, or close it with `None`.
    pub fn edit_fx(&mut self, fx: Option<flikk_core::fx::FxRef>, cx: &mut Context<Self>) {
        if self.editing_fx != fx {
            self.editing_fx = fx;
            cx.notify();
        }
    }
}

/// What came of an approval request.
#[derive(Clone, Debug, PartialEq)]
pub enum ApprovalOutcome {
    Working,
    Done(String),
    Rendering {
        job: flikk_core::JobId,
        file: String,
    },
    Failed(String),
    Denied,
}

// ------------------------------------------------------------- chat stream

impl EditorState {
    /// Send `text` as the user's message, like [`Self::send_chat`] (nothing
    /// while the agent is busy); the agent also reads `context` after it (the resolved `@` references), which
    /// the chat neither shows nor stores.
    pub fn send_chat_with_context(
        &mut self,
        text: String,
        context: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.agent_busy() {
            return;
        }
        self.chat.push(ChatMessage {
            role: ChatRole::User,
            text: text.clone(),
            choices: vec![],
            change_set: None,
            tools: vec![],
            at: now(),
        });
        let message = match context {
            Some(context) => format!("{text}\n\n{context}"),
            None => text,
        };
        let editor = self.context();
        self.agent.send(message, editor, &self.settings);
        self.clear_settled();
        self.save_chat();
        cx.notify();
    }

    /// The user's next message clears the requests they decided, except
    /// those still being carried out: their card stays to show how they
    /// end.
    fn clear_settled(&mut self) {
        let queue = &self.render_queue;
        self.settled.retain(|(_, outcome)| match outcome {
            ApprovalOutcome::Working => true,
            ApprovalOutcome::Rendering { job, .. } => queue
                .jobs()
                .iter()
                .any(|j| j.id == *job && !j.state.is_finished()),
            _ => false,
        });
    }
}
