//! Online providers in the editor: each one's key and account as Settings →
//! Integrations shows them, the provider picked for each capability, the
//! analysis steps, and music, sound effects, voiceovers and videos made from
//! the media pool or approved for the agent. The requests themselves are
//! `koo-cloud`'s.

use super::{ApprovalOutcome, EditorState};
use crate::agent_link::approve;
use crate::notices::{Level, Notice, NoticeAction};
use crate::persistence;
use gpui_kit::*;
use koo_agent::tools::video::FrameGrab;
use koo_cloud::{Account, KeySource, Provider};
use koo_core::MediaId;
use koo_core::analysis::AnalysisSteps;
use koo_core::bus::{Author, CommandBus};
use koo_core::changeset::ChangeSetState;
use koo_core::cloud::{Capability, CloudError, GenerateRequest, GenerationTask, ProviderChoice};
use koo_core::model::MediaItem;
use koo_core::services::AnalysisConfig;

/// Key of the notice for a provider set without a key.
const KEY_NOTICE: &str = "cloud-key-missing";

/// What is known of a provider's key and account.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderStatus {
    /// Where its key comes from; `None` until read from the credential
    /// store.
    pub key: Option<KeySource>,
    pub check: Check,
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
    Failed(String),
}

/// Music, a sound effect, a voiceover or a video being made from the media
/// pool.
#[derive(Clone, Debug)]
pub struct Generating {
    pub id: u64,
    pub request: GenerateRequest,
    /// Where it is, and how to stop it.
    pub task: GenerationTask,
    /// Why it failed; `None` while it runs.
    pub failed: Option<CloudError>,
}

/// How often a running generation's row is redrawn with where it is.
const STAGE_POLL: std::time::Duration = std::time::Duration::from_millis(500);

