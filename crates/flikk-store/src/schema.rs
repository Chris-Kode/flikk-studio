//! Schema and migrations, versioned with `PRAGMA user_version`.
//!
//! Append to `MIGRATIONS`; never edit a step that has shipped. Step `i`
//! moves the database from version `i` to `i + 1`.

use anyhow::{Result, bail};
use rusqlite::{Connection, TransactionBehavior};

const MIGRATIONS: &[&str] = &[
    // 1: initial schema.
    r#"
    CREATE TABLE meta (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );

    CREATE TABLE projects (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        data       TEXT NOT NULL,
        history    TEXT NOT NULL,
        settings   TEXT
    );

    -- Latest debounced snapshot. A row only survives an unclean exit:
    -- a clean close promotes it into `projects` and deletes it.
    CREATE TABLE autosaves (
        project_id TEXT PRIMARY KEY REFERENCES projects (id) ON DELETE CASCADE,
        saved_at   INTEGER NOT NULL,
        data       TEXT NOT NULL,
        history    TEXT NOT NULL
    );

    CREATE TABLE style_profiles (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        presets    TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );

    CREATE TABLE style_rules (
        id            TEXT PRIMARY KEY,
        profile_id    TEXT NOT NULL REFERENCES style_profiles (id) ON DELETE CASCADE,
        text          TEXT NOT NULL,
        scope         TEXT NOT NULL,
        scope_project TEXT,
        status        TEXT NOT NULL,
        supersedes    TEXT,
        created_at    INTEGER NOT NULL,
        evidence      TEXT NOT NULL
    );
    CREATE INDEX style_rules_profile ON style_rules (profile_id, status, scope, created_at);

    CREATE TABLE source_facts (
        id          TEXT PRIMARY KEY,
        fingerprint TEXT NOT NULL,
        kind        TEXT NOT NULL,
        text        TEXT NOT NULL,
        range_start INTEGER,
        range_end   INTEGER,
        status      TEXT NOT NULL,
        created_at  INTEGER NOT NULL
    );
    CREATE INDEX source_facts_fingerprint ON source_facts (fingerprint, status);

    -- Every fingerprint seen at a path, for F2.
    CREATE TABLE media_fingerprints (
        path        TEXT NOT NULL,
        fingerprint TEXT NOT NULL,
        seen_at     INTEGER NOT NULL,
        PRIMARY KEY (path, fingerprint)
    );
    CREATE INDEX media_fingerprints_fp ON media_fingerprints (fingerprint);

    CREATE TABLE recipes (
        id          TEXT PRIMARY KEY,
        name        TEXT NOT NULL UNIQUE,
        description TEXT NOT NULL,
        params      TEXT NOT NULL,
        prompt      TEXT NOT NULL,
        builtin     INTEGER NOT NULL,
        updated_at  INTEGER NOT NULL
    );
    "#,
    // 2: projects moved to `.flikk` files the user saves; recent files replace
    // the last opened project.
    r#"
    DROP TABLE autosaves;
    DROP TABLE projects;
    DELETE FROM meta WHERE key = 'last_project';
    "#,
    // 3: recipes became skills, Markdown files under `paths::skills()`.
    r#"
    DROP TABLE recipes;
    "#,
];

pub fn migrate(conn: &mut Connection) -> Result<()> {
    // Immediate: two connections opening at once must not both migrate.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version = tx.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as usize;
    if version > MIGRATIONS.len() {
        bail!(
            "database schema {version} is newer than this build ({}); update Flikk",
            MIGRATIONS.len()
        );
    }
    if version == MIGRATIONS.len() {
        return Ok(());
    }
    for sql in &MIGRATIONS[version..] {
        tx.execute_batch(sql)?;
    }
    tx.pragma_update(None, "user_version", MIGRATIONS.len() as i64)?;
    tx.commit()?;
    Ok(())
}
