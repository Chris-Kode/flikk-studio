//! Shared editor state. One entity per window; panels observe it and call
//! its methods instead of touching the bus directly, so every change goes
//! through the same path whether it came from a button or the agent.

mod agent;
mod approvals;
mod captions;
mod chat;
mod cloud;
mod document;
mod library;
mod live;
mod models;
mod music_fit;
mod offline;
mod playback;
mod respeak;
mod source;
mod sync;
mod update;
pub mod voices;

pub use approvals::ApprovalOutcome;
pub use cloud::{Check, Generating, ProviderStatus};
pub use source::Source;

use crate::agent_link::AgentLink;
use crate::notices::Notices;
use crate::persistence;
use captions::CaptionStyles;
use chat::ChatLog;
use document::Document;
use flikk_agent::PendingApproval;
use flikk_core::agent::*;
use flikk_core::bus::{AcceptReport, Author, CommandBus, Outcome};
use flikk_core::changeset::ChangeItem;
use flikk_core::command::{Command, Warning};
use flikk_core::model::Project;
use flikk_core::services::{AnalysisService, MediaService, Services, SubjectService};
use flikk_core::{ChangeItemId, ChangeSetId, Time, TimeRange};
use gpui_kit::*;
use offline::OfflineMedia;
use playback::Playback;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// How often agent output and tool calls are drained on the UI thread.
const AGENT_PUMP: Duration = Duration::from_millis(33);
/// How often the model files are looked for on disk.
const MODELS_POLL: Duration = Duration::from_secs(1);
/// How often subject mattes are looked for, so views draw them as they
/// are made.
const SUBJECTS_POLL: Duration = Duration::from_millis(250);

