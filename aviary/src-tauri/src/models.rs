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
pub struct ModelOption {
    /// Passed to `--model` / `-m`. `None` means send no flag at all.
    pub id: Option<String>,
    pub label: String,
    pub note: String,
    /// Aliases track the newest model in a family, so they never go stale.
    pub is_alias: bool,
}

#[derive(Debug, Serialize)]
pub struct ModelCatalogue {
    pub models: Vec<ModelOption>,
    /// What the runner is configured to use when no flag is passed.
    pub configured_default: Option<String>,
    /// Where the list came from, so the UI can be honest about staleness.
    pub source: String,
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

    let mut models = vec![ModelOption {
        id: None,
        label: "Default".into(),
        note: configured
            .as_deref()
            .map(|m| format!("Your CLI setting — currently {m}"))
            .unwrap_or_else(|| "Whatever your CLI is set to".into()),
        is_alias: false,
    }];

    models.extend(aliases.iter().map(|(id, label, note)| ModelOption {
        id: Some((*id).to_string()),
        label: (*label).to_string(),
        note: (*note).to_string(),
        is_alias: true,
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
    }
}
