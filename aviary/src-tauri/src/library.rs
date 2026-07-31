//! Library assembly and the registered-project list.
//!
//! Scope is user-global plus explicitly registered projects — never an
//! auto-crawl of the home directory, which would be slow and would surface
//! repos the user does not care about.
//!
//! The project list itself lives in `store` (SQLite) rather than a JSON file;
//! `store::migrate_settings_json` lifts the old `settings.json` on first run.

use crate::providers::{self, Entry, Runner};
use crate::store::{self, Project};
use serde::{Deserialize, Serialize};

// Round-trips through the scan cache, so it must deserialise too.
#[derive(Debug, Serialize, Deserialize)]
pub struct LibrarySnapshot {
    pub entries: Vec<Entry>,
    pub projects: Vec<Project>,
    /// Which runners were actually found on this machine.
    pub runners: Vec<RunnerStatus>,
    pub scanned_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RunnerStatus {
    pub runner: Runner,
    pub label: String,
    pub root: String,
    pub detected: bool,
}

pub fn scan() -> LibrarySnapshot {
    let started = std::time::Instant::now();
    let projects = store::projects();

    let mut entries = Vec::new();
    entries.extend(providers::claude_code::scan_user());
    entries.extend(providers::codex::scan_user());

    for project in &projects {
        let dir = std::path::PathBuf::from(&project.path);
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
        projects,
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
        // A bare CI runner has no ~/.claude or ~/.codex. Skip rather than
        // fail: the point of this test is to exercise a real machine, and an
        // empty result there is correct, not broken.
        if snap.runners.iter().any(|r| r.detected) {
            assert!(!snap.entries.is_empty(), "a detected runner should yield entries");
        } else {
            eprintln!("skipped assertions: no runner installed on this machine");
        }
    }
}

#[cfg(test)]
mod read_tests {
    use super::*;

    #[test]
    fn reads_a_real_skill() {
        let snap = scan();
        let Some(skill) = snap
            .entries
            .iter()
            .find(|e| matches!(e.kind, crate::providers::Kind::Skill))
        else {
            eprintln!("skipped: no skills installed on this machine");
            return;
        };

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
