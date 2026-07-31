//! SQLite storage.
//!
//! Split across **two** databases, because they have opposite guarantees:
//!
//! * `~/.aviary/data.db` — the user's own data: preferences, registered
//!   projects, media, collections, tags. Nothing here can be recomputed. It is
//!   backed up, migrated, and never dropped.
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
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

const DATA_VERSION: i64 = 1;
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

fn open(name: &str) -> Result<Connection, String> {
    let dir = dir().ok_or("no home directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let conn = Connection::open_with_flags(
        dir.join(name),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .map_err(|e| e.to_string())?;

    // WAL keeps the UI's reads from blocking on a background index write, which
    // is the whole point of caching scans in the first place.
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

fn version(conn: &Connection) -> i64 {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0)
}

fn set_version(conn: &Connection, v: i64) -> Result<(), String> {
    conn.pragma_update(None, "user_version", v)
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- data.db ---

fn migrate_data(conn: &Connection) -> Result<(), String> {
    if version(conn) >= DATA_VERSION {
        return Ok(());
    }

    conn.execute_batch(
        r#"
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
        "#,
    )
    .map_err(|e| e.to_string())?;

    set_version(conn, DATA_VERSION)
}

// --------------------------------------------------------------- cache.db ---

fn migrate_cache(conn: &Connection) -> Result<(), String> {
    if version(conn) >= CACHE_VERSION {
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
        let conn = open("data.db").expect("data.db must be openable");
        migrate_data(&conn).expect("data.db migration must succeed");
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
        .query_row(
            "SELECT value FROM preference WHERE key = ?1",
            [key],
            |r| r.get::<_, String>(0),
        )
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
        let conn = Connection::open_in_memory().unwrap();
        migrate_data(&conn).unwrap();
        migrate_data(&conn).unwrap();
        assert_eq!(version(&conn), DATA_VERSION);

        let cache = Connection::open_in_memory().unwrap();
        migrate_cache(&cache).unwrap();
        migrate_cache(&cache).unwrap();
        assert_eq!(version(&cache), CACHE_VERSION);
    }

    #[test]
    fn media_fts_tracks_the_media_table() {
        let conn = Connection::open_in_memory().unwrap();
        migrate_data(&conn).unwrap();

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

        conn.execute("DELETE FROM media WHERE hash = 'abc'", []).unwrap();
        let after: i64 = conn
            .query_row("SELECT count(*) FROM media_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, 0, "delete trigger must clean the index");
    }

    #[test]
    fn cascades_clean_up_tags_and_membership() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        migrate_data(&conn).unwrap();

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

        conn.execute("DELETE FROM media WHERE hash = 'h1'", []).unwrap();

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
        let parsed: crate::library::LibrarySnapshot =
            serde_json::from_str(&hit.payload).unwrap();
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
