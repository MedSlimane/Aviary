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
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// Round-trips through the scan cache, so it must deserialise too.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibrarySnapshot {
    pub entries: Vec<Entry>,
    pub projects: Vec<Project>,
    /// Which runners were actually found on this machine.
    pub runners: Vec<RunnerStatus>,
    pub scanned_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerStatus {
    pub runner: Runner,
    pub label: String,
    pub root: String,
    pub detected: bool,
}

/// One independently refreshable provider/root pair.
///
/// Entries cannot be patched after global dedupe: a single canonical skill may
/// represent links from several roots. Retaining these fragments is what lets
/// a Claude-only change be rescanned without losing the Codex contribution.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LibraryScope {
    User {
        runner: Runner,
        root: PathBuf,
    },
    Project {
        runner: Runner,
        name: String,
        path: PathBuf,
    },
}

impl LibraryScope {
    pub fn runner(&self) -> Runner {
        match self {
            Self::User { runner, .. } | Self::Project { runner, .. } => *runner,
        }
    }

    /// `scan.kind` is free-form text, so scoped cache rows need no migration.
    pub fn cache_key(&self) -> String {
        let runner = match self.runner() {
            Runner::ClaudeCode => "claude-code",
            Runner::Codex => "codex",
        };
        match self {
            Self::User { .. } => format!("library:scope:{runner}:user"),
            Self::Project { path, .. } => {
                format!("library:scope:{runner}:project:{}", path.to_string_lossy())
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct LibraryPlan {
    home: Option<PathBuf>,
    projects: Vec<Project>,
    scopes: Vec<LibraryScope>,
}

impl LibraryPlan {
    pub fn current() -> Self {
        Self::new(providers::home(), store::projects())
    }

    pub fn for_home(home: PathBuf, projects: Vec<Project>) -> Self {
        Self::new(Some(home), projects)
    }

    fn new(home: Option<PathBuf>, projects: Vec<Project>) -> Self {
        let mut scopes = Vec::new();
        if let Some(home) = home.as_ref() {
            scopes.push(LibraryScope::User {
                runner: Runner::ClaudeCode,
                root: providers::claude_code::root_at(home),
            });
            scopes.push(LibraryScope::User {
                runner: Runner::Codex,
                root: providers::codex::root_at(home),
            });
        }
        for project in &projects {
            let path = PathBuf::from(&project.path);
            scopes.push(LibraryScope::Project {
                runner: Runner::ClaudeCode,
                name: project.name.clone(),
                path: path.clone(),
            });
            scopes.push(LibraryScope::Project {
                runner: Runner::Codex,
                name: project.name.clone(),
                path,
            });
        }
        Self {
            home,
            projects,
            scopes,
        }
    }

    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    pub fn projects(&self) -> &[Project] {
        &self.projects
    }

    pub fn scopes(&self) -> &[LibraryScope] {
        &self.scopes
    }
}

/// The provider output retained before cross-root dedupe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeSnapshot {
    pub entries: Vec<Entry>,
    pub scanned_ms: u64,
}

pub type ScopeSnapshots = BTreeMap<LibraryScope, ScopeSnapshot>;

pub fn scan_scope(scope: &LibraryScope) -> ScopeSnapshot {
    let started = std::time::Instant::now();
    let entries = match scope {
        LibraryScope::User {
            runner: Runner::ClaudeCode,
            root,
        } => providers::claude_code::scan_user_at(root),
        LibraryScope::User {
            runner: Runner::Codex,
            root,
        } => providers::codex::scan_user_at(root),
        LibraryScope::Project {
            runner: Runner::ClaudeCode,
            name,
            path,
        } => providers::claude_code::scan_project(path, name),
        LibraryScope::Project {
            runner: Runner::Codex,
            name,
            path,
        } => providers::codex::scan_project(path, name),
    };
    ScopeSnapshot {
        entries,
        scanned_ms: started.elapsed().as_millis() as u64,
    }
}

pub fn scan_plan(plan: &LibraryPlan) -> (LibrarySnapshot, ScopeSnapshots) {
    let started = std::time::Instant::now();
    let fragments = plan
        .scopes()
        .iter()
        .cloned()
        .map(|scope| {
            let snapshot = scan_scope(&scope);
            (scope, snapshot)
        })
        .collect();
    let snapshot = assemble(plan, &fragments, started.elapsed().as_millis() as u64);
    (snapshot, fragments)
}

pub fn assemble(
    plan: &LibraryPlan,
    fragments: &ScopeSnapshots,
    scanned_ms: u64,
) -> LibrarySnapshot {
    let entries = providers::dedupe(
        fragments
            .values()
            .flat_map(|fragment| fragment.entries.iter().cloned())
            .collect(),
    );
    let runners = [Runner::ClaudeCode, Runner::Codex]
        .into_iter()
        .map(|runner| {
            let root = plan
                .scopes()
                .iter()
                .find_map(|scope| match scope {
                    LibraryScope::User {
                        runner: candidate,
                        root,
                    } if *candidate == runner => Some(root.clone()),
                    _ => None,
                })
                .unwrap_or_default();
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
        projects: plan.projects().to_vec(),
        runners,
        scanned_ms,
    }
}

pub fn scan() -> LibrarySnapshot {
    scan_plan(&LibraryPlan::current()).0
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
        Ok(p) => (
            p.matter.is_empty().then(|| None).unwrap_or(Some(p.matter)),
            p.content,
        ),
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
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_home(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "aviary-library-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn scans_real_machine() {
        let snap = scan();
        eprintln!("scanned in {}ms", snap.scanned_ms);
        for r in &snap.runners {
            eprintln!(
                "runner {:<14} detected={} root={}",
                r.label, r.detected, r.root
            );
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
            assert!(
                !snap.entries.is_empty(),
                "a detected runner should yield entries"
            );
        } else {
            eprintln!("skipped assertions: no runner installed on this machine");
        }
    }

    #[cfg(unix)]
    #[test]
    fn targeted_refresh_preserves_the_other_symlinked_runner() {
        use std::os::unix::fs::symlink;

        let home = temp_home("scope-dedupe");
        let shared = home.join(".agents/skills/demo");
        fs::create_dir_all(&shared).unwrap();
        fs::write(
            shared.join("SKILL.md"),
            "---\nname: Demo\ndescription: shared\n---\nbody\n",
        )
        .unwrap();
        let claude_skills = home.join(".claude/skills");
        let codex_skills = home.join(".codex/skills");
        fs::create_dir_all(&claude_skills).unwrap();
        fs::create_dir_all(&codex_skills).unwrap();
        symlink(&shared, claude_skills.join("demo")).unwrap();
        symlink(&shared, codex_skills.join("demo")).unwrap();

        let plan = LibraryPlan::for_home(home.clone(), vec![]);
        let (initial, mut fragments) = scan_plan(&plan);
        assert_eq!(initial.entries.len(), 1);
        assert_eq!(initial.entries[0].runners.len(), 2);

        fs::remove_file(claude_skills.join("demo")).unwrap();
        let claude = plan
            .scopes()
            .iter()
            .find(|scope| {
                matches!(
                    scope,
                    LibraryScope::User {
                        runner: Runner::ClaudeCode,
                        ..
                    }
                )
            })
            .unwrap()
            .clone();
        fragments.insert(claude.clone(), scan_scope(&claude));
        let refreshed = assemble(&plan, &fragments, 0);

        assert_eq!(refreshed.entries.len(), 1);
        assert_eq!(refreshed.entries[0].runners, vec![Runner::Codex]);
        fs::remove_dir_all(home).unwrap();
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
