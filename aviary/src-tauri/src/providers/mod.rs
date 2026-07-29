//! Discovery of agent configuration across runners.
//!
//! Files are the source of truth. Nothing here caches or mutates — a provider
//! walks a runner's known locations and reports what it finds.
//!
//! A finding that shapes this module: skills are not owned by a runner. They
//! live in a shared pool (`~/.agents/skills`) and are *symlinked* into each
//! runner's skills directory. So "enabled for Claude Code" is not metadata —
//! it is the presence of a symlink. Entries are therefore deduplicated by
//! their canonical path, and the runners that link to them are collected into
//! `runners`.

pub mod claude_code;
pub mod codex;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Runner {
    ClaudeCode,
    Codex,
}

impl Runner {
    pub fn label(&self) -> &'static str {
        match self {
            Runner::ClaudeCode => "Claude Code",
            Runner::Codex => "Codex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Skill,
    Agent,
    Command,
    Prompt,
    Memory,
}

/// Where an entry came from, which determines whether it is editable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// Lives in the user's own directories — editable.
    User,
    /// Installed by a plugin — read-only, and often duplicated per version.
    Plugin,
    /// Belongs to a registered project.
    Project,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Stable identity: the canonical (symlink-resolved) path.
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: Kind,
    pub source: Source,
    /// Every runner that links to this entry.
    pub runners: Vec<Runner>,
    /// The path as the runner sees it (may be a symlink).
    pub path: String,
    /// The resolved path on disk.
    pub real_path: String,
    /// Project name, when `source` is `Project`.
    pub project: Option<String>,
    /// Remaining frontmatter keys, as strings.
    pub meta: BTreeMap<String, String>,
    pub bytes: u64,
    /// Unix seconds.
    pub modified: u64,
}

/// Parsed frontmatter from a markdown file.
pub struct Parsed {
    pub name: Option<String>,
    pub description: Option<String>,
    pub meta: BTreeMap<String, String>,
}

/// Reads YAML frontmatter from a markdown file.
///
/// Falls back to the filename for `name` and an empty description, so a file
/// with malformed or absent frontmatter still surfaces rather than vanishing.
pub fn parse_markdown(path: &Path) -> Parsed {
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    let mut meta = BTreeMap::new();
    let mut name = None;
    let mut description = None;

    let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
    if let Ok(parsed) = matter.parse::<serde_json::Value>(&raw) {
        if let Some(serde_json::Value::Object(map)) = parsed.data {
            for (k, v) in map {
                let value = match v {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                match k.as_str() {
                    "name" => name = Some(value),
                    "description" => description = Some(value),
                    _ => {
                        meta.insert(k, value);
                    }
                }
            }
        }
    }

    Parsed {
        name,
        description,
        meta,
    }
}

/// Builds an entry from a markdown file, resolving symlinks for identity.
pub fn entry_from_file(
    path: &Path,
    kind: Kind,
    source: Source,
    runner: Runner,
    project: Option<String>,
) -> Option<Entry> {
    let meta_fs = std::fs::metadata(path).ok()?;
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let parsed = parse_markdown(path);

    // A skill directory is named for the skill; fall back to it, then to the
    // file stem, so entries are never nameless.
    let fallback = if path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
        path.parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("untitled")
            .to_string()
    } else {
        path.file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled")
            .to_string()
    };

    Some(Entry {
        id: real.to_string_lossy().to_string(),
        name: parsed.name.unwrap_or(fallback),
        description: parsed.description.unwrap_or_default(),
        kind,
        source,
        runners: vec![runner],
        path: path.to_string_lossy().to_string(),
        real_path: real.to_string_lossy().to_string(),
        project,
        meta: parsed.meta,
        bytes: meta_fs.len(),
        modified: meta_fs
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0),
    })
}

/// Scans a directory of skills, where each child is a directory holding a
/// `SKILL.md`. Symlinked children are followed.
pub fn scan_skill_dir(
    dir: &Path,
    source: Source,
    runner: Runner,
    project: Option<String>,
) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    read.filter_map(Result::ok)
        .filter_map(|child| {
            let skill_md = child.path().join("SKILL.md");
            skill_md
                .is_file()
                .then(|| entry_from_file(&skill_md, Kind::Skill, source, runner, project.clone()))
                .flatten()
        })
        .collect()
}

/// Scans a flat directory of markdown files (agents, commands, prompts).
pub fn scan_flat_dir(
    dir: &Path,
    kind: Kind,
    source: Source,
    runner: Runner,
    project: Option<String>,
) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    read.filter_map(Result::ok)
        .filter(|c| c.path().extension().and_then(|e| e.to_str()) == Some("md"))
        .filter_map(|c| entry_from_file(&c.path(), kind, source, runner, project.clone()))
        .collect()
}

/// Merges entries that resolve to the same file, unioning their runners.
///
/// This is what turns "the same skill symlinked into two runners" into a
/// single row that reports both.
pub fn dedupe(entries: Vec<Entry>) -> Vec<Entry> {
    let mut by_id: BTreeMap<String, Entry> = BTreeMap::new();

    for entry in entries {
        by_id
            .entry(entry.id.clone())
            .and_modify(|existing| {
                for r in &entry.runners {
                    if !existing.runners.contains(r) {
                        existing.runners.push(*r);
                    }
                }
                existing.runners.sort();
                // Prefer a user-owned path over a plugin copy.
                if entry.source == Source::User && existing.source != Source::User {
                    existing.source = Source::User;
                    existing.path = entry.path.clone();
                }
            })
            .or_insert(entry);
    }

    let mut out: Vec<Entry> = by_id.into_values().collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

pub fn home() -> Option<PathBuf> {
    dirs::home_dir()
}
