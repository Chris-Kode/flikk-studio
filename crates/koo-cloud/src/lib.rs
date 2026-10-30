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
    Capability, CloudError, CloudResult, CloudSettings, GenerateRequest, Generated, GenerationInfo,
    GenerationService, Heard, SpeechRequest, SpeechService,
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
        };
        write_sidecar(&path, request, &info, cost);
        Ok(Generated { path, info, cost })
    }
}

/// `<dir>/<what>-<a few words of the prompt>`, numbered past any file
/// already there with that name.
fn free_stem(dir: &Path, request: &GenerateRequest) -> PathBuf {
    let what = match request {
        GenerateRequest::Music { .. } => "music",
        GenerateRequest::SoundEffect { .. } => "sfx",
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
