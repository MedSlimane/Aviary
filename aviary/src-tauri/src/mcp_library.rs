//! Read-only MCP retrieval over Aviary's live library and durable bundles.
//!
//! Tool inputs use opaque entry ids, never filesystem paths. Every lookup
//! rebuilds the real provider index before resolving an id, so a moved or
//! removed file becomes an explicit miss rather than stale cached content.

use crate::library::{self, LibraryPlan, LibrarySnapshot};
use crate::mcp_protocol::{first_extra_key, ToolError, ToolResponse, ToolServer};
use crate::providers::{Entry, Kind, Runner, Source};
use crate::store::{self, bundles, Project};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

pub const MAX_CONTENT_BYTES: usize = 256 * 1024;
const MAX_QUERY_CHARS: usize = 512;
const MAX_RESULTS: usize = 50;
const DEFAULT_RESULTS: usize = 20;
const MAX_DESCRIPTION_CHARS: usize = 2_048;

pub struct LibraryServer {
    home: PathBuf,
    data_path: PathBuf,
}

impl LibraryServer {
    pub fn current() -> Result<Self, String> {
        let home = crate::providers::home().ok_or("no home directory")?;
        Ok(Self {
            data_path: home.join(".aviary").join("data.db"),
            home,
        })
    }

    pub fn at(home: PathBuf, data_path: PathBuf) -> Self {
        Self { home, data_path }
    }

    fn scan(&self) -> ScanResult {
        let (projects, database) = match self.open_data() {
            Ok(connection) => match read_projects(&connection) {
                Ok(projects) => (
                    projects,
                    DatabaseStatus::ready(data_version(&connection).ok()),
                ),
                Err(error) => (Vec::new(), DatabaseStatus::unavailable(error)),
            },
            Err(error) => (Vec::new(), DatabaseStatus::unavailable(error)),
        };
        let plan = LibraryPlan::for_home(self.home.clone(), projects);
        let snapshot = library::scan_plan(&plan).0;
        ScanResult { snapshot, database }
    }

    fn open_data(&self) -> Result<Connection, String> {
        if !self.data_path.is_file() {
            return Err("data.db is unavailable; open Aviary to create it".into());
        }
        let connection =
            Connection::open_with_flags(&self.data_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|error| format!("data.db cannot be opened read-only: {error}"))?;
        connection
            .pragma_update(None, "query_only", "ON")
            .map_err(|error| format!("data.db cannot enter query-only mode: {error}"))?;
        Ok(connection)
    }

    fn search_library(&self, arguments: &Map<String, Value>) -> Result<ToolResponse, ToolError> {
        reject_extra(arguments, &["query", "kinds", "runner", "source", "limit"])?;
        let query = required_string(arguments, "query", MAX_QUERY_CHARS)?;
        let requested_kinds = parse_kinds(arguments.get("kinds"))?;
        let runner = parse_runner(arguments.get("runner"))?;
        let source = parse_source(arguments.get("source"))?;
        let limit = bounded_limit(arguments)?;
        let scan = self.scan();
        let needle = query.to_lowercase();
        let mut matches = scan
            .snapshot
            .entries
            .iter()
            .filter(|entry| searchable_kind(entry.kind))
            .filter(|entry| {
                requested_kinds
                    .as_ref()
                    .is_none_or(|kinds| kinds.contains(search_kind(entry.kind)))
            })
            .filter(|entry| runner.is_none_or(|runner| entry.runners.contains(&runner)))
            .filter(|entry| source.is_none_or(|source| entry.source == source))
            .filter_map(|entry| search_score(entry, &needle).map(|score| (score, entry)))
            .collect::<Vec<_>>();
        matches.sort_by(|(left_score, left), (right_score, right)| {
            left_score
                .cmp(right_score)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.id.cmp(&right.id))
        });
        let entries = matches
            .into_iter()
            .take(limit)
            .map(|(_, entry)| entry_summary(entry))
            .collect::<Vec<_>>();
        let text = if entries.is_empty() {
            format!("No skills, prompts, or agents matched {query:?}.")
        } else {
            entries
                .iter()
                .filter_map(|entry| {
                    Some(format!(
                        "{} [{}] — {}",
                        entry.get("name")?.as_str()?,
                        entry.get("kind")?.as_str()?,
                        entry.get("id")?.as_str()?
                    ))
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(ToolResponse::new(
            json!({
                "entries": entries,
                "returned": entries.len(),
                "database": scan.database.as_json()
            }),
            text,
        ))
    }

    fn get_entry(
        &self,
        arguments: &Map<String, Value>,
        requested: RequestedEntry,
    ) -> Result<ToolResponse, ToolError> {
        reject_extra(arguments, &["id"])?;
        let id = required_string(arguments, "id", 64)?;
        if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ToolError::InvalidArguments(
                "id must be an opaque 64-character hexadecimal value".into(),
            ));
        }
        let scan = self.scan();
        let entry = scan
            .snapshot
            .entries
            .iter()
            .find(|entry| opaque_id(entry) == id)
            .ok_or_else(|| {
                ToolError::Failed(
                    "No current library entry has that id; it may have moved or been removed."
                        .into(),
                )
            })?;
        if !requested.accepts(entry.kind) {
            return Err(ToolError::Failed(format!(
                "The current entry is {}, not {}.",
                entry_kind(entry.kind),
                requested.label()
            )));
        }
        let content = read_bounded_content(Path::new(&entry.real_path))?;
        let summary = entry_summary(entry);
        let structured = json!({
            "entry": summary,
            "actualKind": entry_kind(entry.kind),
            "content": content.text,
            "bytes": content.total_bytes,
            "returnedBytes": content.returned_bytes,
            "truncated": content.truncated,
            "modified": entry.modified
        });
        let text = format!(
            "{} ({}){}\n\n{}",
            entry.name,
            entry_kind(entry.kind),
            if content.truncated {
                " — content truncated at 256 KiB"
            } else {
                ""
            },
            content.text
        );
        Ok(ToolResponse::new(structured, text))
    }