pub struct EditorState {
    pub bus: CommandBus,
    /// The open project's file and its unsaved changes.
    document: Document,
    pub services: Services,
    pub workspace: Workspace,
    pub playhead: Time,
    pub in_out: Option<TimeRange>,
    pub selection: Selection,
    /// Whether the program plays, at what speed.
    playback: Playback,
    pub settings: AgentSettings,
    /// The project's chats, the open one's messages and the agent's
    /// stream into them.
    chat: ChatLog,
    /// Keeps the change sets waiting for review saved beside the chats.
    proposals: persistence::Proposals,
    /// Change set whose draft the viewer plays instead of the committed project.
    pub preview: Option<ChangeSetId>,
    /// Uncommitted edit the viewer shows while a control is dragged.
    live: Option<Project>,
    /// Counts changes to `live`, so the viewer knows to redraw.
    pub live_version: u64,
    /// Last error or notice for the status line; cleared on the next action.
    pub status: Option<String>,
    /// A sync by sound is being heard; the status bar shows it.
    pub syncing: bool,
    // --- panels
    /// Pool item loaded in the viewer as source (double-click in the pool).
    /// The item playing on its own, or showing in the viewer.
    pub source: Option<Source>,
    /// In and out marked on pool items in the viewer, for the next edit
    /// with them; for the session.
    pub source_marks: std::collections::HashMap<flikk_core::MediaId, TimeRange>,
    /// The presets in use, the caption style dragged and the styles
    /// `services.lettering` letters in.
    captions: CaptionStyles,
    /// `services.subjects`' version when last looked at: mattes made since
    /// show on the next draw.
    pub subjects_version: u64,
    /// A depth effect whose focus the next click in the viewer picks.
    pub depth_pick: Option<(flikk_core::ClipId, flikk_core::EffectId)>,
    // --- timeline/viewer
    /// Change item picked on the timeline overlay; the chat panel reveals it.
    pub focused_change: Option<(ChangeSetId, ChangeItemId)>,
    /// Transition picked on the timeline: the clip and edge it sits on. The
    /// inspector shows it.
    pub selected_transition: Option<(flikk_core::ClipId, flikk_core::command::Edge)>,
    /// `@` references waiting for the chat composer ("Add to chat" from the
    /// timeline, inspector, transcript…): token text and chip label.
    pending_mentions: Vec<(String, SharedString)>,
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
    offline: OfflineMedia,
    /// Which of `model_tasks` had their model, and `models.finished()`, when
    /// the notices were last brought up to date.
    models_seen: (Vec<bool>, u64),
    _models_poll: Task<()>,
    _offline_poll: Task<()>,
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
    /// pool when it is opened again as the author with it.
    kept: Vec<(
        flikk_core::ProjectId,
        flikk_core::model::MediaItem,
        flikk_core::bus::Author,
    )>,
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
        let mut bus = CommandBus::new(project);
        bus.set_permissions(settings.permissions.clone());
        // Brings back the proposals left waiting for review too.
        let chat = ChatLog::load(persistence::load_chats(&mut bus));
        let document = Document::opened(file, recovered, &bus);
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
            document,
            services: services(&cloud),
            workspace: Workspace::default(),
            playhead: Time::ZERO,
            in_out: None,
            selection: Selection::default(),
            playback: Playback::Stopped,
            settings,
            chat,
            proposals: persistence::Proposals::default(),
            preview: None,
            live: None,
            live_version: 0,
            status: persistence::take_notice(),
            syncing: false,
            source: None,
            source_marks: Default::default(),
            captions: CaptionStyles::default(),
            subjects_version: 0,
            depth_pick: None,
            focused_change: None,
            selected_transition: None,
            pending_mentions: Vec::new(),
            editing_fx: None,
            library: flikk_core::library::Library::load(),
            agent: AgentLink::new(),
            _pump: Self::every(cx, AGENT_PUMP, Self::pump_agent),
            settled: Vec::new(),
            render_queue: flikk_deliver::RenderQueue::new(),
            models_seen: (downloaded, 0),
            model_tasks,
            models: flikk_models::Downloads::default(),
            notices: Notices::default(),
            offline: OfflineMedia::default(),
            _models_poll: Self::every(cx, MODELS_POLL, Self::poll_models),
            _offline_poll: Self::poll_offline(cx),
            _subjects_poll: Self::every(cx, SUBJECTS_POLL, Self::poll_subjects),
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
            this.proposals.sync(&this.bus);
            async {}
        })
        .detach();
        this
    }

    pub fn context(&self) -> EditorContext {
        EditorContext {
            workspace: self.workspace,
            playhead: self.playhead,
            in_out: self.in_out,
            selection: self.selection.clone(),
            history: Vec::new(),
            attachments: self.attachments().to_vec(),
        }
    }

    /// Run `f` every `period` on the UI thread, for as long as the state
    /// lives: the task returned stops it when dropped.
    fn every(
        cx: &mut Context<Self>,
        period: Duration,
        f: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(period).await;
                if this.update(cx, |this, cx| f(this, cx)).is_err() {
                    break;
                }
            }
        })
    }

    /// Show `text` on the status line, until the next action clears it.
    pub fn say(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.status = Some(text.into());
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
        // An import undone takes its media out.
        let before = self.bus.project().media.clone();
        let result = self.bus.undo();
        self.finish(result, cx);
        self.cancel_left(&before);
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        // A removal redone takes its media out.
        let before = self.bus.project().media.clone();
        let result = self.bus.redo();
        self.finish(result, cx);
        self.cancel_left(&before);
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
        self.proposals.sync(&self.bus);
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
            self.proposals.sync(&self.bus);
            // An accepted set, undo or redo may restyle the first caption
            // track, whose style new captions use.
            self.sync_caption_style();
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

    /// Select clips, none to clear the selection. A clip selection and a
    /// transition selection exclude each other, whichever panel sets them,
    /// so this always drops the selected transition.
    pub fn select_clips(&mut self, clips: Vec<flikk_core::ClipId>, cx: &mut Context<Self>) {
        self.selected_transition = None;
        self.selection.clips = clips;
        cx.notify();
    }

    /// The selected transition the inspector shows: until clips are selected
    /// that leave out the clip it sits on.
    pub fn inspected_transition(&self) -> Option<(flikk_core::ClipId, flikk_core::command::Edge)> {
        let clips = &self.selection.clips;
        self.selected_transition
            .filter(|(clip, _)| clips.is_empty() || clips.contains(clip))
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
        let project = self.bus.project();
        let items: Vec<_> = media
            .iter()
            .filter_map(|m| project.media(*m).cloned())
            .collect();
        if self.execute_batch(commands, label, cx).is_some() {
            self.cancel_left(&items);
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

    /// Stop the background analysis and camera path measuring of those of
    /// `items` that left the project, unless an item of the project or a
    /// library sound still has the same file. One that comes back (an
    /// undo) is asked for again as usual; renders measure for themselves.
    fn cancel_left(&self, items: &[flikk_core::model::MediaItem]) {
        let project = self.bus.project();
        let sounds = self.library.sounds.iter().map(|s| &s.fingerprint);
        let kept: HashSet<_> = project
            .media
            .iter()
            .map(|m| &m.fingerprint)
            .chain(sounds)
            .flatten()
            .collect();
        let left = items
            .iter()
            .filter(|m| m.fingerprint.as_ref().is_some_and(|fp| !kept.contains(fp)));
        for item in left {
            if let Some(analysis) = &self.services.analysis {
                analysis.cancel(item);
            }
            if let Some(stabilizer) = &self.services.stabilizer {
                stabilizer.cancel(item);
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

    /// Replace everything tied to the open project; engines stay. Undo
    /// history does not carry over.
    fn switch_to(&mut self, loaded: persistence::Loaded, cx: &mut Context<Self>) {
        let left = self.bus.project().media.clone();
        self.set_playing(false, cx);
        self.agent.cancel();
        self.agent.close_project(self.bus.project().id);
        self.save_chat();
        self.proposals.sync(&self.bus);
        self.bus = CommandBus::new(loaded.project);
        self.proposals = persistence::Proposals::default();
        self.chat = ChatLog::load(persistence::load_chats(&mut self.bus));
        // What the new project has too goes on.
        self.cancel_left(&left);
        self.bus
            .set_permissions(loaded.settings.permissions.clone());
        self.mark_generated_seen();
        self.document = Document::opened(loaded.file, loaded.recovered, &self.bus);
        self.settings = loaded.settings;
        self.playhead = Time::ZERO;
        self.in_out = None;
        self.selection = Selection::default();
        self.selected_transition = None;
        self.source = None;
        self.preview = None;
        self.clear_live();
        self.focused_change = None;
        self.editing_fx = None;
        self.reset_chat_view();
        self.status = persistence::take_notice();
        self.check_offline();
        self.load_agent_memory();
        self.load_presets();
        self.add_kept(cx);
        cx.notify();
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
    let cleanup = media.clone().map(|m| {
        Arc::new(flikk_analysis::LocalCleanup::new(m))
            as Arc<dyn flikk_core::services::CleanupService>
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
        cleanup,
        // Made with the caption styles, see `CaptionStyles::refresh_lettering`.
        lettering: None,
        generation: Some(cloud.clone() as Arc<dyn flikk_core::cloud::GenerationService>),
    }
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
