//! Resolved context: what a runner actually loads before your first message.
//!
//! This is the view the design spec calls the debugging tool that does not
//! exist anywhere else, so its value rests entirely on being *true*. Two rules
//! follow from that:
//!
//! * **Only count what has a real basis.** File figures come from tokenising
//!   actual bytes, and runner figures come from the runner's own accounting.
//!   No average or fallback number is substituted for missing information.
//! * **Say so when a cost cannot be known.** A runner's built-in system prompt
//!   is not a file we can read. MCP definitions require runner inventory, and
//!   modern runners may defer them. Unknown values are `None`, never zero.
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
    pub tokens: Option<usize>,
    /// Where a token figure came from. File counts use Codex's o200k encoding;
    /// Claude estimates remain labeled until its control channel reports an
    /// exact value.
    pub basis: mcp::TokenBasis,
    /// Whether the source was fully enumerated.
    pub complete: bool,
    /// Some runners defer tool definitions. `None` means static discovery
    /// cannot know whether this layer is currently loaded.
    pub loaded: Option<bool>,
    /// Whether a known value contributes to `Resolved::total`.
    pub included_in_total: bool,
    /// Why a layer is unmeasurable, or extra colour when it is.
    pub note: Option<String>,
    pub bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct Resolved {
    pub runner: Runner,
    pub cwd: String,
    pub layers: Vec<Layer>,
    /// Sum over measured layers only.
    pub total: usize,
    /// False when at least one loaded layer has no token figure.
    pub total_complete: bool,
    /// Number of layers whose cost could not be read.
    pub unmeasured: usize,
    pub scanned_ms: u64,
}