    fn list_bundles(&self, arguments: &Map<String, Value>) -> Result<ToolResponse, ToolError> {
        reject_extra(arguments, &["runner", "limit", "cursor"])?;
        let runner = parse_bundle_runner(arguments.get("runner"))?;
        let limit = bounded_limit(arguments)?;
        let cursor = parse_cursor(arguments.get("cursor"))?;
        let connection = self.open_data().map_err(ToolError::Failed)?;
        let version = data_version(&connection).map_err(ToolError::Failed)?;
        if version > store::DATA_VERSION {
            return Err(ToolError::Failed(format!(
                "data.db schema version {version} is newer than this server supports; use the matching Aviary build"
            )));
        }
        if version < store::DATA_VERSION {
            return Err(ToolError::Failed(format!(
                "data.db schema version is {version}, but bundle tools require version {}; open this Aviary build to migrate it",
                store::DATA_VERSION
            )));
        }
        let projects = read_projects(&connection).map_err(ToolError::Failed)?;
        let plan = LibraryPlan::for_home(self.home.clone(), projects.clone());
        let snapshot = library::scan_plan(&plan).0;
        let entries = snapshot
            .entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect::<HashMap<_, _>>();
        let project_names = projects
            .iter()
            .map(|project| (project.path.clone(), project.name.clone()))
            .collect::<HashMap<_, _>>();
        let project_pairs = projects
            .iter()
            .map(|project| (project.name.clone(), PathBuf::from(&project.path)))
            .collect::<Vec<_>>();
        let declarations = crate::mcp::scan(&project_pairs)
            .declarations
            .into_iter()
            .map(|declaration| (declaration.id.clone(), declaration))
            .collect::<HashMap<_, _>>();
        let collections = read_collections(&connection).map_err(ToolError::Failed)?;
        let mut stored = bundles::list_on(&connection)
            .map_err(|error| ToolError::Failed(error.to_string()))?
            .into_iter()
            .filter(|bundle| runner.is_none_or(|runner| bundle.runner == runner))
            .collect::<Vec<_>>();
        stored.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.id.cmp(&right.id))
        });
        if cursor > stored.len() {
            return Err(ToolError::InvalidArguments("cursor is out of range".into()));
        }
        let end = (cursor + limit).min(stored.len());
        let output = stored[cursor..end]
            .iter()
            .map(|bundle| {
                let members = bundle
                    .members
                    .iter()
                    .map(|member| {
                        let (status, reason) = resolve_member(
                            member,
                            bundle.runner,
                            &entries,
                            &project_names,
                            &declarations,
                            &collections,
                        );
                        json!({
                            "id": member.id,
                            "ordinal": member.ordinal,
                            "kind": member.kind.as_str(),
                            "role": member.role.as_str(),
                            "label": member.snapshot_label,
                            "status": status,
                            "reason": reason
                        })
                    })
                    .collect::<Vec<_>>();
                json!({
                    "id": bundle.id,
                    "name": bundle.name,
                    "description": bundle.description,
                    "runner": bundle.runner.as_str(),
                    "modelId": bundle.model_id,
                    "memoryMode": bundle.memory_mode,
                    "revision": bundle.revision,
                    "createdAt": bundle.created_at,
                    "updatedAt": bundle.updated_at,
                    "members": members
                })
            })
            .collect::<Vec<_>>();
        let next_cursor = (end < stored.len()).then(|| end.to_string());
        let text = if output.is_empty() {
            "No bundles are available for this filter.".into()
        } else {
            output
                .iter()
                .filter_map(|bundle| {
                    Some(format!(
                        "{} ({}; revision {})",
                        bundle.get("name")?.as_str()?,
                        bundle.get("runner")?.as_str()?,
                        bundle.get("revision")?.as_i64()?
                    ))
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(ToolResponse::new(
            json!({
                "bundles": output,
                "returned": output.len(),
                "nextCursor": next_cursor
            }),
            text,
        ))
    }
}

impl ToolServer for LibraryServer {
    fn name(&self) -> &'static str {
        "aviary-library"
    }

    fn instructions(&self) -> Option<&'static str> {
        Some("Search and read real Aviary skills, prompts and agents. Memory and arbitrary paths are never exposed.")
    }

    fn tools(&self) -> Vec<Value> {
        vec![
            json!({
                "name": "search_library",
                "description": "Search current skills, prompts and agents by real metadata. Memory is excluded.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "minLength": 1, "maxLength": MAX_QUERY_CHARS },
                        "kinds": {
                            "type": "array", "minItems": 1, "maxItems": 3,
                            "uniqueItems": true,
                            "items": { "enum": ["skill", "prompt", "agent"] }
                        },
                        "runner": { "enum": ["claude-code", "codex"] },
                        "source": { "enum": ["user", "plugin", "project"] },
                        "limit": { "type": "integer", "minimum": 1, "maximum": MAX_RESULTS }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                },
                "outputSchema": search_output_schema(),
                "annotations": read_only_annotations()
            }),
            entry_tool(
                "get_skill",
                "Read one current skill by opaque id, bounded to 256 KiB.",
            ),
            entry_tool(
                "get_prompt",
                "Read one current prompt or command by opaque id, retaining its actual subtype.",
            ),
            entry_tool(
                "get_agent",
                "Read one current agent definition by opaque id, bounded to 256 KiB.",
            ),
            json!({
                "name": "list_bundles",
                "description": "List sanitized durable bundles and the current resolution state of every member.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "runner": { "enum": ["claude-code", "codex"] },
                        "limit": { "type": "integer", "minimum": 1, "maximum": MAX_RESULTS },
                        "cursor": { "type": "string", "pattern": "^[0-9]+$" }
                    },
                    "additionalProperties": false
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "bundles": {
                            "type": "array",
                            "maxItems": MAX_RESULTS,
                            "items": bundle_schema()
                        },
                        "returned": { "type": "integer", "minimum": 0, "maximum": MAX_RESULTS },
                        "nextCursor": { "type": ["string", "null"] }
                    },
                    "required": ["bundles", "returned", "nextCursor"],
                    "additionalProperties": false
                },
                "annotations": read_only_annotations()
            }),
        ]
    }

    fn call(&self, name: &str, arguments: &Map<String, Value>) -> Result<ToolResponse, ToolError> {
        match name {
            "search_library" => self.search_library(arguments),
            "get_skill" => self.get_entry(arguments, RequestedEntry::Skill),
            "get_prompt" => self.get_entry(arguments, RequestedEntry::Prompt),
            "get_agent" => self.get_entry(arguments, RequestedEntry::Agent),
            "list_bundles" => self.list_bundles(arguments),
            other => Err(ToolError::UnknownTool(other.into())),
        }
    }
}

