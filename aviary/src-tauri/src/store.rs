//! SQLite storage.
//!
//! Split across **two** databases, because they have opposite guarantees:
//!
//! * `~/.aviary/data.db` — the user's own data: preferences, registered
//!   projects, media, collections, tags and chat sessions. Nothing here can be
//!   recomputed. It is backed up, migrated, and never dropped.
//! * `~/.aviary/cache.db` — everything derivable from disk: library scans, MCP
//!   snapshots, token counts, thumbnail paths. The design spec's rule is that
//!   *deleting the database must cost nothing but a re-index*, which only holds
//!   if durable data lives elsewhere. Deleting this file is always safe.
//!
//! The split is what makes "don't re-scan projects on every launch" safe to do:
//! a stale cache can be thrown away without asking the user anything.
//!
//! Schema changes go through `user_version` migrations in `migrate_*`. Bumping
//! the constant and appending a step is the only supported way to evolve.

use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

pub mod bundles;
pub mod sessions;

pub const DATA_VERSION: i64 = 3;
const CACHE_VERSION: i64 = 1;

pub fn dir() -> Option<PathBuf> {
    crate::providers::home().map(|h| h.join(".aviary"))
}

/// `~/.aviary/media/<hash>` — the content-addressed media store.
pub fn media_dir() -> Option<PathBuf> {
    dir().map(|d| d.join("media"))
}

/// `~/.aviary/cache/thumbs` — regenerable, so it lives under cache.
pub fn thumb_dir() -> Option<PathBuf> {
    dir().map(|d| d.join("cache").join("thumbs"))
}

/// `~/.aviary/logs` — bounded local diagnostics, never uploaded.
pub fn logs_dir() -> Option<PathBuf> {
    dir().map(|d| d.join("logs"))
}

/// App-owned directories contain prompts, metadata and diagnostics. Tightening
/// an existing directory is intentional: older Aviary builds inherited a
/// permissive umask that let other local accounts traverse `~/.aviary`.
pub fn ensure_private_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn ensure_private_file(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn sqlite_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    PathBuf::from(sidecar)
}

fn open(name: &str) -> Result<Connection, String> {
    let dir = dir().ok_or("no home directory")?;
    ensure_private_dir(&dir)?;
    let database = dir.join(name);
    let conn = Connection::open_with_flags(
        &database,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .map_err(|e| e.to_string())?;

    // WAL keeps the UI's reads from blocking on a background index write, which
    // is the whole point of caching scans in the first place.
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| e.to_string())?;
    ensure_private_file(&database)?;
    ensure_private_file(&sqlite_sidecar(&database, "-wal"))?;
    ensure_private_file(&sqlite_sidecar(&database, "-shm"))?;
    Ok(conn)
}

fn version(conn: &Connection) -> Result<i64, String> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())
}

