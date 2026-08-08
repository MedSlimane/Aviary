//! Static MCP declaration discovery and effective-resolution metadata.
//!
//! A declaration is not a server instance. The same display name can point at
//! unrelated endpoints in Claude Code and Codex, and one runner can resolve a
//! different declaration for every working directory. Keeping declarations
//! intact is what lets Aviary explain shadowing without manufacturing a merged
//! configuration that neither runner actually uses.
//!
//! This module intentionally never serializes launch arguments, URLs, headers,
//! environment values, or runner errors. Those values are needed briefly to
//! resolve identity and precedence, but exposing them would turn a read-only
//! inventory into a credential exfiltration surface.

use crate::providers::Runner;
use crate::{store, tokens};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

const HEALTH_CACHE_TTL_MS: u64 = 5 * 60 * 1_000;
const HEALTH_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_HEALTH_SERVERS: usize = 64;
const MAX_PROTOCOL_PAGES: usize = 64;
const MAX_PROTOCOL_FRAMES: usize = 2_048;
const MAX_PROTOCOL_LINE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PROTOCOL_TOTAL_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOOLS: usize = 4_096;
const MCP_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    Managed,
    Local,
    Project,
    User,
    Plugin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    Stdio,
    Http,
    Sse,
    Websocket,
    RunnerProvided,
    Invalid,
}

