//! Online providers in the editor: each one's key and account as Settings →
//! Integrations shows them, the provider picked for each capability, and
//! music, sound effects, voiceovers, videos and pictures made from the media
//! pool or approved for the agent. The requests themselves are
//! `flikk-cloud`'s.

use super::{ApprovalOutcome, EditorState};
use crate::agent_link::approve;
use crate::notices::{Level, Notice, NoticeAction};
use crate::persistence;
use flikk_agent::tools::video::FrameGrab;
use flikk_cloud::{Account, CloudModel, KeySource, Provider};
use flikk_core::MediaId;
use flikk_core::analysis::AnalysisSteps;
use flikk_core::bus::{Author, CommandBus};
use flikk_core::changeset::ChangeSetState;
use flikk_core::cloud::{Capability, CloudError, GenerateRequest, GenerationTask, ProviderChoice};
use flikk_core::command::Command;
use flikk_core::model::{MediaItem, MediaKind};
use flikk_core::services::AnalysisConfig;
use gpui_kit::*;

/// Key of the notice for a provider set without a key.
const KEY_NOTICE: &str = "cloud-key-missing";

/// What is known of a provider's key and account.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderStatus {
    /// Where its key comes from; `None` until read from the credential
    /// store.
    pub key: Option<KeySource>,
    pub check: Check,
    /// Bumped by each set or removal of its key: a key read or saved
    /// before then is dropped.
    keyed: u64,
    /// Bumped by each check, set or removal of its key: an older check's
    /// answer is dropped.
    checked: u64,
}

impl ProviderStatus {
    pub fn has_key(&self) -> bool {
        matches!(self.key, Some(KeySource::Stored | KeySource::Environment))
    }
}

/// The last check of a provider's key.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Check {
    #[default]
    Unchecked,
    Checking,
    Valid(Account),
    /// What went wrong, and whether checking again may help: the provider
    /// was out of reach or busy, not refusing the key.
    Failed {
        message: String,
        retry: bool,
    },
}

impl Check {
    fn from_account(result: Result<Account, CloudError>) -> Self {
        match result {
            Ok(account) => Self::Valid(account),
            Err(e) => Self::Failed {
                message: e.to_string(),
                retry: !e.needs_settings(),
            },
        }
    }

    /// The credential store could not keep or drop a key.
    fn store_failed(message: String) -> Self {
        Self::Failed {
            message,
            retry: false,
        }
    }
}

/// Music, a sound effect, a voiceover, a video or a picture being made from
/// the media pool.
#[derive(Clone, Debug)]
pub struct Generating {
    pub id: u64,
    /// The project it is made for.
    pub project: flikk_core::ProjectId,
    pub request: GenerateRequest,
    /// Where it is, and how to stop it.
    pub task: GenerationTask,
    /// Why it failed; `None` while it runs.
    pub failed: Option<CloudError>,
}

/// What becomes of the pool item a generation made.
type Then = Box<dyn FnOnce(&mut EditorState, MediaId, &mut Context<EditorState>)>;

impl Generating {
    /// Stopped by the user and not over yet: its row stays until the
    /// provider says. What the provider made by then is kept, as it is paid
    /// for.
    pub fn stopping(&self) -> bool {
        self.failed.is_none() && self.task.cancelled()
    }
}

/// How often a running generation's row is redrawn with where it is.
const STAGE_POLL: std::time::Duration = std::time::Duration::from_millis(500);

