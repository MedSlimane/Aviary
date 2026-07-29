//! Library assembly and the registered-project list.
//!
//! Scope is user-global plus explicitly registered projects — never an
//! auto-crawl of the home directory, which would be slow and would surface
//! repos the user does not care about.

use crate::providers::{self, Entry, Runner};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub projects: Vec<Project>,
}

pub fn config_dir() -> Option<PathBuf> {
    providers::home().map(|h| h.join(".aviary"))
}

fn settings_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.json"))
}

pub fn load_settings() -> Settings {
    let Some(path) = settings_path() else {
        return Settings::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save_settings(settings: &Settings) -> Result<(), String> {
    let dir = config_dir().ok_or("no home directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("settings.json"), json).map_err(|e| e.to_string())
}

#[derive(Debug, Serialize)]
pub struct LibrarySnapshot {
    pub entries: Vec<Entry>,
    pub projects: Vec<Project>,
    /// Which runners were actually found on this machine.
    pub runners: Vec<RunnerStatus>,
    pub scanned_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct RunnerStatus {
    pub runner: Runner,
    pub label: String,
    pub root: String,
    pub detected: bool,
}

pub fn scan() -> LibrarySnapshot {
    let started = std::time::Instant::now();
    let settings = load_settings();

    let mut entries = Vec::new();
    entries.extend(providers::claude_code::scan_user());
    entries.extend(providers::codex::scan_user());

    for project in &settings.projects {
        let dir = PathBuf::from(&project.path);
        if !dir.is_dir() {
            continue;
        }
        entries.extend(providers::claude_code::scan_project(&dir, &project.name));
        entries.extend(providers::codex::scan_project(&dir, &project.name));
    }

    let entries = providers::dedupe(entries);

    let runners = [
        (
            Runner::ClaudeCode,
            providers::claude_code::root(),
        ),
        (Runner::Codex, providers::codex::root()),
    ]
    .into_iter()
    .map(|(runner, root)| {
        let root = root.unwrap_or_default();
        RunnerStatus {
            runner,
            label: runner.label().to_string(),
            detected: root.is_dir(),
            root: root.to_string_lossy().to_string(),
        }
    })
    .collect();

    LibrarySnapshot {
        entries,
        projects: settings.projects,
        runners,
        scanned_ms: started.elapsed().as_millis() as u64,
    }
}

#[derive(Debug, Serialize)]
pub struct EntryContent {
    pub raw: String,
    /// Body with the frontmatter block removed.
    pub body: String,
    /// The raw frontmatter block, if present.
    pub frontmatter: Option<String>,
    /// Real token count via tiktoken, not a byte heuristic.
    pub tokens: usize,
    /// Hash of `raw` at read time. The editor sends this back on save so a
    /// change made behind its back can be detected.
    pub hash: String,
}

/// Reads an entry's content, splitting frontmatter off the body.
///
/// Parsing happens here rather than in the UI because the providers already
/// depend on gray_matter — no reason to ship a second parser to the client.
pub fn read_entry(path: &str) -> Result<EntryContent, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;

    let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
    let (frontmatter, body) = match matter.parse::<serde_json::Value>(&raw) {
        Ok(p) => (p.matter.is_empty().then(|| None).unwrap_or(Some(p.matter)), p.content),
        Err(_) => (None, raw.clone()),
    };

    Ok(EntryContent {
        tokens: crate::tokens::count(&raw),
        hash: crate::writer::hash(&raw),
        raw,
        body,
        frontmatter,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_real_machine() {
        let snap = scan();
        eprintln!("scanned in {}ms", snap.scanned_ms);
        for r in &snap.runners {
            eprintln!("runner {:<14} detected={} root={}", r.label, r.detected, r.root);
        }
        eprintln!("entries: {}", snap.entries.len());

        let both = snap.entries.iter().filter(|e| e.runners.len() == 2).count();
        eprintln!("shared across both runners: {both}");

        for e in snap.entries.iter().take(8) {
            eprintln!(
                "  [{:?}/{:?}] {:<28} runners={:?} desc={:.60}",
                e.kind, e.source, e.name, e.runners, e.description
            );
        }
        assert!(!snap.entries.is_empty(), "expected to find real entries");
    }
}

#[cfg(test)]
mod read_tests {
    use super::*;

    #[test]
    fn reads_a_real_skill() {
        let snap = scan();
        let skill = snap
            .entries
            .iter()
            .find(|e| matches!(e.kind, crate::providers::Kind::Skill))
            .expect("expected at least one skill");

        let c = read_entry(&skill.path).expect("should read");
        eprintln!("entry:       {}", skill.name);
        eprintln!("bytes:       {}", skill.bytes);
        eprintln!("tokens:      {}", c.tokens);
        eprintln!("frontmatter: {}", c.frontmatter.is_some());
        eprintln!("body chars:  {}", c.body.len());
        assert!(c.tokens > 0, "tokeniser returned nothing");
        assert!(!c.body.is_empty(), "body was empty");
        assert!(!c.body.starts_with("---"), "frontmatter not split off");
    }
}