struct ScanResult {
    snapshot: LibrarySnapshot,
    database: DatabaseStatus,
}

struct DatabaseStatus {
    available: bool,
    version: Option<i64>,
    message: Option<String>,
}

impl DatabaseStatus {
    fn ready(version: Option<i64>) -> Self {
        Self {
            available: true,
            version,
            message: None,
        }
    }

    fn unavailable(message: String) -> Self {
        Self {
            available: false,
            version: None,
            message: Some(message),
        }
    }

    fn as_json(&self) -> Value {
        json!({
            "available": self.available,
            "version": self.version,
            "message": self.message
        })
    }
}

#[derive(Clone, Copy)]
enum RequestedEntry {
    Skill,
    Prompt,
    Agent,
}

impl RequestedEntry {
    fn accepts(self, kind: Kind) -> bool {
        match self {
            Self::Skill => kind == Kind::Skill,
            Self::Prompt => matches!(kind, Kind::Prompt | Kind::Command),
            Self::Agent => kind == Kind::Agent,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Skill => "a skill",
            Self::Prompt => "a prompt or command",
            Self::Agent => "an agent",
        }
    }
}

struct BoundedContent {
    text: String,
    total_bytes: u64,
    returned_bytes: usize,
    truncated: bool,
}

fn read_bounded_content(path: &Path) -> Result<BoundedContent, ToolError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let file = options
        .open(path)
        .map_err(|_| ToolError::Failed("The indexed entry is no longer readable.".into()))?;
    let metadata = file
        .metadata()
        .map_err(|_| ToolError::Failed("The indexed entry is no longer readable.".into()))?;
    if !metadata.is_file() {
        return Err(ToolError::Failed(
            "The indexed entry is no longer a regular file.".into(),
        ));
    }
    let mut bytes = Vec::with_capacity((metadata.len() as usize).min(MAX_CONTENT_BYTES + 1));
    file.take((MAX_CONTENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ToolError::Failed("The indexed entry could not be read.".into()))?;
    let truncated = metadata.len() > MAX_CONTENT_BYTES as u64 || bytes.len() > MAX_CONTENT_BYTES;
    let mut end = bytes.len().min(MAX_CONTENT_BYTES);
    let text = loop {
        match std::str::from_utf8(&bytes[..end]) {
            Ok(text) => break text.to_string(),
            Err(error) if error.error_len().is_none() => end = error.valid_up_to(),
            Err(_) => {
                return Err(ToolError::Failed(
                    "The indexed entry is not valid UTF-8 text.".into(),
                ))
            }
        }
    };
    Ok(BoundedContent {
        returned_bytes: end,
        text,
        total_bytes: metadata.len(),
        truncated,
    })
}

