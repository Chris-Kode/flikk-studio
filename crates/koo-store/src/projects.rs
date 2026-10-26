//! Projects: the `Project` and its undo `History` as JSON, plus the
//! autosave snapshot used for crash recovery.

use crate::{Store, now, parse_uuid, uuid_text};
use anyhow::{Context, Result};
use koo_core::history::History;
use koo_core::ids::ProjectId;
use koo_core::model::Project;
use rusqlite::{OptionalExtension, params};

const LAST_PROJECT: &str = "last_project";

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectSummary {
    pub id: ProjectId,
    pub name: String,
    /// Unix seconds, including unrecovered autosaves.
    pub updated_at: i64,
}

/// A project as loaded for editing.
#[derive(Clone, Debug)]
pub struct OpenedProject {
    pub project: Project,
    pub history: History,
    /// The last session did not close cleanly and this is its autosave.
    pub recovered: bool,
}

impl Store {
    /// Insert a new empty project.
    pub fn create_project(&self, name: &str) -> Result<Project> {
        let project = Project::new(name);
        let t = now();
        self.conn.execute(
            "INSERT INTO projects (id, name, created_at, updated_at, data, history)
             VALUES (?1, ?2, ?3, ?3, ?4, ?5)",
            params![
                uuid_text(&project.id.0),
                project.name,
                t,
                serde_json::to_string(&project)?,
                serde_json::to_string(&History::default())?,
            ],
        )?;
        Ok(project)
    }

