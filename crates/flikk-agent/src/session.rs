//! Backend session ids, one per project, chat, backend and role, so each
//! chat session resumes its own conversation. Stored in
//! `paths::home()/agent/sessions.json`.

use flikk_core::ProjectId;
use flikk_core::agent::Backend;
use flikk_core::bus::AgentRole;
use flikk_core::paths;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// `paths::home()/agent`, created on first use.
pub fn agent_dir() -> PathBuf {
    let dir = paths::home().join("agent");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn file() -> PathBuf {
    agent_dir().join("sessions.json")
}

/// The chat every project starts with. It keeps the key format from before
/// projects had several chats, so existing conversations still resume.
pub const MAIN_CHAT: &str = "main";

/// The key a backend session is stored under.
pub fn key(project: ProjectId, chat: &str, backend: Backend, role: AgentRole) -> String {
    let base = format!(
        "{}/{}/{}",
        project.0,
        backend.label().to_lowercase(),
        match role {
            AgentRole::Editor => "editor",
            AgentRole::Builder => "builder",
        }
    );
    if chat == MAIN_CHAT {
        base
    } else {
        format!("{base}/{chat}")
    }
}

fn load_all() -> BTreeMap<String, String> {
    std::fs::read(file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn load(project: ProjectId, chat: &str, backend: Backend, role: AgentRole) -> Option<String> {
    load_all().remove(&key(project, chat, backend, role))
}

/// `None` forgets the session (e.g. the backend no longer knows it).
pub fn store(
    project: ProjectId,
    chat: &str,
    backend: Backend,
    role: AgentRole,
    session: Option<&str>,
) {
    let mut all = load_all();
    let k = key(project, chat, backend, role);
    match session {
        Some(s) => all.insert(k, s.to_owned()),
        None => all.remove(&k),
    };
    save_all(&all);
}

/// Forget a deleted chat's sessions on every backend and role.
pub fn forget_chat(project: ProjectId, chat: &str) {
    let mut all = load_all();
    let before = all.len();
    let mut keys = Vec::new();
    for backend in [Backend::Claude, Backend::Codex] {
        for role in [AgentRole::Editor, AgentRole::Builder] {
            keys.push(key(project, chat, backend, role));
        }
    }
    for k in &keys {
        all.remove(k);
    }
    crate::tools::groups::forget(&keys);
    if all.len() != before {
        save_all(&all);
    }
}

fn save_all(all: &BTreeMap<String, String>) {
    if let Ok(bytes) = serde_json::to_vec_pretty(all) {
        let _ = flikk_os::fs::write_durable(&file(), &bytes);
    }
}
