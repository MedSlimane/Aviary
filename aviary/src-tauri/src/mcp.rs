//! MCP server discovery across runners.
//!
//! Reality on a real machine differs from the obvious model, and the shape
//! here follows what is actually on disk:
//!
//! * Servers arrive from three **sources**, and plugins dominate. Claude Code
//!   users often have zero servers in their own config while running a dozen
//!   supplied by installed plugins, each in its own `.mcp.json`.
//! * Transport is not always a command. `http` servers carry a URL and no
//!   process at all, so "the command that runs it" is the wrong primary field.
//! * Codex declares servers as TOML tables with an explicit `enabled` flag,
//!   which Claude's JSON has no equivalent for.
//!
//! Read-only. Nothing here mutates config.

use crate::providers::Runner;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// The user's own config — editable.
    User,
    /// Supplied by an installed plugin.
    Plugin,
    /// Declared inside a registered project.
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    /// Spawned as a process and spoken to over stdio.
    Stdio,
    /// A remote endpoint. No process is launched.
    Http,
    Sse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub name: String,
    pub transport: Transport,
    /// Command for stdio servers.
    pub command: Option<String>,
    pub args: Vec<String>,
    /// URL for http/sse servers.
    pub url: Option<String>,
    /// Env var names only — values are never read, so secrets cannot leak
    /// into the index or the UI.
    pub env_keys: Vec<String>,
    pub source: Source,
    /// Owning plugin, when `source` is `Plugin`.
    pub origin: Option<String>,
    pub runners: Vec<Runner>,
    /// Codex allows explicit disabling; Claude has no equivalent, so this is
    /// true unless a config says otherwise.
    pub enabled: bool,
    /// Where it was declared.
    pub config_path: String,
}

// Round-trips through the scan cache, so it must deserialise too.
#[derive(Debug, Serialize, Deserialize)]
pub struct McpSnapshot {
    pub servers: Vec<Server>,
    pub scanned_ms: u64,
}

fn json_servers(
    path: &Path,
    source: Source,
    origin: Option<String>,
    runner: Runner,
) -> Vec<Server> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let Some(map) = v.get("mcpServers").and_then(|m| m.as_object()) else {
        return Vec::new();
    };

    map.iter()
        .map(|(name, cfg)| {
            let url = cfg.get("url").and_then(|u| u.as_str()).map(String::from);
            let declared = cfg.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let transport = match declared {
                "http" => Transport::Http,
                "sse" => Transport::Sse,
                // A url with no declared type is still remote.
                _ if url.is_some() => Transport::Http,
                _ => Transport::Stdio,
            };

            Server {
                name: name.clone(),
                transport,
                command: cfg.get("command").and_then(|c| c.as_str()).map(String::from),
                args: cfg
                    .get("args")
                    .and_then(|a| a.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                url,
                env_keys: cfg
                    .get("env")
                    .and_then(|e| e.as_object())
                    .map(|e| e.keys().cloned().collect())
                    .unwrap_or_default(),
                source,
                origin: origin.clone(),
                runners: vec![runner],
                enabled: cfg
                    .get("enabled")
                    .and_then(|e| e.as_bool())
                    .unwrap_or(true),
                config_path: path.to_string_lossy().to_string(),
            }
        })
        .collect()
}

fn toml_servers(path: &Path, source: Source, runner: Runner) -> Vec<Server> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(doc) = raw.parse::<toml::Table>() else {
        return Vec::new();
    };
    let Some(map) = doc.get("mcp_servers").and_then(|m| m.as_table()) else {
        return Vec::new();
    };

    map.iter()
        .map(|(name, cfg)| {
            let url = cfg.get("url").and_then(|u| u.as_str()).map(String::from);
            Server {
                name: name.clone(),
                transport: if url.is_some() {
                    Transport::Http
                } else {
                    Transport::Stdio
                },
                command: cfg.get("command").and_then(|c| c.as_str()).map(String::from),
                args: cfg
                    .get("args")
                    .and_then(|a| a.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                url,
                env_keys: cfg
                    .get("env")
                    .and_then(|e| e.as_table())
                    .map(|e| e.keys().cloned().collect())
                    .unwrap_or_default(),
                source,
                origin: None,
                runners: vec![runner],
                enabled: cfg.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true),
                config_path: path.to_string_lossy().to_string(),
            }
        })
        .collect()
}

