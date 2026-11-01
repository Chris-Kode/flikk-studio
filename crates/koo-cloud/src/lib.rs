//! Online providers: work done by a provider's API instead of this machine.
//! Implements `koo_core::cloud::{SpeechService, GenerationService}`.
//!
//! - [`PROVIDERS`]: each provider as the user sees it (name, where to get
//!   a key) and what it offers: for each [`Capability`], its models, the
//!   default first.
//! - `<provider>/`: its client, one file per capability, behind [`Api`].
//! - [`Keys`]: API keys, in the system's credential store.
//! - [`Cloud`]: runs a request on the provider the user picked for its
//!   capability ([`CloudSettings`]), with that provider's key.
//!
//! A new provider is a folder with its [`Api`], its [`Provider`] in
//! [`PROVIDERS`], and nothing else: settings, analysis and the agent find
//! it through the catalog.

mod elevenlabs;
mod http;
mod keys;

pub use keys::{KeySource, Keys};

use koo_core::cloud::{
    Capability, CloudError, CloudResult, CloudSettings, Designed, GenerateRequest, Generated,
    GenerationInfo, GenerationService, Heard, SpeechRequest, SpeechService, Voice, VoiceClone,
    VoiceDesign, VoicePage, VoicePreview, VoiceQuery,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// Every provider the editor knows.
pub const PROVIDERS: &[&Provider] = &[&elevenlabs::PROVIDER];

/// One provider as the user sees it.
pub struct Provider {
    /// Stable key for settings and the credential store: "elevenlabs".
    pub id: &'static str,
    pub name: &'static str,
    /// Where the user makes an API key.
    pub keys_url: &'static str,
    /// Environment variable whose key wins over the stored one.
    pub key_env: &'static str,
    pub offers: &'static [Offer],
    pub api: &'static dyn Api,
}

/// A capability a provider offers, on these models, the default first.
pub struct Offer {
    pub capability: Capability,
    pub models: &'static [CloudModel],
}

pub struct CloudModel {
    /// Sent to the provider as is: "scribe_v2".
    pub id: &'static str,
    pub name: &'static str,
}

impl Provider {
    pub fn offer(&self, capability: Capability) -> Option<&'static Offer> {
        self.offers.iter().find(|o| o.capability == capability)
    }

    /// `id` if it is one of its models for `capability`, else the default.
    pub fn model(&self, capability: Capability, id: Option<&str>) -> Option<&'static CloudModel> {
        let models = self.offer(capability)?.models;
        models
            .iter()
            .find(|m| Some(m.id) == id)
            .or_else(|| models.first())
    }
}

pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id).copied()
}

/// The providers that offer `capability`.
pub fn providers_for(capability: Capability) -> impl Iterator<Item = &'static Provider> {
    PROVIDERS
        .iter()
        .copied()
        .filter(move |p| p.offer(capability).is_some())
}

/// What a key's account has: its plan and credits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Account {
    /// "creator"; `None` when the key may not read it.
    pub plan: Option<String>,
    pub credits_used: Option<u64>,
    pub credits_limit: Option<u64>,
}

impl Account {
    pub fn credits_left(&self) -> Option<u64> {
        Some(self.credits_limit?.saturating_sub(self.credits_used?))
    }
}

/// A provider's client. Each method is only called for capabilities the
/// provider offers; `key` is its API key, `model` one of its model ids.
pub trait Api: Sync {
    /// Check `key` and read its account.
    fn account(&self, key: &str) -> CloudResult<Account>;

    fn transcribe(
        &self,
        _key: &str,
        _model: &str,
        _request: &SpeechRequest,
        _progress: &(dyn Fn(f32) + Sync),
    ) -> CloudResult<Heard> {
        Err(CloudError::Unavailable("not offered".into()))
    }

    /// Make `request` into `<stem>.<extension it picks>`; the file written
    /// and the credits it cost.
    fn generate(
        &self,
        _key: &str,
        _model: &str,
        _request: &GenerateRequest,
        _stem: &Path,
    ) -> CloudResult<(PathBuf, Option<u64>)> {
        Err(CloudError::Unavailable("not offered".into()))
    }

    /// The voices voiceovers can use with `key`.
    fn voices(&self, _key: &str) -> CloudResult<Vec<Voice>> {
        Err(CloudError::Unavailable("not offered".into()))
    }

    /// The provider's shared voices; only for providers offering
    /// voiceovers, as are the calls below.
    fn search_voices(
        &self,
        _key: &str,
        _query: &VoiceQuery,
        _page: usize,
    ) -> CloudResult<VoicePage> {
        Err(CloudError::Unavailable("not offered".into()))
    }

    fn similar_voices(&self, _key: &str, _audio: &Path) -> CloudResult<Vec<Voice>> {
        Err(CloudError::Unavailable("not offered".into()))
    }