fn set_version(conn: &Connection, v: i64) -> Result<(), String> {
    conn.pragma_update(None, "user_version", v)
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- data.db ---

fn migrate_data(conn: &mut Connection) -> Result<(), String> {
    let current = version(conn)?;
    if current > DATA_VERSION {
        return Err(format!(
            "data.db schema version {current} is newer than this Aviary build supports ({DATA_VERSION})"
        ));
    }

    for target in (current + 1)..=DATA_VERSION {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let schema = match target {
            1 => DATA_V1_SCHEMA,
            2 => DATA_V2_SCHEMA,
            3 => DATA_V3_SCHEMA,
            _ => unreachable!("every data.db migration must be explicit"),
        };
        tx.execute_batch(schema).map_err(|e| e.to_string())?;
        tx.pragma_update(None, "user_version", target)
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    Ok(())
}

const DATA_V1_SCHEMA: &str = r#"
        -- Typed by convention: `value` is JSON so a preference can grow from a
        -- bool into an object without a migration.
        CREATE TABLE IF NOT EXISTS preference (
            key        TEXT PRIMARY KEY,
            value      TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

        -- Registered projects. `path` is the identity: two names for one
        -- directory is a mistake, two directories with one name is not.
        CREATE TABLE IF NOT EXISTS project (
            path         TEXT PRIMARY KEY,
            name         TEXT NOT NULL,
            added_at     INTEGER NOT NULL,
            last_used_at INTEGER
        );

        -- Aviary-only metadata for library entries, keyed by the canonical
        -- (symlink-resolved) path so a favourite survives the file moving
        -- between runner directories.
        CREATE TABLE IF NOT EXISTS entry_meta (
            entry_id   TEXT PRIMARY KEY,
            favorite   INTEGER NOT NULL DEFAULT 0,
            tags       TEXT NOT NULL DEFAULT '[]',
            updated_at INTEGER NOT NULL
        );

        -- Media is content-addressed: the sha256 of the bytes is the identity,
        -- so re-importing the same file twice is a no-op rather than a
        -- duplicate tile, and the stored copy is immutable.
        CREATE TABLE IF NOT EXISTS media (
            hash        TEXT PRIMARY KEY,
            kind        TEXT NOT NULL,
            ext         TEXT NOT NULL,
            bytes       INTEGER NOT NULL,
            width       INTEGER,
            height      INTEGER,
            orientation TEXT,
            dominant    TEXT,
            -- Where it came from. Provenance only; the store never reads it
            -- back, so the tile survives the original being deleted.
            origin      TEXT,
            title       TEXT,
            note        TEXT,
            added_at    INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS media_added_idx ON media(added_at DESC);

        CREATE TABLE IF NOT EXISTS collection (
            id         INTEGER PRIMARY KEY,
            name       TEXT NOT NULL UNIQUE,
            created_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS collection_media (
            collection_id INTEGER NOT NULL
                REFERENCES collection(id) ON DELETE CASCADE,
            media_hash    TEXT NOT NULL
                REFERENCES media(hash) ON DELETE CASCADE,
            added_at      INTEGER NOT NULL,
            PRIMARY KEY (collection_id, media_hash)
        );

        -- `auto = 1` marks a tag derived on import (colour, orientation). Kept
        -- distinguishable so re-deriving never clobbers what the user typed.
        CREATE TABLE IF NOT EXISTS tag (
            media_hash TEXT NOT NULL REFERENCES media(hash) ON DELETE CASCADE,
            tag        TEXT NOT NULL,
            auto       INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (media_hash, tag)
        );
        CREATE INDEX IF NOT EXISTS tag_lookup_idx ON tag(tag);

        -- Backs search_media for the MCP server. Populated by the triggers
        -- below so it cannot drift from `media`.
        CREATE VIRTUAL TABLE IF NOT EXISTS media_fts USING fts5(
            hash UNINDEXED,
            title,
            note,
            origin,
            tags,
            tokenize = 'unicode61'
        );

        CREATE TRIGGER IF NOT EXISTS media_fts_insert AFTER INSERT ON media BEGIN
            INSERT INTO media_fts(hash, title, note, origin, tags)
            VALUES (new.hash, COALESCE(new.title,''), COALESCE(new.note,''),
                    COALESCE(new.origin,''), '');
        END;

        CREATE TRIGGER IF NOT EXISTS media_fts_delete AFTER DELETE ON media BEGIN
            DELETE FROM media_fts WHERE hash = old.hash;
        END;

        CREATE TRIGGER IF NOT EXISTS media_fts_update AFTER UPDATE ON media BEGIN
            UPDATE media_fts
               SET title  = COALESCE(new.title,''),
                   note   = COALESCE(new.note,''),
                   origin = COALESCE(new.origin,'')
             WHERE hash = new.hash;
        END;
"#;

const DATA_V2_SCHEMA: &str = r#"
        -- Aviary owns the local transcript, while `runner_session_id` remains
        -- the runner's identity used for resume. It is nullable because Codex
        -- assigns it only after the process has started.
        CREATE TABLE chat_session (
            id                TEXT PRIMARY KEY,
            runner            TEXT NOT NULL
                CHECK (runner IN ('claude-code', 'codex')),
            runner_session_id TEXT,
            cwd               TEXT NOT NULL,
            title             TEXT NOT NULL,
            created_at        INTEGER NOT NULL,
            updated_at        INTEGER NOT NULL,
            UNIQUE (runner, runner_session_id)
        );
        CREATE INDEX chat_session_recent_idx
            ON chat_session(updated_at DESC, id);

        -- Execution settings live on the turn because model, effort and the
        -- safe permission choice may legitimately change between resumes.
        CREATE TABLE chat_turn (
            id                TEXT PRIMARY KEY,
            session_id        TEXT NOT NULL
                REFERENCES chat_session(id) ON DELETE CASCADE,
            ordinal           INTEGER NOT NULL CHECK (ordinal > 0),
            prompt            TEXT NOT NULL,
            requested_model   TEXT,
            requested_effort  TEXT,
            permission_mode   TEXT NOT NULL,
            status            TEXT NOT NULL
                CHECK (status IN
                    ('queued', 'running', 'completed', 'failed', 'interrupted')),
            failure_kind      TEXT
                CHECK (failure_kind IS NULL OR failure_kind IN
                    ('spawn', 'protocol', 'runner-exit', 'input', 'internal')),
            created_at        INTEGER NOT NULL,
            started_at        INTEGER,
            finished_at       INTEGER,
            duration_ms       INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
            UNIQUE (session_id, ordinal)
        );
        CREATE INDEX chat_turn_session_idx
            ON chat_turn(session_id, ordinal);
        CREATE UNIQUE INDEX chat_turn_one_active_idx
            ON chat_turn(session_id)
            WHERE status IN ('queued', 'running');

        -- Only versioned, normalised events are durable. There is deliberately
        -- no raw-runner event shape: unknown JSON may contain prompts, tool
        -- arguments, environment-adjacent data or file contents.
        CREATE TABLE chat_event (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            turn_id        TEXT NOT NULL
                REFERENCES chat_turn(id) ON DELETE CASCADE,
            sequence       INTEGER NOT NULL CHECK (sequence > 0),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            kind           TEXT NOT NULL,
            payload        TEXT NOT NULL CHECK (json_valid(payload)),
            created_at     INTEGER NOT NULL,
            UNIQUE (turn_id, sequence)
        );
        CREATE INDEX chat_event_turn_idx
            ON chat_event(turn_id, sequence);
"#;

const DATA_V3_SCHEMA: &str = r#"
        -- A bundle records durable intent, not a cached resolution. Targets
        -- deliberately have no foreign keys: removing a file, project, MCP
        -- declaration or collection must leave an honest missing member.
        CREATE TABLE bundle (
            id          TEXT PRIMARY KEY
                CHECK (length(id) = 36),
            name        TEXT NOT NULL
                CHECK (length(trim(name)) BETWEEN 1 AND 120),
            description TEXT NOT NULL DEFAULT ''
                CHECK (length(description) <= 4000),
            runner      TEXT NOT NULL
                CHECK (runner IN ('claude-code', 'codex')),
            model_id    TEXT
                CHECK (model_id IS NULL OR
                       length(trim(model_id)) BETWEEN 1 AND 256),
            memory_mode TEXT NOT NULL DEFAULT 'inherit'
                CHECK (memory_mode IN ('inherit', 'supplement')),
            revision    INTEGER NOT NULL DEFAULT 1
                CHECK (revision >= 1),
            created_at  INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at  INTEGER NOT NULL
                CHECK (updated_at >= created_at)
        );
        CREATE INDEX bundle_recent_idx ON bundle(updated_at DESC, id);

        CREATE TABLE bundle_member (
            id              TEXT PRIMARY KEY
                CHECK (length(id) = 36),
            bundle_id       TEXT NOT NULL
                REFERENCES bundle(id) ON DELETE CASCADE,
            ordinal         INTEGER NOT NULL CHECK (ordinal >= 0),
            kind            TEXT NOT NULL CHECK (kind IN
                ('project', 'skill', 'prompt', 'agent', 'memory', 'mcp',
                 'media-collection')),
            role            TEXT NOT NULL,
            target_text     TEXT,
            target_integer  INTEGER,
            snapshot_label  TEXT NOT NULL
                CHECK (length(trim(snapshot_label)) BETWEEN 1 AND 256),
            created_at      INTEGER NOT NULL CHECK (created_at >= 0),
            UNIQUE (bundle_id, ordinal),
            CHECK (
                (kind = 'media-collection' AND target_text IS NULL AND
                 target_integer > 0) OR
                (kind <> 'media-collection' AND target_text IS NOT NULL AND
                 length(trim(target_text)) BETWEEN 1 AND 16384 AND
                 target_integer IS NULL)
            ),
            CHECK (
                (kind = 'project' AND role = 'working-directory') OR
                (kind = 'skill' AND role IN
                    ('available', 'invoke-first-turn')) OR
                (kind = 'prompt' AND role = 'prefill') OR
                (kind = 'agent' AND role IN ('available', 'primary')) OR
                (kind = 'memory' AND role = 'supplement') OR
                (kind = 'mcp' AND role = 'enabled') OR
                (kind = 'media-collection' AND role = 'retrieval')
            )
        );
        CREATE UNIQUE INDEX bundle_member_target_unique
            ON bundle_member(
                bundle_id,
                kind,
                coalesce(target_text, ''),
                coalesce(target_integer, -1)
            );
        CREATE UNIQUE INDEX bundle_one_project_idx
            ON bundle_member(bundle_id) WHERE kind = 'project';
        CREATE UNIQUE INDEX bundle_one_prompt_idx
            ON bundle_member(bundle_id) WHERE kind = 'prompt';
        CREATE UNIQUE INDEX bundle_one_primary_agent_idx
            ON bundle_member(bundle_id)
            WHERE kind = 'agent' AND role = 'primary';

        -- The source bundle has no foreign key because a chat must retain the
        -- exact, secret-free attachment plan after its bundle is edited or
        -- deleted. The session itself owns the snapshot lifecycle.
        CREATE TABLE chat_session_bundle (
            session_id             TEXT PRIMARY KEY
                REFERENCES chat_session(id) ON DELETE CASCADE,
            source_bundle_id       TEXT NOT NULL
                CHECK (length(source_bundle_id) = 36),
            source_bundle_revision INTEGER NOT NULL
                CHECK (source_bundle_revision >= 1),
            source_bundle_name     TEXT NOT NULL
                CHECK (length(trim(source_bundle_name)) BETWEEN 1 AND 120),
            snapshot_schema_version INTEGER NOT NULL
                CHECK (snapshot_schema_version = 1),
            snapshot_json          TEXT NOT NULL
                CHECK (json_valid(snapshot_json) AND
                       length(CAST(snapshot_json AS BLOB)) <= 262144),
            attached_at            INTEGER NOT NULL CHECK (attached_at >= 0)
        );
"#;

// --------------------------------------------------------------- cache.db ---

fn migrate_cache(conn: &Connection) -> Result<(), String> {
    if version(conn)? >= CACHE_VERSION {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        -- One row per scan kind, holding the JSON snapshot the UI already
        -- consumes. Storing the rendered payload rather than a normalised index
        -- keeps this a pure cache: the shape can change without a migration
        -- because a miss is always recoverable by re-scanning.
        CREATE TABLE IF NOT EXISTS scan (
            kind       TEXT PRIMARY KEY,
            payload    TEXT NOT NULL,
            scanned_at INTEGER NOT NULL,
            took_ms    INTEGER NOT NULL
        );

        -- Tokenising is the expensive part of the Context view. Keyed by
        -- (path, mtime, bytes) so an edited file misses and a moved-but-
        -- unchanged file still hits.
        CREATE TABLE IF NOT EXISTS token_count (
            path   TEXT NOT NULL,
            mtime  INTEGER NOT NULL,
            bytes  INTEGER NOT NULL,
            tokens INTEGER NOT NULL,
            PRIMARY KEY (path, mtime, bytes)
        );

        -- Thumbnails are files under ~/.aviary/cache/thumbs; this maps them.
        -- Regenerable from `media`, which is why it lives here.
        CREATE TABLE IF NOT EXISTS thumb (
            hash TEXT NOT NULL,
            size INTEGER NOT NULL,
            path TEXT NOT NULL,
            PRIMARY KEY (hash, size)
        );
        "#,
    )
    .map_err(|e| e.to_string())?;

    set_version(conn, CACHE_VERSION)
}

// ------------------------------------------------------------ connections ---

/// Process-wide connections.
///
/// SQLite handles concurrency itself, but `Connection` is not `Sync`, so each
/// is behind a mutex. Commands are short-lived and run on the blocking pool, so
/// contention is not a concern in practice.
fn data_conn() -> &'static Mutex<Connection> {
    static DATA: OnceLock<Mutex<Connection>> = OnceLock::new();
    DATA.get_or_init(|| {
        let mut conn = open("data.db").expect("data.db must be openable");
        migrate_data(&mut conn).expect("data.db migration must succeed");
        Mutex::new(conn)
    })
}

fn cache_conn() -> &'static Mutex<Connection> {
    static CACHE: OnceLock<Mutex<Connection>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let conn = open("cache.db").expect("cache.db must be openable");
        migrate_cache(&conn).expect("cache.db migration must succeed");
        Mutex::new(conn)
    })
}

pub fn data() -> MutexGuard<'static, Connection> {
    data_conn().lock().unwrap_or_else(|e| e.into_inner())
}