    /// Most recently updated first.
    pub fn list_projects(&self) -> Result<Vec<ProjectSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.name, max(p.updated_at, coalesce(a.saved_at, 0)) AS t
             FROM projects p LEFT JOIN autosaves a ON a.project_id = p.id
             ORDER BY t DESC, p.name",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (id, name, updated_at) = row?;
            Ok(ProjectSummary {
                id: ProjectId(parse_uuid(&id)?),
                name,
                updated_at,
            })
        })
        .collect()
    }

    pub fn project_exists(&self, id: ProjectId) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM projects WHERE id = ?1",
                [uuid_text(&id.0)],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Load a project for editing and remember it as the last opened one.
    /// An autosave left behind by an unclean exit is recovered (promoted
    /// into the project row) and reported with `recovered`.
    pub fn open_project(&self, id: ProjectId) -> Result<Option<OpenedProject>> {
        let recovered = self.promote_autosave(id)?;
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT data, history FROM projects WHERE id = ?1",
                [uuid_text(&id.0)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((data, history)) = row else {
            return Ok(None);
        };
        let project: Project =
            serde_json::from_str(&data).with_context(|| format!("project {id} is unreadable"))?;
        // A history that no longer parses (older format) is dropped rather
        // than locking the user out of the project.
        let history = serde_json::from_str(&history).unwrap_or_default();
        self.set_last_project(Some(id))?;
        Ok(Some(OpenedProject {
            project,
            history,
            recovered,
        }))
    }

    /// Clean save: write the project and drop any pending autosave.
    pub fn save_project(&self, project: &Project, history: &History) -> Result<()> {
        let id = uuid_text(&project.id.0);
        let data = serde_json::to_string(project)?;
        let history = serde_json::to_string(history)?;
        let t = now();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO projects (id, name, created_at, updated_at, data, history)
             VALUES (?1, ?2, ?3, ?3, ?4, ?5)
             ON CONFLICT (id) DO UPDATE SET name = excluded.name,
                 updated_at = excluded.updated_at, data = excluded.data,
                 history = excluded.history",
            params![id, project.name, t, data, history],
        )?;
        tx.execute("DELETE FROM autosaves WHERE project_id = ?1", [&id])?;
        tx.commit()?;
        Ok(())
    }

    /// Debounced snapshot; see [`crate::Autosaver`]. Creates the project row
    /// if it does not exist yet.
    pub fn write_autosave(&self, project: &Project, history: &History) -> Result<()> {
        let id = uuid_text(&project.id.0);
        let data = serde_json::to_string(project)?;
        let history = serde_json::to_string(history)?;
        let t = now();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO projects (id, name, created_at, updated_at, data, history)
             VALUES (?1, ?2, ?3, ?3, ?4, ?5)",
            params![id, project.name, t, data, history],
        )?;
        tx.execute(
            "INSERT INTO autosaves (project_id, saved_at, data, history)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (project_id) DO UPDATE SET saved_at = excluded.saved_at,
                 data = excluded.data, history = excluded.history",
            params![id, t, data, history],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Move a pending autosave into the project row. Returns whether there
    /// was one. Called on clean close and on open (crash recovery).
    pub fn promote_autosave(&self, id: ProjectId) -> Result<bool> {
        let id = uuid_text(&id.0);
        let tx = self.conn.unchecked_transaction()?;
        let row: Option<(i64, String, String)> = tx
            .query_row(
                "SELECT saved_at, data, history FROM autosaves WHERE project_id = ?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((saved_at, data, history)) = row else {
            return Ok(false);
        };
        let name = serde_json::from_str::<Project>(&data).map(|p| p.name).ok();
        tx.execute(
            "UPDATE projects SET data = ?2, history = ?3,
                 updated_at = max(updated_at, ?4), name = coalesce(?5, name)
             WHERE id = ?1",
            params![id, data, history, saved_at, name],
        )?;
        tx.execute("DELETE FROM autosaves WHERE project_id = ?1", [&id])?;
        tx.commit()?;
        Ok(true)
    }

    /// Projects with an autosave that was never closed cleanly.
    pub fn unrecovered_projects(&self) -> Result<Vec<ProjectId>> {
        let mut stmt = self.conn.prepare("SELECT project_id FROM autosaves")?;
        let ids = stmt.query_map([], |r| r.get::<_, String>(0))?;
        ids.map(|id| Ok(ProjectId(parse_uuid(&id?)?))).collect()
    }

    /// Rename in the list and in the stored project JSON (and its autosave).
    /// An editor that has the project open must also rename its in-memory
    /// copy, or the next save brings the old name back.
    pub fn rename_project(&self, id: ProjectId, name: &str) -> Result<()> {
        let key = uuid_text(&id.0);
        let tx = self.conn.unchecked_transaction()?;
        for table in ["projects", "autosaves"] {
            let key_col = if table == "projects" {
                "id"
            } else {
                "project_id"
            };
            let data: Option<String> = tx
                .query_row(
                    &format!("SELECT data FROM {table} WHERE {key_col} = ?1"),
                    [&key],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(data) = data {
                let mut value: serde_json::Value = serde_json::from_str(&data)?;
                value["name"] = name.into();
                tx.execute(
                    &format!("UPDATE {table} SET data = ?2 WHERE {key_col} = ?1"),
                    params![key, value.to_string()],
                )?;
            }
        }
        tx.execute(
            "UPDATE projects SET name = ?2, updated_at = ?3 WHERE id = ?1",
            params![key, name, now()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Delete the project's rows (project, autosave, overrides). Media files
    /// are never touched; style rules scoped to the project are kept.
    pub fn delete_project(&self, id: ProjectId) -> Result<()> {
        let key = uuid_text(&id.0);
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM autosaves WHERE project_id = ?1", [&key])?;
        tx.execute("DELETE FROM projects WHERE id = ?1", [&key])?;
        tx.commit()?;
        if self.last_project()? == Some(id) {
            self.set_last_project(None)?;
        }
        Ok(())
    }

    pub fn last_project(&self) -> Result<Option<ProjectId>> {
        self.meta(LAST_PROJECT)
    }

    pub fn set_last_project(&self, id: Option<ProjectId>) -> Result<()> {
        match id {
            Some(id) => self.set_meta(LAST_PROJECT, &id),
            None => self.delete_meta(LAST_PROJECT),
        }
    }

    /// Open the last project, or the most recent one, or create "Untitled".
    pub fn open_last_or_new(&self) -> Result<OpenedProject> {
        let candidates = self
            .last_project()?
            .into_iter()
            .chain(self.list_projects()?.into_iter().map(|p| p.id));
        for id in candidates {
            match self.open_project(id) {
                Ok(Some(opened)) => return Ok(opened),
                Ok(None) => {}
                Err(e) => eprintln!("koo-store: cannot open project {id}: {e:#}"),
            }
        }
        let project = self.create_project("Untitled")?;
        self.set_last_project(Some(project.id))?;
        Ok(OpenedProject {
            project,
            history: History::default(),
            recovered: false,
        })
    }
}
