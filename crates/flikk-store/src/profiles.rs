//! Style profiles and their rules (PRD S1, S6).

use crate::{Store, now, parse_uuid, sql_limit, tag, untag, uuid_text};
use anyhow::{Result, bail};
use flikk_core::ids::{ProjectId, RuleId};
use flikk_core::memory::{Presets, RuleScope, RuleStatus, StyleProfile, StyleRule};
use rusqlite::types::Value;
use rusqlite::{OptionalExtension, Row, params, params_from_iter};

#[derive(Clone, Debug, PartialEq)]
pub struct ProfileSummary {
    pub id: RuleId,
    pub name: String,
    pub rules: usize,
    pub proposed: usize,
    pub updated_at: i64,
}

/// Which rules to read. Results are newest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuleFilter {
    pub scope: Option<RuleScope>,
    pub status: Option<RuleStatus>,
    /// Case-insensitive substring of the rule text.
    pub text: Option<String>,
    pub offset: usize,
    pub limit: Option<usize>,
}

const RULE_COLUMNS: &str =
    "id, text, scope, scope_project, status, supersedes, created_at, evidence";

impl Store {
    // ------------------------------------------------------------ profiles

    pub fn create_profile(&self, name: &str) -> Result<StyleProfile> {
        let profile = StyleProfile {
            id: RuleId::new(),
            name: name.to_owned(),
            rules: Vec::new(),
            presets: Presets::default(),
        };
        let t = now();
        self.conn.execute(
            "INSERT INTO style_profiles (id, name, presets, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![
                uuid_text(&profile.id.0),
                profile.name,
                serde_json::to_string(&profile.presets)?,
                t
            ],
        )?;
        Ok(profile)
    }

    pub fn list_profiles(&self) -> Result<Vec<ProfileSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.name, p.updated_at,
                    count(r.id), coalesce(sum(r.status = 'proposed'), 0)
             FROM style_profiles p LEFT JOIN style_rules r ON r.profile_id = p.id
             GROUP BY p.id ORDER BY p.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (id, name, updated_at, rules, proposed) = row?;
            Ok(ProfileSummary {
                id: RuleId(parse_uuid(&id)?),
                name,
                rules: rules as usize,
                proposed: proposed as usize,
                updated_at,
            })
        })
        .collect()
    }

    /// Name and presets, without rules; read rules with [`Store::rules`].
    pub fn profile_header(&self, id: RuleId) -> Result<Option<StyleProfile>> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT name, presets FROM style_profiles WHERE id = ?1",
                [uuid_text(&id.0)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(name, presets)| {
            Ok(StyleProfile {
                id,
                name,
                rules: Vec::new(),
                presets: serde_json::from_str(&presets).unwrap_or_default(),
            })
        })
        .transpose()
    }

    /// The whole profile with every rule (for the settings UI).
    pub fn profile(&self, id: RuleId) -> Result<Option<StyleProfile>> {
        let Some(mut profile) = self.profile_header(id)? else {
            return Ok(None);
        };
        profile.rules = self.rules(id, &RuleFilter::default())?;
        Ok(Some(profile))
    }

    pub fn rename_profile(&self, id: RuleId, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE style_profiles SET name = ?2, updated_at = ?3 WHERE id = ?1",
            params![uuid_text(&id.0), name, now()],
        )?;
        Ok(())
    }

    pub fn set_presets(&self, id: RuleId, presets: &Presets) -> Result<()> {
        self.conn.execute(
            "UPDATE style_profiles SET presets = ?2, updated_at = ?3 WHERE id = ?1",
            params![uuid_text(&id.0), serde_json::to_string(presets)?, now()],
        )?;
        Ok(())
    }

    /// Deletes the profile and its rules.
    pub fn delete_profile(&self, id: RuleId) -> Result<()> {
        self.conn.execute(
            "DELETE FROM style_profiles WHERE id = ?1",
            [uuid_text(&id.0)],
        )?;
        Ok(())
    }

    // --------------------------------------------------------------- rules

    pub fn rules(&self, profile: RuleId, filter: &RuleFilter) -> Result<Vec<StyleRule>> {
        let (where_sql, mut args) = rule_where(profile, filter)?;
        args.push(Value::Integer(sql_limit(filter.limit)));
        args.push(Value::Integer(filter.offset as i64));
        let sql = format!(
            "SELECT {RULE_COLUMNS} FROM style_rules WHERE {where_sql}
             ORDER BY created_at DESC, rowid DESC LIMIT ? OFFSET ?"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), raw_rule)?;
        rows.map(|r| rule_from_raw(r?)).collect()
    }

    /// Number of rules matching `filter` (offset and limit ignored).
    pub fn count_rules(&self, profile: RuleId, filter: &RuleFilter) -> Result<usize> {
        let (where_sql, args) = rule_where(profile, filter)?;
        let n: i64 = self.conn.query_row(
            &format!("SELECT count(*) FROM style_rules WHERE {where_sql}"),
            params_from_iter(args),
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    pub fn rule(&self, id: RuleId) -> Result<Option<StyleRule>> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {RULE_COLUMNS} FROM style_rules WHERE id = ?1"),
                [uuid_text(&id.0)],
                raw_rule,
            )
            .optional()?;
        raw.map(rule_from_raw).transpose()
    }

    /// The profile a rule belongs to.
    pub fn rule_profile(&self, id: RuleId) -> Result<Option<RuleId>> {
        let p: Option<String> = self
            .conn
            .query_row(
                "SELECT profile_id FROM style_rules WHERE id = ?1",
                [uuid_text(&id.0)],
                |r| r.get(0),
            )
            .optional()?;
        p.map(|p| Ok(RuleId(parse_uuid(&p)?))).transpose()
    }

    /// Insert or replace a rule in `profile`.
    pub fn save_rule(&self, profile: RuleId, rule: &StyleRule) -> Result<()> {
        let (scope, scope_project) = scope_columns(&rule.scope);
        let changed = self.conn.execute(
            "INSERT INTO style_rules
                 (id, profile_id, text, scope, scope_project, status, supersedes,
                  created_at, evidence)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
             WHERE EXISTS (SELECT 1 FROM style_profiles WHERE id = ?2)
             ON CONFLICT (id) DO UPDATE SET profile_id = excluded.profile_id,
                 text = excluded.text, scope = excluded.scope,
                 scope_project = excluded.scope_project, status = excluded.status,
                 supersedes = excluded.supersedes, evidence = excluded.evidence",
            params![
                uuid_text(&rule.id.0),
                uuid_text(&profile.0),
                rule.text,
                scope,
                scope_project,
                tag(&rule.status)?,
                rule.supersedes.map(|s| uuid_text(&s.0)),
                rule.created_at,
                serde_json::to_string(&rule.evidence)?,
            ],
        )?;
        if changed == 0 {
            bail!("unknown style profile {profile}");
        }
        self.touch_profile(profile)
    }

    /// Insert several rules in one transaction.
    pub fn save_rules(&self, profile: RuleId, rules: &[StyleRule]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for rule in rules {
            self.save_rule(profile, rule)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Approve, reject or re-propose. Approving a rule that supersedes another
    /// rejects the superseded one.
    pub fn set_rule_status(&self, id: RuleId, status: RuleStatus) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let key = uuid_text(&id.0);
        tx.execute(
            "UPDATE style_rules SET status = ?2 WHERE id = ?1",
            params![key, tag(&status)?],
        )?;
        if status == RuleStatus::Approved {
            tx.execute(
                "UPDATE style_rules SET status = 'rejected'
                 WHERE id = (SELECT supersedes FROM style_rules WHERE id = ?1)",
                [&key],
            )?;
        }
        tx.execute(
            "UPDATE style_profiles SET updated_at = ?2
             WHERE id = (SELECT profile_id FROM style_rules WHERE id = ?1)",
            params![key, now()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete_rule(&self, id: RuleId) -> Result<()> {
        self.conn
            .execute("DELETE FROM style_rules WHERE id = ?1", [uuid_text(&id.0)])?;
        Ok(())
    }

    /// Drop `NextVideo` rules once they have been used (PRD S1 scope).
    pub fn clear_next_video_rules(&self, profile: RuleId) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM style_rules WHERE profile_id = ?1 AND scope = 'next_video'",
            [uuid_text(&profile.0)],
        )?)
    }

    fn touch_profile(&self, profile: RuleId) -> Result<()> {
        self.conn.execute(
            "UPDATE style_profiles SET updated_at = ?2 WHERE id = ?1",
            params![uuid_text(&profile.0), now()],
        )?;
        Ok(())
    }
}

fn scope_columns(scope: &RuleScope) -> (&'static str, Option<String>) {
    match scope {
        RuleScope::Global => ("global", None),
        RuleScope::NextVideo => ("next_video", None),
        RuleScope::Project { project } => ("project", Some(uuid_text(&project.0))),
    }
}

fn rule_where(profile: RuleId, filter: &RuleFilter) -> Result<(String, Vec<Value>)> {
    let mut sql = String::from("profile_id = ?");
    let mut args = vec![Value::Text(uuid_text(&profile.0))];
    if let Some(scope) = &filter.scope {
        let (kind, project) = scope_columns(scope);
        sql.push_str(" AND scope = ?");
        args.push(Value::Text(kind.into()));
        if let Some(project) = project {
            sql.push_str(" AND scope_project = ?");
            args.push(Value::Text(project));
        }
    }
    if let Some(status) = &filter.status {
        sql.push_str(" AND status = ?");
        args.push(Value::Text(tag(status)?));
    }
    if let Some(text) = filter.text.as_deref().filter(|t| !t.is_empty()) {
        sql.push_str(" AND instr(lower(text), lower(?)) > 0");
        args.push(Value::Text(text.into()));
    }
    Ok((sql, args))
}

type RawRule = (
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    i64,
    String,
);

fn raw_rule(r: &Row) -> rusqlite::Result<RawRule> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
    ))
}

fn rule_from_raw(raw: RawRule) -> Result<StyleRule> {
    let (id, text, scope, scope_project, status, supersedes, created_at, evidence) = raw;
    let scope = match (scope.as_str(), scope_project) {
        ("project", Some(p)) => RuleScope::Project {
            project: ProjectId(parse_uuid(&p)?),
        },
        ("next_video", _) => RuleScope::NextVideo,
        _ => RuleScope::Global,
    };
    Ok(StyleRule {
        id: RuleId(parse_uuid(&id)?),
        text,
        scope,
        status: untag(&status)?,
        supersedes: supersedes.map(|s| parse_uuid(&s).map(RuleId)).transpose()?,
        created_at,
        evidence: serde_json::from_str(&evidence).unwrap_or_default(),
    })
}
