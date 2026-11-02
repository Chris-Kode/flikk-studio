//! Voices in the editor: the account's list, read once and again after a
//! change; keeping a voice in the user's library once it is made, added or
//! used, so it is listed and heard without a request; renaming and deleting
//! it on the account and in the library; and the recording of one speaker
//! that a clone or a search for similar voices hears; the sample an
//! approval card plays before the user decides.

use super::{ApprovalOutcome, EditorState};
use gpui_kit::*;
use koo_agent::tools::ApprovalRequest;
use koo_core::cloud::{
    self, CloudError, GenerateRequest, GenerationService, Reading, Voice, VoiceOrigin,
};
use koo_core::library::{self, SavedVoice};
use koo_core::model::{MediaItem, Project};
use koo_core::query::TimelineWord;
use koo_core::{LibraryId, SpeakerId, Time, TimeRange};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The most of one speaker a clone hears: more adds little.
pub const SPEAKER_LIMIT: Time = Time(180 * koo_core::time::FLICKS_PER_SECOND);
/// The most of one recording sent to find voices like it.
pub const SIMILAR_LIMIT: Time = Time(30 * koo_core::time::FLICKS_PER_SECOND);
/// Words of one speaker this close together are one stretch of speech.
const JOIN: f64 = 0.6;
/// Kept around each stretch, so no word is clipped.
const PAD: f64 = 0.08;

/// The account's voices, as last read.
#[derive(Clone, Debug, Default)]
pub struct AccountVoices {
    /// `None` until read.
    pub voices: Option<Vec<Voice>>,
    pub loading: bool,
    pub error: Option<String>,
}

impl AccountVoices {
    /// Whether the account has `voice`; `None` while unknown.
    pub fn has(&self, voice: &str) -> Option<bool> {
        Some(self.voices.as_ref()?.iter().any(|v| v.id == voice))
    }
}

/// How a voice can be heard.
pub enum VoiceSample {
    Here(MediaItem),
    Fetching,
    /// Fetched on asking.
    Remote(Voice),
}

/// The voice approval `request` adds or reads with, and where it can be
/// heard when the agent listed it.
pub fn approval_voice(request: &ApprovalRequest) -> Option<(String, Option<String>)> {
    let voice = match request.tool.as_str() {
        "add_voice" => request.args.get("voice")?.as_str()?.to_owned(),
        "generate_audio" => match koo_agent::tools::generate::from_json(&request.args)?.0 {
            GenerateRequest::Voiceover { reading, .. } => reading.voice,
            _ => return None,
        },
        _ => return None,
    };
    let url = request
        .args
        .get("sample")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Some((voice, url))
}

impl EditorState {
    /// How `voice` can be heard: its sample in the library or fetched
    /// already, on its way, or where to fetch it from (`url`, else the
    /// account's list); `None` when it can't.
    pub fn voice_sample(&self, voice: &str, url: Option<&str>) -> Option<VoiceSample> {
        if let Some(provider) = self.voice_provider()
            && let Some(item) = self
                .library
                .voice_of(&provider, voice)
                .and_then(SavedVoice::sample_item)
        {
            return Some(VoiceSample::Here(item));
        }
        match self.heard_voices.get(voice) {
            Some(Some(item)) => return Some(VoiceSample::Here(item.clone())),
            Some(None) => return Some(VoiceSample::Fetching),
            None => {}
        }
        let listed = self
            .account_voices
            .voices
            .iter()
            .flatten()
            .find(|v| v.id == voice);
        let sample = url
            .map(str::to_owned)
            .or_else(|| listed.and_then(|v| v.sample.clone()))?;
        Some(VoiceSample::Remote(Voice {
            id: voice.to_owned(),
            name: listed.map_or_else(|| voice.to_owned(), |v| v.name.clone()),
            sample: Some(sample),
            ..Voice::default()
        }))
    }