/// A deliberately coarse launcher label. Arbitrary command text is never
/// returned because credentials are sometimes supplied in shell-like command
/// strings even though MCP configuration expects an executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LauncherKind {
    Node,
    Npx,
    Bun,
    Python,
    Uvx,
    Docker,
    Other,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum TransportSummary {
    Stdio {
        launcher: LauncherKind,
        argument_count: usize,
        env_keys: Vec<String>,
        inherited_env_keys: Vec<String>,
        has_working_directory: bool,
    },
    Remote {
        transport: Transport,
        scheme: Option<String>,
        host: Option<String>,
        port: Option<u16>,
        path_segments: usize,
        has_query: bool,
        header_keys: Vec<String>,
        bearer_env_key: Option<String>,
    },
    RunnerProvided,
    Invalid {
        reason: InvalidConfigReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InvalidConfigReason {
    MissingCommand,
    MissingTransportType,
    MissingUrl,
    InvalidUrl,
    ConflictingTransport,
    UnsupportedTransport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeclarationState {
    Enabled,
    Disabled,
    PendingApproval,
    Invalid,
    BlockedByPolicy,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpHealthState {
    Unchecked,
    Checking,
    Reachable,
    Starting,
    Ready,
    Degraded,
    Disabled,
    PendingApproval,
    AuthRequired,
    NeedsAuthentication,
    NotConfigured,
    Failed,
    TimedOut,
    Cancelled,
    Unsupported,
    Shadowed,
    BlockedByPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TokenBasis {
    RunnerExact,
    O200kFileEstimate,
    O200kSchemaEstimate,
    Unavailable,
}

/// Token values are optional by construction. `complete` means the source was
/// fully enumerated, not that the runner necessarily loaded the definitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenMeasurement {
    pub tokens: Option<usize>,
    pub basis: TokenBasis,
    pub complete: bool,
    pub loaded: Option<bool>,
    pub included_in_total: bool,
}

impl TokenMeasurement {
    pub fn unavailable() -> Self {
        Self {
            tokens: None,
            basis: TokenBasis::Unavailable,
            complete: false,
            loaded: None,
            included_in_total: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInventory {
    pub count: Option<usize>,
    pub definitions: TokenMeasurement,
    pub checked_at_ms: Option<u64>,
}

impl ToolInventory {
    pub fn unchecked() -> Self {
        Self {
            count: None,
            definitions: TokenMeasurement::unavailable(),
            checked_at_ms: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpHealth {
    pub state: McpHealthState,
    pub tools: ToolInventory,
    pub checked_at_ms: Option<u64>,
    pub expires_at_ms: Option<u64>,
    pub stale: bool,
}

impl McpHealth {
    pub fn unchecked() -> Self {
        Self {
            state: McpHealthState::Unchecked,
            tools: ToolInventory::unchecked(),
            checked_at_ms: None,
            expires_at_ms: None,
            stale: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToggleUnavailableReason {
    ManagedByPolicy,
    PendingApproval,
    InvalidConfiguration,
    ProjectRequired,
    RunnerProvidedOnly,
    UnsupportedSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToggleCapability {
    pub writable: bool,
    /// Opaque optimistic-concurrency token for the actual target file.
    pub revision: Option<String>,
    pub shared_project_file: bool,
    pub unavailable_reason: Option<ToggleUnavailableReason>,
}

impl ToggleCapability {
    fn unavailable(reason: ToggleUnavailableReason) -> Self {
        Self {
            writable: false,
            revision: None,
            shared_project_file: false,
            unavailable_reason: Some(reason),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpDeclaration {
    pub id: String,
    pub runner: Runner,
    pub name: String,
    /// Runner-visible name. Plugin servers are scoped so two plugins cannot
    /// silently collapse into a single display-name declaration.
    pub effective_name: String,
    pub source: Source,
    pub origin: Option<String>,
    pub project_key: Option<String>,
    pub config_path: String,
    pub pointer: String,
    pub transport: TransportSummary,
    pub state: DeclarationState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub id: String,
    pub runner: Runner,
    pub cwd: Option<String>,
    pub declaration_id: String,
    pub name: String,
    pub source: Source,
    pub transport: TransportSummary,
    pub state: DeclarationState,
    pub shadowed_declaration_ids: Vec<String>,
    pub toggle: ToggleCapability,
    /// Opaque cache identity. It changes when the declaration bytes, selected
    /// working directory, endpoint identity, or effective state changes.
    pub health_revision: String,
    pub health: McpHealth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpHealthResult {
    pub id: String,
    pub declaration_id: Option<String>,
    pub revision: Option<String>,
    pub runner: Runner,
    pub cwd: String,
    pub name: String,
    pub runner_provided: bool,
    pub health: McpHealth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpHealthSnapshot {
    pub runner: Runner,
    pub cwd: String,
    pub results: Vec<McpHealthResult>,
    pub checked_at_ms: u64,
    pub expires_at_ms: u64,
    pub complete: bool,
}

// Round-trips through cache.db, so every public field is sanitized and the
// snapshot remains deserializable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSnapshot {
    pub declarations: Vec<McpDeclaration>,
    /// Effective instances, one per runner and working-directory context.
    pub servers: Vec<Server>,
    /// Last explicit health results, if present in cache. Runner-provided rows
    /// have no declaration and therefore live only in this collection.
    #[serde(default)]
    pub health_results: Vec<McpHealthResult>,
    pub scanned_ms: u64,
}

/// Internal write recipes are intentionally not serializable or debuggable.
#[derive(Clone)]
pub(crate) enum ToggleMutation {
    ClaudeDisabledList {
        project_key: String,
        server_name: String,
    },
    ClaudeEnabledList {
        project_key: String,
        server_name: String,
    },
    CodexServer {
        server_name: String,
    },
    CodexPluginServer {
        plugin_name: String,
        server_name: String,
    },
}

#[derive(Clone)]
pub(crate) struct ToggleTarget {
    pub path: PathBuf,
    pub revision: String,
    pub mutation: ToggleMutation,
}

pub(crate) enum ToggleTargetResolution {
    Writable(ToggleTarget),
    Unavailable(ToggleUnavailableReason),
    Missing,
}

#[derive(Clone)]
enum ToggleBase {
    ClaudeRegular {
        settings_path: PathBuf,
    },
    /// Reserved for default-off built-ins supplied by the live Claude control
    /// inventory. Static config cannot enumerate omitted built-ins safely.
    #[allow(dead_code)]
    ClaudeDefaultOff {
        settings_path: PathBuf,
    },
    CodexServer {
        config_path: PathBuf,
    },
    CodexPlugin {
        config_path: PathBuf,
        plugin_name: String,
    },
    Unavailable(ToggleUnavailableReason),
}

/// Contains endpoint material only long enough to resolve precedence. Do not
/// add Debug or Serialize derives to this type.
#[derive(Clone)]
struct RawDeclaration {
    public: McpDeclaration,
    config_path: PathBuf,
    endpoint_key: Option<String>,
    health_endpoint: HealthEndpoint,
    precedence: u16,
    toggle: ToggleBase,
}

/// Contains private launch material and must never gain Debug or Serialize.
#[derive(Clone)]
pub(crate) enum HealthEndpoint {
    DirectStdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        working_directory: Option<String>,
    },
    RunnerManaged,
    Unsupported,
    Invalid,
}

#[derive(Clone)]
pub(crate) struct HealthTarget {
    pub id: String,
    pub declaration_id: String,
    pub revision: String,
    pub runner: Runner,
    pub cwd: String,
    pub name: String,
    pub raw_name: String,
    pub state: DeclarationState,
    pub endpoint: HealthEndpoint,
}

pub(crate) struct HealthSelection {
    pub targets: Vec<HealthTarget>,
    pub requested_ids: HashSet<String>,
    pub all_effective: bool,
}

#[derive(Default)]
struct ClaudeState {
    disabled_by_project: HashMap<String, HashSet<String>>,
    enabled_default_off_by_project: HashMap<String, HashSet<String>>,
    raw_project_keys: HashMap<String, String>,
    global_approved_project_servers: HashSet<String>,
    global_rejected_project_servers: HashSet<String>,
    global_approve_all_project_servers: bool,
    approved_by_project: HashMap<String, HashSet<String>>,
    rejected_by_project: HashMap<String, HashSet<String>>,
    approve_all_by_project: HashSet<String>,
    global_enabled_plugins: HashMap<String, bool>,
    enabled_plugins_by_project: HashMap<String, HashMap<String, bool>>,
    managed_exclusive: bool,
}

#[derive(Clone)]
struct CodexPluginSetting {
    plugin_name: String,
    enabled: bool,
    config_path: PathBuf,
    project_key: Option<String>,
    precedence: u16,
    server_overrides: BTreeMap<String, bool>,
}

/// Exact installed plugin metadata from `codex plugin list --json`. Paths are
/// used internally to locate the active manifest and are never serialized.
#[derive(Clone)]
struct InstalledCodexPlugin {
    plugin_id: String,
    name: String,
    marketplace: String,
    version: String,
    enabled: bool,
    source_path: PathBuf,
}

struct Discovery {
    snapshot: McpSnapshot,
    toggle_targets: HashMap<String, ToggleTargetResolution>,
    health_targets: HashMap<String, HealthTarget>,
}

pub fn scan(projects: &[(String, PathBuf)]) -> McpSnapshot {
    let started = std::time::Instant::now();
    let Some(home) = crate::providers::home() else {
        return McpSnapshot {
            declarations: Vec::new(),
            servers: Vec::new(),
            health_results: Vec::new(),
            scanned_ms: started.elapsed().as_millis() as u64,
        };
    };
    let mut discovery = discover_at(&home, projects, &managed_paths());
    refresh_cached_health(&mut discovery.snapshot);
    discovery.snapshot
}

/// Resolves an arbitrary folder without registering it as durable project
/// data. Context inspection and folder pickers need user-scope declarations to
/// remain visible before the user has added the folder to Aviary.
pub fn scan_for_context(projects: &[(String, PathBuf)], cwd: &Path) -> McpSnapshot {
    let Some(home) = crate::providers::home() else {
        return McpSnapshot {
            declarations: Vec::new(),
            servers: Vec::new(),
            health_results: Vec::new(),
            scanned_ms: 0,
        };
    };
    scan_for_context_at(&home, projects, cwd, &managed_paths())
}

fn scan_for_context_at(
    home: &Path,
    projects: &[(String, PathBuf)],
    cwd: &Path,
    managed: &[PathBuf],
) -> McpSnapshot {
    let installed_codex_plugins = codex_plugin_inventory();
    let mut snapshot = scan_for_context_at_with_codex_plugins(
        home,
        projects,
        cwd,
        managed,
        installed_codex_plugins.as_deref(),
    );
    refresh_cached_health(&mut snapshot);
    snapshot
}

fn scan_for_context_at_with_codex_plugins(
    home: &Path,
    projects: &[(String, PathBuf)],
    cwd: &Path,
    managed: &[PathBuf],
    installed_codex_plugins: Option<&[InstalledCodexPlugin]>,
) -> McpSnapshot {
    let canonical = canonical_project_key(cwd);
    if projects
        .iter()
        .any(|(_, project)| canonical_project_key(project) == canonical)
    {
        return discover_at_with_codex_plugins(home, projects, managed, installed_codex_plugins)
            .snapshot;
    }
    let mut scoped = projects.to_vec();
    let label = cwd
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Current folder")
        .to_string();
    scoped.push((label, cwd.to_path_buf()));
    discover_at_with_codex_plugins(home, &scoped, managed, installed_codex_plugins).snapshot
}

/// Re-discovers the effective instance so callers cannot turn a frontend path
/// or table key into an arbitrary configuration write.
pub(crate) fn resolve_toggle_target(
    projects: &[(String, PathBuf)],
    cwd: Option<&Path>,
    effective_id: &str,
) -> ToggleTargetResolution {
    let Some(home) = crate::providers::home() else {
        return ToggleTargetResolution::Missing;
    };
    let scoped;
    let projects = if let Some(cwd) = cwd {
        let canonical = canonical_project_key(cwd);
        if projects
            .iter()
            .any(|(_, project)| canonical_project_key(project) == canonical)
        {
            projects
        } else {
            scoped = {
                let mut scoped = projects.to_vec();
                scoped.push(("Current folder".into(), cwd.to_path_buf()));
                scoped
            };
            &scoped
        }
    } else {
        projects
    };
    discover_at(&home, projects, &managed_paths())
        .toggle_targets
        .remove(effective_id)
        .unwrap_or(ToggleTargetResolution::Missing)
}

fn managed_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Library/Application Support/ClaudeCode/managed-mcp.json"),
        PathBuf::from("/etc/claude-code/managed-mcp.json"),
    ]
}

fn discover_at(home: &Path, projects: &[(String, PathBuf)], managed: &[PathBuf]) -> Discovery {
    let installed_codex_plugins = codex_plugin_inventory();
    discover_at_with_codex_plugins(home, projects, managed, installed_codex_plugins.as_deref())
}

fn discover_at_with_codex_plugins(
    home: &Path,
    projects: &[(String, PathBuf)],
    managed: &[PathBuf],
    installed_codex_plugins: Option<&[InstalledCodexPlugin]>,
) -> Discovery {
    let started = std::time::Instant::now();
    let mut raw = Vec::new();
    let mut claude = ClaudeState::default();

    scan_claude(home, projects, managed, &mut claude, &mut raw);
    scan_codex(home, projects, installed_codex_plugins, &mut raw);

    let (servers, toggle_targets, health_targets) =
        resolve_effective(home, projects, &raw, &claude);
    let mut declarations: Vec<McpDeclaration> = raw.iter().map(|d| d.public.clone()).collect();
    declarations.sort_by(|a, b| {
        a.runner
            .cmp(&b.runner)
            .then_with(|| a.project_key.cmp(&b.project_key))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });

    Discovery {
        snapshot: McpSnapshot {
            declarations,
            servers,
            health_results: Vec::new(),
            scanned_ms: started.elapsed().as_millis() as u64,
        },
        toggle_targets,
        health_targets,
    }
}

fn scan_claude(
    home: &Path,
    registered_projects: &[(String, PathBuf)],
    managed_paths: &[PathBuf],
    state: &mut ClaudeState,
    out: &mut Vec<RawDeclaration>,
) {
    let claude_json = home.join(".claude.json");
    let settings_path = home.join(".claude/settings.json");
    let settings = read_json(&settings_path);
    read_claude_settings(settings.as_ref(), state, None);

    if let Some(root) = read_json(&claude_json) {
        if let Some(map) = root
            .get("mcpServers")
            .and_then(serde_json::Value::as_object)
        {
            push_json_map(
                out,
                map,
                JsonDeclarationContext {
                    runner: Runner::ClaudeCode,
                    source: Source::User,
                    origin: None,
                    project_key: None,
                    config_path: &claude_json,
                    pointer_prefix: "/mcpServers",
                    precedence: 30,
                    toggle: ToggleBase::ClaudeRegular {
                        settings_path: claude_json.clone(),
                    },
                    plugin_scope: None,
                    base_state: DeclarationState::Enabled,
                },
            );
        }

        if let Some(project_map) = root.get("projects").and_then(serde_json::Value::as_object) {
            for (project_path, value) in project_map {
                let project_key = canonical_project_key(Path::new(project_path));
                state
                    .raw_project_keys
                    .insert(project_key.clone(), project_path.clone());
                // Claude stores the effective per-folder approval and plugin
                // overrides here on current releases. Project settings files
                // use the same fields, so both feed the same context-keyed
                // state rather than a process-global allowlist.
                read_claude_settings(Some(value), state, Some(&project_key));
                if let Some(disabled) = string_set(value.get("disabledMcpServers")) {
                    state
                        .disabled_by_project
                        .entry(project_key.clone())
                        .or_default()
                        .extend(disabled);
                }
                if let Some(enabled) = string_set(value.get("enabledMcpServers")) {
                    state
                        .enabled_default_off_by_project
                        .entry(project_key.clone())
                        .or_default()
                        .extend(enabled);
                }
                let Some(map) = value
                    .get("mcpServers")
                    .and_then(serde_json::Value::as_object)
                else {
                    continue;
                };
                let prefix = format!("/projects/{}/mcpServers", json_pointer_escape(project_path));
                push_json_map(
                    out,
                    map,
                    JsonDeclarationContext {
                        runner: Runner::ClaudeCode,
                        source: Source::Local,
                        origin: project_name_for_key(&project_key, registered_projects),
                        project_key: Some(project_key),
                        config_path: &claude_json,
                        pointer_prefix: &prefix,
                        precedence: 10,
                        toggle: ToggleBase::ClaudeRegular {
                            settings_path: claude_json.clone(),
                        },
                        plugin_scope: None,
                        base_state: DeclarationState::Enabled,
                    },
                );
            }
        }
    }

    for (name, dir) in registered_projects {
        let project_key = canonical_project_key(dir);
        read_project_claude_settings(dir, &project_key, state);
        let path = dir.join(".mcp.json");
        let Some(root) = read_json(&path) else {
            continue;
        };
        let Some(map) = root
            .get("mcpServers")
            .and_then(serde_json::Value::as_object)
        else {
            continue;
        };
        for (server_name, config) in map {
            let base_state = project_server_state(server_name, &project_key, state);
            push_json_declaration(
                out,
                server_name,
                config,
                JsonDeclarationContext {
                    runner: Runner::ClaudeCode,
                    source: Source::Project,
                    origin: Some(name.clone()),
                    project_key: Some(project_key.clone()),
                    config_path: &path,
                    pointer_prefix: "/mcpServers",
                    precedence: 20,
                    toggle: ToggleBase::ClaudeRegular {
                        settings_path: claude_json.clone(),
                    },
                    plugin_scope: None,
                    base_state,
                },
            );
        }
    }

    scan_claude_plugins(home, out);

    for path in managed_paths {
        let Some(root) = read_json(path) else {
            continue;
        };
        let Some(map) = root
            .get("mcpServers")
            .and_then(serde_json::Value::as_object)
        else {
            continue;
        };
        state.managed_exclusive = true;
        push_json_map(
            out,
            map,
            JsonDeclarationContext {
                runner: Runner::ClaudeCode,
                source: Source::Managed,
                origin: Some("managed configuration".into()),
                project_key: None,
                config_path: path,
                pointer_prefix: "/mcpServers",
                precedence: 0,
                toggle: ToggleBase::Unavailable(ToggleUnavailableReason::ManagedByPolicy),
                plugin_scope: None,
                base_state: DeclarationState::Enabled,
            },
        );
        break;
    }
}

fn read_claude_settings(
    value: Option<&serde_json::Value>,
    state: &mut ClaudeState,
    project_key: Option<&str>,
) {
    let Some(value) = value else { return };
    if let Some(map) = value
        .get("enabledPlugins")
        .and_then(serde_json::Value::as_object)
    {
        for (name, enabled) in map {
            if let Some(enabled) = enabled.as_bool() {
                if let Some(project_key) = project_key {
                    state
                        .enabled_plugins_by_project
                        .entry(project_key.to_string())
                        .or_default()
                        .insert(name.clone(), enabled);
                } else {
                    state.global_enabled_plugins.insert(name.clone(), enabled);
                }
            }
        }
    }
    let approved = string_set(value.get("enabledMcpjsonServers")).unwrap_or_default();
    let rejected = string_set(value.get("disabledMcpjsonServers")).unwrap_or_default();
    let approve_all = value
        .get("enableAllProjectMcpServers")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if let Some(project_key) = project_key {
        state
            .approved_by_project
            .entry(project_key.to_string())
            .or_default()
            .extend(approved);
        state
            .rejected_by_project
            .entry(project_key.to_string())
            .or_default()
            .extend(rejected);
        if approve_all {
            state.approve_all_by_project.insert(project_key.to_string());
        }
    } else {
        state.global_approved_project_servers.extend(approved);
        state.global_rejected_project_servers.extend(rejected);
        state.global_approve_all_project_servers |= approve_all;
    }
}

fn read_project_claude_settings(project: &Path, project_key: &str, state: &mut ClaudeState) {
    for path in [
        project.join(".claude/settings.json"),
        project.join(".claude/settings.local.json"),
    ] {
        let value = read_json(&path);
        read_claude_settings(value.as_ref(), state, Some(project_key));
    }
}

fn project_server_state(name: &str, project_key: &str, state: &ClaudeState) -> DeclarationState {
    let rejected_for_project = state
        .rejected_by_project
        .get(project_key)
        .is_some_and(|servers| servers.contains(name));
    let approved_for_project = state
        .approved_by_project
        .get(project_key)
        .is_some_and(|servers| servers.contains(name));
    if state.global_rejected_project_servers.contains(name) || rejected_for_project {
        DeclarationState::Disabled
    } else if state.global_approve_all_project_servers
        || state.approve_all_by_project.contains(project_key)
        || state.global_approved_project_servers.contains(name)
        || approved_for_project
    {
        DeclarationState::Enabled
    } else {
        DeclarationState::PendingApproval
    }
}

fn scan_claude_plugins(home: &Path, out: &mut Vec<RawDeclaration>) {
    let installed_path = home.join(".claude/plugins/installed_plugins.json");
    let Some(installed) = read_json(&installed_path) else {
        return;
    };
    let Some(plugins) = installed
        .get("plugins")
        .and_then(serde_json::Value::as_object)
    else {
        return;
    };

    for (plugin_id, installs) in plugins {
        let Some(installs) = installs.as_array() else {
            continue;
        };
        for install in installs {
            let Some(install_path) = install
                .get("installPath")
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            let install_root = PathBuf::from(install_path);
            if !install_root.is_dir() {
                continue;
            }
            let project_key = install
                .get("projectPath")
                .and_then(serde_json::Value::as_str)
                .map(|p| canonical_project_key(Path::new(p)));
            let plugin_name = plugin_id.split('@').next().unwrap_or(plugin_id);
            let toggle = ToggleBase::ClaudeRegular {
                settings_path: home.join(".claude.json"),
            };
            for manifest in [
                install_root.join(".mcp.json"),
                install_root.join(".claude-plugin/plugin.json"),
                install_root.join("plugin.json"),
            ] {
                let Some(root) = read_json(&manifest) else {
                    continue;
                };
                let Some(map) = root
                    .get("mcpServers")
                    .and_then(serde_json::Value::as_object)
                else {
                    continue;
                };
                push_json_map(
                    out,
                    map,
                    JsonDeclarationContext {
                        runner: Runner::ClaudeCode,
                        source: Source::Plugin,
                        origin: Some(plugin_id.clone()),
                        project_key: project_key.clone(),
                        config_path: &manifest,
                        pointer_prefix: "/mcpServers",
                        precedence: 40,
                        toggle: toggle.clone(),
                        plugin_scope: Some(plugin_name),
                        base_state: DeclarationState::Enabled,
                    },
                );
            }
        }
    }
}

fn scan_codex(
    home: &Path,
    projects: &[(String, PathBuf)],
    installed_plugins: Option<&[InstalledCodexPlugin]>,
    out: &mut Vec<RawDeclaration>,
) {
    let user_config = home.join(".codex/config.toml");
    let mut plugin_settings = Vec::new();
    scan_codex_config(
        &user_config,
        Source::User,
        None,
        100,
        out,
        &mut plugin_settings,
    );

    for (_, cwd) in projects {
        let project_key = canonical_project_key(cwd);
        let configs = codex_project_configs(home, cwd);
        let count = configs.len();
        for (index, path) in configs.into_iter().enumerate() {
            // A deeper project layer wins over a shallower one.
            let precedence = 60u16.saturating_sub((index.min(count)) as u16);
            scan_codex_config(
                &path,
                Source::Project,
                Some(project_key.clone()),
                precedence,
                out,
                &mut plugin_settings,
            );
        }
    }

    scan_codex_plugins(home, plugin_settings, installed_plugins, out);
}

fn codex_project_configs(home: &Path, cwd: &Path) -> Vec<PathBuf> {
    let user = home.join(".codex/config.toml");
    let mut paths = Vec::new();
    for ancestor in cwd.ancestors() {
        let path = ancestor.join(".codex/config.toml");
        if path != user && path.is_file() {
            paths.push(path);
        }
        if ancestor == home {
            break;
        }
    }
    paths.reverse();
    paths
}

fn scan_codex_config(
    path: &Path,
    source: Source,
    project_key: Option<String>,
    precedence: u16,
    out: &mut Vec<RawDeclaration>,
    plugin_settings: &mut Vec<CodexPluginSetting>,
) {
    let Ok(raw) = fs::read_to_string(path) else {
        return;
    };
    let Ok(doc) = raw.parse::<toml::Table>() else {
        return;
    };

    if let Some(map) = doc.get("mcp_servers").and_then(toml::Value::as_table) {
        for (name, config) in map {
            let summary = summarize_toml_transport(config);
            let health_endpoint = if matches!(summary.0, TransportSummary::Invalid { .. }) {
                HealthEndpoint::Invalid
            } else {
                HealthEndpoint::RunnerManaged
            };
            let state = if matches!(summary.0, TransportSummary::Invalid { .. }) {
                DeclarationState::Invalid
            } else if config
                .get("enabled")
                .and_then(toml::Value::as_bool)
                .unwrap_or(true)
            {
                DeclarationState::Enabled
            } else {
                DeclarationState::Disabled
            };
            let pointer = format!("/mcp_servers/{}", json_pointer_escape(name));
            out.push(raw_declaration(
                Runner::Codex,
                name,
                name,
                source,
                None,
                project_key.clone(),
                path,
                &pointer,
                summary,
                state,
                precedence,
                ToggleBase::CodexServer {
                    config_path: path.to_path_buf(),
                },
                health_endpoint,
            ));
        }
    }

    let Some(plugins) = doc.get("plugins").and_then(toml::Value::as_table) else {
        return;
    };
    for (plugin_name, config) in plugins {
        let enabled = config
            .get("enabled")
            .and_then(toml::Value::as_bool)
            .unwrap_or(true);
        let mut server_overrides = BTreeMap::new();
        if let Some(servers) = config.get("mcp_servers").and_then(toml::Value::as_table) {
            for (server, override_config) in servers {
                let enabled = override_config
                    .get("enabled")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(true);
                server_overrides.insert(server.clone(), enabled);
            }
        }
        plugin_settings.push(CodexPluginSetting {
            plugin_name: plugin_name.clone(),
            enabled,
            config_path: path.to_path_buf(),
            project_key: project_key.clone(),
            precedence,
            server_overrides,
        });
    }
}

fn scan_codex_plugins(
    home: &Path,
    settings: Vec<CodexPluginSetting>,
    installed_plugins: Option<&[InstalledCodexPlugin]>,
    out: &mut Vec<RawDeclaration>,
) {
    let mut settings_by_plugin: HashMap<String, Vec<CodexPluginSetting>> = HashMap::new();
    for setting in settings {
        settings_by_plugin
            .entry(setting.plugin_name.clone())
            .or_default()
            .push(setting);
    }

    for installed in installed_plugins.into_iter().flatten() {
        let configured = settings_by_plugin
            .remove(&installed.plugin_id)
            .unwrap_or_else(|| {
                vec![CodexPluginSetting {
                    plugin_name: installed.plugin_id.clone(),
                    enabled: installed.enabled,
                    config_path: home.join(".codex/config.toml"),
                    project_key: None,
                    precedence: 200,
                    server_overrides: BTreeMap::new(),
                }]
            });
        for mut setting in configured {
            setting.enabled &= installed.enabled;
            let direct_manifest = plugin_manifest_at(&installed.source_path);
            let cache_root = home
                .join(".codex/plugins/cache")
                .join(&installed.marketplace)
                .join(&installed.name)
                .join(&installed.version);
            let manifest = direct_manifest.or_else(|| plugin_manifest_at(&cache_root));
            scan_one_codex_plugin(setting, manifest.as_deref(), out);
        }
    }

    // An explicit server override remains real disk truth even when this Codex
    // build cannot provide an installed-plugin inventory. It stays
    // runner-provided rather than borrowing a random cache artifact.
    if installed_plugins.is_none() {
        for configured in settings_by_plugin.into_values() {
            for setting in configured {
                scan_one_codex_plugin(setting, None, out);
            }
        }
    }
}

fn plugin_manifest_at(root: &Path) -> Option<PathBuf> {
    if root.is_file() {
        return Some(root.to_path_buf());
    }
    [
        root.join(".mcp.json"),
        root.join(".codex-plugin/plugin.json"),
        root.join("plugin.json"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn scan_one_codex_plugin(
    setting: CodexPluginSetting,
    manifest: Option<&Path>,
    out: &mut Vec<RawDeclaration>,
) {
    let mut found = HashSet::new();
    if let Some(manifest) = manifest {
        if let Some(root) = read_json(&manifest) {
            if let Some(map) = root
                .get("mcpServers")
                .and_then(serde_json::Value::as_object)
            {
                for (server_name, config) in map {
                    found.insert(server_name.clone());
                    let override_enabled = setting
                        .server_overrides
                        .get(server_name)
                        .copied()
                        .unwrap_or(true);
                    let state = if override_enabled {
                        DeclarationState::Enabled
                    } else {
                        DeclarationState::Disabled
                    };
                    push_json_declaration(
                        out,
                        server_name,
                        config,
                        JsonDeclarationContext {
                            runner: Runner::Codex,
                            source: Source::Plugin,
                            origin: Some(setting.plugin_name.clone()),
                            project_key: setting.project_key.clone(),
                            config_path: &manifest,
                            pointer_prefix: "/mcpServers",
                            precedence: setting.precedence + 100,
                            toggle: ToggleBase::CodexPlugin {
                                config_path: setting.config_path.clone(),
                                plugin_name: setting.plugin_name.clone(),
                            },
                            plugin_scope: Some(&setting.plugin_name),
                            base_state: if setting.enabled {
                                state
                            } else {
                                DeclarationState::Disabled
                            },
                        },
                    );
                }
            }
        }
    }

    // Keep explicit overrides visible even when the installed runner plugin
    // stores its MCP declaration outside the static cache format we know.
    for (server_name, enabled) in &setting.server_overrides {
        if found.contains(server_name) {
            continue;
        }
        let effective_name = format!("plugin:{}:{}", setting.plugin_name, server_name);
        let pointer = format!(
            "/plugins/{}/mcp_servers/{}",
            json_pointer_escape(&setting.plugin_name),
            json_pointer_escape(server_name)
        );
        out.push(raw_declaration(
            Runner::Codex,
            server_name,
            &effective_name,
            Source::Plugin,
            Some(setting.plugin_name.clone()),
            setting.project_key.clone(),
            &setting.config_path,
            &pointer,
            (TransportSummary::RunnerProvided, None),
            if setting.enabled && *enabled {
                DeclarationState::Enabled
            } else {
                DeclarationState::Disabled
            },
            setting.precedence + 100,
            ToggleBase::CodexPlugin {
                config_path: setting.config_path.clone(),
                plugin_name: setting.plugin_name.clone(),
            },
            HealthEndpoint::RunnerManaged,
        ));
    }
}

fn codex_plugin_inventory() -> Option<Vec<InstalledCodexPlugin>> {
    let Some(bytes) = bounded_command_stdout(
        "codex",
        &["plugin", "list", "--json"],
        Duration::from_secs(3),
        2 * 1024 * 1024,
    ) else {
        return None;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return None;
    };
    Some(
        value
            .get("installed")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|plugin| {
                plugin
                    .get("installed")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            })
            .filter_map(|plugin| {
                Some(InstalledCodexPlugin {
                    plugin_id: plugin.get("pluginId")?.as_str()?.to_string(),
                    name: plugin.get("name")?.as_str()?.to_string(),
                    marketplace: plugin.get("marketplaceName")?.as_str()?.to_string(),
                    version: plugin.get("version")?.as_str()?.to_string(),
                    enabled: plugin
                        .get("enabled")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    source_path: PathBuf::from(plugin.get("source")?.get("path")?.as_str()?),
                })
            })
            .collect(),
    )
}

fn bounded_command_stdout(
    program: &str,
    args: &[&str],
    timeout: Duration,
    max_bytes: usize,
) -> Option<Vec<u8>> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    configure_process_group(&mut command);
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut oversized = false;
        let mut buffer = [0u8; 8192];
        loop {
            let Ok(read) = stdout.read(&mut buffer) else {
                let _ = tx.send(None);
                return;
            };
            if read == 0 {
                break;
            }
            if kept.len().saturating_add(read) <= max_bytes {
                kept.extend_from_slice(&buffer[..read]);
            } else {
                oversized = true;
            }
        }
        let _ = tx.send((!oversized).then_some(kept));
    });
    let started = Instant::now();
    let (bytes, forced_status) = match rx.recv_timeout(timeout) {
        Ok(bytes) => (bytes?, None),
        Err(_) => {
            // Do not call `try_wait` before this point. It reaps an exited
            // group leader and permits PID reuse; killing that stale numeric
            // group later could terminate an unrelated process. Leaving the
            // leader waitable keeps the process-group id reserved while a
            // descendant holds stdout open.
            let status = terminate_process_group(&mut child);
            let bytes = rx.recv_timeout(Duration::from_millis(250)).ok().flatten()?;
            (bytes, status)
        }
    };
    let success = if let Some(status) = forced_status {
        status.success()
    } else {
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.success(),
                Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(20)),
                Ok(None) | Err(_) => {
                    let _ = terminate_process_group(&mut child);
                    break false;
                }
            }
        }
    };
    success.then_some(bytes)
}

fn configure_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
}

fn terminate_process_group(child: &mut std::process::Child) -> Option<std::process::ExitStatus> {
    #[cfg(unix)]
    unsafe {
        let group = -(child.id() as i32);
        let _ = libc::kill(group, libc::SIGTERM);
        thread::sleep(Duration::from_millis(30));
        let _ = libc::kill(group, libc::SIGKILL);
    }
    let _ = child.kill();
    child.wait().ok()
}

struct JsonDeclarationContext<'a> {
    runner: Runner,
    source: Source,
    origin: Option<String>,
    project_key: Option<String>,
    config_path: &'a Path,
    pointer_prefix: &'a str,
    precedence: u16,
    toggle: ToggleBase,
    plugin_scope: Option<&'a str>,
    base_state: DeclarationState,
}

fn push_json_map(
    out: &mut Vec<RawDeclaration>,
    map: &serde_json::Map<String, serde_json::Value>,
    context: JsonDeclarationContext<'_>,
) {
    for (name, config) in map {
        push_json_declaration(
            out,
            name,
            config,
            JsonDeclarationContext {
                runner: context.runner,
                source: context.source,
                origin: context.origin.clone(),
                project_key: context.project_key.clone(),
                config_path: context.config_path,
                pointer_prefix: context.pointer_prefix,
                precedence: context.precedence,
                toggle: context.toggle.clone(),
                plugin_scope: context.plugin_scope,
                base_state: context.base_state,
            },
        );
    }
}

fn push_json_declaration(
    out: &mut Vec<RawDeclaration>,
    name: &str,
    config: &serde_json::Value,
    context: JsonDeclarationContext<'_>,
) {
    let summary = summarize_json_transport(config);
    let state = if matches!(summary.0, TransportSummary::Invalid { .. }) {
        DeclarationState::Invalid
    } else {
        context.base_state
    };
    let effective_name = context
        .plugin_scope
        .map(|plugin| format!("plugin:{plugin}:{name}"))
        .unwrap_or_else(|| name.to_string());
    let pointer = format!("{}/{}", context.pointer_prefix, json_pointer_escape(name));
    let health_endpoint = health_endpoint_for_json(context.runner, config, context.config_path);
    out.push(raw_declaration(
        context.runner,
        name,
        &effective_name,
        context.source,
        context.origin,
        context.project_key,
        context.config_path,
        &pointer,
        summary,
        state,
        context.precedence,
        context.toggle,
        health_endpoint,
    ));
}

#[allow(clippy::too_many_arguments)]
fn raw_declaration(
    runner: Runner,
    name: &str,
    effective_name: &str,
    source: Source,
    origin: Option<String>,
    project_key: Option<String>,
    config_path: &Path,
    pointer: &str,
    summary: (TransportSummary, Option<String>),
    state: DeclarationState,
    precedence: u16,
    toggle: ToggleBase,
    health_endpoint: HealthEndpoint,
) -> RawDeclaration {
    let canonical_origin = canonical_origin(config_path);
    let id = digest_parts(&[
        runner_key(runner),
        &canonical_origin,
        pointer,
        source_key(source),
        project_key.as_deref().unwrap_or(""),
        name,
    ]);
    RawDeclaration {
        public: McpDeclaration {
            id,
            runner,
            name: name.to_string(),
            effective_name: effective_name.to_string(),
            source,
            origin,
            project_key,
            config_path: display_path(config_path),
            pointer: pointer.to_string(),
            transport: summary.0,
            state,
        },
        config_path: config_path.to_path_buf(),
        endpoint_key: summary.1,
        health_endpoint,
        precedence,
        toggle,
    }
}

fn health_endpoint_for_json(
    runner: Runner,
    config: &serde_json::Value,
    _config_path: &Path,
) -> HealthEndpoint {
    if runner == Runner::Codex {
        return if matches!(
            summarize_json_transport(config).0,
            TransportSummary::Invalid { .. }
        ) {
            HealthEndpoint::Invalid
        } else {
            HealthEndpoint::RunnerManaged
        };
    }

    let declared = config
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !matches!(declared, "" | "stdio") || config.get("url").is_some() {
        return HealthEndpoint::Unsupported;
    }
    let Some(command) = config.get("command").and_then(serde_json::Value::as_str) else {
        return HealthEndpoint::Invalid;
    };
    let args = match config.get("args") {
        None => Vec::new(),
        Some(value) => {
            let Some(values) = value.as_array() else {
                return HealthEndpoint::Invalid;
            };
            let Some(args) = values
                .iter()
                .map(|arg| arg.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
            else {
                return HealthEndpoint::Invalid;
            };
            args
        }
    };
    let env = match config.get("env") {
        None => BTreeMap::new(),
        Some(value) => {
            let Some(values) = value.as_object() else {
                return HealthEndpoint::Invalid;
            };
            let Some(env) = values
                .iter()
                .map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                .collect::<Option<BTreeMap<_, _>>>()
            else {
                return HealthEndpoint::Invalid;
            };
            env
        }
    };
    let working_directory = match config.get("cwd") {
        None => None,
        Some(value) => match value.as_str() {
            Some(value) => Some(value.to_string()),
            None => return HealthEndpoint::Invalid,
        },
    };
    HealthEndpoint::DirectStdio {
        command: command.to_string(),
        args,
        env,
        working_directory,
    }
}

fn summarize_json_transport(config: &serde_json::Value) -> (TransportSummary, Option<String>) {
    let declared = config
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let command = config.get("command").and_then(serde_json::Value::as_str);
    let args: Vec<&str> = config
        .get("args")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let url = config.get("url").and_then(serde_json::Value::as_str);
    let env_keys = json_object_keys(config.get("env"));
    let header_keys = json_object_keys(config.get("headers"));

    match declared {
        "http" | "streamable-http" | "sse" | "ws" => {
            let transport = match declared {
                "sse" => Transport::Sse,
                "ws" => Transport::Websocket,
                _ => Transport::Http,
            };
            let Some(url) = url else {
                return (
                    TransportSummary::Invalid {
                        reason: InvalidConfigReason::MissingUrl,
                    },
                    None,
                );
            };
            remote_summary(transport, url, header_keys, None)
        }
        "stdio" | "" if url.is_none() => {
            if command.is_none() {
                return (
                    TransportSummary::Invalid {
                        reason: InvalidConfigReason::MissingCommand,
                    },
                    None,
                );
            }
            stdio_summary(
                command,
                &args,
                env_keys,
                Vec::new(),
                config.get("cwd").is_some(),
            )
        }
        "" => (
            TransportSummary::Invalid {
                reason: InvalidConfigReason::MissingTransportType,
            },
            None,
        ),
        _ => (
            TransportSummary::Invalid {
                reason: InvalidConfigReason::UnsupportedTransport,
            },
            None,
        ),
    }
}

fn summarize_toml_transport(config: &toml::Value) -> (TransportSummary, Option<String>) {
    let command = config.get("command").and_then(toml::Value::as_str);
    let url = config.get("url").and_then(toml::Value::as_str);
    if command.is_some() && url.is_some() {
        return (
            TransportSummary::Invalid {
                reason: InvalidConfigReason::ConflictingTransport,
            },
            None,
        );
    }
    if let Some(url) = url {
        let headers = toml_table_keys(config.get("http_headers"));
        let bearer = config
            .get("bearer_token_env_var")
            .and_then(toml::Value::as_str)
            .map(str::to_string);
        return remote_summary(Transport::Http, url, headers, bearer);
    }
    let Some(command) = command else {
        return (
            TransportSummary::Invalid {
                reason: InvalidConfigReason::MissingCommand,
            },
            None,
        );
    };
    let args: Vec<&str> = config
        .get("args")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .collect();
    let env_keys = toml_table_keys(config.get("env"));
    let inherited_env_keys = config
        .get("env_vars")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry.as_str().map(str::to_string).or_else(|| {
                entry
                    .as_table()
                    .and_then(|table| table.get("name"))
                    .and_then(toml::Value::as_str)
                    .map(str::to_string)
            })
        })
        .collect();
    stdio_summary(
        Some(command),
        &args,
        env_keys,
        inherited_env_keys,
        config.get("cwd").is_some(),
    )
}

fn stdio_summary(
    command: Option<&str>,
    args: &[&str],
    mut env_keys: Vec<String>,
    mut inherited_env_keys: Vec<String>,
    has_working_directory: bool,
) -> (TransportSummary, Option<String>) {
    env_keys.sort();
    env_keys.dedup();
    inherited_env_keys.sort();
    inherited_env_keys.dedup();
    let endpoint_key = command.map(|command| {
        let mut parts = vec!["stdio", command];
        parts.extend(args.iter().copied());
        digest_parts(&parts)
    });
    (
        TransportSummary::Stdio {
            launcher: launcher_kind(command),
            argument_count: args.len(),
            env_keys,
            inherited_env_keys,
            has_working_directory,
        },
        endpoint_key,
    )
}

fn remote_summary(
    transport: Transport,
    raw_url: &str,
    mut header_keys: Vec<String>,
    bearer_env_key: Option<String>,
) -> (TransportSummary, Option<String>) {
    header_keys.sort();
    header_keys.dedup();
    let Ok(parsed) = Url::parse(raw_url) else {
        return (
            TransportSummary::Invalid {
                reason: InvalidConfigReason::InvalidUrl,
            },
            None,
        );
    };
    let scheme = Some(parsed.scheme().to_string());
    let host = parsed.host_str().map(str::to_string);
    let port = parsed.port();
    let path_segments = parsed.path_segments().map(Iterator::count).unwrap_or(0);
    let has_query = parsed.query().is_some();
    (
        TransportSummary::Remote {
            transport,
            scheme,
            host,
            port,
            path_segments,
            has_query,
            header_keys,
            bearer_env_key,
        },
        Some(digest_parts(&["remote", raw_url])),
    )
}

fn launcher_kind(command: Option<&str>) -> LauncherKind {
    let Some(command) = command else {
        return LauncherKind::Missing;
    };
    let basename = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match basename.as_str() {
        "node" | "nodejs" => LauncherKind::Node,
        "npx" => LauncherKind::Npx,
        "bun" | "bunx" => LauncherKind::Bun,
        "python" | "python3" | "python.exe" => LauncherKind::Python,
        "uvx" | "uv" => LauncherKind::Uvx,
        "docker" | "podman" => LauncherKind::Docker,
        _ => LauncherKind::Other,
    }
}

fn resolve_effective(
    home: &Path,
    projects: &[(String, PathBuf)],
    raw: &[RawDeclaration],
    claude: &ClaudeState,
) -> (
    Vec<Server>,
    HashMap<String, ToggleTargetResolution>,
    HashMap<String, HealthTarget>,
) {
    let mut contexts: BTreeSet<String> = projects
        .iter()
        .map(|(_, path)| canonical_project_key(path))
        .collect();
    for declaration in raw {
        if let Some(project) = &declaration.public.project_key {
            contexts.insert(project.clone());
        }
    }
    let contexts: Vec<Option<String>> = if contexts.is_empty() {
        vec![None]
    } else {
        contexts.into_iter().map(Some).collect()
    };

    let mut servers = Vec::new();
    let mut targets = HashMap::new();
    let mut health_targets = HashMap::new();
    for runner in [Runner::ClaudeCode, Runner::Codex] {
        for cwd in &contexts {
            let mut candidates: Vec<&RawDeclaration> = raw
                .iter()
                .filter(|declaration| declaration.public.runner == runner)
                .filter(|declaration| applicable(declaration, cwd.as_deref()))
                .filter(|declaration| {
                    plugin_enabled_for_context(declaration, cwd.as_deref(), claude)
                })
                .collect();
            if runner == Runner::ClaudeCode && claude.managed_exclusive {
                candidates.retain(|declaration| declaration.public.source == Source::Managed);
            }
            if candidates.is_empty() {
                continue;
            }
            candidates.sort_by(|a, b| {
                a.precedence
                    .cmp(&b.precedence)
                    .then_with(|| a.public.id.cmp(&b.public.id))
            });

            let mut winners: Vec<&RawDeclaration> = Vec::new();
            let mut winner_by_name: HashMap<String, usize> = HashMap::new();
            let mut winner_by_endpoint: HashMap<String, usize> = HashMap::new();
            let mut shadowed: HashMap<String, Vec<String>> = HashMap::new();

            for declaration in &candidates {
                let is_plugin = declaration.public.source == Source::Plugin;
                let name_key = declaration.public.effective_name.clone();
                let duplicate = if is_plugin {
                    declaration
                        .endpoint_key
                        .as_ref()
                        .and_then(|key| winner_by_endpoint.get(key).copied())
                        .or_else(|| winner_by_name.get(&name_key).copied())
                } else {
                    winner_by_name.get(&name_key).copied()
                };

                if let Some(index) = duplicate {
                    shadowed
                        .entry(winners[index].public.id.clone())
                        .or_default()
                        .push(declaration.public.id.clone());
                    continue;
                }

                let index = winners.len();
                winner_by_name.insert(name_key, index);
                if let Some(endpoint) = &declaration.endpoint_key {
                    winner_by_endpoint.insert(endpoint.clone(), index);
                }
                winners.push(declaration);
            }

            for winner in winners {
                let state = contextual_state(winner, cwd.as_deref(), claude);
                let id = digest_parts(&[
                    "effective",
                    runner_key(runner),
                    cwd.as_deref().unwrap_or(""),
                    &winner.public.id,
                    &winner.public.effective_name,
                ]);
                let (capability, target) = toggle_for(home, winner, cwd.as_deref(), state, claude);
                targets.insert(id.clone(), target);
                let config_revision = file_revision(&winner.config_path);
                let health_revision = digest_parts(&[
                    "health-v1",
                    runner_key(runner),
                    cwd.as_deref().unwrap_or(""),
                    &winner.public.id,
                    &winner.public.effective_name,
                    &config_revision,
                    winner.endpoint_key.as_deref().unwrap_or(""),
                    declaration_state_key(state),
                ]);
                health_targets.insert(
                    id.clone(),
                    HealthTarget {
                        id: id.clone(),
                        declaration_id: winner.public.id.clone(),
                        revision: health_revision.clone(),
                        runner,
                        cwd: cwd.clone().unwrap_or_default(),
                        name: winner.public.effective_name.clone(),
                        raw_name: winner.public.name.clone(),
                        state,
                        endpoint: winner.health_endpoint.clone(),
                    },
                );
                let mut shadowed_ids = shadowed.remove(&winner.public.id).unwrap_or_default();
                shadowed_ids.sort();
                servers.push(Server {
                    id,
                    runner,
                    cwd: cwd.clone(),
                    declaration_id: winner.public.id.clone(),
                    name: winner.public.effective_name.clone(),
                    source: winner.public.source,
                    transport: winner.public.transport.clone(),
                    state,
                    shadowed_declaration_ids: shadowed_ids,
                    toggle: capability,
                    health_revision,
                    health: McpHealth::unchecked(),
                });
            }
        }
    }

    servers.sort_by(|a, b| {
        a.runner
            .cmp(&b.runner)
            .then_with(|| a.cwd.cmp(&b.cwd))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    (servers, targets, health_targets)
}

fn declaration_state_key(state: DeclarationState) -> &'static str {
    match state {
        DeclarationState::Enabled => "enabled",
        DeclarationState::Disabled => "disabled",
        DeclarationState::PendingApproval => "pending-approval",
        DeclarationState::Invalid => "invalid",
        DeclarationState::BlockedByPolicy => "blocked-by-policy",
        DeclarationState::Unknown => "unknown",
    }
}

fn applicable(declaration: &RawDeclaration, cwd: Option<&str>) -> bool {
    match declaration.public.project_key.as_deref() {
        Some(project) => cwd == Some(project),
        None => true,
    }
}

fn plugin_enabled_for_context(
    declaration: &RawDeclaration,
    cwd: Option<&str>,
    claude: &ClaudeState,
) -> bool {
    if declaration.public.runner != Runner::ClaudeCode
        || declaration.public.source != Source::Plugin
    {
        return true;
    }
    let Some(plugin_id) = declaration.public.origin.as_deref() else {
        return true;
    };
    if let Some(cwd) = cwd {
        if let Some(enabled) = claude
            .enabled_plugins_by_project
            .get(cwd)
            .and_then(|plugins| plugins.get(plugin_id))
        {
            return *enabled;
        }
    }
    claude
        .global_enabled_plugins
        .get(plugin_id)
        .copied()
        .unwrap_or(true)
}

fn contextual_state(
    declaration: &RawDeclaration,
    cwd: Option<&str>,
    claude: &ClaudeState,
) -> DeclarationState {
    let mut state = declaration.public.state;
    if declaration.public.runner == Runner::ClaudeCode
        && declaration.public.source != Source::Managed
        && state == DeclarationState::Enabled
    {
        if let Some(cwd) = cwd {
            if claude
                .disabled_by_project
                .get(cwd)
                .is_some_and(|disabled| disabled.contains(&declaration.public.effective_name))
            {
                state = DeclarationState::Disabled;
            }
        } else {
            state = DeclarationState::Unknown;
        }
    }
    state
}

/// Performs the only active MCP operation in the inventory subsystem.
///
/// The identifiers are opaque effective-instance ids from a fresh static scan;
/// callers cannot supply an executable, URL, config path, or server name. An
/// omitted selection checks every effective server for this runner and folder;
/// an explicit empty selection intentionally checks none.
pub fn check_health(
    projects: &[(String, PathBuf)],
    runner: Runner,
    cwd: &str,
    effective_ids: Option<&[String]>,
) -> Result<McpHealthSnapshot, String> {
    let cwd = canonical_context_cwd(cwd)?;
    let cwd_text = cwd.to_string_lossy().to_string();
    let home = crate::providers::home().ok_or("home directory is unavailable")?;
    let selection = resolve_health_selection(
        &home,
        projects,
        &cwd,
        runner,
        effective_ids,
        &managed_paths(),
    )?;
    let started = Instant::now();
    let deadline = started + HEALTH_TOTAL_TIMEOUT;
    let checked_at_ms = now_ms();
    let expires_at_ms = checked_at_ms.saturating_add(HEALTH_CACHE_TTL_MS);

    let (mut results, probe_complete) = match runner {
        Runner::ClaudeCode => {
            check_claude_targets(&selection.targets, checked_at_ms, expires_at_ms, deadline)
        }
        Runner::Codex => check_codex_targets(
            &selection.targets,
            &cwd,
            checked_at_ms,
            expires_at_ms,
            deadline,
        ),
    };
    results.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    let complete = probe_complete
        && results.len() >= selection.requested_ids.len()
        && results.iter().all(|result| {
            matches!(
                result.health.state,
                McpHealthState::Ready
                    | McpHealthState::Disabled
                    | McpHealthState::PendingApproval
                    | McpHealthState::AuthRequired
                    | McpHealthState::NeedsAuthentication
                    | McpHealthState::NotConfigured
                    | McpHealthState::Unsupported
                    | McpHealthState::BlockedByPolicy
            )
        });
    let snapshot = McpHealthSnapshot {
        runner,
        cwd: cwd_text,
        results,
        checked_at_ms,
        expires_at_ms,
        complete,
    };
    let took_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    cache_health_snapshot(
        &snapshot,
        &selection.targets,
        selection.all_effective,
        took_ms,
    );
    Ok(snapshot)
}

pub(crate) fn canonical_context_cwd(cwd: &str) -> Result<PathBuf, String> {
    let expanded = match (cwd.strip_prefix("~/"), crate::providers::home()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ if cwd == "~" => crate::providers::home().ok_or("home directory is unavailable")?,
        _ => PathBuf::from(cwd),
    };
    if cwd.trim().is_empty() {
        return Err("working directory is required".to_string());
    }
    let canonical = fs::canonicalize(&expanded)
        .map_err(|_| "working directory does not exist or cannot be read".to_string())?;
    if !canonical.is_dir() {
        return Err("working directory must be a directory".to_string());
    }
    Ok(canonical)
}

fn resolve_health_selection(
    home: &Path,
    projects: &[(String, PathBuf)],
    cwd: &Path,
    runner: Runner,
    effective_ids: Option<&[String]>,
    managed: &[PathBuf],
) -> Result<HealthSelection, String> {
    let canonical = cwd.to_string_lossy().to_string();
    let mut scoped = projects.to_vec();
    if !scoped
        .iter()
        .any(|(_, project)| canonical_project_key(project) == canonical)
    {
        scoped.push(("Current folder".into(), cwd.to_path_buf()));
    }
    let mut discovery = discover_at(home, &scoped, managed);
    let mut available: Vec<HealthTarget> = discovery
        .health_targets
        .drain()
        .map(|(_, target)| target)
        .filter(|target| target.runner == runner && target.cwd == canonical)
        .collect();
    available.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });

    let all_effective = effective_ids.is_none();
    let requested_ids: HashSet<String> = match effective_ids {
        Some(ids) => {
            if ids.len() > MAX_HEALTH_SERVERS {
                return Err(format!(
                    "at most {MAX_HEALTH_SERVERS} MCP servers can be checked at once"
                ));
            }
            let ids: HashSet<String> = ids.iter().cloned().collect();
            if ids.len() != effective_ids.map_or(0, |selected| selected.len()) {
                return Err("MCP server selection contains duplicate identifiers".to_string());
            }
            ids
        }
        None => available.iter().map(|target| target.id.clone()).collect(),
    };
    if requested_ids.len() > MAX_HEALTH_SERVERS {
        return Err(format!(
            "this context has more than {MAX_HEALTH_SERVERS} MCP servers; select a smaller set"
        ));
    }
    let targets: Vec<HealthTarget> = available
        .into_iter()
        .filter(|target| requested_ids.contains(&target.id))
        .collect();
    if targets.len() != requested_ids.len() {
        return Err(
            "one or more MCP servers no longer resolve for this runner and folder; refresh first"
                .to_string(),
        );
    }
    Ok(HealthSelection {
        targets,
        requested_ids,
        all_effective,
    })
}

fn immediate_health_result(
    target: &HealthTarget,
    checked_at_ms: u64,
    expires_at_ms: u64,
) -> Option<McpHealthResult> {
    let state = match (target.state, &target.endpoint) {
        (DeclarationState::Disabled, _) => McpHealthState::Disabled,
        (DeclarationState::PendingApproval, _) => McpHealthState::PendingApproval,
        (DeclarationState::BlockedByPolicy, _) => McpHealthState::BlockedByPolicy,
        (DeclarationState::Invalid, _) | (_, HealthEndpoint::Invalid) => McpHealthState::Failed,
        (_, HealthEndpoint::Unsupported) => McpHealthState::Unsupported,
        _ => return None,
    };
    Some(health_result(
        target,
        health_without_tools(state, checked_at_ms, expires_at_ms),
    ))
}

fn health_result(target: &HealthTarget, health: McpHealth) -> McpHealthResult {
    McpHealthResult {
        id: target.id.clone(),
        declaration_id: Some(target.declaration_id.clone()),
        revision: Some(target.revision.clone()),
        runner: target.runner,
        cwd: target.cwd.clone(),
        name: target.name.clone(),
        runner_provided: false,
        health,
    }
}

fn health_without_tools(
    state: McpHealthState,
    checked_at_ms: u64,
    expires_at_ms: u64,
) -> McpHealth {
    McpHealth {
        state,
        tools: ToolInventory {
            count: None,
            definitions: TokenMeasurement::unavailable(),
            checked_at_ms: Some(checked_at_ms),
        },
        checked_at_ms: Some(checked_at_ms),
        expires_at_ms: Some(expires_at_ms),
        stale: false,
    }
}

fn measured_health(
    state: McpHealthState,
    tools: &[Value],
    loaded: bool,
    checked_at_ms: u64,
    expires_at_ms: u64,
) -> Result<McpHealth, ProbeFailure> {
    if tools.len() > MAX_TOOLS {
        return Err(ProbeFailure::Oversized);
    }
    let normalized = Value::Array(tools.to_vec());
    let encoded = serde_json::to_string(&normalized).map_err(|_| ProbeFailure::Protocol)?;
    if encoded.len() > MAX_PROTOCOL_TOTAL_BYTES {
        return Err(ProbeFailure::Oversized);
    }
    Ok(McpHealth {
        state,
        tools: ToolInventory {
            count: Some(tools.len()),
            definitions: TokenMeasurement {
                tokens: Some(tokens::count(&encoded)),
                basis: TokenBasis::O200kSchemaEstimate,
                complete: true,
                loaded: Some(loaded),
                included_in_total: loaded,
            },
            checked_at_ms: Some(checked_at_ms),
        },
        checked_at_ms: Some(checked_at_ms),
        expires_at_ms: Some(expires_at_ms),
        stale: false,
    })
}

fn failure_health(failure: ProbeFailure, checked_at_ms: u64, expires_at_ms: u64) -> McpHealth {
    let state = match failure {
        ProbeFailure::Timeout => McpHealthState::TimedOut,
        ProbeFailure::Auth => McpHealthState::AuthRequired,
        ProbeFailure::Unsupported | ProbeFailure::ProtocolVersion => McpHealthState::Unsupported,
        ProbeFailure::Cancelled => McpHealthState::Cancelled,
        ProbeFailure::Launch
        | ProbeFailure::Io
        | ProbeFailure::Eof
        | ProbeFailure::Oversized
        | ProbeFailure::Protocol
        | ProbeFailure::Remote => McpHealthState::Failed,
    };
    health_without_tools(state, checked_at_ms, expires_at_ms)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn cache_result_key(runner: Runner, cwd: &str, id: &str, revision: &str) -> String {
    let context = digest_parts(&["context", runner_key(runner), cwd]);
    let identity = digest_parts(&["identity", id, revision]);
    format!("mcp-health-v1:{context}:{identity}")
}

fn context_cache_key(runner: Runner, cwd: &str, targets: &[HealthTarget]) -> String {
    let mut identities: Vec<String> = targets
        .iter()
        .map(|target| format!("{}:{}", target.id, target.revision))
        .collect();
    identities.sort();
    let identity_refs: Vec<&str> = identities.iter().map(String::as_str).collect();
    let inventory = digest_parts(&identity_refs);
    let context = digest_parts(&["context", runner_key(runner), cwd]);
    format!("mcp-health-context-v1:{context}:{inventory}")
}

fn cache_health_snapshot(
    snapshot: &McpHealthSnapshot,
    targets: &[HealthTarget],
    all_effective: bool,
    took_ms: u64,
) {
    for result in &snapshot.results {
        let Some(revision) = result.revision.as_deref() else {
            continue;
        };
        let key = cache_result_key(result.runner, &result.cwd, &result.id, revision);
        if let Ok(payload) = serde_json::to_string(result) {
            let _ = store::write_scan(&key, &payload, took_ms);
        }
    }
    if all_effective {
        let key = context_cache_key(snapshot.runner, &snapshot.cwd, targets);
        if let Ok(payload) = serde_json::to_string(snapshot) {
            let _ = store::write_scan(&key, &payload, took_ms);
        }
    }
}

/// Replaces any serialized health embedded in a broader static-scan cache with
/// exact per-context cache rows and recomputes expiry. Call this after decoding
/// a cached `McpSnapshot`; otherwise a five-minute health result could look
/// fresh for the lifetime of the generic scan row.
pub fn refresh_cached_health(snapshot: &mut McpSnapshot) {
    snapshot.health_results.clear();
    for server in &mut snapshot.servers {
        server.health = McpHealth::unchecked();
    }
    let now = now_ms();
    let mut results = Vec::new();
    for server in &mut snapshot.servers {
        let Some(cwd) = server.cwd.as_deref() else {
            continue;
        };
        let key = cache_result_key(server.runner, cwd, &server.id, &server.health_revision);
        let Some(hit) = store::read_scan(&key) else {
            continue;
        };
        let Some(result) = decode_cached_result(
            &hit.payload,
            server.runner,
            cwd,
            &server.id,
            &server.health_revision,
            now,
        ) else {
            continue;
        };
        server.health = result.health.clone();
        results.push(result);
    }

    let contexts: BTreeSet<(Runner, String)> = snapshot
        .servers
        .iter()
        .filter_map(|server| Some((server.runner, server.cwd.clone()?)))
        .collect();
    for (runner, cwd) in contexts {
        let targets: Vec<HealthTarget> = snapshot
            .servers
            .iter()
            .filter(|server| server.runner == runner && server.cwd.as_deref() == Some(&cwd))
            .map(|server| HealthTarget {
                id: server.id.clone(),
                declaration_id: server.declaration_id.clone(),
                revision: server.health_revision.clone(),
                runner,
                cwd: cwd.clone(),
                name: server.name.clone(),
                raw_name: server.name.clone(),
                state: server.state,
                endpoint: HealthEndpoint::Unsupported,
            })
            .collect();
        let key = context_cache_key(runner, &cwd, &targets);
        let Some(hit) = store::read_scan(&key) else {
            continue;
        };
        let Ok(cached) = serde_json::from_str::<McpHealthSnapshot>(&hit.payload) else {
            continue;
        };
        if cached.runner != runner || cached.cwd != cwd {
            continue;
        }
        for mut extra in cached
            .results
            .into_iter()
            .filter(|result| result.runner_provided)
        {
            if extra.runner != runner || extra.cwd != cwd || !valid_cached_health(&extra.health) {
                continue;
            }
            mark_health_stale(&mut extra.health, now);
            results.push(extra);
        }
    }
    results.sort_by(|a, b| a.id.cmp(&b.id));
    results.dedup_by(|a, b| a.id == b.id && a.revision == b.revision);
    snapshot.health_results = results;
}

fn decode_cached_result(
    payload: &str,
    runner: Runner,
    cwd: &str,
    id: &str,
    revision: &str,
    now: u64,
) -> Option<McpHealthResult> {
    let mut result: McpHealthResult = serde_json::from_str(payload).ok()?;
    if result.runner != runner
        || result.cwd != cwd
        || result.id != id
        || result.revision.as_deref() != Some(revision)
        || result.runner_provided
        || !valid_cached_health(&result.health)
    {
        return None;
    }
    mark_health_stale(&mut result.health, now);
    Some(result)
}

fn valid_cached_health(health: &McpHealth) -> bool {
    if matches!(
        health.state,
        McpHealthState::Checking | McpHealthState::Starting
    ) {
        return false;
    }
    if health
        .checked_at_ms
        .zip(health.expires_at_ms)
        .is_some_and(|(checked, expires)| expires < checked)
    {
        return false;
    }
    let measurement = &health.tools.definitions;
    if measurement.basis == TokenBasis::Unavailable && measurement.tokens.is_some() {
        return false;
    }
    if measurement.complete && measurement.tokens.is_none() {
        return false;
    }
    if measurement.included_in_total
        && (measurement.tokens.is_none() || measurement.loaded != Some(true))
    {
        return false;
    }
    true
}

fn mark_health_stale(health: &mut McpHealth, now: u64) {
    health.stale = health
        .expires_at_ms
        .is_some_and(|expires_at_ms| now > expires_at_ms);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeFailure {
    Launch,
    Io,
    Eof,
    Timeout,
    Oversized,
    Protocol,
    ProtocolVersion,
    Auth,
    Remote,
    Unsupported,
    Cancelled,
}

#[derive(Debug, Clone, Copy)]
enum WireDialect {
    JsonRpc,
    Codex,
}

enum WireFrame {
    Line(Vec<u8>),
    Eof,
    Oversized,
    Io,
}

struct WireWrite {
    bytes: Vec<u8>,
    acknowledged: mpsc::Sender<Result<(), ProbeFailure>>,
}

struct RpcProcess {
    child: std::process::Child,
    writer: mpsc::Sender<WireWrite>,
    frames: mpsc::Receiver<WireFrame>,
    dialect: WireDialect,
    deadline: Instant,
    next_id: u64,
    seen_frames: usize,
    stopped: bool,
}

impl RpcProcess {
    fn spawn(
        mut command: Command,
        dialect: WireDialect,
        deadline: Instant,
    ) -> Result<Self, ProbeFailure> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        configure_process_group(&mut command);
        let mut child = command.spawn().map_err(|_| ProbeFailure::Launch)?;
        let stdin = child.stdin.take().ok_or(ProbeFailure::Launch)?;
        let stdout = child.stdout.take().ok_or(ProbeFailure::Launch)?;
        let writer = spawn_wire_writer(stdin);
        let frames = spawn_wire_reader(stdout);
        Ok(Self {
            child,
            writer,
            frames,
            dialect,
            deadline,
            next_id: 1,
            seen_frames: 0,
            stopped: false,
        })
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, ProbeFailure> {
        let id = Value::Number(self.next_id.into());
        self.next_id = self.next_id.saturating_add(1);
        let request = match self.dialect {
            WireDialect::JsonRpc => json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            }),
            WireDialect::Codex => json!({
                "id": id,
                "method": method,
                "params": params,
            }),
        };
        self.send(&request)?;
        self.response(&id)
    }

    fn notify(&mut self, method: &str) -> Result<(), ProbeFailure> {
        let notification = match self.dialect {
            WireDialect::JsonRpc => json!({"jsonrpc": "2.0", "method": method}),
            WireDialect::Codex => json!({"method": method}),
        };
        self.send(&notification)
    }

    fn send(&mut self, value: &Value) -> Result<(), ProbeFailure> {
        let encoded = serde_json::to_vec(value).map_err(|_| ProbeFailure::Protocol)?;
        if encoded.len() > 64 * 1024 {
            return Err(ProbeFailure::Oversized);
        }
        let mut line = encoded;
        line.push(b'\n');
        let (acknowledged, received) = mpsc::channel();
        self.writer
            .send(WireWrite {
                bytes: line,
                acknowledged,
            })
            .map_err(|_| ProbeFailure::Io)?;
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .ok_or(ProbeFailure::Timeout)?
            .min(HEALTH_REQUEST_TIMEOUT);
        received
            .recv_timeout(remaining)
            .map_err(|_| ProbeFailure::Timeout)?
    }

    fn response(&mut self, expected_id: &Value) -> Result<Value, ProbeFailure> {
        loop {
            self.seen_frames = self.seen_frames.saturating_add(1);
            if self.seen_frames > MAX_PROTOCOL_FRAMES {
                return Err(ProbeFailure::Oversized);
            }
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .ok_or(ProbeFailure::Timeout)?
                .min(HEALTH_REQUEST_TIMEOUT);
            let frame = self
                .frames
                .recv_timeout(remaining)
                .map_err(|_| ProbeFailure::Timeout)?;
            let bytes = match frame {
                WireFrame::Line(bytes) => bytes,
                WireFrame::Eof => return Err(ProbeFailure::Eof),
                WireFrame::Oversized => return Err(ProbeFailure::Oversized),
                WireFrame::Io => return Err(ProbeFailure::Io),
            };
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| ProbeFailure::Protocol)?;
            let Some(object) = value.as_object() else {
                return Err(ProbeFailure::Protocol);
            };
            if object.get("id") == Some(expected_id) && object.get("method").is_none() {
                if let Some(error) = object.get("error") {
                    return Err(classify_remote_error(error));
                }
                return object.get("result").cloned().ok_or(ProbeFailure::Protocol);
            }
            if object.get("id").is_some() && object.get("method").is_some() {
                // Health checks never grant capabilities or perform work for a
                // server. Replying with method-not-found prevents a bounded
                // inventory request from waiting forever on an elicitation.
                let mut response = json!({
                    "id": object.get("id").cloned().unwrap_or(Value::Null),
                    "error": {"code": -32601, "message": "unsupported during health check"}
                });
                if matches!(self.dialect, WireDialect::JsonRpc) {
                    response["jsonrpc"] = Value::String("2.0".into());
                }
                self.send(&response)?;
            }
        }
    }

    fn stop(&mut self) {
        if !self.stopped {
            self.stopped = true;
            terminate_process_group(&mut self.child);
        }
    }
}

impl Drop for RpcProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

fn spawn_wire_writer(stdin: std::process::ChildStdin) -> mpsc::Sender<WireWrite> {
    let (tx, rx) = mpsc::channel::<WireWrite>();
    thread::spawn(move || {
        let mut stdin = stdin;
        for request in rx {
            let outcome = stdin
                .write_all(&request.bytes)
                .and_then(|_| stdin.flush())
                .map_err(|_| ProbeFailure::Io);
            let failed = outcome.is_err();
            let _ = request.acknowledged.send(outcome);
            if failed {
                return;
            }
        }
    });
    tx
}

fn spawn_wire_reader(stdout: std::process::ChildStdout) -> mpsc::Receiver<WireFrame> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut total = 0usize;
        loop {
            match read_protocol_line(&mut reader, &mut total) {
                Ok(Some(bytes)) => {
                    if tx.send(WireFrame::Line(bytes)).is_err() {
                        return;
                    }
                }
                Ok(None) => {
                    let _ = tx.send(WireFrame::Eof);
                    return;
                }
                Err(ProbeFailure::Oversized) => {
                    let _ = tx.send(WireFrame::Oversized);
                    return;
                }
                Err(_) => {
                    let _ = tx.send(WireFrame::Io);
                    return;
                }
            }
        }
    });
    rx
}

fn read_protocol_line(
    reader: &mut impl BufRead,
    total: &mut usize,
) -> Result<Option<Vec<u8>>, ProbeFailure> {
    let mut bytes = Vec::new();
    let mut saw_data = false;
    let mut oversized = false;
    loop {
        let buffer = reader.fill_buf().map_err(|_| ProbeFailure::Io)?;
        if buffer.is_empty() {
            if !saw_data {
                return Ok(None);
            }
            break;
        }
        saw_data = true;
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(buffer.len(), |position| position + 1);
        let content_len = newline.unwrap_or(buffer.len());
        *total = total.saturating_add(consumed);
        if *total > MAX_PROTOCOL_TOTAL_BYTES {
            return Err(ProbeFailure::Oversized);
        }
        if !oversized {
            if bytes.len().saturating_add(content_len) > MAX_PROTOCOL_LINE_BYTES {
                oversized = true;
                bytes.clear();
            } else {
                bytes.extend_from_slice(&buffer[..content_len]);
            }
        }
        reader.consume(consumed);
        if newline.is_some() {
            break;
        }
    }
    if oversized {
        return Err(ProbeFailure::Oversized);
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    Ok(Some(bytes))
}

fn classify_remote_error(error: &Value) -> ProbeFailure {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let code = error.get("code").and_then(Value::as_i64);
    if matches!(code, Some(401 | 403))
        || [
            "auth",
            "unauthor",
            "forbidden",
            "login",
            "log in",
            "credential",
        ]
        .iter()
        .any(|needle| message.contains(needle))
    {
        ProbeFailure::Auth
    } else if ["protocol", "version"]
        .iter()
        .any(|needle| message.contains(needle))
    {
        ProbeFailure::ProtocolVersion
    } else {
        ProbeFailure::Remote
    }
}

fn normalize_tool(tool: &Value, expected_name: Option<&str>) -> Result<Value, ProbeFailure> {
    let object = tool.as_object().ok_or(ProbeFailure::Protocol)?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .or(expected_name)
        .filter(|name| !name.is_empty() && name.len() <= 256)
        .ok_or(ProbeFailure::Protocol)?;
    if let Some(expected) = expected_name {
        if object
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|reported| reported != expected)
        {
            return Err(ProbeFailure::Protocol);
        }
    }
    let input_schema = object
        .get("inputSchema")
        .cloned()
        .ok_or(ProbeFailure::Protocol)?;
    let mut normalized = serde_json::Map::new();
    normalized.insert("name".into(), Value::String(name.to_string()));
    for key in [
        "title",
        "description",
        "inputSchema",
        "outputSchema",
        "annotations",
    ] {
        if let Some(value) = object.get(key) {
            normalized.insert(key.into(), value.clone());
        }
    }
    normalized.insert("inputSchema".into(), input_schema);
    Ok(Value::Object(normalized))
}

fn check_claude_targets(
    targets: &[HealthTarget],
    checked_at_ms: u64,
    expires_at_ms: u64,
    deadline: Instant,
) -> (Vec<McpHealthResult>, bool) {
    let mut results = Vec::with_capacity(targets.len());
    let mut complete = true;
    for target in targets {
        if let Some(result) = immediate_health_result(target, checked_at_ms, expires_at_ms) {
            results.push(result);
            continue;
        }
        if Instant::now() >= deadline {
            complete = false;
            results.push(health_result(
                target,
                failure_health(ProbeFailure::Cancelled, checked_at_ms, expires_at_ms),
            ));
            continue;
        }
        let health = match &target.endpoint {
            HealthEndpoint::DirectStdio { .. } => {
                match run_claude_probe(target, checked_at_ms, expires_at_ms, deadline) {
                    Ok(health) => health,
                    Err(failure) => {
                        complete = false;
                        failure_health(failure, checked_at_ms, expires_at_ms)
                    }
                }
            }
            HealthEndpoint::RunnerManaged | HealthEndpoint::Unsupported => {
                complete = false;
                failure_health(ProbeFailure::Unsupported, checked_at_ms, expires_at_ms)
            }
            HealthEndpoint::Invalid => {
                complete = false;
                failure_health(ProbeFailure::Protocol, checked_at_ms, expires_at_ms)
            }
        };
        results.push(health_result(target, health));
    }
    (results, complete)
}

fn run_claude_probe(
    target: &HealthTarget,
    checked_at_ms: u64,
    expires_at_ms: u64,
    deadline: Instant,
) -> Result<McpHealth, ProbeFailure> {
    let mut last = ProbeFailure::ProtocolVersion;
    for version in MCP_PROTOCOL_VERSIONS {
        if Instant::now() >= deadline {
            return Err(ProbeFailure::Timeout);
        }
        match run_claude_probe_version(target, version, checked_at_ms, expires_at_ms, deadline) {
            Err(ProbeFailure::ProtocolVersion) => last = ProbeFailure::ProtocolVersion,
            result => return result,
        }
    }
    Err(last)
}

fn run_claude_probe_version(
    target: &HealthTarget,
    version: &str,
    checked_at_ms: u64,
    expires_at_ms: u64,
    deadline: Instant,
) -> Result<McpHealth, ProbeFailure> {
    let HealthEndpoint::DirectStdio {
        command,
        args,
        env,
        working_directory,
    } = &target.endpoint
    else {
        return Err(ProbeFailure::Unsupported);
    };
    let mut launch = Command::new(command);
    launch.args(args).envs(env);
    let current_dir = configured_working_directory(target, working_directory.as_deref())?;
    launch.current_dir(current_dir);
    let request_deadline = deadline.min(Instant::now() + HEALTH_REQUEST_TIMEOUT);
    let mut client = RpcProcess::spawn(launch, WireDialect::JsonRpc, request_deadline)?;
    let initialized = client.request(
        "initialize",
        json!({
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": {
                "name": "aviary-health",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    )?;
    let negotiated = initialized
        .get("protocolVersion")
        .and_then(Value::as_str)
        .ok_or(ProbeFailure::ProtocolVersion)?;
    if !MCP_PROTOCOL_VERSIONS.contains(&negotiated) {
        return Err(ProbeFailure::ProtocolVersion);
    }
    client.notify("notifications/initialized")?;

    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen_cursors = HashSet::new();
    for _ in 0..MAX_PROTOCOL_PAGES {
        let params = cursor
            .as_ref()
            .map(|cursor| json!({"cursor": cursor}))
            .unwrap_or_else(|| json!({}));
        let result = client.request("tools/list", params)?;
        let page = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or(ProbeFailure::Protocol)?;
        for tool in page {
            if tools.len() >= MAX_TOOLS {
                return Err(ProbeFailure::Oversized);
            }
            tools.push(normalize_tool(tool, None)?);
        }
        cursor = match result.get("nextCursor") {
            None | Some(Value::Null) => None,
            Some(Value::String(cursor)) if !cursor.is_empty() => Some(cursor.clone()),
            _ => return Err(ProbeFailure::Protocol),
        };
        let Some(next) = cursor.as_ref() else {
            return measured_health(
                McpHealthState::Ready,
                &tools,
                true,
                checked_at_ms,
                expires_at_ms,
            );
        };
        if !seen_cursors.insert(next.clone()) {
            return Err(ProbeFailure::Protocol);
        }
    }
    Err(ProbeFailure::Oversized)
}

fn configured_working_directory(
    target: &HealthTarget,
    configured: Option<&str>,
) -> Result<PathBuf, ProbeFailure> {
    let path = match configured {
        None => PathBuf::from(&target.cwd),
        Some("~") => crate::providers::home().ok_or(ProbeFailure::Launch)?,
        Some(value) if value.starts_with("~/") => crate::providers::home()
            .ok_or(ProbeFailure::Launch)?
            .join(value.trim_start_matches("~/")),
        Some(value) => {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                // The runner launches stdio servers from the selected
                // workspace. Resolving a relative override beside the config
                // file would silently probe a different process than Claude.
                Path::new(&target.cwd).join(path)
            }
        }
    };
    let canonical = fs::canonicalize(path).map_err(|_| ProbeFailure::Launch)?;
    canonical
        .is_dir()
        .then_some(canonical)
        .ok_or(ProbeFailure::Launch)
}

struct CodexInventoryRow {
    name: String,
    health: McpHealth,
}

fn check_codex_targets(
    targets: &[HealthTarget],
    cwd: &Path,
    checked_at_ms: u64,
    expires_at_ms: u64,
    deadline: Instant,
) -> (Vec<McpHealthResult>, bool) {
    let mut results = Vec::with_capacity(targets.len());
    let mut active = Vec::new();
    for target in targets {
        if let Some(result) = immediate_health_result(target, checked_at_ms, expires_at_ms) {
            results.push(result);
        } else if matches!(target.endpoint, HealthEndpoint::RunnerManaged) {
            active.push(target);
        } else {
            results.push(health_result(
                target,
                failure_health(ProbeFailure::Unsupported, checked_at_ms, expires_at_ms),
            ));
        }
    }
    if active.is_empty() {
        return (results, true);
    }

    let inventory = match run_codex_probe(cwd, checked_at_ms, expires_at_ms, deadline) {
        Ok(inventory) => inventory,
        Err(failure) => {
            for target in active {
                results.push(health_result(
                    target,
                    failure_health(failure, checked_at_ms, expires_at_ms),
                ));
            }
            return (results, false);
        }
    };

    let mut effective: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut raw: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, target) in active.iter().enumerate() {
        effective
            .entry(target.name.as_str())
            .or_default()
            .push(index);
        raw.entry(target.raw_name.as_str()).or_default().push(index);
    }
    let mut assigned = HashSet::new();
    let mut runner_provided_count = 0usize;
    for row in inventory {
        let index = effective
            .get(row.name.as_str())
            .filter(|matches| matches.len() == 1)
            .map(|matches| matches[0])
            .or_else(|| {
                raw.get(row.name.as_str())
                    .filter(|matches| matches.len() == 1)
                    .map(|matches| matches[0])
            });
        if let Some(index) = index.filter(|index| assigned.insert(*index)) {
            results.push(health_result(active[index], row.health));
        } else {
            runner_provided_count = runner_provided_count.saturating_add(1);
            let name = format!("Runner-provided server {runner_provided_count}");
            results.push(McpHealthResult {
                id: digest_parts(&[
                    "runner-provided",
                    runner_key(Runner::Codex),
                    &cwd.to_string_lossy(),
                    &row.name,
                ]),
                declaration_id: None,
                revision: None,
                runner: Runner::Codex,
                cwd: cwd.to_string_lossy().to_string(),
                name,
                runner_provided: true,
                health: row.health,
            });
        }
    }
    for (index, target) in active.into_iter().enumerate() {
        if !assigned.contains(&index) {
            results.push(health_result(
                target,
                health_without_tools(McpHealthState::NotConfigured, checked_at_ms, expires_at_ms),
            ));
        }
    }
    (results, true)
}

fn run_codex_probe(
    cwd: &Path,
    checked_at_ms: u64,
    expires_at_ms: u64,
    deadline: Instant,
) -> Result<Vec<CodexInventoryRow>, ProbeFailure> {
    let mut launch = Command::new("codex");
    launch.args(["app-server", "--stdio"]).current_dir(cwd);
    run_codex_probe_command(launch, checked_at_ms, expires_at_ms, deadline)
}

fn run_codex_probe_command(
    launch: Command,
    checked_at_ms: u64,
    expires_at_ms: u64,
    deadline: Instant,
) -> Result<Vec<CodexInventoryRow>, ProbeFailure> {
    let mut client = RpcProcess::spawn(launch, WireDialect::Codex, deadline)?;
    let initialized = client.request(
        "initialize",
        json!({
            "clientInfo": {
                "name": "aviary-health",
                "title": "Aviary",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": {"experimentalApi": true}
        }),
    )?;
    if !initialized.is_object() {
        return Err(ProbeFailure::Protocol);
    }
    client.notify("initialized")?;

    let mut rows = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen_names = HashSet::new();
    let mut seen_cursors = HashSet::new();
    for _ in 0..MAX_PROTOCOL_PAGES {
        let mut params = json!({"limit": 100, "detail": "toolsAndAuthOnly"});
        if let Some(cursor) = &cursor {
            params["cursor"] = Value::String(cursor.clone());
        }
        let result = client.request("mcpServerStatus/list", params)?;
        let data = result
            .get("data")
            .and_then(Value::as_array)
            .ok_or(ProbeFailure::Protocol)?;
        for status in data {
            if rows.len() >= MAX_HEALTH_SERVERS * 4 {
                return Err(ProbeFailure::Oversized);
            }
            let object = status.as_object().ok_or(ProbeFailure::Protocol)?;
            let name = object
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty() && name.len() <= 512)
                .ok_or(ProbeFailure::Protocol)?
                .to_string();
            if !seen_names.insert(name.clone()) {
                return Err(ProbeFailure::Protocol);
            }
            let tools_object = object
                .get("tools")
                .and_then(Value::as_object)
                .ok_or(ProbeFailure::Protocol)?;
            let mut tools = Vec::with_capacity(tools_object.len());
            for (tool_name, tool) in tools_object {
                tools.push(normalize_tool(tool, Some(tool_name))?);
            }
            let auth = object
                .get("authStatus")
                .and_then(Value::as_str)
                .ok_or(ProbeFailure::Protocol)?;
            let (state, loaded) = match auth {
                "notLoggedIn" => (McpHealthState::AuthRequired, false),
                "unsupported" | "bearerToken" | "oAuth" => {
                    if object.get("serverInfo").is_some_and(Value::is_object) || !tools.is_empty() {
                        (McpHealthState::Ready, true)
                    } else {
                        (McpHealthState::Degraded, false)
                    }
                }
                _ => return Err(ProbeFailure::Protocol),
            };
            let health = measured_health(state, &tools, loaded, checked_at_ms, expires_at_ms)?;
            rows.push(CodexInventoryRow { name, health });
        }
        cursor = match result.get("nextCursor") {
            None | Some(Value::Null) => None,
            Some(Value::String(cursor)) if !cursor.is_empty() => Some(cursor.clone()),
            _ => return Err(ProbeFailure::Protocol),
        };
        let Some(next) = cursor.as_ref() else {
            return Ok(rows);
        };
        if !seen_cursors.insert(next.clone()) {
            return Err(ProbeFailure::Protocol);
        }
    }
    Err(ProbeFailure::Oversized)
}

fn toggle_for(
    _home: &Path,
    declaration: &RawDeclaration,
    cwd: Option<&str>,
    state: DeclarationState,
    claude: &ClaudeState,
) -> (ToggleCapability, ToggleTargetResolution) {
    if state == DeclarationState::PendingApproval {
        let reason = ToggleUnavailableReason::PendingApproval;
        return (
            ToggleCapability::unavailable(reason),
            ToggleTargetResolution::Unavailable(reason),
        );
    }
    if state == DeclarationState::Invalid {
        let reason = ToggleUnavailableReason::InvalidConfiguration;
        return (
            ToggleCapability::unavailable(reason),
            ToggleTargetResolution::Unavailable(reason),
        );
    }

    match &declaration.toggle {
        ToggleBase::ClaudeRegular { settings_path } => {
            let Some(cwd) = cwd else {
                let reason = ToggleUnavailableReason::ProjectRequired;
                return (
                    ToggleCapability::unavailable(reason),
                    ToggleTargetResolution::Unavailable(reason),
                );
            };
            let revision = file_revision(settings_path);
            let target = ToggleTarget {
                path: settings_path.clone(),
                revision: revision.clone(),
                mutation: ToggleMutation::ClaudeDisabledList {
                    project_key: claude
                        .raw_project_keys
                        .get(cwd)
                        .cloned()
                        .unwrap_or_else(|| cwd.to_string()),
                    server_name: declaration.public.effective_name.clone(),
                },
            };
            (
                ToggleCapability {
                    writable: true,
                    revision: Some(revision),
                    shared_project_file: false,
                    unavailable_reason: None,
                },
                ToggleTargetResolution::Writable(target),
            )
        }
        ToggleBase::ClaudeDefaultOff { settings_path } => {
            let Some(cwd) = cwd else {
                let reason = ToggleUnavailableReason::ProjectRequired;
                return (
                    ToggleCapability::unavailable(reason),
                    ToggleTargetResolution::Unavailable(reason),
                );
            };
            let revision = file_revision(settings_path);
            let target = ToggleTarget {
                path: settings_path.clone(),
                revision: revision.clone(),
                mutation: ToggleMutation::ClaudeEnabledList {
                    project_key: claude
                        .raw_project_keys
                        .get(cwd)
                        .cloned()
                        .unwrap_or_else(|| cwd.to_string()),
                    server_name: declaration.public.effective_name.clone(),
                },
            };
            (
                ToggleCapability {
                    writable: true,
                    revision: Some(revision),
                    shared_project_file: false,
                    unavailable_reason: None,
                },
                ToggleTargetResolution::Writable(target),
            )
        }
        ToggleBase::CodexServer { config_path } => {
            let revision = file_revision(config_path);
            let target = ToggleTarget {
                path: config_path.clone(),
                revision: revision.clone(),
                mutation: ToggleMutation::CodexServer {
                    server_name: declaration.public.name.clone(),
                },
            };
            (
                ToggleCapability {
                    writable: true,
                    revision: Some(revision),
                    shared_project_file: declaration.public.source == Source::Project,
                    unavailable_reason: None,
                },
                ToggleTargetResolution::Writable(target),
            )
        }
        ToggleBase::CodexPlugin {
            config_path,
            plugin_name,
        } => {
            let revision = file_revision(config_path);
            let target = ToggleTarget {
                path: config_path.clone(),
                revision: revision.clone(),
                mutation: ToggleMutation::CodexPluginServer {
                    plugin_name: plugin_name.clone(),
                    server_name: declaration.public.name.clone(),
                },
            };
            (
                ToggleCapability {
                    writable: true,
                    revision: Some(revision),
                    shared_project_file: declaration.public.source == Source::Project,
                    unavailable_reason: None,
                },
                ToggleTargetResolution::Writable(target),
            )
        }
        ToggleBase::Unavailable(reason) => (
            ToggleCapability::unavailable(*reason),
            ToggleTargetResolution::Unavailable(*reason),
        ),
    }
}

pub(crate) fn file_revision(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"aviary-mcp-revision-v1\0");
    if let Ok(bytes) = fs::read(path) {
        hasher.update(b"present\0");
        hasher.update(bytes);
    } else {
        hasher.update(b"absent\0");
        hasher.update(canonical_origin(path).as_bytes());
    }
    format!("r1:{}", hex_digest(hasher.finalize()))
}

fn digest_parts(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    format!("mcp:{}", hex_digest(hasher.finalize()))
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    let mut out = String::with_capacity(bytes.as_ref().len() * 2);
    for byte in bytes.as_ref() {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn string_set(value: Option<&serde_json::Value>) -> Option<HashSet<String>> {
    value?.as_array().map(|values| {
        values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect()
    })
}

fn json_object_keys(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(serde_json::Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

fn toml_table_keys(value: Option<&toml::Value>) -> Vec<String> {
    value
        .and_then(toml::Value::as_table)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

fn canonical_project_key(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn canonical_origin(path: &Path) -> String {
    if let Ok(path) = fs::canonicalize(path) {
        return path.to_string_lossy().to_string();
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        if let Ok(parent) = fs::canonicalize(parent) {
            return parent.join(name).to_string_lossy().to_string();
        }
    }
    path.to_string_lossy().to_string()
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn project_name_for_key(key: &str, projects: &[(String, PathBuf)]) -> Option<String> {
    projects
        .iter()
        .find(|(_, path)| canonical_project_key(path) == key)
        .map(|(name, _)| name.clone())
        .or_else(|| {
            Path::new(key)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
}

fn json_pointer_escape(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn runner_key(runner: Runner) -> &'static str {
    match runner {
        Runner::ClaudeCode => "claude-code",
        Runner::Codex => "codex",
    }
}

fn source_key(source: Source) -> &'static str {
    match source {
        Source::Managed => "managed",
        Source::Local => "local",
        Source::Project => "project",
        Source::User => "user",
        Source::Plugin => "plugin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        write(&path, body);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn direct_target(command: &Path, cwd: &Path) -> HealthTarget {
        HealthTarget {
            id: "effective-id".into(),
            declaration_id: "declaration-id".into(),
            revision: "revision-id".into(),
            runner: Runner::ClaudeCode,
            cwd: cwd.to_string_lossy().to_string(),
            name: "fixture".into(),
            raw_name: "fixture".into(),
            state: DeclarationState::Enabled,
            endpoint: HealthEndpoint::DirectStdio {
                command: command.to_string_lossy().to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
                working_directory: None,
            },
        }
    }

    #[test]
    fn same_name_declarations_never_merge_across_runner_or_scope() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let project = home.join("work/demo");
        fs::create_dir_all(&project).unwrap();
        write(
            &home.join(".claude.json"),
            r#"{"mcpServers":{"shared":{"type":"http","url":"https://user.example/mcp"}}}"#,
        );
        write(
            &project.join(".mcp.json"),
            r#"{"mcpServers":{"shared":{"type":"http","url":"https://project.example/mcp"}}}"#,
        );
        write(
            &home.join(".claude/settings.json"),
            r#"{"enabledMcpjsonServers":["shared"]}"#,
        );
        write(
            &home.join(".codex/config.toml"),
            "[mcp_servers.shared]\nurl = \"https://codex.example/mcp\"\n",
        );

        let snapshot = discover_at_with_codex_plugins(
            &home,
            &[("demo".into(), project.clone())],
            &[],
            Some(&[]),
        )
        .snapshot;
        let declarations: Vec<_> = snapshot
            .declarations
            .iter()
            .filter(|declaration| declaration.name == "shared")
            .collect();
        assert_eq!(declarations.len(), 3);
        assert_eq!(
            declarations
                .iter()
                .map(|declaration| declaration.id.as_str())
                .collect::<HashSet<_>>()
                .len(),
            3
        );

        let cwd = canonical_project_key(&project);
        let claude = snapshot
            .servers
            .iter()
            .find(|server| {
                server.runner == Runner::ClaudeCode && server.cwd.as_deref() == Some(&cwd)
            })
            .unwrap();
        assert_eq!(claude.source, Source::Project);
        assert_eq!(claude.shadowed_declaration_ids.len(), 1);
        let codex = snapshot
            .servers
            .iter()
            .find(|server| server.runner == Runner::Codex && server.cwd.as_deref() == Some(&cwd))
            .unwrap();
        assert_eq!(codex.source, Source::User);
        assert_ne!(claude.id, codex.id);
    }

    #[test]
    fn cached_claude_plugin_is_inert_until_installed_and_enabled() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let active = home.join(".claude/plugins/cache/market/active/1");
        let orphan = home.join(".claude/plugins/cache/market/orphan/9");
        write(
            &active.join(".mcp.json"),
            r#"{"mcpServers":{"active-server":{"command":"npx","args":["active"]}}}"#,
        );
        write(
            &orphan.join(".mcp.json"),
            r#"{"mcpServers":{"orphan-server":{"command":"npx","args":["orphan"]}}}"#,
        );
        write(
            &home.join(".claude/plugins/installed_plugins.json"),
            &serde_json::json!({
                "version": 2,
                "plugins": {
                    "active@market": [{"installPath": active, "scope": "user"}]
                }
            })
            .to_string(),
        );
        write(
            &home.join(".claude/settings.json"),
            r#"{"enabledPlugins":{"active@market":true}}"#,
        );

        let snapshot = discover_at_with_codex_plugins(&home, &[], &[], Some(&[])).snapshot;
        assert!(snapshot
            .declarations
            .iter()
            .any(|declaration| declaration.name == "active-server"));
        assert!(!snapshot
            .declarations
            .iter()
            .any(|declaration| declaration.name == "orphan-server"));
    }

    #[test]
    fn serialized_inventory_contains_no_launch_secrets() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        write(
            &home.join(".claude.json"),
            r#"{
              "mcpServers": {
                "stdio": {
                  "command": "npx",
                  "args": ["--token", "ARG_SECRET_CANARY"],
                  "env": {"API_TOKEN": "ENV_SECRET_CANARY"}
                },
                "remote": {
                  "type": "http",
                  "url": "https://user:URL_SECRET_CANARY@example.com/private/token?key=QUERY_SECRET_CANARY",
                  "headers": {"Authorization": "HEADER_SECRET_CANARY"}
                }
              }
            }"#,
        );

        let snapshot = discover_at_with_codex_plugins(&home, &[], &[], Some(&[])).snapshot;
        let serialized = serde_json::to_string(&snapshot).unwrap();
        for secret in [
            "ARG_SECRET_CANARY",
            "ENV_SECRET_CANARY",
            "URL_SECRET_CANARY",
            "QUERY_SECRET_CANARY",
            "HEADER_SECRET_CANARY",
            "/private/token",
        ] {
            assert!(!serialized.contains(secret), "leaked {secret}");
        }
        assert!(serialized.contains("API_TOKEN"));
        assert!(serialized.contains("Authorization"));
        assert!(serialized.contains("example.com"));
    }

    #[test]
    fn unknown_inventory_never_uses_zero_as_a_sentinel() {
        let inventory = ToolInventory::unchecked();
        assert_eq!(inventory.count, None);
        assert_eq!(inventory.definitions.tokens, None);
        assert!(!inventory.definitions.complete);
        assert_eq!(inventory.definitions.loaded, None);
        assert!(!inventory.definitions.included_in_total);
    }

    #[test]
    fn transport_summary_wire_shape_matches_the_ipc_normalizer() {
        let value = serde_json::to_value(TransportSummary::Stdio {
            launcher: LauncherKind::Node,
            argument_count: 2,
            env_keys: vec!["TOKEN".into()],
            inherited_env_keys: vec!["PATH".into()],
            has_working_directory: true,
        })
        .unwrap();
        assert_eq!(value["kind"], "stdio");
        assert_eq!(value["argument_count"], 2);
        assert!(value.get("argumentCount").is_none());

        let runner = serde_json::to_value(TransportSummary::RunnerProvided).unwrap();
        assert_eq!(runner["kind"], "runnerProvided");
    }

    #[test]
    fn static_discovery_never_starts_declared_servers() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let project = home.join("work/demo");
        fs::create_dir_all(&project).unwrap();
        let marker = tmp.path().join("SERVER_WAS_STARTED");
        write(
            &project.join(".mcp.json"),
            &serde_json::json!({
                "mcpServers": {
                    "trap": {
                        "command": "/usr/bin/touch",
                        "args": [marker]
                    }
                }
            })
            .to_string(),
        );

        let snapshot =
            discover_at_with_codex_plugins(&home, &[("demo".into(), project)], &[], Some(&[]))
                .snapshot;
        assert!(snapshot.servers.iter().any(|server| server.name == "trap"));
        assert!(!marker.exists(), "a passive scan launched an MCP server");
    }

    #[cfg(unix)]
    #[test]
    fn claude_health_client_initializes_and_paginates_tools() {
        let tmp = TempDir::new().unwrap();
        let server = script(
            tmp.path(),
            "fake-claude-mcp",
            r#"#!/bin/sh
IFS= read -r _init || exit 1
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}}}}'
IFS= read -r _initialized || exit 1
IFS= read -r _first || exit 1
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"first","description":"SECRET_SCHEMA_CANARY","inputSchema":{"type":"object"}}],"nextCursor":"page-2"}}'
IFS= read -r _second || exit 1
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"tools":[{"name":"second","inputSchema":{"type":"object","properties":{"value":{"type":"string"}}}}]}}'
"#,
        );
        let target = direct_target(&server, tmp.path());
        let health =
            run_claude_probe(&target, 100, 200, Instant::now() + Duration::from_secs(3)).unwrap();
        assert_eq!(health.state, McpHealthState::Ready);
        assert_eq!(health.tools.count, Some(2));
        assert_eq!(
            health.tools.definitions.basis,
            TokenBasis::O200kSchemaEstimate
        );
        assert!(health
            .tools
            .definitions
            .tokens
            .is_some_and(|tokens| tokens > 0));
        assert!(health.tools.definitions.complete);
        assert_eq!(health.tools.definitions.loaded, Some(true));
        let public = serde_json::to_string(&health).unwrap();
        assert!(!public.contains("SECRET_SCHEMA_CANARY"));
        assert!(!public.contains("inputSchema"));
    }

    #[cfg(unix)]
    #[test]
    fn claude_health_surfaces_auth_without_forwarding_runner_error() {
        let tmp = TempDir::new().unwrap();
        let server = script(
            tmp.path(),
            "fake-auth-mcp",
            r#"#!/bin/sh
IFS= read -r _init || exit 1
printf '%s\n' '{"jsonrpc":"2.0","id":1,"error":{"code":401,"message":"unauthorized SECRET_ERROR_CANARY"}}'
"#,
        );
        let target = direct_target(&server, tmp.path());
        let failure = run_claude_probe(&target, 100, 200, Instant::now() + Duration::from_secs(2))
            .unwrap_err();
        assert_eq!(failure, ProbeFailure::Auth);
        let health = failure_health(failure, 100, 200);
        assert_eq!(health.state, McpHealthState::AuthRequired);
        assert!(!serde_json::to_string(&health)
            .unwrap()
            .contains("SECRET_ERROR_CANARY"));
    }

    #[cfg(unix)]
    #[test]
    fn health_timeout_kills_the_isolated_process_group() {
        let tmp = TempDir::new().unwrap();
        let server = script(
            tmp.path(),
            "fake-hung-mcp",
            r#"#!/bin/sh
IFS= read -r _init || exit 1
/bin/sleep 10
"#,
        );
        let target = direct_target(&server, tmp.path());
        let started = Instant::now();
        let failure = run_claude_probe(
            &target,
            100,
            200,
            Instant::now() + Duration::from_millis(150),
        )
        .unwrap_err();
        assert_eq!(failure, ProbeFailure::Timeout);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn protocol_writes_are_bounded_when_a_server_never_reads_stdin() {
        let mut command = Command::new("/bin/sleep");
        command.arg("10");
        let started = Instant::now();
        let mut client = RpcProcess::spawn(
            command,
            WireDialect::JsonRpc,
            Instant::now() + Duration::from_millis(250),
        )
        .unwrap();
        let payload = serde_json::json!({"fill": "x".repeat(60_000)});
        let mut timed_out = false;
        for _ in 0..16 {
            match client.send(&payload) {
                Ok(()) => {}
                Err(ProbeFailure::Timeout) => {
                    timed_out = true;
                    break;
                }
                Err(other) => panic!("unexpected write result: {other:?}"),
            }
        }
        assert!(
            timed_out,
            "the non-reading child never applied backpressure"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn protocol_reader_rejects_oversized_frames_without_allocating_the_rest() {
        let input = vec![b'x'; MAX_PROTOCOL_LINE_BYTES + 1];
        let mut reader = std::io::Cursor::new(input);
        let mut total = 0;
        assert_eq!(
            read_protocol_line(&mut reader, &mut total).unwrap_err(),
            ProbeFailure::Oversized
        );
    }

    #[cfg(unix)]
    #[test]
    fn codex_health_client_uses_only_status_inventory_and_paginates() {
        let tmp = TempDir::new().unwrap();
        let server = script(
            tmp.path(),
            "fake-codex-app-server",
            r#"#!/bin/sh
IFS= read -r _init || exit 1
printf '%s\n' '{"id":1,"result":{"userAgent":"fixture"}}'
IFS= read -r _initialized || exit 1
IFS= read -r _first || exit 1
printf '%s\n' '{"id":2,"result":{"data":[{"name":"ready","authStatus":"unsupported","serverInfo":{"name":"ready","version":"1"},"tools":{"read":{"name":"read","description":"SECRET_CODEX_SCHEMA","inputSchema":{"type":"object"}}},"resources":[],"resourceTemplates":[]}],"nextCursor":"next"}}'
IFS= read -r _second || exit 1
printf '%s\n' '{"id":3,"result":{"data":[{"name":"login","authStatus":"notLoggedIn","tools":{},"resources":[],"resourceTemplates":[]}]}}'
"#,
        );
        let mut command = Command::new(&server);
        command.current_dir(tmp.path());
        let rows =
            run_codex_probe_command(command, 100, 200, Instant::now() + Duration::from_secs(3))
                .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "ready");
        assert_eq!(rows[0].health.state, McpHealthState::Ready);
        assert_eq!(rows[0].health.tools.count, Some(1));
        assert_eq!(rows[1].health.state, McpHealthState::AuthRequired);
        assert_eq!(rows[1].health.tools.definitions.loaded, Some(false));
        assert!(!serde_json::to_string(&rows[0].health)
            .unwrap()
            .contains("SECRET_CODEX_SCHEMA"));
    }

    #[test]
    fn cached_health_requires_exact_context_identity_and_marks_expiry() {
        let health = measured_health(
            McpHealthState::Ready,
            &[serde_json::json!({
                "name": "tool",
                "inputSchema": {"type": "object"}
            })],
            true,
            100,
            200,
        )
        .unwrap();
        let result = McpHealthResult {
            id: "id".into(),
            declaration_id: Some("declaration".into()),
            revision: Some("revision".into()),
            runner: Runner::Codex,
            cwd: "/work".into(),
            name: "server".into(),
            runner_provided: false,
            health,
        };
        let payload = serde_json::to_string(&result).unwrap();
        let cached =
            decode_cached_result(&payload, Runner::Codex, "/work", "id", "revision", 201).unwrap();
        assert!(cached.health.stale);
        assert!(decode_cached_result(
            &payload,
            Runner::Codex,
            "/work",
            "id",
            "different-revision",
            150,
        )
        .is_none());
    }

    #[test]
    fn health_selection_re_resolves_opaque_ids_after_config_changes() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let cwd = home.join("work/demo");
        fs::create_dir_all(&cwd).unwrap();
        let config = cwd.join(".mcp.json");
        write(
            &config,
            r#"{"mcpServers":{"demo":{"command":"first-server","args":["one"]}}}"#,
        );
        let initial =
            discover_at_with_codex_plugins(&home, &[("demo".into(), cwd.clone())], &[], Some(&[]));
        let initial_target = initial
            .health_targets
            .values()
            .find(|target| target.runner == Runner::ClaudeCode && target.name == "demo")
            .unwrap();
        let id = initial_target.id.clone();
        let revision = initial_target.revision.clone();
        write(
            &config,
            r#"{"mcpServers":{"demo":{"command":"second-server","args":["two"]}}}"#,
        );

        let canonical_cwd = fs::canonicalize(&cwd).unwrap();
        let selection = resolve_health_selection(
            &home,
            &[("demo".into(), cwd.clone())],
            &canonical_cwd,
            Runner::ClaudeCode,
            Some(std::slice::from_ref(&id)),
            &[],
        )
        .unwrap();
        assert_eq!(selection.targets.len(), 1);
        assert_ne!(selection.targets[0].revision, revision);
        match &selection.targets[0].endpoint {
            HealthEndpoint::DirectStdio { command, args, .. } => {
                assert_eq!(command, "second-server");
                assert_eq!(args, &["two"]);
            }
            _ => panic!("expected a freshly resolved stdio target"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn bounded_command_does_not_join_a_descendant_owned_stdout_pipe() {
        let started = Instant::now();
        let output = bounded_command_stdout(
            "/bin/sh",
            &["-c", "(/bin/sleep 10) & printf done"],
            Duration::from_millis(500),
            64,
        )
        .unwrap();
        assert_eq!(output, b"done");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn remote_urls_require_an_explicit_type_and_valid_url() {
        let no_type = summarize_json_transport(&serde_json::json!({
            "url": "https://example.com/mcp"
        }));
        assert_eq!(
            no_type.0,
            TransportSummary::Invalid {
                reason: InvalidConfigReason::MissingTransportType
            }
        );
        let malformed = summarize_json_transport(&serde_json::json!({
            "type": "http",
            "url": "definitely not a URL"
        }));
        assert_eq!(
            malformed.0,
            TransportSummary::Invalid {
                reason: InvalidConfigReason::InvalidUrl
            }
        );
    }

    #[test]
    fn unregistered_context_still_resolves_user_declarations() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let cwd = home.join("picked-but-not-registered");
        fs::create_dir_all(&cwd).unwrap();
        write(
            &home.join(".claude.json"),
            r#"{"mcpServers":{"global":{"command":"npx","args":["server"]}}}"#,
        );

        let snapshot = scan_for_context_at_with_codex_plugins(&home, &[], &cwd, &[], Some(&[]));
        let cwd = canonical_project_key(&cwd);
        assert!(snapshot.servers.iter().any(|server| {
            server.runner == Runner::ClaudeCode
                && server.cwd.as_deref() == Some(cwd.as_str())
                && server.name == "global"
        }));
    }

    #[test]
    fn codex_project_toggles_are_marked_as_shared_writes() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let cwd = home.join("work/demo");
        fs::create_dir_all(&cwd).unwrap();
        write(
            &home.join(".codex/config.toml"),
            "[mcp_servers.user]\ncommand = \"user-server\"\n",
        );
        write(
            &cwd.join(".codex/config.toml"),
            "[mcp_servers.project]\ncommand = \"project-server\"\n",
        );
        let snapshot =
            discover_at_with_codex_plugins(&home, &[("demo".into(), cwd.clone())], &[], Some(&[]))
                .snapshot;
        let cwd = canonical_project_key(&cwd);
        let project = snapshot
            .servers
            .iter()
            .find(|server| {
                server.runner == Runner::Codex
                    && server.cwd.as_deref() == Some(&cwd)
                    && server.name == "project"
            })
            .unwrap();
        assert!(project.toggle.writable);
        assert!(project.toggle.shared_project_file);
        let user = snapshot
            .servers
            .iter()
            .find(|server| {
                server.runner == Runner::Codex
                    && server.cwd.as_deref() == Some(&cwd)
                    && server.name == "user"
            })
            .unwrap();
        assert!(user.toggle.writable);
        assert!(!user.toggle.shared_project_file);
    }

    #[test]
    fn claude_project_approvals_and_plugin_overrides_do_not_leak_between_folders() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let project_a = home.join("work/a");
        let project_b = home.join("work/b");
        fs::create_dir_all(&project_a).unwrap();
        fs::create_dir_all(&project_b).unwrap();
        for project in [&project_a, &project_b] {
            write(
                &project.join(".mcp.json"),
                r#"{"mcpServers":{"same":{"command":"npx","args":["server"]}}}"#,
            );
        }

        let plugin_root = home.join(".claude/plugins/cache/market/demo/1");
        write(
            &plugin_root.join(".mcp.json"),
            r#"{"mcpServers":{"plugin-server":{"command":"node","args":["server.js"]}}}"#,
        );
        write(
            &home.join(".claude/plugins/installed_plugins.json"),
            &serde_json::json!({
                "version": 2,
                "plugins": {
                    "demo@market": [{"installPath": plugin_root, "scope": "user"}]
                }
            })
            .to_string(),
        );

        let mut projects = serde_json::Map::new();
        projects.insert(
            project_a.to_string_lossy().to_string(),
            serde_json::json!({
                "enabledMcpjsonServers": ["same"],
                "enabledPlugins": {"demo@market": true}
            }),
        );
        projects.insert(
            project_b.to_string_lossy().to_string(),
            serde_json::json!({
                "disabledMcpjsonServers": ["same"],
                "enabledPlugins": {"demo@market": false}
            }),
        );
        write(
            &home.join(".claude.json"),
            &serde_json::json!({"projects": projects}).to_string(),
        );

        let snapshot = discover_at_with_codex_plugins(
            &home,
            &[
                ("a".into(), project_a.clone()),
                ("b".into(), project_b.clone()),
            ],
            &[],
            Some(&[]),
        )
        .snapshot;
        let key_a = canonical_project_key(&project_a);
        let key_b = canonical_project_key(&project_b);

        let state = |project_key: &str| {
            snapshot
                .declarations
                .iter()
                .find(|declaration| {
                    declaration.runner == Runner::ClaudeCode
                        && declaration.source == Source::Project
                        && declaration.name == "same"
                        && declaration.project_key.as_deref() == Some(project_key)
                })
                .map(|declaration| declaration.state)
                .unwrap()
        };
        assert_eq!(state(&key_a), DeclarationState::Enabled);
        assert_eq!(state(&key_b), DeclarationState::Disabled);

        let has_plugin = |project_key: &str| {
            snapshot.servers.iter().any(|server| {
                server.runner == Runner::ClaudeCode
                    && server.cwd.as_deref() == Some(project_key)
                    && server.name == "plugin:demo:plugin-server"
            })
        };
        assert!(has_plugin(&key_a));
        assert!(!has_plugin(&key_b));
    }

    #[test]
    fn codex_inventory_uses_only_the_reported_version_cache_fallback() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let exact = home.join(".codex/plugins/cache/market/demo/1");
        let stale_newer = home.join(".codex/plugins/cache/market/demo/999");
        write(
            &exact.join(".mcp.json"),
            r#"{"mcpServers":{"active":{"command":"node","args":["active.js"]}}}"#,
        );
        write(
            &stale_newer.join(".mcp.json"),
            r#"{"mcpServers":{"stale":{"command":"node","args":["stale.js"]}}}"#,
        );
        let installed = [InstalledCodexPlugin {
            plugin_id: "demo@market".into(),
            name: "demo".into(),
            marketplace: "market".into(),
            version: "1".into(),
            enabled: true,
            source_path: home.join("missing-source"),
        }];

        let snapshot = discover_at_with_codex_plugins(&home, &[], &[], Some(&installed)).snapshot;
        assert!(snapshot.declarations.iter().any(|declaration| {
            declaration.runner == Runner::Codex && declaration.name == "active"
        }));
        assert!(!snapshot
            .declarations
            .iter()
            .any(|declaration| declaration.name == "stale"));
    }

    #[test]
    fn scans_real_machine_without_printing_private_configuration() {
        let snapshot = scan(&[]);
        eprintln!(
            "MCP declarations={} effective={} scan={}ms",
            snapshot.declarations.len(),
            snapshot.servers.len(),
            snapshot.scanned_ms
        );
    }
}
