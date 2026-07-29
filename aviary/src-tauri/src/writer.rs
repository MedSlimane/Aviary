//! Safe writes to live agent configuration.
//!
//! This is the highest-risk code in the app: it edits files that Claude Code
//! and Codex read on their next turn. Three guarantees, in order of how much
//! they matter:
//!
//! 1. **Snapshot before every write.** The prior content is copied to
//!    `~/.aviary/history` *before* the new content lands, so any edit is
//!    reversible even if the app crashes mid-write.
//! 2. **Conflict detection.** A write is refused if the file's current hash
//!    does not match what the editor loaded. Something else changed it, and
//!    silently clobbering that is the worst outcome available.
//! 3. **Atomic replace.** Write to a temp file in the same directory, fsync,
//!    then rename. A crash leaves either the old file or the new one, never a
//!    truncated one.

use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Cheap content hash. Not cryptographic — it only needs to detect that a
/// file changed underneath us.
pub fn hash(content: &str) -> String {
    // FNV-1a, 64-bit.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in content.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{h:016x}")
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case", tag = "status")]
pub enum WriteOutcome {
    /// Written. Carries the new hash so the editor can keep tracking.
    Written { hash: String, snapshot: String },
    /// Refused: the file changed since it was loaded.
    Conflict { disk_hash: String, disk_content: String },
}

fn history_dir() -> Result<PathBuf, String> {
    let dir = dirs::home_dir()
        .ok_or("no home directory")?
        .join(".aviary")
        .join("history");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Copies the current content into `~/.aviary/history` before it is replaced.
///
/// Named by content hash and timestamp so repeated saves never collide and a
/// snapshot can be matched back to the version it captured.
fn snapshot(path: &Path, content: &str) -> Result<PathBuf, String> {
    let dir = history_dir()?;
    let stem = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("entry")
        .replace('/', "_");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let target = dir.join(format!("{ts}-{}-{stem}", &hash(content)[..8]));
    fs::write(&target, content).map_err(|e| e.to_string())?;
    Ok(target)
}

/// Replaces a file atomically: temp file in the same directory, fsync, rename.
///
/// Same directory matters — rename is only atomic within a filesystem, and a
/// temp dir may well be on another one.
fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let parent = path.parent().ok_or("path has no parent directory")?;

    // The temp name must be unique per call, not per process: two saves landing
    // in the same directory at the same time would otherwise race on one temp
    // file and one of them would write the other's bytes.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = parent.join(format!(
        ".aviary-tmp-{}-{nonce}-{nanos}",
        std::process::id()
    ));

    {
        let mut f = fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }

    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e.to_string()
    })
}

/// Writes an entry, refusing if it changed on disk since `expected_hash`.
///
/// Pass `force` only after the user has seen the conflict and chosen to
/// overwrite.
pub fn write_entry(
    path: &str,
    content: &str,
    expected_hash: &str,
    force: bool,
) -> Result<WriteOutcome, String> {
    let p = Path::new(path);
    let current = fs::read_to_string(p).map_err(|e| e.to_string())?;
    let current_hash = hash(&current);

    if !force && current_hash != expected_hash {
        return Ok(WriteOutcome::Conflict {
            disk_hash: current_hash,
            disk_content: current,
        });
    }

    // Snapshot first — if this fails, nothing is written.
    let snap = snapshot(p, &current)?;
    atomic_write(p, content)?;

    Ok(WriteOutcome::Written {
        hash: hash(content),
        snapshot: snap.to_string_lossy().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(name: &str, body: &str) -> PathBuf {
        let p = std::env::temp_dir().join(name);
        fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn writes_and_snapshots() {
        let p = tmp_file("aviary-write-test.md", "original");
        let h = hash("original");

        let out = write_entry(p.to_str().unwrap(), "updated", &h, false).unwrap();
        match out {
            WriteOutcome::Written { snapshot, .. } => {
                assert_eq!(fs::read_to_string(&p).unwrap(), "updated");
                assert_eq!(
                    fs::read_to_string(&snapshot).unwrap(),
                    "original",
                    "snapshot must hold the pre-write content"
                );
                fs::remove_file(snapshot).ok();
            }
            other => panic!("expected Written, got {other:?}"),
        }
        fs::remove_file(p).ok();
    }

    #[test]
    fn refuses_when_changed_on_disk() {
        let p = tmp_file("aviary-conflict-test.md", "original");
        let stale = hash("what the editor loaded");

        let out = write_entry(p.to_str().unwrap(), "updated", &stale, false).unwrap();
        match out {
            WriteOutcome::Conflict { disk_content, .. } => {
                assert_eq!(disk_content, "original");
                assert_eq!(
                    fs::read_to_string(&p).unwrap(),
                    "original",
                    "a refused write must not touch the file"
                );
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        fs::remove_file(p).ok();
    }

    #[test]
    fn force_overwrites_a_conflict() {
        let p = tmp_file("aviary-force-test.md", "original");
        let out = write_entry(p.to_str().unwrap(), "forced", "stale-hash", true).unwrap();
        assert!(matches!(out, WriteOutcome::Written { .. }));
        assert_eq!(fs::read_to_string(&p).unwrap(), "forced");
        fs::remove_file(p).ok();
    }
}

#[cfg(test)]
mod roundtrip {
    use super::*;

    /// Exercises the exact sequence the UI performs: read → edit → save →
    /// re-read, including the hash handshake.
    #[test]
    fn ui_roundtrip_preserves_and_reverts() {
        let dir = std::env::temp_dir().join("aviary-roundtrip");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("SKILL.md");
        let original = "---\nname: demo\ndescription: before\n---\n\n# Demo\n\nBody.\n";
        fs::write(&path, original).unwrap();
        let ps = path.to_str().unwrap();

        // read
        let c = crate::library::read_entry(ps).unwrap();
        assert_eq!(c.hash, hash(original));

        // edit + save with the hash the reader handed out
        let edited = original.replace("before", "after");
        let out = write_entry(ps, &edited, &c.hash, false).unwrap();
        let snap = match out {
            WriteOutcome::Written { ref snapshot, .. } => snapshot.clone(),
            other => panic!("expected Written, got {other:?}"),
        };

        // re-read reflects the edit, and frontmatter still parses
        let after = crate::library::read_entry(ps).unwrap();
        assert!(after.raw.contains("after"));
        assert!(after.frontmatter.is_some(), "frontmatter survived the write");
        assert!(!after.body.starts_with("---"));
        assert_eq!(after.hash, hash(&edited));

        // the snapshot can restore the original
        let restored = fs::read_to_string(&snap).unwrap();
        assert_eq!(restored, original, "snapshot must round-trip the original");

        // a stale save is refused
        let stale = write_entry(ps, "clobber", &c.hash, false).unwrap();
        assert!(matches!(stale, WriteOutcome::Conflict { .. }));
        assert!(fs::read_to_string(ps).unwrap().contains("after"));

        fs::remove_file(&snap).ok();
        fs::remove_dir_all(&dir).ok();
        eprintln!("roundtrip ok — edit applied, snapshot restores, stale write refused");
    }
}