pub fn cache() -> MutexGuard<'static, Connection> {
    cache_conn().lock().unwrap_or_else(|e| e.into_inner())
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ------------------------------------------------------------ preferences ---

pub fn get_pref(key: &str) -> Option<String> {
    data()
        .query_row("SELECT value FROM preference WHERE key = ?1", [key], |r| {
            r.get::<_, String>(0)
        })
        .ok()
}

pub fn set_pref(key: &str, value: &str) -> Result<(), String> {
    data()
        .execute(
            "INSERT INTO preference(key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = ?3",
            rusqlite::params![key, value, now()],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub fn all_prefs() -> Vec<(String, String)> {
    let conn = data();
    let Ok(mut stmt) = conn.prepare("SELECT key, value FROM preference") else {
        return Vec::new();
    };
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map(|rows| rows.filter_map(Result::ok).collect());
    rows.unwrap_or_default()
}

// -------------------------------------------------------------- projects ---

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Project {
    pub name: String,
    pub path: String,
}

/// Moves `settings.json` projects into the database, once.
///
/// The JSON file is left on disk rather than deleted: if a future version
/// regresses, the user's list is still there. The marker preference is what
/// makes this idempotent, not the presence of rows.
pub fn migrate_settings_json() {
    if get_pref("migrated.settings_json").is_some() {
        return;
    }
    let Some(path) = dir().map(|d| d.join("settings.json")) else {
        return;
    };

    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(projects) = v.get("projects").and_then(|p| p.as_array()) {
                for p in projects {
                    let (Some(name), Some(path)) = (
                        p.get("name").and_then(|n| n.as_str()),
                        p.get("path").and_then(|n| n.as_str()),
                    ) else {
                        continue;
                    };
                    let _ = add_project(name, path);
                }
            }
        }
    }
    let _ = set_pref("migrated.settings_json", "true");
}