    /// Fetch the sample of `voice`, off the UI thread, then play it.
    pub fn hear_voice(&mut self, voice: Voice, cx: &mut Context<Self>) {
        let Some(generation) = self.generation() else {
            return;
        };
        let media = self.services.media.clone();
        let id = voice.id.clone();
        self.heard_voices.insert(id.clone(), None);
        let fetch = cx.background_executor().spawn(async move {
            let path = generation.voice_sample(&voice)?;
            Ok::<_, CloudError>(cloud::sample_item(path, media.as_deref()))
        });
        cx.spawn(async move |this, cx| {
            let result = fetch.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(item) => {
                        this.toggle_source(&item, cx);
                        this.heard_voices.insert(id, Some(item));
                    }
                    Err(e) => {
                        this.heard_voices.remove(&id);
                        this.status = Some(e.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The user kept `voice` in the Voices dialog that approving design
    /// request `id` opened: tell the agent which.
    pub fn settle_design(&mut self, id: u64, voice: &Voice, cx: &mut Context<Self>) {
        let Some(pending) = self.agent.resolve_approval(id) else {
            return;
        };
        self.agent.report_outcome(
            id,
            format!(
                "approved: {}; the user heard the takes and kept voice {} (“{}”); read with it in generate_audio",
                pending.request.summary, voice.id, voice.name
            ),
        );
        self.settled
            .push((pending, ApprovalOutcome::Done(voice.name.clone())));
        cx.notify();
    }

    pub fn generation(&self) -> Option<Arc<dyn GenerationService>> {
        self.services.generation.clone()
    }

    /// Provider id doing voiceovers, which voices belong to.
    pub fn voice_provider(&self) -> Option<String> {
        self.services.generation.as_ref()?.voice_provider()
    }

    /// Read the account's voices again, off the UI thread.
    pub fn refresh_voices(&mut self, cx: &mut Context<Self>) {
        let Some(generation) = self.generation() else {
            return;
        };
        if self.account_voices.loading {
            return;
        }
        self.account_voices.loading = true;
        let load = cx
            .background_executor()
            .spawn(async move { generation.voices() });
        cx.spawn(async move |this, cx| {
            let result = load.await;
            let _ = this.update(cx, |this, cx| {
                let list = &mut this.account_voices;
                list.loading = false;
                match result {
                    Ok(voices) => {
                        list.voices = Some(voices);
                        list.error = None;
                    }
                    Err(e) => list.error = Some(e.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Read the account's voices if they haven't been.
    pub fn want_voices(&mut self, cx: &mut Context<Self>) {
        let list = &self.account_voices;
        if list.voices.is_none() && !list.loading && list.error.is_none() {
            self.refresh_voices(cx);
        }
    }

    /// Keep `voice`, on the account now, in the library, with `sample` as
    /// what it sounds like; the next voiceover starts on it. Its entry.
    pub fn keep_voice(
        &mut self,
        voice: &Voice,
        origin: VoiceOrigin,
        sample: Option<&MediaItem>,
        cx: &mut Context<Self>,
    ) -> Option<LibraryId> {
        let provider = self.voice_provider()?;
        let entry = SavedVoice::new(&provider, voice, origin);
        let saved = library::save_voice(&self.library, &entry, sample);
        self.remember_voice(&voice.id);
        if let Some(list) = self.account_voices.voices.as_mut()
            && !list.iter().any(|v| v.id == voice.id)
        {
            list.insert(0, voice.clone());
        }
        match saved {
            Ok(saved) => {
                self.reload_library(cx);
                Some(saved.id)
            }
            Err(e) => {
                self.status = Some(e);
                cx.notify();
                None
            }
        }
    }

    /// Keep `reading`'s stability and similarity with its voice, when the
    /// voice is in the library.
    pub fn keep_reading(&mut self, reading: &Reading, cx: &mut Context<Self>) {
        let Some(provider) = self.voice_provider() else {
            return;
        };
        let Some(saved) = self.library.voice_of(&provider, &reading.voice).cloned() else {
            return;
        };
        if saved.stability == reading.stability && saved.similarity == reading.similarity {
            return;
        }
        if library::set_voice_reading(&saved, reading).is_ok() {
            self.reload_library(cx);
        }
    }

    /// Rename library voice `id`, there and on the account.
    pub fn rename_saved_voice(&mut self, id: LibraryId, name: &str, cx: &mut Context<Self>) {
        let Some(saved) = self.library.voice(id).cloned() else {
            return;
        };
        let name = name.trim().to_owned();
        if name.is_empty() || name == saved.name {
            return;
        }
        match library::rename_voice(&saved, &name) {
            Ok(_) => self.reload_library(cx),
            Err(e) => {
                self.status = Some(e);
                cx.notify();
                return;
            }
        }
        // Voices of the provider's own, or shared ones, keep their name
        // there.
        if !matches!(
            saved.origin,
            VoiceOrigin::Designed { .. } | VoiceOrigin::Cloned { .. }
        ) {
            return;
        }
        let Some(generation) = self.generation() else {
            return;
        };
        let voice = saved.voice.clone();
        let task = cx
            .background_executor()
            .spawn(async move { generation.rename_voice(&voice, &name) });
        cx.spawn(async move |this, cx| {
            if let Err(e) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.status = Some(format!("Renamed here only: {e}"));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Delete library voice `id` from the account, then the library. What
    /// it read stays.
    pub fn delete_saved_voice(&mut self, id: LibraryId, cx: &mut Context<Self>) {
        let Some(saved) = self.library.voice(id).cloned() else {
            return;
        };
        let Some(generation) = self.generation() else {
            return;
        };
        let voice = saved.voice.clone();
        let task = cx
            .background_executor()
            .spawn(async move { generation.delete_voice(&voice) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        if let Some(list) = this.account_voices.voices.as_mut() {
                            list.retain(|v| v.id != saved.voice);
                        }
                        this.forget_voice(id, cx);
                    }
                    Err(e) => this.status = Some(e.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Take library voice `id` out of the library; it stays on the account.
    pub fn forget_voice(&mut self, id: LibraryId, cx: &mut Context<Self>) {
        let Some(saved) = self.library.voice(id).cloned() else {
            return;
        };
        if let Err(e) = library::remove(&saved.path) {
            self.status = Some(e);
        }
        if let Some(sample) = saved.sample_file() {
            let _ = std::fs::remove_file(sample);
        }
        self.reload_library(cx);
    }
}

/// Where the voice of `speaker` is clearest: the source and stretches of
/// it where they speak most, up to [`SPEAKER_LIMIT`].
pub fn speaker_spans(
    project: &Project,
    words: &[TimelineWord],
    speaker: SpeakerId,
) -> Option<(PathBuf, Vec<TimeRange>)> {
    let theirs: Vec<&TimelineWord> = words
        .iter()
        .filter(|w| w.word.speaker == Some(speaker))
        .collect();
    let mut by_media: HashMap<koo_core::MediaId, Vec<TimeRange>> = HashMap::new();
    for w in &theirs {
        by_media.entry(w.media).or_default().push(w.source);
    }
    let (media, mut ranges) = by_media
        .into_iter()
        .max_by_key(|(_, r)| r.iter().map(|r| r.duration().0).sum::<i64>())?;
    let path = project.media(media)?.path.clone();
    ranges.sort_by_key(|r| r.start);
    let mut spans: Vec<TimeRange> = Vec::new();
    let mut total = Time::ZERO;
    for r in ranges {
        let start = Time::from_secs_f64((r.start.as_secs_f64() - PAD).max(0.0));
        let end = Time::from_secs_f64(r.end.as_secs_f64() + PAD);
        match spans.last_mut() {
            Some(last) if start.as_secs_f64() - last.end.as_secs_f64() < JOIN => {
                total = total + (end - last.end).max(Time::ZERO);
                last.end = last.end.max(end);
            }
            _ => {
                if total >= SPEAKER_LIMIT {
                    break;
                }
                total = total + (end - start);
                spans.push(TimeRange::new(start, end));
            }
        }
    }
    (!spans.is_empty()).then_some((path, spans))
}

/// `spans` of `source` (all of it when none) as one WAV under the cache,
/// at most `limit` long, named after `name`. Slow: runs ffmpeg.
pub fn recording(
    source: &Path,
    spans: &[TimeRange],
    limit: Time,
    name: &str,
) -> Result<PathBuf, String> {
    let dir = koo_core::paths::cache().join("voice-sources");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stem: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let out = dir.join(format!("{stem}-{at}.wav"));
    koo_analysis::cut_speech(source, spans, limit, &out).map_err(|e| format!("{e:#}"))?;
    Ok(out)
}