#[cfg(test)]
fn open_read_only(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    connection
        .pragma_update(None, "query_only", "ON")
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

fn data_version(connection: &Connection) -> Result<i64, String> {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())
}

fn read_projects(connection: &Connection) -> Result<Vec<Project>, String> {
    let version = data_version(connection)?;
    if version < 1 {
        return Err("data.db has not been initialized; open Aviary first".into());
    }
    let mut statement = connection
        .prepare("SELECT name, path FROM project ORDER BY added_at, path")
        .map_err(|error| error.to_string())?;
    let projects = statement
        .query_map([], |row| {
            Ok(Project {
                name: row.get(0)?,
                path: row.get(1)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(projects)
}

fn read_collections(connection: &Connection) -> Result<HashMap<i64, String>, String> {
    let mut statement = connection
        .prepare("SELECT id, name FROM collection ORDER BY id")
        .map_err(|error| error.to_string())?;
    let collections = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<HashMap<_, _>>>()
        .map_err(|error| error.to_string())?;
    Ok(collections)
}

fn opaque_id(entry: &Entry) -> String {
    let mut digest = Sha256::new();
    digest.update(entry_kind(entry.kind).as_bytes());
    digest.update([0]);
    digest.update(entry.id.as_bytes());
    format!("{:x}", digest.finalize())
}

fn entry_summary(entry: &Entry) -> Value {
    json!({
        "id": opaque_id(entry),
        "name": truncate_chars(&entry.name, 256),
        "description": truncate_chars(&entry.description, MAX_DESCRIPTION_CHARS),
        "kind": entry_kind(entry.kind),
        "source": source_name(entry.source),
        "runners": entry.runners.iter().map(|runner| runner_name(*runner)).collect::<Vec<_>>(),
        "project": entry.project.as_deref().map(|value| truncate_chars(value, 256)),
        "group": entry.group.as_deref().map(|value| truncate_chars(value, 256)),
        "bytes": entry.bytes,
        "modified": entry.modified
    })
}

fn search_score(entry: &Entry, needle: &str) -> Option<u8> {
    let name = entry.name.to_lowercase();
    if name == needle {
        return Some(0);
    }
    if name.starts_with(needle) {
        return Some(1);
    }
    if name.contains(needle) {
        return Some(2);
    }
    let metadata = format!(
        "{} {} {}",
        entry.description,
        entry.group.as_deref().unwrap_or(""),
        entry.project.as_deref().unwrap_or("")
    )
    .to_lowercase();
    metadata.contains(needle).then_some(3)
}

fn searchable_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Skill | Kind::Prompt | Kind::Command | Kind::Agent
    )
}

fn search_kind(kind: Kind) -> &'static str {
    match kind {
        Kind::Skill => "skill",
        Kind::Prompt | Kind::Command => "prompt",
        Kind::Agent => "agent",
        Kind::Memory => "memory",
    }
}

fn entry_kind(kind: Kind) -> &'static str {
    match kind {
        Kind::Skill => "skill",
        Kind::Agent => "agent",
        Kind::Command => "command",
        Kind::Prompt => "prompt",
        Kind::Memory => "memory",
    }
}