    fn add_voice(&self, _key: &str, _voice: &Voice, _name: &str) -> CloudResult<Voice> {
        Err(CloudError::Unavailable("not offered".into()))
    }

    fn design_voice(&self, _key: &str, _design: &VoiceDesign) -> CloudResult<Designed> {
        Err(CloudError::Unavailable("not offered".into()))
    }

    fn save_voice(
        &self,
        _key: &str,
        _design: &VoiceDesign,
        _preview: &VoicePreview,
        _name: &str,
    ) -> CloudResult<Voice> {
        Err(CloudError::Unavailable("not offered".into()))
    }

    fn clone_voice(&self, _key: &str, _clone: &VoiceClone) -> CloudResult<Voice> {
        Err(CloudError::Unavailable("not offered".into()))
    }

    fn rename_voice(&self, _key: &str, _voice: &str, _name: &str) -> CloudResult<()> {
        Err(CloudError::Unavailable("not offered".into()))
    }

    fn delete_voice(&self, _key: &str, _voice: &str) -> CloudResult<()> {
        Err(CloudError::Unavailable("not offered".into()))
    }
}

/// Runs each request on the provider picked for its capability. Clones of
/// the `Arc` share settings and keys.
pub struct Cloud {
    settings: Mutex<CloudSettings>,
    keys: Keys,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Cloud {
    pub fn new(settings: CloudSettings) -> Self {
        Self {
            settings: Mutex::new(settings),
            keys: Keys::default(),
        }
    }

    pub fn set_settings(&self, settings: CloudSettings) {
        *lock(&self.settings) = settings;
    }

    pub fn settings(&self) -> CloudSettings {
        lock(&self.settings).clone()
    }

    pub fn keys(&self) -> &Keys {
        &self.keys
    }

    /// Check `provider`'s key and read its account. Slow: a request.
    pub fn account(&self, provider: &Provider) -> CloudResult<Account> {
        let key = self.key(provider)?;
        provider.api.account(&key)
    }

    /// The transcriber to give analysis: `None` while transcription runs
    /// on this machine.
    pub fn speech(self: &Arc<Self>) -> Option<Arc<dyn SpeechService>> {
        self.picked(Capability::Transcription)?;
        Some(self.clone())
    }

    /// The provider and model picked for `capability`, if a provider is.
    pub fn picked(
        &self,
        capability: Capability,
    ) -> Option<(&'static Provider, &'static CloudModel)> {
        let choice = lock(&self.settings).choice(capability);
        let provider = provider(choice.provider.as_deref()?)?;
        let model = provider.model(capability, choice.model.as_deref())?;
        Some((provider, model))
    }

    fn key(&self, provider: &Provider) -> CloudResult<String> {
        self.keys.get(provider).ok_or_else(|| {
            CloudError::Unavailable(format!(
                "No {} API key; add one in Settings → Integrations",
                provider.name
            ))
        })
    }

    /// The client and key of the provider doing voiceovers: voices belong
    /// to it.
    fn voice_api(&self) -> CloudResult<(&'static dyn Api, String)> {
        let (provider, _) = self.require(Capability::Voiceover)?;
        Ok((provider.api, self.key(provider)?))
    }

    fn require(
        &self,
        capability: Capability,
    ) -> CloudResult<(&'static Provider, &'static CloudModel)> {
        self.picked(capability).ok_or_else(|| {
            CloudError::Unavailable(format!(
                "No provider is set for {}; pick one in Settings → Integrations",
                capability.label().to_lowercase()
            ))
        })
    }
}

impl SpeechService for Cloud {
    fn name(&self) -> String {
        self.picked(Capability::Transcription)
            .map_or_else(String::new, |(p, _)| p.name.to_owned())
    }

    fn transcribe(
        &self,
        request: &SpeechRequest,
        progress: &(dyn Fn(f32) + Sync),
    ) -> CloudResult<Heard> {
        let (provider, model) = self.require(Capability::Transcription)?;
        let key = self.key(provider)?;
        provider.api.transcribe(&key, model.id, request, progress)
    }
}

impl GenerationService for Cloud {
    fn provider_for(&self, capability: Capability) -> Option<String> {
        let (provider, _) = self.picked(capability)?;
        self.keys.get(provider).map(|_| provider.name.to_owned())
    }

    fn generate(&self, request: &GenerateRequest, dir: &Path) -> CloudResult<Generated> {
        let capability = request.capability();
        let (provider, model) = self.require(capability)?;
        let key = self.key(provider)?;
        std::fs::create_dir_all(dir)
            .map_err(|e| CloudError::Failed(format!("could not write {}: {e}", dir.display())))?;
        let stem = free_stem(dir, request);
        let (path, cost) = provider.api.generate(&key, model.id, request, &stem)?;
        let info = GenerationInfo {
            capability,
            provider: provider.id.to_owned(),
            model: model.id.to_owned(),
            prompt: request.prompt().to_owned(),
            reading: request.reading().cloned(),
        };
        write_sidecar(&path, request, &info, cost);
        Ok(Generated { path, info, cost })
    }

