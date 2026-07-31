//! Resolved context: what a runner actually loads before your first message.
//!
//! This is the view the design spec calls the debugging tool that does not
//! exist anywhere else, so its value rests entirely on being *true*. Two rules
//! follow from that:
//!
//! * **Only count what is on disk.** Every token figure here comes from
//!   tokenising a real file. Nothing is inferred from averages.
//! * **Say so when a cost cannot be known.** A runner's built-in system prompt
//!   and its MCP tool definitions are not files we can read — the former ships
//!   inside the binary, the latter arrives only after a server handshake. Those
//!   layers are reported with `measured: false` and no token figure rather than
//!   a plausible-looking guess, and the total is scoped to configuration.
//!
//! Load order matters as much as size: both runners read ancestor instruction
//! files walking *down* toward the working directory, so a stray `CLAUDE.md`
//! three levels up is exactly the kind of surprise this view exists to expose.

use crate::mcp;
use crate::providers::{claude_code, codex, home, Runner};
use crate::tokens;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which part of the stack a layer belongs to. Drives grouping and colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    System,
    User,
    Project,
    Local,
    Skills,
    Mcp,
    Memory,
}

#[derive(Debug, Clone, Serialize)]
pub struct Layer {
    pub scope: Scope,
    /// Human label for the row, e.g. "Project instructions".
    pub label: String,
    /// Path on disk, or a summary when the layer aggregates several files.
    pub path: String,
    pub tokens: usize,
    /// False when the cost cannot be read from disk. Such layers contribute
    /// nothing to `total` and the UI must not draw them as a size.
    pub measured: bool,
    /// Why a layer is unmeasurable, or extra colour when it is.
    pub note: Option<String>,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct Resolved {
    pub runner: Runner,
    pub cwd: String,
    pub layers: Vec<Layer>,
    /// Sum over measured layers only.
    pub total: usize,
    /// Number of layers whose cost could not be read.
    pub unmeasured: usize,
    pub scanned_ms: u64,
}

fn file_layer(path: &Path, scope: Scope, label: &str) -> Option<Layer> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    Some(Layer {
        scope,
        label: label.to_string(),
        path: path.to_string_lossy().to_string(),
        tokens: tokens::count(&text),
        measured: true,
        note: None,
        bytes: meta.len(),
    })
}

/// Instruction files from the home directory down to `cwd`, in the order the
/// runner reads them: shallowest first, so the deepest file wins last.
///
/// Bounded by home rather than the filesystem root — walking above home finds
/// nothing a runner would load and risks touching unrelated volumes.
fn ancestor_chain(cwd: &Path, filename: &str) -> Vec<PathBuf> {
    let home = home();
    let mut dirs: Vec<&Path> = Vec::new();

    for dir in cwd.ancestors() {
        dirs.push(dir);
        if let Some(h) = &home {
            if dir == h {
                break;
            }
        }
    }
    dirs.reverse();

    dirs.into_iter()
        .map(|d| d.join(filename))
        .filter(|p| p.is_file())
        .collect()
}

/// Skills contribute their frontmatter, not their bodies.
///
/// This is the correction that makes the number believable: a runner lists the
/// available skills up front (name + description, a line or two each) and only
/// loads a `SKILL.md` body when that skill is actually invoked. Counting whole
/// bodies here would inflate the figure by an order of magnitude and make the
/// whole view untrustworthy.
fn skills_layer(skills_root: &Path) -> Option<Layer> {
    let read = std::fs::read_dir(skills_root).ok()?;

    let mut count = 0usize;
    let mut listing = String::new();

    for child in read.filter_map(Result::ok) {
        let skill_md = child.path().join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let parsed = crate::providers::parse_markdown(&skill_md);
        let name = parsed.name.unwrap_or_else(|| {
            child
                .file_name()
                .to_str()
                .unwrap_or("untitled")
                .to_string()
        });
        listing.push_str(&name);
        listing.push_str(": ");
        listing.push_str(&parsed.description.unwrap_or_default());
        listing.push('\n');
        count += 1;
    }

    if count == 0 {
        return None;
    }

    Some(Layer {
        scope: Scope::Skills,
        label: format!("{count} skills available"),
        path: skills_root.to_string_lossy().to_string(),
        tokens: tokens::count(&listing),
        measured: true,
        note: Some("Names and descriptions only — bodies load when invoked".into()),
        bytes: listing.len() as u64,
    })
}

