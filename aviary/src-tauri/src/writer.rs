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

use crate::mcp;
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};
use toml_edit::{value as toml_value, DocumentMut, Item, TableLike};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

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
    Conflict {
        disk_hash: String,
        disk_content: String,
    },
}

/// Structured runner-config writes deliberately expose less than the generic
/// editor. A conflict says only that the intent must be rescanned and retried;
/// returning either side's config would disclose credentials to the webview.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", tag = "status")]
pub enum McpToggleOutcome {
    Written {
        revision: String,
        snapshot_id: String,
    },
    Unchanged {
        revision: String,
    },
    Conflict,
    Unavailable {
        reason: mcp::ToggleUnavailableReason,
    },
    NotFound,
}

fn history_dir() -> Result<PathBuf, String> {
    let dir = dirs::home_dir()
        .ok_or("no home directory")?
        .join(".aviary")
        .join("history");
    prepare_history_dir(&dir)?;
    Ok(dir)
}

fn prepare_history_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    Ok(())
}

/// `None` records that the target did not exist before a structured write.
/// The marker makes future undo support able to distinguish absence from an
/// existing empty file without putting that metadata in the webview.
fn snapshot_in(dir: &Path, path: &Path, content: Option<&str>) -> Result<PathBuf, String> {
    prepare_history_dir(dir)?;
    let stem = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("entry")
        .replace('/', "_");
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    static SNAPSHOT_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SNAPSHOT_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let marker = content.unwrap_or("aviary: source file did not exist before this write\n");
    let target = dir.join(format!(
        "{}-{}-{}-{sequence}-{stem}",
        elapsed.as_secs(),
        elapsed.subsec_nanos(),
        &hash(marker)[..8]
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let result = (|| {
        let mut file = options.open(&target).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
        file.write_all(marker.as_bytes())
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        sync_directory(dir)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&target);
    }
    result.map(|()| target)
}

/// Replaces a file atomically: temp file in the same directory, fsync, rename.
///
/// Same directory matters — rename is only atomic within a filesystem, and a
/// temp dir may well be on another one.
fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let parent = path.parent().ok_or("path has no parent directory")?;
    let source_mode = source_mode(path);

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

    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(source_mode);
        let mut file = options.open(&tmp).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        fs::set_permissions(&tmp, fs::Permissions::from_mode(source_mode))
            .map_err(|e| e.to_string())?;
        file.write_all(content.as_bytes())
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(unix)]
fn source_mode(path: &Path) -> u32 {
    fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o777)
        .unwrap_or(0o600)
}

#[cfg(not(unix))]
fn source_mode(_path: &Path) -> u32 {
    0
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| e.to_string())
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
    write_entry_with_history(path, content, expected_hash, force, None)
}

fn write_entry_with_history(
    path: &str,
    content: &str,
    expected_hash: &str,
    force: bool,
    history_override: Option<&Path>,
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
    let default_history;
    let history = if let Some(history) = history_override {
        history
    } else {
        default_history = history_dir()?;
        &default_history
    };
    let snap = snapshot_in(history, p, Some(&current))?;
    atomic_write(p, content)?;

    Ok(WriteOutcome::Written {
        hash: hash(content),
        snapshot: snap.to_string_lossy().to_string(),
    })
}

/// Applies a narrow persistent MCP switch after resolving its target from a
/// fresh backend inventory. `cwd` is accepted because Claude switches are
/// project-specific and arbitrary folder-picker contexts are valid even before
/// registration.
pub fn set_mcp_enabled(
    projects: &[(String, PathBuf)],
    cwd: Option<&Path>,
    effective_id: &str,
    enabled: bool,
    expected_revision: &str,
) -> Result<McpToggleOutcome, String> {
    static MCP_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = MCP_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "MCP writer lock is poisoned".to_string())?;

    let target = match mcp::resolve_toggle_target(projects, cwd, effective_id) {
        mcp::ToggleTargetResolution::Writable(target) => target,
        mcp::ToggleTargetResolution::Unavailable(reason) => {
            return Ok(McpToggleOutcome::Unavailable { reason });
        }
        mcp::ToggleTargetResolution::Missing => return Ok(McpToggleOutcome::NotFound),
    };
    apply_mcp_toggle_target(target, enabled, expected_revision, None)
}