/// Extracts the plugin name from a `.mcp.json` path, mirroring how skills are
/// attributed to their pack.
fn plugin_origin(path: &Path) -> Option<String> {
    let parts: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    let at = parts.iter().position(|p| *p == "plugins")?;
    match parts.get(at + 1)? {
        // .../cache/<marketplace>/<plugin>/<version>/.mcp.json
        &"cache" => parts.get(at + 3).map(|s| s.to_string()),
        // .../marketplaces/<marketplace>/.../<plugin>/.mcp.json — the plugin
        // is whichever directory actually holds the file.
        &"marketplaces" => path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(String::from),
        _ => None,
    }
}

pub fn scan(projects: &[(String, PathBuf)]) -> McpSnapshot {
    let started = std::time::Instant::now();
    let mut out: Vec<Server> = Vec::new();

    let home = crate::providers::home();

    if let Some(home) = home.as_ref() {
        // Claude — user scope lives in two places.
        for p in [home.join(".claude.json"), home.join(".claude/settings.json")] {
            out.extend(json_servers(&p, Source::User, None, Runner::ClaudeCode));
        }

        // Claude — plugins. Deduped by name, newest wins, since the cache keeps
        // every installed version side by side.
        let plugins_root = home.join(".claude/plugins");
        if plugins_root.is_dir() {
            let mut newest: BTreeMap<String, (u64, Server)> = BTreeMap::new();
            for e in walkdir::WalkDir::new(&plugins_root)
                .max_depth(8)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
            {
                if e.file_name() != ".mcp.json" {
                    continue;
                }
                if e.path()
                    .components()
                    .any(|c| c.as_os_str().to_string_lossy().ends_with(".bak"))
                {
                    continue;
                }
                let mtime = e
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let origin = plugin_origin(e.path());
                for s in json_servers(e.path(), Source::Plugin, origin, Runner::ClaudeCode) {
                    newest
                        .entry(s.name.clone())
                        .and_modify(|slot| {
                            if mtime > slot.0 {
                                *slot = (mtime, s.clone());
                            }
                        })
                        .or_insert((mtime, s));
                }
            }
            out.extend(newest.into_values().map(|(_, s)| s));
        }

        // Codex — a single TOML file.
        out.extend(toml_servers(
            &home.join(".codex/config.toml"),
            Source::User,
            Runner::Codex,
        ));
    }

    // Registered projects.
    for (name, dir) in projects {
        for s in json_servers(
            &dir.join(".mcp.json"),
            Source::Project,
            Some(name.clone()),
            Runner::ClaudeCode,
        ) {
            out.push(s);
        }
        out.extend(toml_servers(
            &dir.join(".codex/config.toml"),
            Source::Project,
            Runner::Codex,
        ));
    }

    // Same server name across runners is one row reporting both.
    let mut merged: BTreeMap<String, Server> = BTreeMap::new();
    for s in out {
        merged
            .entry(s.name.clone())
            .and_modify(|e| {
                for r in &s.runners {
                    if !e.runners.contains(r) {
                        e.runners.push(*r);
                    }
                }
                e.runners.sort();
                if s.source == Source::User && e.source != Source::User {
                    e.source = Source::User;
                    e.config_path = s.config_path.clone();
                }
            })
            .or_insert(s);
    }

    let mut servers: Vec<Server> = merged.into_values().collect();
    servers.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    McpSnapshot {
        servers,
        scanned_ms: started.elapsed().as_millis() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_real_machine() {
        let snap = scan(&[]);
        eprintln!("scanned in {}ms", snap.scanned_ms);
        eprintln!("servers: {}", snap.servers.len());
        for s in &snap.servers {
            eprintln!(
                "  {:<22} {:?} {:?} origin={:?} runners={:?} enabled={} env={:?}",
                s.name, s.transport, s.source, s.origin, s.runners, s.enabled, s.env_keys
            );
        }
    }
}