fn runner_name(runner: Runner) -> &'static str {
    match runner {
        Runner::ClaudeCode => "claude-code",
        Runner::Codex => "codex",
    }
}

fn source_name(source: Source) -> &'static str {
    match source {
        Source::User => "user",
        Source::Plugin => "plugin",
        Source::Project => "project",
    }
}

fn resolve_member(
    member: &bundles::BundleMember,
    runner: bundles::BundleRunner,
    entries: &HashMap<String, Entry>,
    projects: &HashMap<String, String>,
    declarations: &HashMap<String, crate::mcp::McpDeclaration>,
    collections: &HashMap<i64, String>,
) -> (&'static str, Option<String>) {
    match &member.target {
        bundles::MemberTarget::Project { path } => match projects.get(path) {
            Some(_) if Path::new(path).is_dir() => ("ready", None),
            Some(_) => (
                "missing",
                Some("registered project directory is missing".into()),
            ),
            None => ("missing", Some("project is no longer registered".into())),
        },
        bundles::MemberTarget::Entry { id } => match entries.get(id) {
            None => ("missing", Some("library entry is no longer present".into())),
            Some(entry) if !entry.runners.contains(&bundle_provider_runner(runner)) => (
                "incompatible",
                Some("entry is not available to the bundle runner".into()),
            ),
            Some(entry) if !entry_matches_member(entry.kind, member.kind) => (
                "incompatible",
                Some("entry kind no longer matches the bundle member".into()),
            ),
            Some(_) => ("ready", None),
        },
        bundles::MemberTarget::McpDeclaration { id } => match declarations.get(id) {
            None => (
                "missing",
                Some("MCP declaration is no longer present".into()),
            ),
            Some(declaration) if declaration.runner != bundle_provider_runner(runner) => (
                "incompatible",
                Some("MCP declaration belongs to another runner".into()),
            ),
            Some(declaration) if declaration.state == crate::mcp::DeclarationState::Invalid => {
                ("incompatible", Some("MCP declaration is invalid".into()))
            }
            Some(_) => ("ready", None),
        },
        bundles::MemberTarget::MediaCollection { id } => {
            if collections.contains_key(id) {
                ("ready", None)
            } else {
                (
                    "missing",
                    Some("media collection is no longer present".into()),
                )
            }
        }
    }
}

fn entry_matches_member(kind: Kind, member: bundles::MemberKind) -> bool {
    match member {
        bundles::MemberKind::Skill => kind == Kind::Skill,
        bundles::MemberKind::Prompt => matches!(kind, Kind::Prompt | Kind::Command),
        bundles::MemberKind::Agent => kind == Kind::Agent,
        bundles::MemberKind::Memory => kind == Kind::Memory,
        _ => false,
    }
}

fn bundle_provider_runner(runner: bundles::BundleRunner) -> Runner {
    match runner {
        bundles::BundleRunner::ClaudeCode => Runner::ClaudeCode,
        bundles::BundleRunner::Codex => Runner::Codex,
    }
}

