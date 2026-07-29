//! Claude Code provider.
//!
//! Known locations (user scope):
//!   ~/.claude/skills/<name>/SKILL.md    often a symlink into ~/.agents/skills
//!   ~/.claude/agents/*.md
//!   ~/.claude/commands/*.md
//!   ~/.claude/CLAUDE.md                 user memory
//!   ~/.claude/plugins/**/skills/*/SKILL.md
//!
//! Project scope (registered projects only):
//!   <project>/.claude/skills|agents|commands
//!   <project>/CLAUDE.md, <project>/.claude/CLAUDE.local.md

use super::*;

pub const RUNNER: Runner = Runner::ClaudeCode;

pub fn root() -> Option<PathBuf> {
    home().map(|h| h.join(".claude"))
}

pub fn scan_user() -> Vec<Entry> {
    let Some(root) = root() else {
        return Vec::new();
    };
    let mut out = Vec::new();

    out.extend(scan_skill_dir(
        &root.join("skills"),
        Source::User,
        RUNNER,
        None,
    ));
    out.extend(scan_flat_dir(
        &root.join("agents"),
        Kind::Agent,
        Source::User,
        RUNNER,
        None,
    ));
    out.extend(scan_flat_dir(
        &root.join("commands"),
        Kind::Command,
        Source::User,
        RUNNER,
        None,
    ));

    let memory = root.join("CLAUDE.md");
    if memory.is_file() {
        if let Some(mut e) = entry_from_file(&memory, Kind::Memory, Source::User, RUNNER, None) {
            if e.name == "CLAUDE" {
                e.name = "CLAUDE.md".into();
            }
            if e.description.is_empty() {
                e.description = "User-scope instructions, loaded in every session".into();
            }
            out.push(e);
        }
    }

    out.extend(scan_plugins(&root.join("plugins")));
    out
}

/// Plugin skills are versioned by content hash, so the same skill appears many
/// times under different cache directories. Only the newest copy of each name
/// is kept — otherwise the library is drowned in duplicates.
fn scan_plugins(plugins_root: &Path) -> Vec<Entry> {
    if !plugins_root.is_dir() {
        return Vec::new();
    }

    let mut newest: BTreeMap<String, Entry> = BTreeMap::new();

    for found in walkdir::WalkDir::new(plugins_root)
        .max_depth(8)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if found.file_name() != "SKILL.md" {
            continue;
        }
        let Some(entry) = entry_from_file(found.path(), Kind::Skill, Source::Plugin, RUNNER, None)
        else {
            continue;
        };

        newest
            .entry(entry.name.clone())
            .and_modify(|existing| {
                if entry.modified > existing.modified {
                    *existing = entry.clone();
                }
            })
            .or_insert(entry);
    }

    newest.into_values().collect()
}

pub fn scan_project(dir: &Path, project: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let dot = dir.join(".claude");

    out.extend(scan_skill_dir(
        &dot.join("skills"),
        Source::Project,
        RUNNER,
        Some(project.to_string()),
    ));
    out.extend(scan_flat_dir(
        &dot.join("agents"),
        Kind::Agent,
        Source::Project,
        RUNNER,
        Some(project.to_string()),
    ));
    out.extend(scan_flat_dir(
        &dot.join("commands"),
        Kind::Command,
        Source::Project,
        RUNNER,
        Some(project.to_string()),
    ));

    for (path, label) in [
        (dir.join("CLAUDE.md"), "Project instructions"),
        (dot.join("CLAUDE.local.md"), "Local overrides (gitignored)"),
    ] {
        if path.is_file() {
            if let Some(mut e) = entry_from_file(
                &path,
                Kind::Memory,
                Source::Project,
                RUNNER,
                Some(project.to_string()),
            ) {
                e.name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("CLAUDE.md")
                    .to_string();
                if e.description.is_empty() {
                    e.description = label.into();
                }
                out.push(e);
            }
        }
    }

    out
}