    fn voice_provider(&self) -> Option<String> {
        self.picked(Capability::Voiceover)
            .map(|(p, _)| p.id.to_owned())
    }

    fn voices(&self) -> CloudResult<Vec<Voice>> {
        let (api, key) = self.voice_api()?;
        api.voices(&key)
    }

    fn search_voices(&self, query: &VoiceQuery, page: usize) -> CloudResult<VoicePage> {
        let (api, key) = self.voice_api()?;
        api.search_voices(&key, query, page)
    }

    fn similar_voices(&self, audio: &Path) -> CloudResult<Vec<Voice>> {
        let (api, key) = self.voice_api()?;
        api.similar_voices(&key, audio)
    }

    fn add_voice(&self, voice: &Voice, name: &str) -> CloudResult<Voice> {
        let (api, key) = self.voice_api()?;
        api.add_voice(&key, voice, name.trim())
    }

    fn design_voice(&self, design: &VoiceDesign) -> CloudResult<Designed> {
        let (api, key) = self.voice_api()?;
        api.design_voice(&key, design)
    }

    fn save_voice(
        &self,
        design: &VoiceDesign,
        preview: &VoicePreview,
        name: &str,
    ) -> CloudResult<Voice> {
        let (api, key) = self.voice_api()?;
        api.save_voice(&key, design, preview, name.trim())
    }

    fn clone_voice(&self, clone: &VoiceClone) -> CloudResult<Voice> {
        let (api, key) = self.voice_api()?;
        api.clone_voice(&key, clone)
    }

    fn rename_voice(&self, voice: &str, name: &str) -> CloudResult<()> {
        let (api, key) = self.voice_api()?;
        api.rename_voice(&key, voice, name.trim())
    }

    fn delete_voice(&self, voice: &str) -> CloudResult<()> {
        let (api, key) = self.voice_api()?;
        api.delete_voice(&key, voice)
    }

    fn voice_sample(&self, voice: &Voice) -> CloudResult<PathBuf> {
        let fail = |what: String| CloudError::Failed(what);
        let name: String = voice
            .id
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        let dir = koo_core::paths::cache().join("voices");
        let path = dir.join(format!("{name}.mp3"));
        if path.exists() {
            return Ok(path);
        }
        let url = voice
            .sample
            .as_deref()
            .filter(|url| url.starts_with("https://"))
            .ok_or_else(|| fail(format!("{} has no sample", voice.name)))?;
        // A public file, on another host: the key doesn't go with it.
        let reply = http::get(url, &[])?;
        if !reply.ok() {
            return Err(fail(format!("No sample of {}", voice.name)));
        }
        let written = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(path.with_extension("mp3.part"), &reply.body))
            .and_then(|()| std::fs::rename(path.with_extension("mp3.part"), &path));
        written.map_err(|e| fail(format!("could not write {}: {e}", path.display())))?;
        Ok(path)
    }
}

/// `<dir>/<what>-<a few words of the prompt>`, numbered past any file
/// already there with that name.
fn free_stem(dir: &Path, request: &GenerateRequest) -> PathBuf {
    let what = match request {
        GenerateRequest::Music { .. } => "music",
        GenerateRequest::SoundEffect { .. } => "sfx",
        GenerateRequest::Voiceover { .. } => "voiceover",
    };
    let words: Vec<String> = request
        .prompt()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(5)
        .map(str::to_lowercase)
        .collect();
    let base = if words.is_empty() {
        what.to_owned()
    } else {
        format!("{what}-{}", words.join("-"))
    };
    let taken = |stem: &str| {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                e.path()
                    .file_stem()
                    .is_some_and(|s| s.to_string_lossy() == stem)
            })
    };
    let mut stem = base.clone();
    let mut n = 2;
    while taken(&stem) {
        stem = format!("{base}-{n}");
        n += 1;
    }
    dir.join(stem)
}

/// `<file>.json` beside a generated file: what was asked, of whom, and
/// what it cost, so it can be made again or searched.
fn write_sidecar(path: &Path, request: &GenerateRequest, info: &GenerationInfo, cost: Option<u64>) {
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let json = serde_json::json!({
        "request": request,
        "made_by": info,
        "credits": cost,
        "at": at,
    });
    let mut sidecar = path.as_os_str().to_owned();
    sidecar.push(".json");
    if let Ok(text) = serde_json::to_string_pretty(&json) {
        let _ = std::fs::write(sidecar, text);
    }
}