fn parse_kinds(value: Option<&Value>) -> Result<Option<HashSet<&'static str>>, ToolError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let array = value
        .as_array()
        .ok_or_else(|| ToolError::InvalidArguments("kinds must be an array".into()))?;
    if array.is_empty() || array.len() > 3 {
        return Err(ToolError::InvalidArguments(
            "kinds must contain 1 to 3 unique values".into(),
        ));
    }
    let mut kinds = HashSet::new();
    for value in array {
        let kind = match value.as_str() {
            Some("skill") => "skill",
            Some("prompt") => "prompt",
            Some("agent") => "agent",
            _ => {
                return Err(ToolError::InvalidArguments(
                    "kinds may contain skill, prompt, or agent".into(),
                ))
            }
        };
        if !kinds.insert(kind) {
            return Err(ToolError::InvalidArguments(
                "kinds must not contain duplicates".into(),
            ));
        }
    }
    Ok(Some(kinds))
}

fn parse_runner(value: Option<&Value>) -> Result<Option<Runner>, ToolError> {
    match value {
        None => Ok(None),
        Some(Value::String(value)) if value == "claude-code" => Ok(Some(Runner::ClaudeCode)),
        Some(Value::String(value)) if value == "codex" => Ok(Some(Runner::Codex)),
        Some(_) => Err(ToolError::InvalidArguments(
            "runner must be claude-code or codex".into(),
        )),
    }
}

fn parse_bundle_runner(value: Option<&Value>) -> Result<Option<bundles::BundleRunner>, ToolError> {
    parse_runner(value).map(|runner| {
        runner.map(|runner| match runner {
            Runner::ClaudeCode => bundles::BundleRunner::ClaudeCode,
            Runner::Codex => bundles::BundleRunner::Codex,
        })
    })
}

fn parse_source(value: Option<&Value>) -> Result<Option<Source>, ToolError> {
    match value {
        None => Ok(None),
        Some(Value::String(value)) if value == "user" => Ok(Some(Source::User)),
        Some(Value::String(value)) if value == "plugin" => Ok(Some(Source::Plugin)),
        Some(Value::String(value)) if value == "project" => Ok(Some(Source::Project)),
        Some(_) => Err(ToolError::InvalidArguments(
            "source must be user, plugin, or project".into(),
        )),
    }
}

fn parse_cursor(value: Option<&Value>) -> Result<usize, ToolError> {
    match value {
        None => Ok(0),
        Some(Value::String(cursor)) => cursor
            .parse()
            .map_err(|_| ToolError::InvalidArguments("cursor is invalid".into())),
        Some(_) => Err(ToolError::InvalidArguments(
            "cursor must be a string".into(),
        )),
    }
}

fn bounded_limit(arguments: &Map<String, Value>) -> Result<usize, ToolError> {
    match arguments.get("limit") {
        None => Ok(DEFAULT_RESULTS),
        Some(Value::Number(number)) => number
            .as_u64()
            .filter(|limit| (1..=MAX_RESULTS as u64).contains(limit))
            .map(|limit| limit as usize)
            .ok_or_else(|| {
                ToolError::InvalidArguments(format!("limit must be between 1 and {MAX_RESULTS}"))
            }),
        Some(_) => Err(ToolError::InvalidArguments(
            "limit must be an integer".into(),
        )),
    }
}

fn required_string(
    arguments: &Map<String, Value>,
    field: &str,
    max_chars: usize,
) -> Result<String, ToolError> {
    let value = arguments
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidArguments(format!("{field} is required")))?;
    let count = value.chars().count();
    if count == 0 || count > max_chars {
        return Err(ToolError::InvalidArguments(format!(
            "{field} must contain 1 to {max_chars} characters"
        )));
    }
    Ok(value.to_string())
}

fn reject_extra(arguments: &Map<String, Value>, allowed: &[&str]) -> Result<(), ToolError> {
    if let Some(extra) = first_extra_key(arguments, allowed) {
        return Err(ToolError::InvalidArguments(format!(
            "unknown argument: {extra}"
        )));
    }
    Ok(())
}

fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn read_only_annotations() -> Value {
    json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false
    })
}

fn entry_tool(name: &str, description: &str) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "minLength": 64,
                    "maxLength": 64,
                    "pattern": "^[0-9a-fA-F]{64}$"
                }
            },
            "required": ["id"],
            "additionalProperties": false
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "entry": entry_schema(),
                "actualKind": { "enum": ["skill", "prompt", "command", "agent"] },
                "content": { "type": "string" },
                "bytes": { "type": "integer", "minimum": 0 },
                "returnedBytes": { "type": "integer", "minimum": 0, "maximum": MAX_CONTENT_BYTES },
                "truncated": { "type": "boolean" },
                "modified": { "type": "integer", "minimum": 0 }
            },
            "required": [
                "entry", "actualKind", "content", "bytes", "returnedBytes",
                "truncated", "modified"
            ],
            "additionalProperties": false
        },
        "annotations": read_only_annotations()
    })
}

