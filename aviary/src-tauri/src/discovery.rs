//! Lightweight project discovery.
//!
//! Finds directories that carry agent configuration and *offers* them. It does
//! not index anything — registration stays opt-in, so a scan can never quietly
//! pull hundreds of repos into the library.
//!
//! Kept cheap on purpose: a bounded-depth walk over a few likely roots, with
//! aggressive pruning of directories that are large and never interesting.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Files and directories that mark a project as agent-configured.
const MARKERS: &[(&str, &str, bool)] = &[
    // (path, label, is_dir)
    (".claude", "Claude Code", true),
    ("CLAUDE.md", "Claude Code", false),
    (".codex", "Codex", true),
    ("AGENTS.md", "Codex", false),
    (".cursor", "Cursor", true),
    (".cursorrules", "Cursor", false),
    ("GEMINI.md", "Gemini", false),
    (".windsurfrules", "Windsurf", false),
    (".github/copilot-instructions.md", "Copilot", false),
];

/// Never worth descending into — big, and never a project root themselves.
const PRUNE: &[&str] = &[
    "node_modules", "target", "dist", "build", "out", ".next", ".nuxt",
    "vendor", "venv", ".venv", "__pycache__", ".cache", "Library",
    "Applications", ".Trash", ".git", ".svn", "Pods", "DerivedData",
    ".gradle", ".cargo", ".rustup", ".npm", ".bun", "go", "Music",
    "Movies", "Pictures", "Photos Library.photoslibrary",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub name: String,
    pub path: String,
    /// Runners this project appears configured for, e.g. ["Claude Code"].
    pub runners: Vec<String>,
    /// The marker files found, for display.
    pub markers: Vec<String>,
    /// Already in the registered list.
    pub registered: bool,
}

#[derive(Debug, Serialize)]
pub struct DiscoveryResult {
    pub candidates: Vec<Candidate>,
    pub scanned_ms: u64,
    pub roots: Vec<String>,
}

fn markers_in(dir: &Path) -> (Vec<String>, Vec<String>) {
    let mut markers = Vec::new();
    let mut runners = BTreeSet::new();

    for (rel, runner, is_dir) in MARKERS {
        let p = dir.join(rel);
        let hit = if *is_dir { p.is_dir() } else { p.is_file() };
        if hit {
            markers.push((*rel).to_string());
            runners.insert((*runner).to_string());
        }
    }
    (markers, runners.into_iter().collect())
}

/// Roots worth looking under: the home directory itself plus the usual
/// places people keep code.
fn roots() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut out = vec![home.clone()];
    for name in [
        "Projects", "projects", "work", "Work", "dev", "Developer", "Code",
        "code", "src", "repos", "Repos", "Documents/GitHub", "git",
    ] {
        let p = home.join(name);
        if p.is_dir() {
            out.push(p);
        }
    }
    out
}

/// Scans for agent-configured projects.
///
/// `registered` marks candidates already in settings so the UI can separate
/// "add this" from "already tracked".
pub fn discover(registered: &[String]) -> DiscoveryResult {
    let started = std::time::Instant::now();
    let roots = roots();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut candidates = Vec::new();

    for root in &roots {
        // Depth 3 from each root catches ~/work/client/repo without walking
        // an entire home directory.
        let walker = walkdir::WalkDir::new(root)
            .max_depth(3)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                if e.depth() == 0 {
                    return true;
                }
                let name = e.file_name().to_string_lossy();
                if PRUNE.contains(&name.as_ref()) {
                    return false;
                }
                // Hidden directories are rarely project roots; the markers we
                // care about live *inside* a visible project.
                if name.starts_with('.') && e.file_type().is_dir() {
                    return false;
                }
                e.file_type().is_dir()
            });

        for entry in walker.filter_map(Result::ok) {
            let dir = entry.path();
            if !dir.is_dir() || seen.contains(dir) {
                continue;
            }
            // The home directory holds ~/.claude and ~/.codex, which are the
            // user scope — not a project. It would otherwise always match.
            if dirs::home_dir().is_some_and(|h| h == dir) {
                continue;
            }
            let (markers, runners) = markers_in(dir);
            if markers.is_empty() {
                continue;
            }
            seen.insert(dir.to_path_buf());

            let path = dir.to_string_lossy().to_string();
            candidates.push(Candidate {
                name: dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("project")
                    .to_string(),
                registered: registered.iter().any(|r| r == &path),
                path,
                runners,
                markers,
            });
        }
    }

    // Richest configuration first — those are the projects worth adding.
    candidates.sort_by(|a, b| {
        b.markers
            .len()
            .cmp(&a.markers.len())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    DiscoveryResult {
        candidates,
        scanned_ms: started.elapsed().as_millis() as u64,
        roots: roots.iter().map(|r| r.to_string_lossy().to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_on_real_machine() {
        let r = discover(&[]);
        eprintln!("scanned {} roots in {}ms", r.roots.len(), r.scanned_ms);
        eprintln!("candidates: {}", r.candidates.len());
        for c in r.candidates.iter().take(12) {
            eprintln!("  {:<28} {:?} {:?}", c.name, c.runners, c.markers);
        }
    }
}
