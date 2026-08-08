//! Codex provider.
//!
//! Known locations (user scope):
//!   ~/.codex/skills/<name>/SKILL.md     often a symlink into ~/.agents/skills
//!   ~/.codex/prompts/*.md
//!   ~/.codex/AGENTS.md                  user memory
//!
//! Project scope (registered projects only):
//!   <project>/AGENTS.md

use super::*;

pub const RUNNER: Runner = Runner::Codex;

pub fn root_at(home: &Path) -> PathBuf {
    home.join(".codex")
}

pub fn root() -> Option<PathBuf> {
    home().map(|h| root_at(&h))
}

pub fn scan_user() -> Vec<Entry> {
    let Some(root) = root() else {
        return Vec::new();
    };
    scan_user_at(&root)
}

/// Path-injectable counterpart to `scan_user`, used by the live index and its
/// real-filesystem tests.
pub fn scan_user_at(root: &Path) -> Vec<Entry> {
    let mut out = Vec::new();

    out.extend(scan_skill_dir(
        &root.join("skills"),
        Source::User,
        RUNNER,
        None,
    ));
    out.extend(scan_flat_dir(
        &root.join("prompts"),
        Kind::Prompt,
        Source::User,
        RUNNER,
        None,
    ));

    let memory = root.join("AGENTS.md");
    if memory.is_file() {
        if let Some(mut e) = entry_from_file(&memory, Kind::Memory, Source::User, RUNNER, None) {
            e.name = "AGENTS.md".into();
            if e.description.is_empty() {
                e.description = "User-scope instructions for Codex".into();
            }
            out.push(e);
        }
    }

    out
}

pub fn user_recursive_roots(root: &Path) -> Vec<PathBuf> {
    ["skills", "prompts"]
        .into_iter()
        .map(|name| root.join(name))
        .collect()
}

pub fn project_recursive_roots(project: &Path) -> Vec<PathBuf> {
    vec![project.join(".codex").join("skills")]
}

pub fn affects_user(root: &Path, path: &Path) -> bool {
    path == root
        || path == root.join("AGENTS.md")
        || user_recursive_roots(root)
            .into_iter()
            .any(|candidate| path.starts_with(candidate))
}

pub fn affects_project(project: &Path, path: &Path) -> bool {
    path == project
        || path == project.join(".codex")
        || path == project.join("AGENTS.md")
        || project_recursive_roots(project)
            .into_iter()
            .any(|candidate| path.starts_with(candidate))
}

pub fn scan_project(dir: &Path, project: &str) -> Vec<Entry> {
    let mut out = Vec::new();

    out.extend(scan_skill_dir(
        &dir.join(".codex").join("skills"),
        Source::Project,
        RUNNER,
        Some(project.to_string()),
    ));

    let agents = dir.join("AGENTS.md");
    if agents.is_file() {
        if let Some(mut e) = entry_from_file(
            &agents,
            Kind::Memory,
            Source::Project,
            RUNNER,
            Some(project.to_string()),
        ) {
            e.name = "AGENTS.md".into();
            if e.description.is_empty() {
                e.description = "Project instructions for Codex".into();
            }
            out.push(e);
        }
    }

    out
}