fn search_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "entries": { "type": "array", "maxItems": MAX_RESULTS, "items": entry_schema() },
            "returned": { "type": "integer", "minimum": 0, "maximum": MAX_RESULTS },
            "database": {
                "type": "object",
                "properties": {
                    "available": { "type": "boolean" },
                    "version": { "type": ["integer", "null"] },
                    "message": { "type": ["string", "null"] }
                },
                "required": ["available", "version", "message"],
                "additionalProperties": false
            }
        },
        "required": ["entries", "returned", "database"],
        "additionalProperties": false
    })
}

fn entry_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "name": { "type": "string" },
            "description": { "type": "string" },
            "kind": { "enum": ["skill", "prompt", "command", "agent"] },
            "source": { "enum": ["user", "plugin", "project"] },
            "runners": {
                "type": "array", "items": { "enum": ["claude-code", "codex"] }
            },
            "project": { "type": ["string", "null"] },
            "group": { "type": ["string", "null"] },
            "bytes": { "type": "integer", "minimum": 0 },
            "modified": { "type": "integer", "minimum": 0 }
        },
        "required": [
            "id", "name", "description", "kind", "source", "runners",
            "project", "group", "bytes", "modified"
        ],
        "additionalProperties": false
    })
}

fn bundle_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
            },
            "name": { "type": "string" },
            "description": { "type": "string" },
            "runner": { "enum": ["claude-code", "codex"] },
            "modelId": { "type": ["string", "null"] },
            "memoryMode": { "enum": ["inherit", "supplement"] },
            "revision": { "type": "integer", "minimum": 1 },
            "createdAt": { "type": "integer", "minimum": 0 },
            "updatedAt": { "type": "integer", "minimum": 0 },
            "members": {
                "type": "array",
                "maxItems": bundles::MAX_MEMBERS,
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                        },
                        "ordinal": { "type": "integer", "minimum": 0 },
                        "kind": {
                            "enum": [
                                "project", "skill", "prompt", "agent", "memory",
                                "mcp", "media-collection"
                            ]
                        },
                        "role": {
                            "enum": [
                                "working-directory", "available", "invoke-first-turn",
                                "prefill", "primary", "supplement", "enabled", "retrieval"
                            ]
                        },
                        "label": { "type": "string" },
                        "status": { "enum": ["ready", "missing", "incompatible"] },
                        "reason": { "type": ["string", "null"] }
                    },
                    "required": [
                        "id", "ordinal", "kind", "role", "label", "status", "reason"
                    ],
                    "additionalProperties": false
                }
            }
        },
        "required": [
            "id", "name", "description", "runner", "modelId", "memoryMode",
            "revision", "createdAt", "updatedAt", "members"
        ],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_protocol::ToolServer;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, LibraryServer) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        fs::create_dir_all(home.join(".claude/skills/demo")).unwrap();
        fs::create_dir_all(home.join(".claude/agents")).unwrap();
        fs::write(
            home.join(".claude/skills/demo/SKILL.md"),
            "---\nname: Demo skill\ndescription: Finds teal references\n---\n\nDo the real work.\n",
        )
        .unwrap();
        fs::write(
            home.join(".claude/agents/reviewer.md"),
            "---\nname: Reviewer\ndescription: Reviews changes\n---\nReview carefully.\n",
        )
        .unwrap();
        fs::write(home.join(".claude/CLAUDE.md"), "private memory").unwrap();
        let data = root.path().join("data.db");
        (root, LibraryServer::at(home, data))
    }

    #[test]
    fn search_returns_opaque_ids_and_excludes_memory() {
        let (_root, server) = fixture();
        let output = server
            .call(
                "search_library",
                json!({ "query": "" })
                    .as_object()
                    .expect("object arguments"),
            )
            .unwrap_err();
        assert!(matches!(output, ToolError::InvalidArguments(_)));

        let output = server
            .call(
                "search_library",
                json!({ "query": "e" })
                    .as_object()
                    .expect("object arguments"),
            )
            .unwrap();
        let entries = output.structured["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| {
            let id = entry["id"].as_str().unwrap();
            id.len() == 64 && !id.contains('/') && entry["kind"] != "memory"
        }));
        assert_eq!(output.structured["database"]["available"], false);
    }

    #[test]
    fn get_refreshes_before_resolving_and_never_accepts_paths() {
        let (root, server) = fixture();
        let search = server
            .call(
                "search_library",
                json!({ "query": "Demo" }).as_object().unwrap(),
            )
            .unwrap();
        let id = search.structured["entries"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let content = server
            .call("get_skill", json!({ "id": id }).as_object().unwrap())
            .unwrap();
        assert!(content.structured["content"]
            .as_str()
            .unwrap()
            .contains("Do the real work"));
        let extra = server.call(
            "get_skill",
            json!({ "id": id, "path": "/etc/passwd" })
                .as_object()
                .unwrap(),
        );
        assert!(matches!(extra, Err(ToolError::InvalidArguments(_))));

        fs::remove_file(root.path().join("home/.claude/skills/demo/SKILL.md")).unwrap();
        let stale = server.call("get_skill", json!({ "id": id }).as_object().unwrap());
        assert!(matches!(stale, Err(ToolError::Failed(_))));
    }

    #[test]
    fn prompt_tool_verifies_actual_kind() {
        let (_root, server) = fixture();
        let search = server
            .call(
                "search_library",
                json!({ "query": "Reviewer" }).as_object().unwrap(),
            )
            .unwrap();
        let id = search.structured["entries"][0]["id"].as_str().unwrap();
        let wrong = server.call("get_prompt", json!({ "id": id }).as_object().unwrap());
        assert!(matches!(wrong, Err(ToolError::Failed(_))));
    }

    #[test]
    fn content_cap_is_utf8_safe_and_truthfully_reported() {
        let (root, server) = fixture();
        let path = root.path().join("home/.claude/skills/demo/SKILL.md");
        let content = format!(
            "---\nname: Huge\ndescription: bounded\n---\n{}é",
            "x".repeat(MAX_CONTENT_BYTES)
        );
        fs::write(&path, content).unwrap();
        let search = server
            .call(
                "search_library",
                json!({ "query": "Huge" }).as_object().unwrap(),
            )
            .unwrap();
        let id = search.structured["entries"][0]["id"].as_str().unwrap();
        let output = server
            .call("get_skill", json!({ "id": id }).as_object().unwrap())
            .unwrap();
        assert_eq!(output.structured["truncated"], true);
        assert!(output.structured["returnedBytes"].as_u64().unwrap() <= MAX_CONTENT_BYTES as u64);
        assert!(output.structured["content"].as_str().is_some());
    }

    #[test]
    fn pre_v3_database_is_never_migrated_or_written() {
        let (root, server) = fixture();
        let connection = Connection::open(&server.data_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE project(path TEXT PRIMARY KEY, name TEXT, added_at INTEGER);\n\
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        drop(connection);

        let output = server.call("list_bundles", json!({}).as_object().unwrap());
        assert!(
            matches!(output, Err(ToolError::Failed(message)) if message.contains("open this Aviary build"))
        );
        let readonly = open_read_only(&server.data_path).unwrap();
        assert!(readonly
            .execute(
                "INSERT INTO project(path, name, added_at) VALUES ('/tmp', 'No', 0)",
                [],
            )
            .is_err());
        assert_eq!(data_version(&readonly).unwrap(), 2);
        drop(readonly);
        let writable = Connection::open(root.path().join("data.db")).unwrap();
        let bundles: i64 = writable
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name = 'bundle'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bundles, 0);

        writable
            .pragma_update(None, "user_version", store::DATA_VERSION + 1)
            .unwrap();
        drop(writable);
        let output = server.call("list_bundles", json!({}).as_object().unwrap());
        assert!(matches!(output, Err(ToolError::Failed(message)) if message.contains("newer")));
    }

    #[test]
    fn every_tool_schema_is_strict_read_only_and_has_output_shape() {
        let (_root, server) = fixture();
        let tools = server.tools();
        assert_eq!(tools.len(), 5);
        for tool in tools {
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            assert!(tool["outputSchema"].is_object());
            assert_eq!(tool["annotations"]["readOnlyHint"], true);
            assert_eq!(tool["annotations"]["destructiveHint"], false);
            assert_eq!(tool["annotations"]["openWorldHint"], false);
        }
    }
}