fn apply_mcp_toggle_target(
    target: mcp::ToggleTarget,
    enabled: bool,
    expected_revision: &str,
    history_override: Option<&Path>,
) -> Result<McpToggleOutcome, String> {
    if target.revision != expected_revision || mcp::file_revision(&target.path) != target.revision {
        return Ok(McpToggleOutcome::Conflict);
    }

    let existing = read_optional_utf8(&target.path)?;
    let updated = mutate_mcp_config(existing.as_deref(), &target.mutation, enabled)?;
    if existing.as_deref() == Some(updated.as_str()) {
        return Ok(McpToggleOutcome::Unchanged {
            revision: target.revision,
        });
    }

    // Snapshot first. A final revision check narrows the race with runner CLIs
    // that may update the same file while Aviary is preparing the replacement.
    let default_history;
    let history = if let Some(history) = history_override {
        history
    } else {
        default_history = history_dir()?;
        &default_history
    };
    let snapshot = snapshot_in(history, &target.path, existing.as_deref())?;
    if mcp::file_revision(&target.path) != target.revision {
        return Ok(McpToggleOutcome::Conflict);
    }
    atomic_write(&target.path, &updated)?;
    let revision = mcp::file_revision(&target.path);
    let snapshot_id = snapshot
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("snapshot")
        .to_string();
    Ok(McpToggleOutcome::Written {
        revision,
        snapshot_id,
    })
}