impl EditorState {
    /// Read each provider's key off the UI thread (the credential store may
    /// wait, or ask the user), then bring the notices up to date.
    pub(super) fn load_keys(&mut self, cx: &mut Context<Self>) {
        let cloud = self.cloud.clone();
        // A key set or removed while they are read is newer.
        let asked: Vec<u64> = (flikk_cloud::PROVIDERS.iter())
            .map(|p| self.provider_status(p).keyed)
            .collect();
        let read = cx.background_executor().spawn(async move {
            flikk_cloud::PROVIDERS
                .iter()
                .map(|p| (p.id, cloud.keys().source(p)))
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let found = read.await;
            let _ = this.update(cx, |this, cx| {
                for ((id, source), keyed) in found.into_iter().zip(asked) {
                    let status = this.providers.entry(id).or_default();
                    if status.keyed == keyed {
                        status.key = Some(source);
                    }
                }
                this.refresh_key_notice();
                cx.notify();
            });
        })
        .detach();
    }

    pub fn provider_status(&self, provider: &Provider) -> ProviderStatus {
        self.providers.get(provider.id).cloned().unwrap_or_default()
    }

    /// Check `provider`'s key and read its plan and credits.
    pub fn check_account(&mut self, provider: &'static Provider, cx: &mut Context<Self>) {
        if !self.provider_status(provider).has_key() {
            return;
        }
        self.providers.entry(provider.id).or_default().check = Check::Checking;
        self.read_account(provider, cx);
    }

    /// Check every key checked before again, showing the last result until
    /// the new one comes in.
    pub fn refresh_accounts(&mut self, cx: &mut Context<Self>) {
        for &provider in flikk_cloud::PROVIDERS {
            let status = self.provider_status(provider);
            if status.has_key() && matches!(status.check, Check::Valid(_) | Check::Failed { .. }) {
                self.read_account(provider, cx);
            }
        }
    }

