//! A project's chat sessions with the agent, saved as JSON beside its
//! other files under `paths::projects()`, and the chats earlier demos left.

use super::proposals::load_proposals;
use super::{notify, project_file};
use flikk_agent::session::MAIN_CHAT;
use flikk_core::agent::{Attachment, ChatMessage};
use flikk_core::bus::CommandBus;
use flikk_core::ids::ProjectId;
use serde::{Deserialize, Serialize};

/// A project's chat sessions, as JSON beside the project's other files
/// under `paths::projects()`.
fn chats_path(project: ProjectId) -> std::path::PathBuf {
    flikk_core::paths::projects().join(format!("{}.chats.json", project.0.simple()))
}

/// The single conversation projects had before chat sessions.
fn legacy_chat_path(project: ProjectId) -> std::path::PathBuf {
    flikk_core::paths::projects().join(format!("{}.chat.json", project.0.simple()))
}

/// One conversation with the agent. Each resumes its own backend session,
/// so a new chat starts without the context of the others.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatSession {
    pub id: String,
    pub title: String,
    /// Unix seconds of the last message, or of creation.
    pub at: i64,
    pub messages: Vec<ChatMessage>,
    /// Files and folders the agent may read in this chat.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

impl ChatSession {
    pub fn new(id: impl Into<String>, at: i64) -> Self {
        Self {
            id: id.into(),
            title: String::new(),
            at,
            messages: Vec::new(),
            attachments: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Chats {
    pub active: String,
    pub sessions: Vec<ChatSession>,
}

/// The project's chat sessions, never empty, with the proposals still
/// waiting for review in them restored to `bus` (see [`super::Proposals`]). A
/// message keeps its link to a change set only if that set came back.
pub fn load_chats(bus: &mut CommandBus) -> Chats {
    let project = bus.project().id;
    let mut chats = std::fs::read(chats_path(project))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Chats>(&bytes).ok())
        .unwrap_or_else(|| legacy_chats(project));
    if chats.sessions.is_empty() {
        chats.sessions.push(ChatSession::new(MAIN_CHAT, 0));
    }
    if !chats.sessions.iter().any(|s| s.id == chats.active) {
        chats.active = chats.sessions[0].id.clone();
    }
    // A deleted chat's proposals have nowhere to show.
    let sets = load_proposals(bus.project())
        .into_iter()
        .filter(|set| {
            set.chat
                .as_ref()
                .is_none_or(|c| chats.sessions.iter().any(|s| &s.id == c))
        })
        .collect();
    let restored = bus.restore_change_sets(sets);
    for m in chats.sessions.iter_mut().flat_map(|s| &mut s.messages) {
        m.change_set = m.change_set.and_then(|id| restored.get(&id).copied());
    }
    chats
}

/// The pre-sessions chat becomes the main session, which keeps resuming
/// the same backend conversation.
fn legacy_chats(project: ProjectId) -> Chats {
    let messages: Vec<ChatMessage> = std::fs::read(legacy_chat_path(project))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut main = ChatSession::new(MAIN_CHAT, messages.last().map_or(0, |m| m.at));
    main.messages = messages;
    Chats {
        active: main.id.clone(),
        sessions: vec![main],
    }
}

/// Save the project's chats. A demo's are not kept: like its project, they
/// would never be opened again.
pub fn save_chats(project: ProjectId, chats: &Chats) {
    if crate::demo::enabled() {
        return;
    }
    let written = serde_json::to_vec(chats)
        .map_err(std::io::Error::other)
        .and_then(|bytes| flikk_os::fs::write_durable(&chats_path(project), &bytes));
    match written {
        Ok(()) => {
            let _ = std::fs::remove_file(legacy_chat_path(project));
        }
        Err(e) => notify(format!("Couldn't save the chat: {e}")),
    }
}

/// Remove the chats earlier demos left: each demo was a new project and its
/// chats were saved, though nothing opens it again. One is known by the
/// demo's seeded conversation, and kept when its project has an autosave
/// or a backup (it was saved to a file, which may still open it).
pub(super) fn sweep_demo_chats() {
    let Ok(entries) = std::fs::read_dir(flikk_core::paths::projects()) else {
        return;
    };
    let backups: Vec<String> = std::fs::read_dir(flikk_core::paths::backups())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    for path in entries.flatten().map(|e| e.path()) {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let (id, sessions) = if let Some(id) = name.strip_suffix(".chats.json") {
            (id, true)
        } else if let Some(id) = name.strip_suffix(".chat.json") {
            (id, false)
        } else {
            continue;
        };
        let Ok(id) = uuid::Uuid::parse_str(id).map(ProjectId) else {
            continue;
        };
        // Backup folders end with the project's short id.
        let short = format!(" {id}");
        if project_file::autosave_path(id).exists() || backups.iter().any(|b| b.ends_with(&short)) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let demo = if sessions {
            serde_json::from_slice::<Chats>(&bytes).is_ok_and(|chats| {
                chats
                    .sessions
                    .iter()
                    .any(|s| crate::demo::is_seed_chat(&s.messages))
            })
        } else {
            serde_json::from_slice::<Vec<ChatMessage>>(&bytes)
                .is_ok_and(|messages| crate::demo::is_seed_chat(&messages))
        };
        if demo {
            let _ = std::fs::remove_file(&path);
        }
    }
}