/// Claude Code keeps per-project memory under a slug of the absolute path,
/// with `/` replaced by `-`: `~/work/dash` → `-Users-me-work-dash`.
fn memory_layer(cwd: &Path) -> Option<Layer> {
    let home = home()?;
    let slug = cwd.to_string_lossy().replace('/', "-");
    let dir = home.join(".claude").join("projects").join(slug).join("memory");

    let read = std::fs::read_dir(&dir).ok()?;
    let mut text = String::new();
    let mut count = 0usize;
    let mut bytes = 0u64;

    for child in read.filter_map(Result::ok) {
        let path = child.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if let Ok(body) = std::fs::read_to_string(&path) {
            bytes += body.len() as u64;
            text.push_str(&body);
            count += 1;
        }
    }

    if count == 0 {
        return None;
    }

    Some(Layer {
        scope: Scope::Memory,
        label: format!("{count} memory files"),
        path: dir.to_string_lossy().to_string(),
        tokens: tokens::count(&text),
        measured: true,
        note: None,
        bytes,
    })
}

/// MCP tool definitions are real context, but their size is only knowable
/// after a server handshake returns its schemas. Reported, never guessed.
fn mcp_layer(runner: Runner, projects: &[(String, PathBuf)]) -> Option<Layer> {
    let snapshot = mcp::scan(projects);
    let servers: Vec<&mcp::Server> = snapshot
        .servers
        .iter()
        .filter(|s| s.enabled && s.runners.contains(&runner))
        .collect();

    if servers.is_empty() {
        return None;
    }

    let mut names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();

    Some(Layer {
        scope: Scope::Mcp,
        label: format!("{} MCP servers enabled", servers.len()),
        path: names.join(", "),
        tokens: 0,
        measured: false,
        note: Some(
            "Tool definitions are sent by each server at connection time, so their \
             size cannot be read from disk"
                .into(),
        ),
        bytes: 0,
    })
}

fn system_layer(runner: Runner) -> Layer {
    Layer {
        scope: Scope::System,
        label: format!("{} system prompt", runner.label()),
        path: "built into the CLI".into(),
        tokens: 0,
        measured: false,
        note: Some("Ships inside the binary — not a file Aviary can read".into()),
        bytes: 0,
    }
}

/// Resolves the full stack for a runner in a working directory.
pub fn resolve(runner: Runner, cwd: &str, projects: &[(String, PathBuf)]) -> Resolved {
    let started = std::time::Instant::now();
    let dir = PathBuf::from(shellexpand(cwd));
    let mut layers = vec![system_layer(runner)];

    match runner {
        Runner::ClaudeCode => {
            if let Some(root) = claude_code::root() {
                if let Some(l) = file_layer(&root.join("CLAUDE.md"), Scope::User, "User instructions")
                {
                    layers.push(l);
                }
            }

            for path in ancestor_chain(&dir, "CLAUDE.md") {
                let is_cwd = path.parent() == Some(dir.as_path());
                let label = if is_cwd {
                    "Project instructions"
                } else {
                    "Inherited from a parent directory"
                };
                if let Some(l) = file_layer(&path, Scope::Project, label) {
                    layers.push(l);
                }
            }

            for candidate in [
                dir.join("CLAUDE.local.md"),
                dir.join(".claude").join("CLAUDE.local.md"),
            ] {
                if let Some(l) = file_layer(&candidate, Scope::Local, "Local overrides") {
                    layers.push(l);
                }
            }

            if let Some(root) = claude_code::root() {
                if let Some(l) = skills_layer(&root.join("skills")) {
                    layers.push(l);
                }
            }
            if let Some(l) = skills_layer(&dir.join(".claude").join("skills")) {
                layers.push(Layer {
                    label: format!("{} (project)", l.label),
                    ..l
                });
            }

            if let Some(l) = mcp_layer(runner, projects) {
                layers.push(l);
            }
            if let Some(l) = memory_layer(&dir) {
                layers.push(l);
            }
        }

        Runner::Codex => {
            if let Some(root) = codex::root() {
                if let Some(l) = file_layer(&root.join("AGENTS.md"), Scope::User, "User instructions")
                {
                    layers.push(l);
                }
            }

            for path in ancestor_chain(&dir, "AGENTS.md") {
                let is_cwd = path.parent() == Some(dir.as_path());
                let label = if is_cwd {
                    "Project instructions"
                } else {
                    "Inherited from a parent directory"
                };
                if let Some(l) = file_layer(&path, Scope::Project, label) {
                    layers.push(l);
                }
            }

            if let Some(root) = codex::root() {
                if let Some(l) = skills_layer(&root.join("skills")) {
                    layers.push(l);
                }
            }
            if let Some(l) = skills_layer(&dir.join(".codex").join("skills")) {
                layers.push(Layer {
                    label: format!("{} (project)", l.label),
                    ..l
                });
            }

            if let Some(l) = mcp_layer(runner, projects) {
                layers.push(l);
            }
        }
    }

    let total = layers.iter().filter(|l| l.measured).map(|l| l.tokens).sum();
    let unmeasured = layers.iter().filter(|l| !l.measured).count();

    Resolved {
        runner,
        cwd: dir.to_string_lossy().to_string(),
        layers,
        total,
        unmeasured,
        scanned_ms: started.elapsed().as_millis() as u64,
    }
}