impl EditorState {
    /// Read each provider's key off the UI thread (the credential store may
    /// wait, or ask the user), then bring the notices up to date.
    pub(super) fn load_keys(&mut self, cx: &mut Context<Self>) {
        let cloud = self.cloud.clone();
        let read = cx.background_executor().spawn(async move {
            koo_cloud::PROVIDERS
                .iter()
                .map(|p| (p.id, cloud.keys().source(p)))
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let found = read.await;
            let _ = this.update(cx, |this, cx| {
                for (id, source) in found {
                    this.providers.entry(id).or_default().key = Some(source);
                }
                this.refresh_model_notices();
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
        let cloud = self.cloud.clone();
        let check = cx
            .background_executor()
            .spawn(async move { cloud.account(provider) });
        cx.spawn(async move |this, cx| {
            let result = check.await;
            let _ = this.update(cx, |this, cx| {
                this.providers.entry(provider.id).or_default().check = match result {
                    Ok(account) => Check::Valid(account),
                    Err(e) => Check::Failed(e.to_string()),
                };
                cx.notify();
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
                match result {
                    Ok((source, account)) => {
                        status.key = Some(source);
                        status.check = match account {
                            Ok(account) => Check::Valid(account),
                            Err(e) => Check::Failed(e.to_string()),
                        };
                    }
                    Err(e) => status.check = Check::Failed(e),
                }
                if matches!(status.check, Check::Valid(_)) {
                    for capability in [
                        Capability::Music,
                        Capability::SoundEffects,
                        Capability::Voiceover,
                        Capability::Video,
                    ] {
                        let unset = this.cloud.settings().provider(capability).is_none();
                        if unset && provider.offer(capability).is_some() {
                            this.set_provider(capability, Some(provider), cx);
                        }
                    }
                }
                this.refresh_model_notices();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn remove_api_key(&mut self, provider: &'static Provider, cx: &mut Context<Self>) {
        let cloud = self.cloud.clone();
        let removed = cx.background_executor().spawn(async move {
            let result = cloud.keys().remove(provider);
            (cloud.keys().source(provider), result)
        });
        cx.spawn(async move |this, cx| {
            let (source, result) = removed.await;
            let _ = this.update(cx, |this, cx| {
                this.providers.insert(
                    provider.id,
                    ProviderStatus {
                        key: Some(source),
                        check: match result {
                            Ok(()) => Check::Unchecked,
                            Err(e) => Check::Failed(e),
                        },
                    },
                );
                this.refresh_model_notices();
                cx.notify();
            });
        })
        .detach();
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
        settings.set(capability, choice);
        persistence::with_store(|s| s.save_cloud_settings(&settings));
        self.cloud.set_settings(settings);
        self.configure_analysis();
        self.refresh_model_notices();
        cx.notify();
    }

    pub fn set_analysis_steps(&mut self, steps: AnalysisSteps, cx: &mut Context<Self>) {
        if steps == self.analysis_steps {
            return;
        }
        persistence::with_store(|s| s.save_analysis_steps(&steps));
        self.analysis_steps = steps;
        self.configure_analysis();
        self.refresh_model_notices();
        cx.notify();
    }

    /// Hand analysis the steps and the transcriber for the items analyzed
    /// from now on.
    pub(super) fn configure_analysis(&self) {
        if let Some(analysis) = &self.services.analysis {
            analysis.configure(AnalysisConfig {
                steps: self.analysis_steps,
                speech: self.cloud.speech(),
            });
        }
    }

    /// The provider transcribing in place of the local speech models.
    pub fn online_transcription(&self) -> Option<&'static Provider> {
        self.cloud
            .picked(Capability::Transcription)
            .map(|(provider, _)| provider)
    }

    /// The tasks whose models must be on disk before media can be
    /// analyzed: none while a provider transcribes.
    pub fn required_tasks(&self) -> Vec<koo_models::Task> {
        koo_analysis::LocalAnalysis::required_tasks(
            &self.analysis_steps,
            self.online_transcription().is_some(),
        )
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
        self.generate_video(request, Vec::new(), cx);
    }

    /// [`Self::generate`], first grabbing the program's frames that
    /// `request` starts or ends on.
    pub fn generate_video(
        &mut self,
        request: GenerateRequest,
        grabs: Vec<FrameGrab>,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_generation;
        self.next_generation += 1;
        let task = GenerationTask::default();
        self.generating.push(Generating {
            id,
            request: request.clone(),
            task: task.clone(),
            failed: None,
        });
        let (cloud, services) = (self.cloud.clone(), self.services.clone());
        let project = (!grabs.is_empty()).then(|| self.bus.project().clone());
        let following = task.clone();
        let made = cx.background_executor().spawn(async move {
            use koo_core::cloud::GenerationService as _;
            if let Some(project) = &project {
                koo_agent::tools::video::grab_all(project, &services, &grabs)
                    .map_err(CloudError::Failed)?;
            }
            cloud
                .generate(&request, &koo_core::paths::generated(), &following)
                .map(|g| koo_core::cloud::media_item(&g, services.media.as_deref()))
        });
        cx.spawn(async move |this, cx| {
            let result = made.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(item) => {
                        this.generating.retain(|g| g.id != id);
                        this.add_generated(item, cx);
                    }
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
    }

    /// Ask the provider to stop generation `id`; its row goes when it has.
    pub fn stop_generation(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(g) = self.generating.iter().find(|g| g.id == id) {
            g.task.cancel();
        }
        cx.notify();
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

    /// Forget a generation that failed.
    pub fn dismiss_generation(&mut self, id: u64, cx: &mut Context<Self>) {
        self.generating.retain(|g| g.id != id);
        cx.notify();
    }

    /// Add `item`, which a provider made, to the pool's Generated bin as
    /// one undo step, and to the drafts waiting for review so a proposal
    /// can place it, then analyze it ([`Self::analyze_generated`]). Its id
    /// when added.
    pub(crate) fn add_generated(
        &mut self,
        item: MediaItem,
        cx: &mut Context<Self>,
    ) -> Option<MediaId> {
        let id = item.id;
        let label = match item.generated.as_deref().map(|g| g.capability) {
            Some(Capability::SoundEffects) => "Generate sound effect",
            Some(Capability::Voiceover) => "Generate voiceover",
            Some(Capability::Video) => "Generate video",
            _ => "Generate music",
        };
        let bin = koo_core::cloud::generated_bin(&item);
        let commands = koo_core::cloud::pool_commands(self.bus.project(), item, bin);
        let result = self.bus.execute_shared(commands, label);
        self.finish(result, cx)?;
        self.analyze_generated(cx);
        Some(id)
    }

    /// Take what is in the pool now as looked at: a project just opened
    /// keeps its generated items as they were.
    pub(super) fn mark_generated_seen(&mut self) {
        self.generated_seen = self
            .bus
            .project()
            .media
            .iter()
            .filter(|m| m.generated.is_some())
            .map(|m| m.id)
            .collect();
    }

    /// Analyze each item a provider made as it reaches the pool, once,
    /// however it got there (the media pool, an approval, an agent job): a
    /// video like footage, so the agent can see and hear it, once the
    /// analysis models are ready; a voiceover when transcripts are on (its
    /// words are its script, so it costs no credits). Music and sound
    /// effects are left to be analyzed when asked.
    pub(super) fn analyze_generated(&mut self, cx: &mut Context<Self>) {
        let new: Vec<(MediaId, Capability)> = self
            .bus
            .project()
            .media
            .iter()
            .filter(|m| m.fingerprint.is_some() && !self.generated_seen.contains(&m.id))
            .filter_map(|m| Some((m.id, m.generated.as_deref()?.capability)))
            .collect();
        for (id, capability) in new {
            self.generated_seen.insert(id);
            let analyze = match capability {
                Capability::Video => self.models_ready(),
                Capability::Voiceover => self.analysis_steps.transcript,
                _ => false,
            };
            if analyze {
                self.analyze(id, false, cx);
            }
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
            placement,
            summary,
        } = made;
        let name = approve::file_name(&item.path);
        let bin = koo_core::cloud::generated_bin(&item);
        let Some(media) = self.add_generated(item, cx) else {
            let e = self.status.clone().unwrap_or_default();
            self.agent.report_failure(
                id,
                format!("approved: {summary}; made {name}, but could not add it to the pool: {e}"),
            );
            return ApprovalOutcome::Failed(e);
        };
        let mut outcome =
            format!("approved: {summary}; made media {media} ({name}) in the {bin} bin");
        if let Some(placement) = placement {
            match self.with_agent_set(cx, |bus, author| {
                koo_agent::tools::generate::propose_placement(
                    bus, author, media, &placement, &summary,
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
    fn with_agent_set<T>(
        &mut self,
        cx: &mut Context<Self>,
        propose: impl FnOnce(&mut CommandBus, &Author) -> Result<T, String>,
    ) -> Result<T, String> {
        let author = Author::Agent {
            role: self.settings.role,
        };
        let turn = self.agent.is_busy();
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