fn read_optional_utf8(path: &Path) -> Result<Option<String>, String> {
    match fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| format!("{} is not valid UTF-8", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

fn mutate_mcp_config(
    current: Option<&str>,
    mutation: &mcp::ToggleMutation,
    enabled: bool,
) -> Result<String, String> {
    match mutation {
        mcp::ToggleMutation::ClaudeDisabledList {
            project_key,
            server_name,
        } => mutate_claude_server_list(
            current,
            project_key,
            server_name,
            "disabledMcpServers",
            !enabled,
        ),
        mcp::ToggleMutation::ClaudeEnabledList {
            project_key,
            server_name,
        } => mutate_claude_server_list(
            current,
            project_key,
            server_name,
            "enabledMcpServers",
            enabled,
        ),
        mcp::ToggleMutation::CodexServer { server_name } => {
            mutate_codex_enabled(current, &["mcp_servers", server_name], enabled)
        }
        mcp::ToggleMutation::CodexPluginServer {
            plugin_name,
            server_name,
        } => mutate_codex_enabled(
            current,
            &["plugins", plugin_name, "mcp_servers", server_name],
            enabled,
        ),
    }
}

fn mutate_claude_server_list(
    current: Option<&str>,
    project_key: &str,
    server_name: &str,
    list_name: &str,
    present: bool,
) -> Result<String, String> {
    let mut root = match current {
        Some(raw) if !raw.trim().is_empty() => {
            serde_json::from_str::<serde_json::Value>(raw).map_err(|e| e.to_string())?
        }
        _ => serde_json::json!({}),
    };
    let root = root
        .as_object_mut()
        .ok_or("Claude config root must be a JSON object")?;
    let projects = root
        .entry("projects")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("Claude config projects must be a JSON object")?;
    let project = projects
        .entry(project_key)
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("Claude project config must be a JSON object")?;

    let mut servers: Vec<String> = project
        .get(list_name)
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| format!("{list_name} must be an array"))?
                .iter()
                .map(|entry| {
                    entry
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("{list_name} entries must be strings"))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    servers.retain(|name| name != server_name);
    if present {
        servers.push(server_name.to_string());
    }
    if servers.is_empty() {
        project.remove(list_name);
    } else {
        project.insert(
            list_name.into(),
            serde_json::Value::Array(servers.into_iter().map(serde_json::Value::String).collect()),
        );
    }

    let mut updated = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    updated.push('\n');
    Ok(updated)
}

fn mutate_codex_enabled(
    current: Option<&str>,
    table_path: &[&str],
    enabled: bool,
) -> Result<String, String> {
    let mut document = DocumentMut::from_str(current.unwrap_or("")).map_err(|e| e.to_string())?;
    set_toml_bool(document.as_table_mut(), table_path, "enabled", enabled)?;
    Ok(document.to_string())
}

fn set_toml_bool(
    table: &mut dyn TableLike,
    path: &[&str],
    key: &str,
    enabled: bool,
) -> Result<(), String> {
    let Some((head, tail)) = path.split_first() else {
        if let Some(item) = table.get_mut(key) {
            let existing = item
                .as_value_mut()
                .ok_or_else(|| format!("TOML key {key} is not a scalar value"))?;
            if existing.as_bool().is_none() {
                return Err(format!("TOML key {key} is not a boolean"));
            }
            // Replacing an Item drops the scalar's decor, including comments
            // immediately above it. Swap only the typed value and carry that
            // decor forward so the config remains recognizably the user's.
            let decor = existing.decor().clone();
            let mut replacement = toml_edit::Value::from(enabled);
            *replacement.decor_mut() = decor;
            *existing = replacement;
        } else {
            table.insert(key, toml_value(enabled));
        }
        return Ok(());
    };
    if !table.contains_key(head) {
        table.insert(head, Item::Table(toml_edit::Table::new()));
    }
    let child = table
        .get_mut(head)
        .ok_or_else(|| format!("missing TOML table {head}"))?;
    let child = child
        .as_table_like_mut()
        .ok_or_else(|| format!("TOML key {head} is not a table"))?;
    set_toml_bool(child, tail, key, enabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp_file(body: &str) -> (TempDir, PathBuf, PathBuf) {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("entry.md");
        let history = dir.path().join("history");
        fs::write(&p, body).unwrap();
        (dir, p, history)
    }

    #[test]
    fn writes_and_snapshots() {
        let (_tmp, p, history) = tmp_file("original");
        let h = hash("original");

        let out =
            write_entry_with_history(p.to_str().unwrap(), "updated", &h, false, Some(&history))
                .unwrap();
        match out {
            WriteOutcome::Written { snapshot, .. } => {
                assert_eq!(fs::read_to_string(&p).unwrap(), "updated");
                assert_eq!(
                    fs::read_to_string(&snapshot).unwrap(),
                    "original",
                    "snapshot must hold the pre-write content"
                );
            }
            other => panic!("expected Written, got {other:?}"),
        }
    }

    #[test]
    fn refuses_when_changed_on_disk() {
        let (_tmp, p, history) = tmp_file("original");
        let stale = hash("what the editor loaded");

        let out = write_entry_with_history(
            p.to_str().unwrap(),
            "updated",
            &stale,
            false,
            Some(&history),
        )
        .unwrap();
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
        assert!(!history.exists());
    }

    #[test]
    fn force_overwrites_a_conflict() {
        let (_tmp, p, history) = tmp_file("original");
        let out = write_entry_with_history(
            p.to_str().unwrap(),
            "forced",
            "stale-hash",
            true,
            Some(&history),
        )
        .unwrap();
        assert!(matches!(out, WriteOutcome::Written { .. }));
        assert_eq!(fs::read_to_string(&p).unwrap(), "forced");
    }
}

#[cfg(test)]
mod structured_mcp_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn claude_regular_and_default_off_lists_round_trip_without_losing_unknown_fields() {
        let original = r#"{
          "account": {"token": "SECRET_CANARY", "unknown": true},
          "projects": {
            "/work/demo": {
              "disabledMcpServers": ["keep-disabled"],
              "enabledMcpServers": ["keep-enabled"],
              "other": {"nested": 7}
            }
          }
        }"#;
        let disabled = mcp::ToggleMutation::ClaudeDisabledList {
            project_key: "/work/demo".into(),
            server_name: "regular".into(),
        };
        let enabled = mcp::ToggleMutation::ClaudeEnabledList {
            project_key: "/work/demo".into(),
            server_name: "default-off".into(),
        };

        let after_disable = mutate_mcp_config(Some(original), &disabled, false).unwrap();
        let after_enable = mutate_mcp_config(Some(&after_disable), &enabled, true).unwrap();
        let value: serde_json::Value = serde_json::from_str(&after_enable).unwrap();
        assert_eq!(value["account"]["token"], "SECRET_CANARY");
        assert_eq!(value["projects"]["/work/demo"]["other"]["nested"], 7);
        let disabled_names = value["projects"]["/work/demo"]["disabledMcpServers"]
            .as_array()
            .unwrap();
        assert!(disabled_names.iter().any(|name| name == "keep-disabled"));
        assert!(disabled_names.iter().any(|name| name == "regular"));
        let enabled_names = value["projects"]["/work/demo"]["enabledMcpServers"]
            .as_array()
            .unwrap();
        assert!(enabled_names.iter().any(|name| name == "keep-enabled"));
        assert!(enabled_names.iter().any(|name| name == "default-off"));

        let after_regular_enable = mutate_mcp_config(Some(&after_enable), &disabled, true).unwrap();
        let after_default_disable =
            mutate_mcp_config(Some(&after_regular_enable), &enabled, false).unwrap();
        let value: serde_json::Value = serde_json::from_str(&after_default_disable).unwrap();
        let disabled_names = value["projects"]["/work/demo"]["disabledMcpServers"]
            .as_array()
            .unwrap();
        assert_eq!(disabled_names, &[serde_json::json!("keep-disabled")]);
        let enabled_names = value["projects"]["/work/demo"]["enabledMcpServers"]
            .as_array()
            .unwrap();
        assert_eq!(enabled_names, &[serde_json::json!("keep-enabled")]);
    }

    #[test]
    fn codex_server_and_plugin_toggles_preserve_comments_and_secrets() {
        let original = r#"# operator note
secret = "SECRET_CANARY"

[mcp_servers.demo]
command = "node"
args = ["server.js"]
# keep this comment
enabled = true

[plugins."demo@market".mcp_servers.tool]
enabled = true
unknown = "preserve-me"
"#;
        let direct = mcp::ToggleMutation::CodexServer {
            server_name: "demo".into(),
        };
        let plugin = mcp::ToggleMutation::CodexPluginServer {
            plugin_name: "demo@market".into(),
            server_name: "tool".into(),
        };
        let updated = mutate_mcp_config(Some(original), &direct, false).unwrap();
        let updated = mutate_mcp_config(Some(&updated), &plugin, false).unwrap();

        assert!(updated.contains("# operator note"));
        assert!(updated.contains("# keep this comment"));
        assert!(updated.contains("SECRET_CANARY"));
        assert!(updated.contains("preserve-me"));
        let value = updated.parse::<toml::Table>().unwrap();
        assert_eq!(
            value["mcp_servers"]["demo"]["enabled"].as_bool(),
            Some(false)
        );
        assert_eq!(
            value["plugins"]["demo@market"]["mcp_servers"]["tool"]["enabled"].as_bool(),
            Some(false)
        );
    }

    #[test]
    fn structured_write_is_atomic_snapshotted_and_mode_preserving() {
        let tmp = TempDir::new().unwrap();
        let config = tmp.path().join("config.toml");
        let history = tmp.path().join("history");
        let original = "secret = \"SECRET_CANARY\"\n[mcp_servers.demo]\nenabled = true\n";
        fs::write(&config, original).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&config, fs::Permissions::from_mode(0o640)).unwrap();
        let revision = mcp::file_revision(&config);
        let target = mcp::ToggleTarget {
            path: config.clone(),
            revision: revision.clone(),
            mutation: mcp::ToggleMutation::CodexServer {
                server_name: "demo".into(),
            },
        };

        let outcome = apply_mcp_toggle_target(target, false, &revision, Some(&history)).unwrap();
        let snapshot_id = match outcome {
            McpToggleOutcome::Written { snapshot_id, .. } => snapshot_id,
            other => panic!("expected written outcome, got {other:?}"),
        };
        let updated = fs::read_to_string(&config).unwrap();
        assert!(updated.contains("SECRET_CANARY"));
        assert_eq!(
            updated.parse::<toml::Table>().unwrap()["mcp_servers"]["demo"]["enabled"].as_bool(),
            Some(false)
        );
        assert_eq!(
            fs::read_to_string(history.join(&snapshot_id)).unwrap(),
            original
        );

        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(&config).unwrap().permissions().mode() & 0o777,
                0o640
            );
            assert_eq!(
                fs::metadata(&history).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(history.join(snapshot_id))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn structured_stale_conflict_exposes_no_config_content() {
        let tmp = TempDir::new().unwrap();
        let config = tmp.path().join("config.toml");
        let history = tmp.path().join("history");
        fs::write(&config, "[mcp_servers.demo]\nenabled = true\n").unwrap();
        let revision = mcp::file_revision(&config);
        let target = mcp::ToggleTarget {
            path: config.clone(),
            revision: revision.clone(),
            mutation: mcp::ToggleMutation::CodexServer {
                server_name: "demo".into(),
            },
        };
        fs::write(
            &config,
            "secret = \"STALE_SECRET_CANARY\"\n[mcp_servers.demo]\nenabled = true\n",
        )
        .unwrap();

        let outcome = apply_mcp_toggle_target(target, false, &revision, Some(&history)).unwrap();
        assert_eq!(outcome, McpToggleOutcome::Conflict);
        assert_eq!(
            serde_json::to_value(&outcome).unwrap(),
            serde_json::json!({"status": "conflict"})
        );
        assert!(fs::read_to_string(&config)
            .unwrap()
            .contains("STALE_SECRET_CANARY"));
        assert!(!history.exists());
    }

    #[test]
    fn structured_write_refuses_non_utf8_config_without_overwriting_it() {
        let tmp = TempDir::new().unwrap();
        let config = tmp.path().join("config.toml");
        let history = tmp.path().join("history");
        let original = [0xff, 0xfe, 0xfd];
        fs::write(&config, original).unwrap();
        let revision = mcp::file_revision(&config);
        let target = mcp::ToggleTarget {
            path: config.clone(),
            revision: revision.clone(),
            mutation: mcp::ToggleMutation::CodexServer {
                server_name: "demo".into(),
            },
        };

        let error = apply_mcp_toggle_target(target, false, &revision, Some(&history)).unwrap_err();
        assert!(error.contains("not valid UTF-8"));
        assert_eq!(fs::read(&config).unwrap(), original);
        assert!(!history.exists());
    }
}

#[cfg(test)]
mod roundtrip {
    use super::*;
    use tempfile::TempDir;

    /// Exercises the exact sequence the UI performs: read → edit → save →
    /// re-read, including the hash handshake.
    #[test]
    fn ui_roundtrip_preserves_and_reverts() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("SKILL.md");
        let history = dir.path().join("history");
        let original = "---\nname: demo\ndescription: before\n---\n\n# Demo\n\nBody.\n";
        fs::write(&path, original).unwrap();
        let ps = path.to_str().unwrap();

        // read
        let c = crate::library::read_entry(ps).unwrap();
        assert_eq!(c.hash, hash(original));

        // edit + save with the hash the reader handed out
        let edited = original.replace("before", "after");
        let out = write_entry_with_history(ps, &edited, &c.hash, false, Some(&history)).unwrap();
        let snap = match out {
            WriteOutcome::Written { ref snapshot, .. } => snapshot.clone(),
            other => panic!("expected Written, got {other:?}"),
        };

        // re-read reflects the edit, and frontmatter still parses
        let after = crate::library::read_entry(ps).unwrap();
        assert!(after.raw.contains("after"));
        assert!(
            after.frontmatter.is_some(),
            "frontmatter survived the write"
        );
        assert!(!after.body.starts_with("---"));
        assert_eq!(after.hash, hash(&edited));

        // the snapshot can restore the original
        let restored = fs::read_to_string(&snap).unwrap();
        assert_eq!(restored, original, "snapshot must round-trip the original");

        // a stale save is refused
        let stale =
            write_entry_with_history(ps, "clobber", &c.hash, false, Some(&history)).unwrap();
        assert!(matches!(stale, WriteOutcome::Conflict { .. }));
        assert!(fs::read_to_string(ps).unwrap().contains("after"));

        eprintln!("roundtrip ok — edit applied, snapshot restores, stale write refused");
    }
}