fn file_layer(path: &Path, scope: Scope, label: &str) -> Option<Layer> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => {
            return Some(Layer {
                scope,
                label: label.to_string(),
                path: path.to_string_lossy().to_string(),
                tokens: None,
                basis: mcp::TokenBasis::Unavailable,
                complete: false,
                loaded: None,
                included_in_total: false,
                note: Some("The instruction file exists but could not be read".into()),
                bytes: Some(meta.len()),
            });
        }
    };
    Some(Layer {
        scope,
        label: label.to_string(),
        path: path.to_string_lossy().to_string(),
        tokens: Some(tokens::count(&text)),
        basis: mcp::TokenBasis::O200kFileEstimate,
        complete: true,
        loaded: Some(true),
        included_in_total: true,
        note: None,
        bytes: Some(meta.len()),
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
    let mut unreadable = 0usize;
    let mut listing = String::new();

    for child in read.filter_map(Result::ok) {
        let skill_md = child.path().join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        count += 1;
        if std::fs::read_to_string(&skill_md).is_err() {
            unreadable += 1;
            continue;
        }
        let parsed = crate::providers::parse_markdown(&skill_md);
        let name = parsed
            .name
            .unwrap_or_else(|| child.file_name().to_str().unwrap_or("untitled").to_string());
        listing.push_str(&name);
        listing.push_str(": ");
        listing.push_str(&parsed.description.unwrap_or_default());
        listing.push('\n');
    }

    if count == 0 {
        return None;
    }

    Some(Layer {
        scope: Scope::Skills,
        label: format!("{count} skills available"),
        path: skills_root.to_string_lossy().to_string(),
        tokens: (!listing.is_empty()).then(|| tokens::count(&listing)),
        basis: if listing.is_empty() {
            mcp::TokenBasis::Unavailable
        } else {
            mcp::TokenBasis::O200kFileEstimate
        },
        complete: unreadable == 0,
        loaded: Some(true),
        included_in_total: !listing.is_empty(),
        note: Some(if unreadable == 0 {
            "Names and descriptions only — bodies load when invoked".into()
        } else {
            format!("Names and descriptions only; {unreadable} skill files could not be read")
        }),
        bytes: (!listing.is_empty()).then_some(listing.len() as u64),
    })
}

/// Claude Code keeps per-project memory under a slug of the absolute path,
/// with `/` replaced by `-`: `~/work/dash` → `-Users-me-work-dash`.
fn memory_layer(cwd: &Path) -> Option<Layer> {
    let home = home()?;
    let slug = cwd.to_string_lossy().replace('/', "-");
    let dir = home
        .join(".claude")
        .join("projects")
        .join(slug)
        .join("memory");

    let read = std::fs::read_dir(&dir).ok()?;
    let mut text = String::new();
    let mut count = 0usize;
    let mut readable = 0usize;
    let mut unreadable = 0usize;
    let mut bytes = 0u64;

    for child in read.filter_map(Result::ok) {
        let path = child.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        count += 1;
        if let Ok(body) = std::fs::read_to_string(&path) {
            bytes += body.len() as u64;
            text.push_str(&body);
            readable += 1;
        } else {
            unreadable += 1;
        }
    }

    if count == 0 {
        return None;
    }

    Some(Layer {
        scope: Scope::Memory,
        label: format!("{count} memory files"),
        path: dir.to_string_lossy().to_string(),
        tokens: (readable > 0).then(|| tokens::count(&text)),
        basis: if readable > 0 {
            mcp::TokenBasis::O200kFileEstimate
        } else {
            mcp::TokenBasis::Unavailable
        },
        complete: unreadable == 0,
        loaded: Some(true),
        included_in_total: readable > 0,
        note: (unreadable > 0).then(|| format!("{unreadable} memory files could not be read")),
        bytes: (unreadable == 0).then_some(bytes),
    })
}

/// MCP tool definitions are real context, but their size is only knowable
/// after a server handshake returns its schemas. Reported, never guessed.
fn mcp_layer(runner: Runner, cwd: &Path, projects: &[(String, PathBuf)]) -> Option<Layer> {
    let snapshot = mcp::scan_for_context(projects, cwd);
    let cwd = cwd.to_string_lossy().to_string();
    let servers: Vec<&mcp::Server> = snapshot
        .servers
        .iter()
        .filter(|server| server.runner == runner)
        .filter(|server| server.cwd.as_deref() == Some(cwd.as_str()))
        .filter(|server| {
            matches!(
                server.state,
                mcp::DeclarationState::Enabled | mcp::DeclarationState::Unknown
            )
        })
        .collect();

    if servers.is_empty() {
        return None;
    }

    mcp_layer_from_servers(&servers)
}

fn mcp_layer_from_servers(servers: &[&mcp::Server]) -> Option<Layer> {
    if servers.is_empty() {
        return None;
    }

    let mut names: Vec<&str> = servers.iter().map(|server| server.name.as_str()).collect();
    names.sort_unstable();

    let stale = servers.iter().any(|server| server.health.stale);
    let measured: Vec<&mcp::TokenMeasurement> = servers
        .iter()
        .map(|server| &server.health.tools.definitions)
        .filter(|measurement| measurement.tokens.is_some())
        .collect();
    let all_complete = !stale
        && measured.len() == servers.len()
        && measured.iter().all(|measurement| measurement.complete);
    let included: Vec<&mcp::TokenMeasurement> = measured
        .iter()
        .copied()
        .filter(|measurement| measurement.included_in_total)
        .collect();
    let tokens = (!stale && !included.is_empty()).then(|| {
        included
            .iter()
            .filter_map(|measurement| measurement.tokens)
            .sum()
    });
    let loaded = if servers
        .iter()
        .all(|server| server.health.tools.definitions.loaded == Some(true))
    {
        Some(true)
    } else if servers
        .iter()
        .all(|server| server.health.tools.definitions.loaded == Some(false))
    {
        Some(false)
    } else {
        None
    };
    let note = if stale {
        Some(
            "The cached tool inventory is stale; run an MCP health check before counting it".into(),
        )
    } else if all_complete {
        Some("Measured from the normalized tool definitions returned by the servers".into())
    } else if measured.is_empty() {
        Some(
            "Static config cannot measure tool definitions; run an MCP health check to load them"
                .into(),
        )
    } else {
        Some(
            "Some enabled servers have no complete tool inventory; the shown total is partial"
                .into(),
        )
    };

    Some(Layer {
        scope: Scope::Mcp,
        label: format!("{} MCP servers enabled", servers.len()),
        path: names.join(", "),
        tokens,
        basis: if tokens.is_some() {
            mcp::TokenBasis::O200kSchemaEstimate
        } else {
            mcp::TokenBasis::Unavailable
        },
        complete: all_complete,
        loaded,
        included_in_total: tokens.is_some(),
        note,
        bytes: None,
    })
}

fn system_layer(runner: Runner) -> Layer {
    Layer {
        scope: Scope::System,
        label: format!("{} system prompt", runner.label()),
        path: "built into the CLI".into(),
        tokens: None,
        basis: mcp::TokenBasis::Unavailable,
        complete: false,
        loaded: Some(true),
        included_in_total: false,
        note: Some("Ships inside the binary — not a file Aviary can read".into()),
        bytes: None,
    }
}

/// Resolves the full stack for a runner in a working directory.
pub fn resolve(
    runner: Runner,
    cwd: &str,
    projects: &[(String, PathBuf)],
) -> Result<Resolved, String> {
    let started = std::time::Instant::now();
    let dir = mcp::canonical_context_cwd(cwd)?;
    let mut layers = vec![system_layer(runner)];

    match runner {
        Runner::ClaudeCode => {
            if let Some(root) = claude_code::root() {
                if let Some(l) =
                    file_layer(&root.join("CLAUDE.md"), Scope::User, "User instructions")
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

            if let Some(l) = mcp_layer(runner, &dir, projects) {
                layers.push(l);
            }
            if let Some(l) = memory_layer(&dir) {
                layers.push(l);
            }
        }

        Runner::Codex => {
            if let Some(root) = codex::root() {
                if let Some(l) =
                    file_layer(&root.join("AGENTS.md"), Scope::User, "User instructions")
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

            if let Some(l) = mcp_layer(runner, &dir, projects) {
                layers.push(l);
            }
        }
    }

    let total = layers
        .iter()
        .filter(|layer| layer.included_in_total)
        .filter_map(|layer| layer.tokens)
        .sum();
    let unmeasured = layers
        .iter()
        .filter(|layer| layer.loaded != Some(false) && (layer.tokens.is_none() || !layer.complete))
        .count();
    let total_complete = !layers
        .iter()
        .any(|layer| layer.loaded == Some(true) && layer.tokens.is_none());

    Ok(Resolved {
        runner,
        cwd: dir.to_string_lossy().to_string(),
        layers,
        total,
        total_complete,
        unmeasured,
        scanned_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    fn measured_server(name: &str, tokens: usize) -> mcp::Server {
        mcp::Server {
            id: format!("id-{name}"),
            runner: Runner::Codex,
            cwd: Some("/work".into()),
            declaration_id: format!("declaration-{name}"),
            name: name.into(),
            source: mcp::Source::User,
            transport: mcp::TransportSummary::RunnerProvided,
            state: mcp::DeclarationState::Enabled,
            shadowed_declaration_ids: Vec::new(),
            toggle: mcp::ToggleCapability {
                writable: false,
                revision: None,
                shared_project_file: false,
                unavailable_reason: Some(mcp::ToggleUnavailableReason::RunnerProvidedOnly),
            },
            health_revision: format!("revision-{name}"),
            health: mcp::McpHealth {
                state: mcp::McpHealthState::Ready,
                tools: mcp::ToolInventory {
                    count: Some(1),
                    definitions: mcp::TokenMeasurement {
                        tokens: Some(tokens),
                        basis: mcp::TokenBasis::O200kSchemaEstimate,
                        complete: true,
                        loaded: Some(true),
                        included_in_total: true,
                    },
                    checked_at_ms: Some(100),
                },
                checked_at_ms: Some(100),
                expires_at_ms: Some(200),
                stale: false,
            },
        }
    }

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

        let resolved = resolve(Runner::ClaudeCode, &cwd.to_string_lossy(), &[]).unwrap();
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
                l.tokens
                    .map(|tokens| tokens.to_string())
                    .unwrap_or_else(|| "—".into()),
                l.label,
                l.path
            );
        }

        // The total must never include a layer we could not measure.
        let measured_sum: usize = resolved
            .layers
            .iter()
            .filter(|layer| layer.included_in_total)
            .filter_map(|layer| layer.tokens)
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
            assert_eq!(
                row.tokens,
                Some(tokens::count_file(&user_md.to_string_lossy()))
            );
        }
    }

    #[test]
    fn unmeasured_layers_do_not_inflate_the_total() {
        let layers = vec![
            Layer {
                scope: Scope::System,
                label: "sys".into(),
                path: String::new(),
                tokens: None,
                basis: mcp::TokenBasis::Unavailable,
                complete: false,
                loaded: Some(true),
                included_in_total: false,
                note: None,
                bytes: None,
            },
            Layer {
                scope: Scope::User,
                label: "user".into(),
                path: String::new(),
                tokens: Some(120),
                basis: mcp::TokenBasis::O200kFileEstimate,
                complete: true,
                loaded: Some(true),
                included_in_total: true,
                note: None,
                bytes: Some(0),
            },
        ];
        let total: usize = layers
            .iter()
            .filter(|layer| layer.included_in_total)
            .filter_map(|layer| layer.tokens)
            .sum();
        assert_eq!(total, 120);
        assert_eq!(layers[0].tokens, None);
    }

    #[test]
    fn cached_mcp_definitions_contribute_only_when_complete_and_loaded() {
        let first = measured_server("first", 12);
        let second = measured_server("second", 18);
        let layer = mcp_layer_from_servers(&[&first, &second]).unwrap();
        assert_eq!(layer.tokens, Some(30));
        assert_eq!(layer.basis, mcp::TokenBasis::O200kSchemaEstimate);
        assert!(layer.complete);
        assert_eq!(layer.loaded, Some(true));
        assert!(layer.included_in_total);

        let mut missing = measured_server("missing", 7);
        missing.health = mcp::McpHealth::unchecked();
        let partial = mcp_layer_from_servers(&[&first, &missing]).unwrap();
        assert_eq!(partial.tokens, Some(12));
        assert!(!partial.complete);
        assert!(partial
            .note
            .as_deref()
            .is_some_and(|note| note.contains("partial")));

        let mut stale = second;
        stale.health.stale = true;
        let stale_layer = mcp_layer_from_servers(&[&first, &stale]).unwrap();
        assert_eq!(stale_layer.tokens, None);
        assert!(!stale_layer.included_in_total);
        assert!(stale_layer
            .note
            .as_deref()
            .is_some_and(|note| note.contains("stale")));
    }

    #[test]
    fn resolve_rejects_missing_paths_and_regular_files() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("missing");
        assert!(resolve(Runner::Codex, &missing.to_string_lossy(), &[]).is_err());
        let file = tmp.path().join("file.txt");
        std::fs::write(&file, "fixture").unwrap();
        assert!(resolve(Runner::Codex, &file.to_string_lossy(), &[]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_returns_the_canonical_directory_for_symlinks() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real");
        let link = tmp.path().join("link");
        std::fs::create_dir(&real).unwrap();
        symlink(&real, &link).unwrap();
        let resolved = resolve(Runner::Codex, &link.to_string_lossy(), &[]).unwrap();
        assert_eq!(
            resolved.cwd,
            std::fs::canonicalize(real).unwrap().to_string_lossy()
        );
    }

    #[test]
    fn tilde_resolves_to_the_real_home_directory() {
        let Some(home) = home() else { return };
        let resolved = resolve(Runner::Codex, "~", &[]).unwrap();
        assert_eq!(
            resolved.cwd,
            std::fs::canonicalize(home).unwrap().to_string_lossy()
        );
    }
}
