//! Model catalogue, discovered rather than hardcoded.
//!
//! A baked-in list goes stale the day a model ships — it shipped wrong here,
//! missing a model that already existed. Two mechanisms avoid that:
//!
//! * **Claude Code takes aliases.** `opus`, `sonnet`, `haiku` and `fable`
//!   always resolve to the newest model in that family, so a new Opus is
//!   reachable the day it lands with no change here. Aliases are offered
//!   first for exactly that reason.
//! * **Codex publishes a live cache.** `~/.codex/models_cache.json` is written
//!   by the CLI itself and carries slugs, display names and descriptions.
//!
//! Anything not listed can still be typed in, so an unknown id is never a
//! wall. And the session's own `init` event reports the model that actually
//! ran, which is the only fully trustworthy answer.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ReasoningLevel {
    pub effort: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelOption {
    /// Passed to `--model` / `-m`. `None` means send no flag at all.
    pub id: Option<String>,
    pub label: String,
    pub note: String,
    /// Aliases track the newest model in a family, so they never go stale.
    pub is_alias: bool,
    /// Effort levels this model accepts, lowest first. Empty when the runner
    /// does not expose the notion.
    pub reasoning_levels: Vec<ReasoningLevel>,
    /// The level used when none is chosen.
    pub default_effort: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ModelCatalogue {
    pub models: Vec<ModelOption>,
    /// What the runner is configured to use when no flag is passed.
    pub configured_default: Option<String>,
    /// Where the list came from, so the UI can be honest about staleness.
    pub source: String,
}

/// Candidate effort levels that exist but are absent from `--help`.
///
/// The help text is not a complete list — `ultracode` is accepted and works,
/// yet the printed "Valid values" line omits it. Each candidate is probed
/// against the CLI rather than assumed, so a wrong guess here can only fail
/// closed.
const UNDOCUMENTED_EFFORT_CANDIDATES: &[&str] = &["ultracode", "ultrathink", "ultra"];

/// Asks the CLI whether an effort value is real.
///
/// `claude --effort X --help` runs the same validation as a real invocation
/// but exits at argument parsing, so this costs a process spawn and no turn.
/// An unknown value prints "Unknown --effort value"; a valid one prints
/// nothing.
fn claude_effort_is_valid(effort: &str) -> bool {
    std::process::Command::new("claude")
        .arg("--effort")
        .arg(effort)
        .arg("--help")
        .output()
        .map(|o| {
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            !combined.contains("Unknown --effort value")
        })
        .unwrap_or(false)
}

/// Reads the effort levels out of `claude --help`, then probes for levels the
/// help text does not list.
///
/// Parsed and probed rather than hardcoded for the same reason the model list
/// is: the set changes, and the CLI is the only thing that knows the current
/// one. Cached because spawning the binary is not free.
fn claude_effort_levels() -> Vec<ReasoningLevel> {
    static LEVELS: std::sync::OnceLock<Vec<ReasoningLevel>> = std::sync::OnceLock::new();
    LEVELS
        .get_or_init(|| {
            let help = std::process::Command::new("claude")
                .arg("--help")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default();

            // "--effort <level>  Effort level for the current session (low, medium, high, xhigh, max)"
            let found = help
                .split("--effort")
                .nth(1)
                .and_then(|tail| {
                    let open = tail.find('(')?;
                    let close = tail[open..].find(')')? + open;
                    Some(tail[open + 1..close].to_string())
                })
                .map(|inner| {
                    inner
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty() && !s.contains(' '))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let descriptions = |e: &str| match e {
                "low" => "Fast responses with lighter reasoning",
                "medium" => "Balances speed and depth for everyday tasks",
                "high" => "Greater depth for complex problems",
                "xhigh" => "Extra depth for complex problems",
                "max" => "Maximum depth for the hardest problems",
                _ => "",
            };

            let mut levels: Vec<ReasoningLevel> = found
                .iter()
                .map(|e| ReasoningLevel {
                    description: descriptions(e).to_string(),
                    effort: e.clone(),
                })
                .collect();

            // Undocumented levels sit above the documented ones in effort.
            for candidate in UNDOCUMENTED_EFFORT_CANDIDATES {
                if found.iter().any(|f| f == candidate) {
                    continue;
                }
                if claude_effort_is_valid(candidate) {
                    levels.push(ReasoningLevel {
                        effort: (*candidate).to_string(),
                        description: match *candidate {
                            "ultracode" => "Maximum depth, tuned for code",
                            "ultrathink" => "Maximum depth, extended thinking",
                            _ => "Maximum reasoning depth",
                        }
                        .to_string(),
                    });
                }
            }

            levels
        })
        .clone()
}

fn claude_effort_default() -> Option<String> {
    let path = crate::providers::home()?.join(".claude/settings.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("effortLevel")?.as_str().map(String::from)
}

fn claude_configured_default() -> Option<String> {
    let path = crate::providers::home()?.join(".claude/settings.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("model")?.as_str().map(String::from)
}

fn claude_catalogue() -> ModelCatalogue {
    let configured = claude_configured_default();

    // Aliases only. Pinning specific version strings here is what caused the
    // list to be wrong in the first place.
    let aliases = [
        ("opus", "Opus", "Deepest reasoning — always the newest Opus"),
        ("sonnet", "Sonnet", "Balanced speed and depth"),
        ("haiku", "Haiku", "Fastest and cheapest"),
        ("fable", "Fable", "Newest frontier family"),
    ];

    let levels = claude_effort_levels();
    let effort_default = claude_effort_default();

    let mut models = vec![ModelOption {
        id: None,
        label: "Default".into(),
        note: configured
            .as_deref()
            .map(|m| format!("Your CLI setting — currently {m}"))
            .unwrap_or_else(|| "Whatever your CLI is set to".into()),
        is_alias: false,
        reasoning_levels: levels.clone(),
        default_effort: effort_default.clone(),
    }];

    models.extend(aliases.iter().map(|(id, label, note)| ModelOption {
        id: Some((*id).to_string()),
        label: (*label).to_string(),
        note: (*note).to_string(),
        is_alias: true,
        reasoning_levels: levels.clone(),
        default_effort: effort_default.clone(),
    }));

    ModelCatalogue {
        models,
        configured_default: configured,
        source: "aliases (always resolve to the latest)".into(),
    }
}

fn codex_catalogue() -> ModelCatalogue {
    let configured = crate::providers::home()
        .map(|h| h.join(".codex/config.toml"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|raw| raw.parse::<toml::Table>().ok())
        .and_then(|t| t.get("model").and_then(|m| m.as_str()).map(String::from));

    let mut models = vec![ModelOption {
        id: None,
        label: "Default".into(),
        note: configured
            .as_deref()
            .map(|m| format!("Your config.toml setting — currently {m}"))
            .unwrap_or_else(|| "Whatever your config is set to".into()),
        is_alias: false,
        reasoning_levels: Vec::new(),
        default_effort: None,
    }];

    // The CLI maintains this itself, so it stays current without our help.
    let cache = crate::providers::home()
        .map(|h| h.join(".codex/models_cache.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());

    let mut source = "no cache found".to_string();
    if let Some(cache) = cache {
        if let Some(list) = cache.get("models").and_then(|m| m.as_array()) {
            source = cache
                .get("fetched_at")
                .and_then(|f| f.as_str())
                .map(|f| format!("~/.codex/models_cache.json · fetched {f}"))
                .unwrap_or_else(|| "~/.codex/models_cache.json".into());

            models.extend(list.iter().filter_map(|m| {
                let slug = m.get("slug")?.as_str()?.to_string();
                Some(ModelOption {
                    label: m
                        .get("display_name")
                        .and_then(|d| d.as_str())
                        .unwrap_or(&slug)
                        .to_string(),
                    note: m
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    reasoning_levels: m
                        .get("supported_reasoning_levels")
                        .and_then(|l| l.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|lv| {
                                    Some(ReasoningLevel {
                                        effort: lv.get("effort")?.as_str()?.to_string(),
                                        description: lv
                                            .get("description")
                                            .and_then(|d| d.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    default_effort: m
                        .get("default_reasoning_level")
                        .and_then(|d| d.as_str())
                        .map(String::from),
                    id: Some(slug),
                    is_alias: false,
                })
            }));
        }
    }

    ModelCatalogue {
        models,
        configured_default: configured,
        source,
    }
}

pub fn catalogue(runner: crate::runner::Runner) -> ModelCatalogue {
    match runner {
        crate::runner::Runner::ClaudeCode => claude_catalogue(),
        crate::runner::Runner::Codex => codex_catalogue(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::Runner;

    #[test]
    fn catalogues_come_from_the_machine() {
        let c = catalogue(Runner::ClaudeCode);
        eprintln!("claude source: {}", c.source);
        eprintln!("claude default: {:?}", c.configured_default);
        for m in &c.models {
            eprintln!("  {:<10} alias={} {}", m.label, m.is_alias, m.note);
        }
        assert!(c.models.iter().any(|m| m.id.as_deref() == Some("opus")));

        let x = catalogue(Runner::Codex);
        eprintln!("codex source: {}", x.source);
        for m in x.models.iter().take(6) {
            eprintln!("  {:<18} {:?}", m.label, m.id);
        }
        assert!(x.models.len() > 1, "expected models from the live cache");

        let sol = x.models.iter().find(|m| m.id.as_deref() == Some("gpt-5.6-sol"));
        if let Some(sol) = sol {
            eprintln!(
                "  sol levels: {:?} default={:?}",
                sol.reasoning_levels.iter().map(|l| &l.effort).collect::<Vec<_>>(),
                sol.default_effort
            );
            assert!(!sol.reasoning_levels.is_empty());
        }

        let opus = c.models.iter().find(|m| m.id.as_deref() == Some("opus")).unwrap();
        eprintln!(
            "  claude levels: {:?} default={:?}",
            opus.reasoning_levels.iter().map(|l| &l.effort).collect::<Vec<_>>(),
            opus.default_effort
        );
        assert!(!opus.reasoning_levels.is_empty(), "effort levels should parse from --help");
        assert!(
            opus.reasoning_levels.iter().any(|l| l.effort == "ultracode"),
            "ultracode is accepted by the CLI but missing from --help; the probe should find it"
        );
    }
}