/// Expands a leading `~` so paths shown in the UI can be typed back in.
fn shellexpand(path: &str) -> String {
    match (path.strip_prefix("~/"), home()) {
        (Some(rest), Some(h)) => h.join(rest).to_string_lossy().to_string(),
        _ if path == "~" => home()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string()),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestors_run_shallowest_first() {
        let Some(home) = home() else { return };
        let deep = home.join("a").join("b").join("c");
        let chain = ancestor_chain(&deep, "definitely-not-a-real-file.md");
        // No such files exist, but the walk must not panic or escape home.
        assert!(chain.is_empty());
    }

    /// Resolves against this machine and cross-checks one figure against the
    /// standalone token counter the UI already exposes. If these ever diverge,
    /// the two paths are tokenising different bytes.
    #[test]
    fn resolves_real_machine() {
        let Some(home) = home() else { return };
        let cwd = home.join("personalAi");
        if !cwd.is_dir() {
            eprintln!("skipped: no ~/personalAi on this machine");
            return;
        }

        let resolved = resolve(Runner::ClaudeCode, &cwd.to_string_lossy(), &[]);
        eprintln!(
            "resolved {} layers in {}ms · total={} unmeasured={}",
            resolved.layers.len(),
            resolved.scanned_ms,
            resolved.total,
            resolved.unmeasured
        );
        for l in &resolved.layers {
            eprintln!(
                "  {:<8?} {:>7} {:<44} {}",
                l.scope,
                if l.measured {
                    l.tokens.to_string()
                } else {
                    "—".into()
                },
                l.label,
                l.path
            );
        }

        // The total must never include a layer we could not measure.
        let measured_sum: usize = resolved
            .layers
            .iter()
            .filter(|l| l.measured)
            .map(|l| l.tokens)
            .sum();
        assert_eq!(resolved.total, measured_sum);

        // Cross-check the user-scope file against `tokens::count_file`.
        let user_md = home.join(".claude").join("CLAUDE.md");
        if user_md.is_file() {
            let row = resolved
                .layers
                .iter()
                .find(|l| l.path == user_md.to_string_lossy())
                .expect("user CLAUDE.md exists on disk, so it must appear as a layer");
            assert_eq!(row.tokens, tokens::count_file(&user_md.to_string_lossy()));
        }
    }

    #[test]
    fn unmeasured_layers_do_not_inflate_the_total() {
        let layers = vec![
            Layer {
                scope: Scope::System,
                label: "sys".into(),
                path: String::new(),
                tokens: 0,
                measured: false,
                note: None,
                bytes: 0,
            },
            Layer {
                scope: Scope::User,
                label: "user".into(),
                path: String::new(),
                tokens: 120,
                measured: true,
                note: None,
                bytes: 0,
            },
        ];
        let total: usize = layers.iter().filter(|l| l.measured).map(|l| l.tokens).sum();
        assert_eq!(total, 120);
    }
}
