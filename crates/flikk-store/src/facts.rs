//! Source facts keyed by fingerprint (PRD F1) and their suspension when a
//! file is replaced (F2), lifted when that content is seen again.

use crate::{Store, now, parse_uuid, sql_limit, tag, untag, uuid_text};
use anyhow::Result;
use flikk_core::ids::FactId;
use flikk_core::memory::{FactKind, FactStatus, SourceFact};
use flikk_core::model::Fingerprint;
use flikk_core::time::{Time, TimeRange};
use rusqlite::types::Value;
use rusqlite::{OptionalExtension, Row, params, params_from_iter};
use std::path::{Path, PathBuf};

/// Which facts to read. Results are ordered by source time, then creation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FactFilter {
    pub fingerprint: Option<Fingerprint>,
    pub kind: Option<FactKind>,
    pub status: Option<FactStatus>,
    /// Facts whose range overlaps this one; facts without a range always match.
    pub overlapping: Option<TimeRange>,
    pub offset: usize,
    pub limit: Option<usize>,
}

const FACT_COLUMNS: &str = "id, fingerprint, kind, text, range_start, range_end, status";

impl Store {
    /// Insert or replace a fact.
    pub fn save_fact(&self, fact: &SourceFact) -> Result<()> {
        self.conn.execute(
            "INSERT INTO source_facts
                 (id, fingerprint, kind, text, range_start, range_end, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (id) DO UPDATE SET fingerprint = excluded.fingerprint,
                 kind = excluded.kind, text = excluded.text,
                 range_start = excluded.range_start, range_end = excluded.range_end,
                 status = excluded.status",
            params![
                uuid_text(&fact.id.0),
                fact.fingerprint.0,
                tag(&fact.kind)?,
                fact.text,
                fact.range.map(|r| r.start.0),
                fact.range.map(|r| r.end.0),
                tag(&fact.status)?,
                now(),
            ],
        )?;
        Ok(())
    }

    pub fn fact(&self, id: FactId) -> Result<Option<SourceFact>> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {FACT_COLUMNS} FROM source_facts WHERE id = ?1"),
                [uuid_text(&id.0)],
                raw_fact,
            )
            .optional()?;
        raw.map(fact_from_raw).transpose()
    }

    pub fn facts(&self, filter: &FactFilter) -> Result<Vec<SourceFact>> {
        let (where_sql, mut args) = fact_where(filter)?;
        args.push(Value::Integer(sql_limit(filter.limit)));
        args.push(Value::Integer(filter.offset as i64));
        let sql = format!(
            "SELECT {FACT_COLUMNS} FROM source_facts WHERE {where_sql}
             ORDER BY fingerprint, range_start IS NULL, range_start, created_at, rowid
             LIMIT ? OFFSET ?"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), raw_fact)?;
        rows.map(|r| fact_from_raw(r?)).collect()
    }

    /// Number of facts matching `filter` (offset and limit ignored).
    pub fn count_facts(&self, filter: &FactFilter) -> Result<usize> {
        let (where_sql, args) = fact_where(filter)?;
        let n: i64 = self.conn.query_row(
            &format!("SELECT count(*) FROM source_facts WHERE {where_sql}"),
            params_from_iter(args),
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    pub fn set_fact_status(&self, id: FactId, status: FactStatus) -> Result<()> {
        self.conn.execute(
            "UPDATE source_facts SET status = ?2 WHERE id = ?1",
            params![uuid_text(&id.0), tag(&status)?],
        )?;
        Ok(())
    }

    pub fn delete_fact(&self, id: FactId) -> Result<()> {
        self.conn
            .execute("DELETE FROM source_facts WHERE id = ?1", [uuid_text(&id.0)])?;
        Ok(())
    }

    // ------------------------------------------------------ fingerprints (F2)

    /// Remember that `path` had `fingerprint` (on import or probe).
    pub fn record_fingerprint(&self, path: &Path, fingerprint: &Fingerprint) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        record_seen(&tx, &path_key(path), fingerprint)?;
        tx.commit()?;
        Ok(())
    }

    /// Fingerprints seen at `path`, newest first.
    pub fn fingerprint_history(&self, path: &Path) -> Result<Vec<(Fingerprint, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT fingerprint, seen_at FROM media_fingerprints
             WHERE path = ?1 ORDER BY seen_at DESC, rowid DESC",
        )?;
        let rows = stmt.query_map([path_key(path)], |r| {
            Ok((Fingerprint(r.get(0)?), r.get(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Paths a fingerprint was seen at, newest first.
    pub fn paths_for_fingerprint(&self, fingerprint: &Fingerprint) -> Result<Vec<PathBuf>> {
        let mut stmt = self.conn.prepare(
            "SELECT path FROM media_fingerprints WHERE fingerprint = ?1
             ORDER BY seen_at DESC, rowid DESC",
        )?;
        let rows = stmt.query_map([&fingerprint.0], |r| r.get::<_, String>(0))?;
        Ok(rows
            .map(|r| r.map(PathBuf::from))
            .collect::<rusqlite::Result<_>>()?)
    }

    /// PRD F2: the file at `path` now hashes to `new_fingerprint`. Facts
    /// bound to the fingerprints the path had before are suspended, unless
    /// that content is still the current file at another path. Returns the
    /// suspended facts' ids.
    pub fn on_media_reimported(
        &self,
        path: &Path,
        new_fingerprint: &Fingerprint,
    ) -> Result<Vec<FactId>> {
        let key = path_key(path);
        let tx = self.conn.unchecked_transaction()?;
        let mut stmt = tx.prepare(
            "SELECT f.id FROM source_facts f
             WHERE f.status != 'suspended'
               AND f.fingerprint != ?2
               AND f.fingerprint IN (SELECT fingerprint FROM media_fingerprints WHERE path = ?1)
               AND NOT EXISTS (
                   -- the old content is still the latest fingerprint somewhere else
                   SELECT 1 FROM media_fingerprints m
                   WHERE m.fingerprint = f.fingerprint AND m.path != ?1
                     AND m.seen_at = (SELECT max(seen_at) FROM media_fingerprints
                                      WHERE path = m.path))",
        )?;
        let ids: Vec<String> = stmt
            .query_map(params![key, new_fingerprint.0], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        for id in &ids {
            tx.execute(
                "UPDATE source_facts SET status = 'suspended' WHERE id = ?1",
                [id],
            )?;
        }
        record_seen(&tx, &key, new_fingerprint)?;
        tx.commit()?;
        ids.iter().map(|id| Ok(FactId(parse_uuid(id)?))).collect()
    }
}

/// Records `fingerprint` as the latest one at `key`. `seen_at` stays unique
/// per path, so two fingerprints seen in the same second never tie for
/// latest. Facts on that content that were suspended apply again; they come
/// back as proposed, since what they were before is not kept.
fn record_seen(conn: &rusqlite::Connection, key: &str, fingerprint: &Fingerprint) -> Result<()> {
    conn.execute(
        "INSERT INTO media_fingerprints (path, fingerprint, seen_at)
         VALUES (?1, ?2, max(?3, coalesce((SELECT max(seen_at) + 1 FROM media_fingerprints
                                           WHERE path = ?1 AND fingerprint != ?2), ?3)))
         ON CONFLICT (path, fingerprint) DO UPDATE SET seen_at = excluded.seen_at",
        params![key, fingerprint.0, now()],
    )?;
    conn.execute(
        "UPDATE source_facts SET status = 'proposed'
         WHERE fingerprint = ?1 AND status = 'suspended'",
        [&fingerprint.0],
    )?;
    Ok(())
}

fn path_key(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn fact_where(filter: &FactFilter) -> Result<(String, Vec<Value>)> {
    let mut sql = String::from("1");
    let mut args = Vec::new();
    if let Some(fp) = &filter.fingerprint {
        sql.push_str(" AND fingerprint = ?");
        args.push(Value::Text(fp.0.clone()));
    }
    if let Some(kind) = &filter.kind {
        sql.push_str(" AND kind = ?");
        args.push(Value::Text(tag(kind)?));
    }
    if let Some(status) = &filter.status {
        sql.push_str(" AND status = ?");
        args.push(Value::Text(tag(status)?));
    }
    if let Some(range) = &filter.overlapping {
        sql.push_str(" AND (range_start IS NULL OR (range_start < ? AND range_end > ?))");
        args.push(Value::Integer(range.end.0));
        args.push(Value::Integer(range.start.0));
    }
    Ok((sql, args))
}

type RawFact = (
    String,
    String,
    String,
    String,
    Option<i64>,
    Option<i64>,
    String,
);

fn raw_fact(r: &Row) -> rusqlite::Result<RawFact> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
    ))
}

fn fact_from_raw(raw: RawFact) -> Result<SourceFact> {
    let (id, fingerprint, kind, text, start, end, status) = raw;
    Ok(SourceFact {
        id: FactId(parse_uuid(&id)?),
        fingerprint: Fingerprint(fingerprint),
        kind: untag(&kind)?,
        text,
        range: start
            .zip(end)
            .map(|(s, e)| TimeRange::new(Time(s), Time(e))),
        status: untag(&status)?,
    })
}
