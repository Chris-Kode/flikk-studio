//! Backend session ids, one per project, backend and role, so each project
//! chat resumes its own conversation. Stored in
//! `paths::home()/agent/sessions.json`.

use koo_core::ProjectId;
use koo_core::agent::Backend;
use koo_core::bus::AgentRole;
use koo_core::paths;
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

fn key(project: ProjectId, backend: Backend, role: AgentRole) -> String {
    format!(
        "{}/{}/{}",
        project.0,
        backend.label().to_lowercase(),
        match role {
            AgentRole::Editor => "editor",
            AgentRole::Builder => "builder",
        }
    )
}

fn load_all() -> BTreeMap<String, String> {
    std::fs::read(file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn load(project: ProjectId, backend: Backend, role: AgentRole) -> Option<String> {
    load_all().remove(&key(project, backend, role))
}

/// `None` forgets the session (e.g. the backend no longer knows it).
pub fn store(project: ProjectId, backend: Backend, role: AgentRole, session: Option<&str>) {
    let mut all = load_all();
    let k = key(project, backend, role);
    match session {
        Some(s) => all.insert(k, s.to_owned()),
        None => all.remove(&k),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&all) {
        let tmp = file().with_extension("json.tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, file());
        }
    }
}