pub fn projects() -> Vec<Project> {
    let conn = data();
    let Ok(mut stmt) = conn.prepare("SELECT name, path FROM project ORDER BY added_at") else {
        return Vec::new();
    };
    stmt.query_map([], |r| {
        Ok(Project {
            name: r.get(0)?,
            path: r.get(1)?,
        })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

pub fn add_project(name: &str, path: &str) -> Result<(), String> {
    data()
        .execute(
            "INSERT INTO project(path, name, added_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET name = ?2",
            rusqlite::params![path, name, now()],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub fn remove_project(path: &str) -> Result<(), String> {
    data()
        .execute("DELETE FROM project WHERE path = ?1", [path])
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Projects in the `(name, path)` shape the scanners take.
pub fn project_pairs() -> Vec<(String, PathBuf)> {
    projects()
        .into_iter()
        .map(|p| (p.name, PathBuf::from(p.path)))
        .collect()
}

// ----------------------------------------------------------------- cached ---

/// A cached scan payload and how old it is.
pub struct Cached {
    pub payload: String,
    pub scanned_at: i64,
    pub took_ms: i64,
}

pub fn read_scan(kind: &str) -> Option<Cached> {
    cache()
        .query_row(
            "SELECT payload, scanned_at, took_ms FROM scan WHERE kind = ?1",
            [kind],
            |r| {
                Ok(Cached {
                    payload: r.get(0)?,
                    scanned_at: r.get(1)?,
                    took_ms: r.get(2)?,
                })
            },
        )
        .ok()
}

pub fn write_scan(kind: &str, payload: &str, took_ms: u64) -> Result<(), String> {
    cache()
        .execute(
            "INSERT INTO scan(kind, payload, scanned_at, took_ms) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(kind) DO UPDATE SET payload = ?2, scanned_at = ?3, took_ms = ?4",
            rusqlite::params![kind, payload, now(), took_ms as i64],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Writes several related scan payloads as one visible cache revision.
///
/// A targeted library refresh updates one or more provider fragments and the
/// assembled `library` row. Committing those separately would let a concurrent
/// cold-start read observe half of the new index.
pub fn write_scan_batch(
    rows: &[(String, String, u64)],
    clear_prefix: Option<&str>,
) -> Result<(), String> {
    let mut conn = cache();
    write_scan_batch_on(&mut conn, rows, clear_prefix)
}

fn write_scan_batch_on(
    conn: &mut Connection,
    rows: &[(String, String, u64)],
    clear_prefix: Option<&str>,
) -> Result<(), String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    if let Some(prefix) = clear_prefix {
        tx.execute(
            "DELETE FROM scan WHERE substr(kind, 1, length(?1)) = ?1",
            [prefix],
        )
        .map_err(|e| e.to_string())?;
    }
    let scanned_at = now();
    for (kind, payload, took_ms) in rows {
        tx.execute(
            "INSERT INTO scan(kind, payload, scanned_at, took_ms) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(kind) DO UPDATE SET payload = ?2, scanned_at = ?3, took_ms = ?4",
            rusqlite::params![kind, payload, scanned_at, *took_ms as i64],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}

pub fn delete_scan(kind: &str) -> Result<(), String> {
    cache()
        .execute("DELETE FROM scan WHERE kind = ?1", [kind])
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub fn delete_scan_prefix(prefix: &str) -> Result<(), String> {
    cache()
        .execute(
            "DELETE FROM scan WHERE substr(kind, 1, length(?1)) = ?1",
            [prefix],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Token count for a file, memoised on (path, mtime, size).
pub fn cached_tokens(path: &str) -> usize {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let bytes = meta.len() as i64;

    if let Ok(hit) = cache().query_row(
        "SELECT tokens FROM token_count WHERE path = ?1 AND mtime = ?2 AND bytes = ?3",
        rusqlite::params![path, mtime, bytes],
        |r| r.get::<_, i64>(0),
    ) {
        return hit as usize;
    }

    let tokens = crate::tokens::count_file(path);
    let _ = cache().execute(
        "INSERT OR REPLACE INTO token_count(path, mtime, bytes, tokens) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![path, mtime, bytes, tokens as i64],
    );
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_data(&mut conn).unwrap();
        migrate_data(&mut conn).unwrap();
        assert_eq!(version(&conn).unwrap(), DATA_VERSION);

        let cache = Connection::open_in_memory().unwrap();
        migrate_cache(&cache).unwrap();
        migrate_cache(&cache).unwrap();
        assert_eq!(version(&cache).unwrap(), CACHE_VERSION);
    }

    #[test]
    fn v3_migration_preserves_every_older_durable_surface() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = Connection::open(dir.path().join("data.db")).unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(DATA_V1_SCHEMA).unwrap();
        set_version(&conn, 1).unwrap();
        conn.execute(
            "INSERT INTO preference(key, value, updated_at) VALUES ('theme', '\"dark\"', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project(path, name, added_at) VALUES ('/tmp/real', 'Real', 2)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media(hash, kind, ext, bytes, title, added_at)
             VALUES ('keep', 'image', 'png', 4, 'Keep me', 3)",
            [],
        )
        .unwrap();

        migrate_data(&mut conn).unwrap();

        assert_eq!(version(&conn).unwrap(), 3);
        let preference: String = conn
            .query_row(
                "SELECT value FROM preference WHERE key = 'theme'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let project: String = conn
            .query_row(
                "SELECT name FROM project WHERE path = '/tmp/real'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let media: String = conn
            .query_row("SELECT title FROM media WHERE hash = 'keep'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            (preference.as_str(), project.as_str(), media.as_str()),
            ("\"dark\"", "Real", "Keep me")
        );
        for table in [
            "chat_session",
            "chat_turn",
            "chat_event",
            "bundle",
            "bundle_member",
            "chat_session_bundle",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "{table} must be created by v2");
        }
    }

    #[test]
    fn failed_step_rolls_back_schema_and_user_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(DATA_V1_SCHEMA).unwrap();
        set_version(&conn, 1).unwrap();
        // The conflict occurs after v2 has created `chat_session`, proving the
        // whole step rolls back rather than leaving an unversioned half-schema.
        conn.execute("CREATE TABLE chat_turn(unexpected TEXT)", [])
            .unwrap();

        assert!(migrate_data(&mut conn).is_err());
        assert_eq!(version(&conn).unwrap(), 1);
        let sessions: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_schema
                  WHERE type = 'table' AND name = 'chat_session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sessions, 0);
    }

    #[test]
    fn failed_v3_step_preserves_v2_sessions_and_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(DATA_V1_SCHEMA).unwrap();
        conn.execute_batch(DATA_V2_SCHEMA).unwrap();
        set_version(&conn, 2).unwrap();
        conn.execute(
            "INSERT INTO chat_session(
                id, runner, cwd, title, created_at, updated_at
             ) VALUES ('session', 'codex', '/tmp/project', 'Keep', 1, 1)",
            [],
        )
        .unwrap();
        // The collision occurs after v3 creates `bundle`, proving the complete
        // step and version bump share one transaction.
        conn.execute("CREATE TABLE bundle_member(unexpected TEXT)", [])
            .unwrap();

        assert!(migrate_data(&mut conn).is_err());
        assert_eq!(version(&conn).unwrap(), 2);
        let title: String = conn
            .query_row(
                "SELECT title FROM chat_session WHERE id = 'session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let bundles: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_schema
                  WHERE type = 'table' AND name = 'bundle'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(title, "Keep");
        assert_eq!(bundles, 0);
    }

    #[test]
    fn newer_durable_database_is_refused() {
        let mut conn = Connection::open_in_memory().unwrap();
        set_version(&conn, DATA_VERSION + 1).unwrap();
        let error = migrate_data(&mut conn).unwrap_err();
        assert!(error.contains("newer"));
        assert_eq!(version(&conn).unwrap(), DATA_VERSION + 1);
    }

    #[test]
    fn attachment_snapshot_limit_is_measured_in_bytes() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        migrate_data(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO chat_session(
                id, runner, cwd, title, created_at, updated_at
             ) VALUES ('session', 'codex', '/tmp', 'Bounded', 1, 1)",
            [],
        )
        .unwrap();
        // Fewer than 262,144 Unicode scalar values, but more than 262,144
        // UTF-8 bytes. The persistence and IPC limits are byte limits.
        let oversized = serde_json::to_string(&"é".repeat(140_000)).unwrap();
        assert!(oversized.chars().count() < 262_144);
        assert!(oversized.len() > 262_144);
        let result = conn.execute(
            "INSERT INTO chat_session_bundle(
                session_id, source_bundle_id, source_bundle_revision,
                source_bundle_name, snapshot_schema_version, snapshot_json,
                attached_at
             ) VALUES (
                'session', '00000000-0000-0000-0000-000000000000', 1,
                'Bounded', 1, ?1, 1
             )",
            [&oversized],
        );
        assert!(result.is_err());
    }

    #[test]
    fn scan_batch_replaces_scopes_and_aggregate_together() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_cache(&conn).unwrap();
        conn.execute(
            "INSERT INTO scan(kind, payload, scanned_at, took_ms)
             VALUES ('library:scope:stale', 'old-part', 0, 0),
                    ('library', 'old-index', 0, 0)",
            [],
        )
        .unwrap();

        let rows = vec![
            (
                "library:scope:codex:user".to_string(),
                "new-part".to_string(),
                3,
            ),
            ("library".to_string(), "new-index".to_string(), 4),
        ];
        write_scan_batch_on(&mut conn, &rows, Some("library:scope:")).unwrap();

        let aggregate: String = conn
            .query_row("SELECT payload FROM scan WHERE kind = 'library'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let stale: i64 = conn
            .query_row(
                "SELECT count(*) FROM scan WHERE kind = 'library:scope:stale'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(aggregate, "new-index");
        assert_eq!(stale, 0);
    }

    #[test]
    fn media_fts_tracks_the_media_table() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_data(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO media(hash, kind, ext, bytes, title, added_at)
             VALUES ('abc', 'image', 'png', 10, 'grainy teal gradient', 0)",
            [],
        )
        .unwrap();

        let hits: i64 = conn
            .query_row(
                "SELECT count(*) FROM media_fts WHERE media_fts MATCH 'teal'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hits, 1, "insert trigger must populate the index");

        conn.execute("DELETE FROM media WHERE hash = 'abc'", [])
            .unwrap();
        let after: i64 = conn
            .query_row("SELECT count(*) FROM media_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, 0, "delete trigger must clean the index");
    }

    #[test]
    fn cascades_clean_up_tags_and_membership() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        migrate_data(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO media(hash, kind, ext, bytes, added_at)
             VALUES ('h1', 'image', 'png', 1, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO collection(id, name, created_at) VALUES (1, 'Gradients', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO collection_media(collection_id, media_hash, added_at)
             VALUES (1, 'h1', 0)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO tag(media_hash, tag) VALUES ('h1', 'teal')", [])
            .unwrap();

        conn.execute("DELETE FROM media WHERE hash = 'h1'", [])
            .unwrap();

        let tags: i64 = conn
            .query_row("SELECT count(*) FROM tag", [], |r| r.get(0))
            .unwrap();
        let members: i64 = conn
            .query_row("SELECT count(*) FROM collection_media", [], |r| r.get(0))
            .unwrap();
        assert_eq!((tags, members), (0, 0));
    }
}

#[cfg(test)]
mod cache_timing {
    /// Substantiates the reason the cache exists: serving the stored snapshot
    /// must be dramatically cheaper than re-walking the filesystem.
    #[test]
    fn cached_read_beats_a_fresh_scan() {
        let t0 = std::time::Instant::now();
        let fresh = crate::library::scan();
        let fresh_ms = t0.elapsed().as_micros();

        let json = serde_json::to_string(&fresh).unwrap();
        super::write_scan("library", &json, fresh.scanned_ms).unwrap();

        let t1 = std::time::Instant::now();
        let hit = super::read_scan("library").expect("just wrote it");
        let parsed: crate::library::LibrarySnapshot = serde_json::from_str(&hit.payload).unwrap();
        let cached_us = t1.elapsed().as_micros();

        eprintln!(
            "fresh scan: {}us ({} entries) | cached: {}us | {:.1}x faster",
            fresh_ms,
            parsed.entries.len(),
            cached_us,
            fresh_ms as f64 / cached_us.max(1) as f64
        );
        assert_eq!(parsed.entries.len(), fresh.entries.len());
        assert!(cached_us < fresh_ms, "cache must be faster than scanning");
    }
}
