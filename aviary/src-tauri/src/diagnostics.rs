//! Local crash diagnostics.
//!
//! Logs are deliberately ordinary files under `~/.aviary/logs`: they rotate,
//! stay bounded, and never leave the machine unless the user copies them. The
//! copied bundle is similarly narrow — runtime facts, the reported failure,
//! and a bounded tail of Aviary's own logs. It never reads runner config,
//! prompts, environment variables, databases, or media metadata.

use serde::{Deserialize, Serialize};
use std::backtrace::Backtrace;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Once, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const LOG_TAIL_BYTES: usize = 200_000;
const FAILURE_FIELD_CHARS: usize = 16_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendFailure {
    pub source: String,
    pub context: Option<String>,
    pub message: String,
    pub stack: Option<String>,
    pub component_stack: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticsBundle {
    pub text: String,
    pub logs_dir: Option<String>,
}

static ACTIVE_LOGS_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Selects a writable log directory before the logging plugin is installed.
/// A broken home-directory permission must not turn diagnostics into a launch
/// failure, so the process temp directory is a deliberate second choice.
pub fn prepare_logs_dir() -> Option<PathBuf> {
    let preferred = crate::store::logs_dir();
    let fallback = std::env::temp_dir()
        .join(format!("aviary-{}", std::process::id()))
        .join("logs");
    let selected = preferred
        .into_iter()
        .chain(std::iter::once(fallback))
        .find(|path| log_file_is_writable(path));
    let _ = ACTIVE_LOGS_DIR.set(selected.clone());
    selected
}

fn log_file_is_writable(dir: &Path) -> bool {
    let Some(parent) = dir.parent() else {
        return false;
    };
    if crate::store::ensure_private_dir(parent).is_err()
        || crate::store::ensure_private_dir(dir).is_err()
    {
        return false;
    }
    let Ok(file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("aviary.log"))
    else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if file
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .is_err()
        {
            return false;
        }
    }
    enforce_log_file_bound(dir).is_ok()
}

fn is_aviary_log(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == "aviary.log"
        || (name.starts_with("aviary_") && (name.ends_with(".log") || name.ends_with(".log.bak")))
}

/// Plugin-log can leave collision backups that its rotation count does not
/// include. Prune those alongside normal archives before the plugin opens the
/// active file so the documented five-file ceiling remains true.
fn enforce_log_file_bound(dir: &Path) -> Result<(), String> {
    let mut archives: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            is_aviary_log(path)
                && path.file_name().and_then(|name| name.to_str()) != Some("aviary.log")
        })
        .collect();
    archives.sort_by_key(|path| {
        (
            path.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH),
            path.clone(),
        )
    });
    let excess = archives.len().saturating_sub(4);
    for path in archives.into_iter().take(excess) {
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn is_local_log_target(target: &str) -> bool {
    target.starts_with("aviary") || target.starts_with(tauri_plugin_log::WEBVIEW_TARGET)
}

fn active_logs_dir() -> Option<PathBuf> {
    match ACTIVE_LOGS_DIR.get() {
        Some(path) => path.clone(),
        None => crate::store::logs_dir(),
    }
}

/// Installs once because replacing a panic hook more than once would chain the
/// same logger repeatedly. The previous hook stays in place for development
/// stderr and the platform's normal panic handling.
pub fn install_panic_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let payload = if let Some(message) = info.payload().downcast_ref::<&str>() {
                (*message).to_string()
            } else if let Some(message) = info.payload().downcast_ref::<String>() {
                message.clone()
            } else {
                "non-string panic payload".to_string()
            };
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "unknown location".to_string());

            log::error!(
                target: "aviary::panic",
                "panic at {location}: {payload}\n{}",
                Backtrace::force_capture()
            );
            previous(info);
        }));
    });
}

pub fn log_startup(version: &str) {
    let build = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    log::info!(
        target: "aviary::startup",
        "started version={version} os={} arch={} build={build}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
}

pub fn collect(
    version: &str,
    failure: Option<FrontendFailure>,
) -> Result<DiagnosticsBundle, String> {
    let logs_dir = active_logs_dir();
    collect_from(
        logs_dir.as_deref(),
        version,
        failure,
        crate::providers::home().as_deref(),
    )
}

fn collect_from(
    logs_dir: Option<&Path>,
    version: &str,
    failure: Option<FrontendFailure>,
    home: Option<&Path>,
) -> Result<DiagnosticsBundle, String> {
    let generated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let build = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };

    let mut text = format!(
        "Aviary diagnostics\nGenerated (Unix seconds): {generated_at}\nVersion: {version}\nTarget: {} {}\nBuild: {build}\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
    );

    if let Some(failure) = failure {
        text.push_str("\nReported failure\n");
        text.push_str(&format!(
            "Source: {}\n",
            clipped(&failure.source, FAILURE_FIELD_CHARS)
        ));
        if let Some(context) = failure.context.filter(|s| !s.trim().is_empty()) {
            text.push_str(&format!(
                "Context: {}\n",
                clipped(&context, FAILURE_FIELD_CHARS)
            ));
        }
        text.push_str(&format!(
            "Message: {}\n",
            clipped(&failure.message, FAILURE_FIELD_CHARS)
        ));
        if let Some(stack) = failure.stack.filter(|s| !s.trim().is_empty()) {
            text.push_str("Stack:\n");
            text.push_str(&clipped(&stack, FAILURE_FIELD_CHARS));
            text.push('\n');
        }
        if let Some(stack) = failure.component_stack.filter(|s| !s.trim().is_empty()) {
            text.push_str("React component stack:\n");
            text.push_str(&clipped(&stack, FAILURE_FIELD_CHARS));
            text.push('\n');
        }
    }

    text.push_str("\nRecent Aviary log output\n");
    match logs_dir {
        Some(logs_dir) => {
            let tail = recent_log_tail(logs_dir, LOG_TAIL_BYTES)?;
            if tail.is_empty() {
                text.push_str("(no log records found)\n");
            } else {
                text.push_str(&tail);
            }
        }
        None => text.push_str("(file logging unavailable; check process stderr)\n"),
    }

    Ok(DiagnosticsBundle {
        text: redact_home(&text, home),
        logs_dir: logs_dir.map(|path| path.to_string_lossy().into_owned()),
    })
}