    fn read_account(&mut self, provider: &'static Provider, cx: &mut Context<Self>) {
        let status = self.providers.entry(provider.id).or_default();
        status.checked += 1;
        let checked = status.checked;
        let cloud = self.cloud.clone();
        let check = cx
            .background_executor()
            .spawn(async move { cloud.account(provider) });
        cx.spawn(async move |this, cx| {
            let result = check.await;
            let _ = this.update(cx, |this, cx| {
                let status = this.providers.entry(provider.id).or_default();
                if status.checked == checked {
                    status.check = Check::from_account(result);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Keep `key` for `provider` and check it. A first key that works also
    /// turns on the generation the provider offers, where none is set.
    pub fn set_api_key(
        &mut self,
        provider: &'static Provider,
        key: String,
        cx: &mut Context<Self>,
    ) {
        let keyed = self.rekey(provider);
        self.providers.entry(provider.id).or_default().check = Check::Checking;
        let cloud = self.cloud.clone();
        let saved = cx.background_executor().spawn(async move {
            cloud.keys().set(provider, &key)?;
            Ok::<_, String>((cloud.keys().source(provider), cloud.account(provider)))
        });
        cx.spawn(async move |this, cx| {
            let result = saved.await;
            let _ = this.update(cx, |this, cx| {
                let status = this.providers.entry(provider.id).or_default();
                // Set or removed again since.
                if status.keyed != keyed {
                    return;
                }
                // This checked the new key; one asked for meanwhile may have
                // read the old one.
                status.checked += 1;
                match result {
                    Ok((source, account)) => {
                        status.key = Some(source);
                        status.check = Check::from_account(account);
                    }
                    Err(e) => status.check = Check::store_failed(e),
                }
                if matches!(status.check, Check::Valid(_)) {
                    for capability in [
                        Capability::Music,
                        Capability::SoundEffects,
                        Capability::Voiceover,
                        Capability::Video,
                        Capability::Image,
                        Capability::Upscale,
                    ] {
                        let unset = this.cloud.settings().provider(capability).is_none();
                        if unset && provider.offer(capability).is_some() {
                            this.set_provider(capability, Some(provider), cx);
                        }
                    }
                }
                this.key_changed(provider, cx);
            });
        })
        .detach();
        cx.notify();
    }

    pub fn remove_api_key(&mut self, provider: &'static Provider, cx: &mut Context<Self>) {
        let keyed = self.rekey(provider);
        let cloud = self.cloud.clone();
        let removed = cx.background_executor().spawn(async move {
            let result = cloud.keys().remove(provider);
            (cloud.keys().source(provider), result)
        });
        cx.spawn(async move |this, cx| {
            let (source, result) = removed.await;
            let _ = this.update(cx, |this, cx| {
                let status = this.providers.entry(provider.id).or_default();
                // Set again since.
                if status.keyed != keyed {
                    return;
                }
                // A check asked for meanwhile may have read the old key.
                status.checked += 1;
                status.key = Some(source);
                status.check = match result {
                    Ok(()) => Check::Unchecked,
                    Err(e) => Check::store_failed(e),
                };
                this.key_changed(provider, cx);
            });
        })
        .detach();
    }

    /// A set or removal of `provider`'s key starts: keys read and checks
    /// asked for before it are dropped. Its count.
    fn rekey(&mut self, provider: &'static Provider) -> u64 {
        let status = self.providers.entry(provider.id).or_default();
        status.keyed += 1;
        status.checked += 1;
        status.keyed
    }

    /// `provider`'s key was set or removed: the notices, and the account's
    /// voices when it does voiceovers.
    fn key_changed(&mut self, provider: &'static Provider, cx: &mut Context<Self>) {
        if self.voice_provider().as_deref() == Some(provider.id) {
            self.invalidate_voices(cx);
        }
        self.refresh_key_notice();
        cx.notify();
    }

    /// Have `provider` do `capability` on its default model, or this
    /// machine (or nobody, for what can't run here) with `None`.
    pub fn set_provider(
        &mut self,
        capability: Capability,
        provider: Option<&'static Provider>,
        cx: &mut Context<Self>,
    ) {
        let choice = ProviderChoice {
            provider: provider.map(|p| p.id.to_owned()),
            model: None,
        };
        self.set_cloud_choice(capability, choice, cx);
    }

    /// Run `capability` on model `model` of its provider.
    pub fn set_cloud_model(&mut self, capability: Capability, model: &str, cx: &mut Context<Self>) {
        let mut choice = self.cloud.settings().choice(capability);
        choice.model = Some(model.to_owned());
        self.set_cloud_choice(capability, choice, cx);
    }

    fn set_cloud_choice(
        &mut self,
        capability: Capability,
        choice: ProviderChoice,
        cx: &mut Context<Self>,
    ) {
        let mut settings = self.cloud.settings();
        if settings.choice(capability) == choice {
            return;
        }
        let voices = self.voice_provider();
        settings.set(capability, choice);
        persistence::with_store(|s| s.save_cloud_settings(&settings));
        self.cloud.set_settings(settings);
        // The voices listed are another provider's.
        if self.voice_provider() != voices {
            self.invalidate_voices(cx);
        }
        self.configure_analysis();
        if capability == Capability::Transcription
            && self.online_transcription().is_some()
            && let Some(analysis) = &self.services.analysis
        {
            // Automatic analysis never spends money.
            analysis.cancel_background_speech();
        }
        self.refresh_key_notice();
        cx.notify();
    }

    /// Hand analysis every step and the transcriber for the items analyzed
    /// from now on.
    pub(super) fn configure_analysis(&self) {
        if let Some(analysis) = &self.services.analysis {
            analysis.configure(AnalysisConfig {
                steps: AnalysisSteps::default(),
                speech: self.cloud.speech(),
            });
        }
    }

    /// The provider and model making pictures, when one is set and has a
    /// key: it makes the agent's too, whatever the agent's own setting.
    pub fn image_provider(&self) -> Option<(&'static Provider, &'static CloudModel)> {
        let (provider, model) = self.cloud.picked(Capability::Image)?;
        self.provider_status(provider)
            .has_key()
            .then_some((provider, model))
    }

    /// The provider transcribing in place of the local speech models.
    pub fn online_transcription(&self) -> Option<&'static Provider> {
        self.cloud
            .picked(Capability::Transcription)
            .map(|(provider, _)| provider)
    }

    /// A notice while a capability is set to a provider with no key.
    /// Whether it changed.
    pub(super) fn refresh_key_notice(&mut self) -> bool {
        let missing: Vec<String> = Capability::ALL
            .iter()
            .filter_map(|&c| {
                let (provider, _) = self.cloud.picked(c)?;
                let status = self.providers.get(provider.id)?;
                (status.key.is_some() && !status.has_key())
                    .then(|| format!("{} · {}", c.label(), provider.name))
            })
            .collect();
        if missing.is_empty() {
            return self.notices.clear(KEY_NOTICE);
        }
        self.notices.set(Notice {
            key: KEY_NOTICE.into(),
            level: Level::Warning,
            title: "An API key is missing".into(),
            items: missing,
            detail: None,
            action: Some(NoticeAction::OpenIntegrations),
        })
    }

    // ------------------------------------------------------------ generation

    /// Make `request` with the provider picked for it, then add the file
    /// to the pool's Generated bin; the user asked, from the media pool.
    pub fn generate(&mut self, request: GenerateRequest, cx: &mut Context<Self>) {
        self.generate_with_frames(request, Vec::new(), cx);
    }

    /// [`Self::generate`], first grabbing the program's frames that
    /// `request` names: those a video starts or ends on, or a picture
    /// follows.
    pub fn generate_with_frames(
        &mut self,
        request: GenerateRequest,
        grabs: Vec<FrameGrab>,
        cx: &mut Context<Self>,
    ) {
        self.start_generation(request, grabs, None, cx);
    }

    /// [`Self::generate_with_frames`], then `then` with the pool item it
    /// made, when its project is still open. The generation's id, as in
    /// [`Self::generating`] while it runs or shows its failure.
    pub fn generate_then(
        &mut self,
        request: GenerateRequest,
        grabs: Vec<FrameGrab>,
        then: impl FnOnce(&mut Self, MediaId, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> u64 {
        self.start_generation(request, grabs, Some(Box::new(then)), cx)
    }

    /// Make pool video `source` larger as `request` says (an
    /// [`GenerateRequest::Upscale`] of it), into the Video Upscaled bin;
    /// with `replace`, its clips on picture tracks then show the upscaled
    /// video, as a step of its own to undo.
    pub fn upscale(
        &mut self,
        source: MediaId,
        request: GenerateRequest,
        replace: bool,
        cx: &mut Context<Self>,
    ) {
        let swap: Option<Then> = replace.then(|| -> Then {
            Box::new(move |this, to, cx| {
                this.execute(Command::SwapMedia { from: source, to }, cx);
            })
        });
        self.start_generation(request, Vec::new(), swap, cx);
    }

    /// Make `request`, after grabbing `grabs`; `then` gets the pool item it
    /// made when the project is still open. The generation's id.
    fn start_generation(
        &mut self,
        request: GenerateRequest,
        grabs: Vec<FrameGrab>,
        then: Option<Then>,
        cx: &mut Context<Self>,
    ) -> u64 {
        let id = self.next_generation;
        self.next_generation += 1;
        let task = GenerationTask::default();
        let owner = self.bus.project().id;
        self.generating.push(Generating {
            id,
            project: owner,
            request: request.clone(),
            task: task.clone(),
            failed: None,
        });
        let (cloud, services) = (self.cloud.clone(), self.services.clone());
        let project = (!grabs.is_empty()).then(|| self.bus.project().clone());
        let following = task.clone();
        let stopped = task.clone();
        let made = cx.background_executor().spawn(async move {
            use flikk_core::cloud::GenerationService as _;
            if let Some(project) = &project {
                flikk_agent::tools::video::grab_all(project, &services, &grabs)
                    .map_err(CloudError::Failed)?;
            }
            cloud
                .generate(&request, &flikk_core::paths::generated(), &following)
                .map(|g| flikk_core::cloud::media_item(&g, services.media.as_deref()))
        });
        cx.spawn(async move |this, cx| {
            let result = made.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(item) => {
                        this.generating.retain(|g| g.id != id);
                        if this.bus.project().id == owner {
                            let made = this.add_generated(item, cx);
                            if let Some((then, made)) = then.zip(made) {
                                then(this, made, cx);
                            }
                        } else {
                            this.keep_for(owner, item, Author::User);
                        }
                    }
                    // Stopped: however the provider ended it, it didn't fail.
                    Err(_) if stopped.cancelled() => this.generating.retain(|g| g.id != id),
                    Err(CloudError::Cancelled) => this.generating.retain(|g| g.id != id),
                    Err(e) => {
                        if let Some(g) = this.generating.iter_mut().find(|g| g.id == id) {
                            g.failed = Some(e);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        // Redraw its row as it moves on, until it is done.
        cx.spawn(async move |this, cx| {
            let mut shown = task.stage();
            loop {
                cx.background_executor().timer(STAGE_POLL).await;
                let running = this.update(cx, |this, cx| {
                    let stage = task.stage();
                    if stage != shown {
                        shown = stage;
                        cx.notify();
                    }
                    this.generating
                        .iter()
                        .any(|g| g.id == id && g.failed.is_none())
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
        id
    }

    /// Ask the provider to stop generation `id`; its row goes when it has.
    pub fn stop_generation(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(g) = self.generating.iter().find(|g| g.id == id) else {
            return;
        };
        if !g.stopping() {
            g.task.cancel();
            cx.notify();
        }
    }

    /// The voice that last read a voiceover.
    pub fn last_voice(&self) -> Option<String> {
        self.cloud.settings().voice
    }

    /// Start the next voiceover on `voice`.
    pub fn remember_voice(&mut self, voice: &str) {
        let mut settings = self.cloud.settings();
        if settings.voice.as_deref() != Some(voice) {
            settings.voice = Some(voice.to_owned());
            persistence::with_store(|s| s.save_cloud_settings(&settings));
            self.cloud.set_settings(settings);
        }
    }

    /// Whether videos are made with sound.
    pub fn video_sound(&self) -> bool {
        !self.cloud.settings().silent_video
    }

    /// Make videos with sound, or without, from now on.
    pub fn set_video_sound(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut settings = self.cloud.settings();
        if settings.silent_video != on {
            return;
        }
        settings.silent_video = !on;
        persistence::with_store(|s| s.save_cloud_settings(&settings));
        self.cloud.set_settings(settings);
        cx.notify();
    }

    /// The resolution videos start at ("1080p"); `None` takes the model's.
    pub fn video_resolution(&self) -> Option<String> {
        self.cloud.settings().video_resolution
    }

    /// Make videos at `resolution` from now on, fitted to each model;
    /// `None` takes the model's default.
    pub fn set_video_resolution(&mut self, resolution: Option<String>, cx: &mut Context<Self>) {
        let mut settings = self.cloud.settings();
        if settings.video_resolution == resolution {
            return;
        }
        settings.video_resolution = resolution;
        persistence::with_store(|s| s.save_cloud_settings(&settings));
        self.cloud.set_settings(settings);
        cx.notify();
    }

    /// Forget a generation that failed.
    pub fn dismiss_generation(&mut self, id: u64, cx: &mut Context<Self>) {
        self.generating.retain(|g| g.id != id);
        cx.notify();
    }

    /// Add `item`, which a provider made at the user's request, to the
    /// pool's Generated bin as one undo step, and to the drafts waiting for
    /// review so a proposal can place it, then analyze it
    /// ([`Self::analyze_new`]). Its id when added.
    pub(crate) fn add_generated(
        &mut self,
        item: MediaItem,
        cx: &mut Context<Self>,
    ) -> Option<MediaId> {
        self.add_generated_as(item, Author::User, cx)
    }

    /// [`Self::add_generated`] as `author`: the agent's for what it asked
    /// for and the user approved, applied at once (the approval was the
    /// permission), so its own proposals to place it don't conflict.
    pub(crate) fn add_generated_as(
        &mut self,
        item: MediaItem,
        author: Author,
        cx: &mut Context<Self>,
    ) -> Option<MediaId> {
        let id = item.id;
        let label = match item.generated.as_deref().map(|g| g.capability) {
            _ if item.kind == MediaKind::Image => "Generate image",
            Some(Capability::SoundEffects) => "Generate sound effect",
            Some(Capability::Voiceover) => "Generate voiceover",
            Some(Capability::Video) => "Generate video",
            Some(Capability::Upscale) => "Upscale video",
            _ => "Generate music",
        };
        let bin = flikk_core::cloud::generated_bin(&item);
        let commands = flikk_core::cloud::pool_commands(self.bus.project(), item, bin);
        let result = self.bus.execute_shared_as(commands, label, author);
        self.finish(result, cx)?;
        Some(id)
    }

    /// Keep `item`, made for `project` after it was closed, for when it is
    /// opened again, when it is added as `author`.
    pub(super) fn keep_for(
        &mut self,
        project: flikk_core::ProjectId,
        item: MediaItem,
        author: Author,
    ) {
        self.kept.push((project, item, author));
    }

    /// The agent, as what it asked for is added.
    pub(super) fn agent_author(&self) -> Author {
        Author::Agent
    }

    /// Add what was made for the project just opened while it was closed.
    pub(super) fn add_kept(&mut self, cx: &mut Context<Self>) {
        let open = self.bus.project().id;
        let (ours, others) = std::mem::take(&mut self.kept)
            .into_iter()
            .partition::<Vec<_>, _>(|(p, _, _)| *p == open);
        self.kept = others;
        let n = ours
            .into_iter()
            .filter_map(|(_, item, author)| self.add_generated_as(item, author, cx))
            .count();
        if n > 0 {
            self.status = Some(match n {
                1 => "Added 1 item made while the project was closed".into(),
                n => format!("Added {n} items made while the project was closed"),
            });
        }
    }

    /// An approved generation finished after its project was closed: keep
    /// what it made for that project, and tell the agent.
    pub(super) fn keep_approved(
        &mut self,
        id: u64,
        project: flikk_core::ProjectId,
        item: MediaItem,
        summary: &str,
    ) -> ApprovalOutcome {
        let name = approve::file_name(&item.path);
        let author = self.agent_author();
        self.keep_for(project, item, author);
        self.agent.report_outcome(
            id,
            format!(
                "approved: {summary}; made {name}, but its project was closed meanwhile; it \
                 goes into that project's media pool when it is opened again"
            ),
        );
        ApprovalOutcome::Done(name)
    }

    /// Take what is in the pool now as looked at: a project just opened
    /// keeps its items as they were, analyzed or not.
    pub(crate) fn mark_media_seen(&mut self) {
        self.media_seen = self.bus.project().media.iter().map(|m| m.id).collect();
    }

    /// Imported video and audio follow "Analyze on import"; it never
    /// spends money, so nothing is analyzed while a provider transcribes.
    pub fn set_analyze_on_import(&mut self, on: bool, cx: &mut Context<Self>) {
        self.analyze_on_import = on;
        persistence::with_store(|s| s.save_analyze_on_import(on));
        cx.notify();
    }

    /// Analyze each item as it reaches the pool, once, however it got
    /// there (an import, the media pool's Generate, an approval). Imported
    /// video and audio in the background, when "Analyze on import" is on
    /// and no provider transcribes: it never spends money. What a provider
    /// made, as asked for (the user or the agent asked for it): a video
    /// like footage, so the agent can see and hear it; a voiceover too
    /// (its words are its script, so it costs no credits); music and sound
    /// effects are left to be analyzed when asked. Pictures are never
    /// analyzed.
    pub(super) fn analyze_new(&mut self, cx: &mut Context<Self>) {
        let new: Vec<(MediaId, MediaKind, Option<Capability>)> = self
            .bus
            .project()
            .media
            .iter()
            .filter(|m| m.fingerprint.is_some() && !self.media_seen.contains(&m.id))
            .map(|m| (m.id, m.kind, m.generated.as_deref().map(|g| g.capability)))
            .collect();
        let Some(analysis) = self.services.analysis.clone() else {
            return;
        };
        for (id, kind, capability) in new {
            self.media_seen.insert(id);
            let Some(item) = self.bus.project().media(id) else {
                continue;
            };
            match capability {
                // An upscaled video is new footage to the agent: its
                // results are keyed by the file.
                Some(Capability::Video | Capability::Upscale) if self.analysis_ready() => {
                    analysis.analyze(item);
                }
                Some(Capability::Voiceover) => {
                    analysis.analyze(item);
                }
                None if kind != MediaKind::Image
                    && self.analyze_on_import
                    && self.online_transcription().is_none() =>
                {
                    analysis.analyze_in_background(item);
                }
                _ => continue,
            }
            cx.notify();
        }
    }

    /// Add what an approved generation made to the pool and, where the
    /// agent asked, propose it on the timeline; tell the agent on its next
    /// turn.
    pub(super) fn place_generated(
        &mut self,
        id: u64,
        made: approve::Generated,
        cx: &mut Context<Self>,
    ) -> ApprovalOutcome {
        let approve::Generated {
            item,
            destination,
            summary,
        } = made;
        let name = approve::file_name(&item.path);
        let bin = flikk_core::cloud::generated_bin(&item);
        let author = self.agent_author();
        let Some(media) = self.add_generated_as(item, author, cx) else {
            let e = self.status.clone().unwrap_or_default();
            self.agent.report_failure(
                id,
                format!("approved: {summary}; made {name}, but could not add it to the pool: {e}"),
            );
            return ApprovalOutcome::Failed(e);
        };
        let mut outcome =
            format!("approved: {summary}; made media {media} ({name}) in the {bin} bin");
        if destination != flikk_agent::tools::generate::Destination::Pool {
            match self.with_agent_set(cx, |bus, author| {
                flikk_agent::tools::generate::propose_destination(
                    bus,
                    author,
                    media,
                    &destination,
                    &summary,
                )
            }) {
                Ok(text) => outcome.push_str(&format!("; {text}")),
                Err(e) => outcome.push_str(&format!("; could not place it: {e}")),
            }
        }
        self.agent.report_outcome(id, outcome);
        ApprovalOutcome::Done(name)
    }

    /// Run `propose` as the agent outside its turn (an approved request
    /// carried out later): its proposals go into the open chat's set, which
    /// then goes to review again. During a turn the set is open, and goes
    /// to review when the turn ends.
    pub(super) fn with_agent_set<T>(
        &mut self,
        cx: &mut Context<Self>,
        propose: impl FnOnce(&mut CommandBus, &Author) -> Result<T, String>,
    ) -> Result<T, String> {
        let author = Author::Agent;
        // A queued message has no turn yet, so its set is not open.
        let turn = self.agent.status().in_turn();
        let reopened = if turn {
            None
        } else {
            self.bus.reopen_change_set(&author)
        };
        let result = propose(&mut self.bus, &author);
        if !turn {
            // Whatever it opened goes to review if it grew, even when a
            // later step failed; else it closes again.
            let open: Vec<_> = self
                .bus
                .change_sets()
                .iter()
                .filter(|s| s.author == author && s.state == ChangeSetState::Open)
                .map(|s| (s.id, s.items.len()))
                .collect();
            for (set, len) in open {
                if len == 0 || Some((set, len)) == reopened {
                    self.bus.close_change_set(set);
                } else {
                    self.post_change_set(set);
                }
            }
        }
        cx.notify();
        result
    }
}