fn recent_log_tail(dir: &Path, limit: usize) -> Result<String, String> {
    if !dir.exists() {
        return Ok(String::new());
    }

    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_aviary_log(path))
        .collect();

    files.sort_by_key(|path| {
        let modified = path
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH);
        let active = path.file_name().and_then(|n| n.to_str()) == Some("aviary.log");
        (modified, active)
    });

    let mut remaining = limit;
    let mut chunks = Vec::new();
    for path in files.into_iter().rev() {
        if remaining == 0 {
            break;
        }
        // Rotation can rename an archive after `read_dir` but before open. A
        // disappearing file should not make the copy action fail.
        let Ok((content, bytes_read)) = read_tail(&path, remaining) else {
            continue;
        };
        remaining = remaining.saturating_sub(bytes_read);
        if content.is_empty() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("log");
        chunks.push(format!("\n--- {name} ---\n{content}"));
    }
    chunks.reverse();
    Ok(chunks.concat())
}

fn read_tail(path: &Path, limit: usize) -> Result<(String, usize), String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    let bytes = len.min(limit as u64) as usize;
    let offset = len.saturating_sub(bytes as u64);
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;

    let mut raw = Vec::with_capacity(bytes);
    file.take(bytes as u64)
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    if offset > 0 {
        if let Some(newline) = raw.iter().position(|b| *b == b'\n') {
            raw.drain(..=newline);
        }
    }
    Ok((String::from_utf8_lossy(&raw).into_owned(), bytes))
}

fn clipped(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let mut out: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        out.push_str("\n…[truncated]");
    }
    out
}

fn redact_home(value: &str, home: Option<&Path>) -> String {
    let Some(home) = home.and_then(Path::to_str).filter(|p| !p.is_empty()) else {
        return value.to_string();
    };
    value.replace(home, "~")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "aviary-diagnostics-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn bundle_reads_real_logs_and_redacts_home() {
        let root = test_dir("bundle");
        let logs = root.join("logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(
            logs.join("aviary.log"),
            format!("failed to read {}/project/AGENTS.md\n", root.display()),
        )
        .unwrap();

        let failure = FrontendFailure {
            source: "react".into(),
            context: Some("library".into()),
            message: format!("render failed below {}", root.display()),
            stack: None,
            component_stack: None,
        };
        let bundle = collect_from(Some(&logs), "1.2.3", Some(failure), Some(&root)).unwrap();

        assert!(bundle.text.contains("Version: 1.2.3"));
        assert!(bundle.text.contains("~/project/AGENTS.md"));
        assert!(bundle.text.contains("render failed below ~"));
        assert!(!bundle.text.contains(root.to_string_lossy().as_ref()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn log_tail_is_bounded_and_keeps_the_newest_bytes() {
        let root = test_dir("tail");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("aviary.log"),
            format!("{}\nLATEST RECORD\n", "old record\n".repeat(100)),
        )
        .unwrap();

        let tail = recent_log_tail(&root, 80).unwrap();
        assert!(tail.contains("LATEST RECORD"));
        assert!(!tail.contains(&"old record\n".repeat(10)));
        assert!(tail.len() < 160, "file heading is the only overhead");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failure_fields_are_capped() {
        let clipped = clipped(&"x".repeat(FAILURE_FIELD_CHARS + 20), FAILURE_FIELD_CHARS);
        assert!(clipped.ends_with("…[truncated]"));
        assert!(clipped.len() < FAILURE_FIELD_CHARS + 40);
    }

    #[test]
    fn bundle_stays_available_without_file_logging() {
        let bundle = collect_from(None, "1.2.3", None, None).unwrap();

        assert!(bundle.text.contains("file logging unavailable"));
        assert!(bundle.logs_dir.is_none());
    }

    #[test]
    fn located_webview_failures_are_kept() {
        assert!(is_local_log_target("webview"));
        assert!(is_local_log_target(
            "webview:reportFrontendFailure@http://tauri.localhost/src/lib/api.ts:42:3"
        ));
        assert!(is_local_log_target("aviary::watcher"));
        assert!(!is_local_log_target("notify::inotify"));
    }

    #[test]
    fn collision_backups_respect_the_five_file_bound() {
        let root = test_dir("rotation");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("aviary.log"), "active").unwrap();
        for index in 0..7 {
            fs::write(root.join(format!("aviary_{index}.log.bak")), "archive").unwrap();
        }

        enforce_log_file_bound(&root).unwrap();

        let count = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| is_aviary_log(path))
            .count();
        assert_eq!(count, 5);
        fs::remove_dir_all(root).unwrap();
    }
}
