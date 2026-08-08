//! Durable bundle aggregates and immutable chat attachment snapshots.
//!
//! Bundle rows keep the identity the user selected even after its source goes
//! away. Resolution is therefore a separate operation: saves require real
//! targets, while later reads report missing or incompatible members without
//! deleting or guessing replacements.

use crate::providers::{Kind as EntryKind, Runner as ProviderRunner};
use crate::{library, mcp, media};
use rusqlite::{params, types::Type, Connection, OptionalExtension, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use uuid::Uuid;

pub const MAX_MEMBERS: usize = 128;
pub const ATTACHMENT_SCHEMA_VERSION: i64 = 1;
const MAX_NAME_CHARS: usize = 120;
const MAX_DESCRIPTION_CHARS: usize = 4_000;
const MAX_MODEL_CHARS: usize = 256;
const MAX_TARGET_CHARS: usize = 16_384;
const MAX_LABEL_CHARS: usize = 256;
const MAX_ATTACHMENT_BYTES: usize = 256 * 1024;
const MAX_NOTE_CHARS: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BundleRunner {
    ClaudeCode,
    Codex,
}

impl BundleRunner {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }

    fn from_db(value: &str, column: usize) -> rusqlite::Result<Self> {
        match value {
            "claude-code" => Ok(Self::ClaudeCode),
            "codex" => Ok(Self::Codex),
            _ => Err(invalid_db_value(column, "unknown bundle runner")),
        }
    }

    pub fn provider(self) -> ProviderRunner {
        match self {
            Self::ClaudeCode => ProviderRunner::ClaudeCode,
            Self::Codex => ProviderRunner::Codex,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryMode {
    Inherit,
    Supplement,
}

impl MemoryMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Supplement => "supplement",
        }
    }

    fn from_db(value: &str, column: usize) -> rusqlite::Result<Self> {
        match value {
            "inherit" => Ok(Self::Inherit),
            "supplement" => Ok(Self::Supplement),
            _ => Err(invalid_db_value(column, "unknown bundle memory mode")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemberKind {
    Project,
    Skill,
    Prompt,
    Agent,
    Memory,
    Mcp,
    MediaCollection,
}

impl MemberKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Skill => "skill",
            Self::Prompt => "prompt",
            Self::Agent => "agent",
            Self::Memory => "memory",
            Self::Mcp => "mcp",
            Self::MediaCollection => "media-collection",
        }
    }

    fn from_db(value: &str, column: usize) -> rusqlite::Result<Self> {
        match value {
            "project" => Ok(Self::Project),
            "skill" => Ok(Self::Skill),
            "prompt" => Ok(Self::Prompt),
            "agent" => Ok(Self::Agent),
            "memory" => Ok(Self::Memory),
            "mcp" => Ok(Self::Mcp),
            "media-collection" => Ok(Self::MediaCollection),
            _ => Err(invalid_db_value(column, "unknown bundle member kind")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemberRole {
    WorkingDirectory,
    Available,
    InvokeFirstTurn,
    Prefill,
    Primary,
    Supplement,
    Enabled,
    Retrieval,
}

impl MemberRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkingDirectory => "working-directory",
            Self::Available => "available",
            Self::InvokeFirstTurn => "invoke-first-turn",
            Self::Prefill => "prefill",
            Self::Primary => "primary",
            Self::Supplement => "supplement",
            Self::Enabled => "enabled",
            Self::Retrieval => "retrieval",
        }
    }

    fn from_db(value: &str, column: usize) -> rusqlite::Result<Self> {
        match value {
            "working-directory" => Ok(Self::WorkingDirectory),
            "available" => Ok(Self::Available),
            "invoke-first-turn" => Ok(Self::InvokeFirstTurn),
            "prefill" => Ok(Self::Prefill),
            "primary" => Ok(Self::Primary),
            "supplement" => Ok(Self::Supplement),
            "enabled" => Ok(Self::Enabled),
            "retrieval" => Ok(Self::Retrieval),
            _ => Err(invalid_db_value(column, "unknown bundle member role")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum MemberTarget {
    Project { path: String },
    Entry { id: String },
    McpDeclaration { id: String },
    MediaCollection { id: i64 },
}

impl MemberTarget {
    fn text(&self) -> Option<&str> {
        match self {
            Self::Project { path } => Some(path),
            Self::Entry { id } | Self::McpDeclaration { id } => Some(id),
            Self::MediaCollection { .. } => None,
        }
    }

    fn integer(&self) -> Option<i64> {
        match self {
            Self::MediaCollection { id } => Some(*id),
            _ => None,
        }
    }

    fn identity_key(&self) -> String {
        match self {
            Self::Project { path } => format!("project:{path}"),
            Self::Entry { id } => format!("entry:{id}"),
            Self::McpDeclaration { id } => format!("mcp:{id}"),
            Self::MediaCollection { id } => format!("collection:{id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleMemberDraft {
    /// Present only when preserving a member returned by Aviary during update.
    pub id: Option<String>,
    pub ordinal: u32,
    pub kind: MemberKind,
    pub role: MemberRole,
    pub target: MemberTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleDraft {
    pub name: String,
    pub description: String,
    pub runner: BundleRunner,
    pub model_id: Option<String>,
    pub memory_mode: MemoryMode,
    pub members: Vec<BundleMemberDraft>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleMember {
    pub id: String,
    pub ordinal: u32,
    pub kind: MemberKind,
    pub role: MemberRole,
    pub target: MemberTarget,
    pub snapshot_label: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bundle {
    pub id: String,
    pub name: String,
    pub description: String,
    pub runner: BundleRunner,
    pub model_id: Option<String>,
    pub memory_mode: MemoryMode,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub members: Vec<BundleMember>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolutionStatus {
    Ready,
    Missing,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetResolution {
    pub status: ResolutionStatus,
    pub current_label: Option<String>,
    pub reason: Option<String>,
}

impl TargetResolution {
    pub fn ready(label: impl Into<String>) -> Self {
        Self {
            status: ResolutionStatus::Ready,
            current_label: Some(label.into()),
            reason: None,
        }
    }

    pub fn missing(reason: impl Into<String>) -> Self {
        Self {
            status: ResolutionStatus::Missing,
            current_label: None,
            reason: Some(reason.into()),
        }
    }

    pub fn incompatible(label: Option<String>, reason: impl Into<String>) -> Self {
        Self {
            status: ResolutionStatus::Incompatible,
            current_label: label,
            reason: Some(reason.into()),
        }
    }
}

pub trait TargetCatalog {
    fn resolve(
        &self,
        runner: BundleRunner,
        kind: MemberKind,
        target: &MemberTarget,
    ) -> Result<TargetResolution, String>;
}

/// A single fresh view of every real target type a bundle can reference.
///
/// It is intentionally built before opening a bundle write transaction: the
/// library and media scanners briefly acquire the same process-wide data.db
/// mutex, so resolving from inside the transaction would deadlock.
pub struct LiveTargetCatalog {
    entries: HashMap<String, crate::providers::Entry>,
    projects: HashMap<String, String>,
    declarations: HashMap<String, mcp::McpDeclaration>,
    collections: HashMap<i64, String>,
}

impl LiveTargetCatalog {
    pub fn scan() -> Self {
        let snapshot = library::scan();
        let projects = snapshot
            .projects
            .iter()
            .map(|project| (project.path.clone(), project.name.clone()))
            .collect::<HashMap<_, _>>();
        let project_pairs = snapshot
            .projects
            .iter()
            .map(|project| (project.name.clone(), PathBuf::from(&project.path)))
            .collect::<Vec<_>>();
        let declarations = mcp::scan(&project_pairs)
            .declarations
            .into_iter()
            .map(|declaration| (declaration.id.clone(), declaration))
            .collect();
        let collections = media::collections()
            .into_iter()
            .map(|collection| (collection.id, collection.name))
            .collect();
        Self {
            entries: snapshot
                .entries
                .into_iter()
                .map(|entry| (entry.id.clone(), entry))
                .collect(),
            projects,
            declarations,
            collections,
        }
    }
}

impl TargetCatalog for LiveTargetCatalog {
    fn resolve(
        &self,
        runner: BundleRunner,
        kind: MemberKind,
        target: &MemberTarget,
    ) -> Result<TargetResolution, String> {
        validate_kind_target(kind, target).map_err(|error| error.to_string())?;
        match target {
            MemberTarget::Project { path } => match self.projects.get(path) {
                Some(name) if PathBuf::from(path).is_dir() => Ok(TargetResolution::ready(name)),
                Some(name) => Ok(TargetResolution {
                    status: ResolutionStatus::Missing,
                    current_label: Some(name.clone()),
                    reason: Some("the registered project directory is missing".into()),
                }),
                None => Ok(TargetResolution::missing(
                    "the project is no longer registered",
                )),
            },
            MemberTarget::Entry { id } => {
                let Some(entry) = self.entries.get(id) else {
                    return Ok(TargetResolution::missing(
                        "the library entry is no longer present",
                    ));
                };
                let expected_kind = match kind {
                    MemberKind::Skill => entry.kind == EntryKind::Skill,
                    MemberKind::Prompt => {
                        matches!(entry.kind, EntryKind::Prompt | EntryKind::Command)
                    }
                    MemberKind::Agent => entry.kind == EntryKind::Agent,
                    MemberKind::Memory => entry.kind == EntryKind::Memory,
                    _ => false,
                };
                if !expected_kind {
                    return Ok(TargetResolution::incompatible(
                        Some(entry.name.clone()),
                        "the entry kind no longer matches the bundle member",
                    ));
                }
                if !entry.runners.contains(&runner.provider()) {
                    return Ok(TargetResolution::incompatible(
                        Some(entry.name.clone()),
                        "the entry is not available to this bundle runner",
                    ));
                }
                Ok(TargetResolution::ready(&entry.name))
            }
            MemberTarget::McpDeclaration { id } => {
                let Some(declaration) = self.declarations.get(id) else {
                    return Ok(TargetResolution::missing(
                        "the MCP declaration is no longer present",
                    ));
                };
                if declaration.runner != runner.provider() {
                    return Ok(TargetResolution::incompatible(
                        Some(declaration.name.clone()),
                        "the MCP declaration belongs to another runner",
                    ));
                }
                if declaration.state == mcp::DeclarationState::Invalid {
                    return Ok(TargetResolution::incompatible(
                        Some(declaration.name.clone()),
                        "the MCP declaration is invalid",
                    ));
                }
                Ok(TargetResolution::ready(&declaration.name))
            }
            MemberTarget::MediaCollection { id } => match self.collections.get(id) {
                Some(name) => Ok(TargetResolution::ready(name)),
                None => Ok(TargetResolution::missing(
                    "the media collection is no longer present",
                )),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedBundleMember {
    pub member: BundleMember,
    pub resolution: TargetResolution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedBundle {
    pub bundle: Bundle,
    pub members: Vec<ResolvedBundleMember>,
}

pub fn resolve_bundle(
    bundle: Bundle,
    catalog: &impl TargetCatalog,
) -> Result<ResolvedBundle, BundleError> {
    let members = bundle
        .members
        .iter()
        .cloned()
        .map(|member| {
            catalog
                .resolve(bundle.runner, member.kind, &member.target)
                .map(|resolution| ResolvedBundleMember { member, resolution })
                .map_err(|message| BundleError::Invalid { message })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ResolvedBundle { bundle, members })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SnapshotDisposition {
    Apply,
    Available,
    Inherited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotMember {
    pub member_id: String,
    pub ordinal: u32,
    pub kind: MemberKind,
    pub role: MemberRole,
    pub target: MemberTarget,
    pub snapshot_label: String,
    pub disposition: SnapshotDisposition,
    pub note: Option<String>,
}

/// Secret-free, versioned execution intent owned by a chat session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleAttachmentSnapshot {
    pub schema_version: i64,
    pub bundle_id: String,
    pub bundle_revision: i64,
    pub bundle_name: String,
    pub runner: BundleRunner,
    pub model_id: Option<String>,
    pub cwd: String,
    pub members: Vec<SnapshotMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBundleAttachment {
    pub session_id: String,
    pub attached_at: i64,
    pub snapshot: BundleAttachmentSnapshot,
}

/// The immutable, secret-free plan paired with the fields a chat supervisor
/// must lock before spawning a runner. This type is produced only from a fresh
/// target catalogue; the webview never supplies an attachment snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreparedBundleAttachment {
    pub snapshot: BundleAttachmentSnapshot,
    pub runner: ProviderRunner,
    pub cwd: String,
    pub model_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum BundleError {
    Invalid { message: String },
    NotFound { id: String },
    RevisionConflict { expected: i64, actual: i64 },
    Database { message: String },
}

impl fmt::Display for BundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { message } | Self::Database { message } => formatter.write_str(message),
            Self::NotFound { id } => write!(formatter, "bundle {id} was not found"),
            Self::RevisionConflict { expected, actual } => write!(
                formatter,
                "bundle revision changed (expected {expected}, found {actual})"
            ),
        }
    }
}

impl std::error::Error for BundleError {}

impl From<rusqlite::Error> for BundleError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database {
            message: error.to_string(),
        }
    }
}

pub fn create(draft: BundleDraft, catalog: &impl TargetCatalog) -> Result<Bundle, BundleError> {
    let mut connection = super::data();
    create_on(&mut connection, draft, catalog)
}

pub fn create_on(
    connection: &mut Connection,
    draft: BundleDraft,
    catalog: &impl TargetCatalog,
) -> Result<Bundle, BundleError> {
    let timestamp = super::now();
    let prepared = prepare_draft(draft, catalog, None, timestamp)?;
    let bundle_id = Uuid::new_v4().to_string();
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO bundle(
            id, name, description, runner, model_id, memory_mode,
            revision, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?7)",
        params![
            bundle_id,
            prepared.name,
            prepared.description,
            prepared.runner.as_str(),
            prepared.model_id,
            prepared.memory_mode.as_str(),
            timestamp,
        ],
    )?;
    insert_members(&transaction, &bundle_id, &prepared.members)?;
    transaction.commit()?;
    get_on(connection, &bundle_id)?.ok_or_else(|| BundleError::NotFound { id: bundle_id })
}

pub fn get(id: &str) -> Result<Option<Bundle>, BundleError> {
    get_on(&super::data(), id)
}

pub fn get_on(connection: &Connection, id: &str) -> Result<Option<Bundle>, BundleError> {
    let header = connection
        .query_row(
            "SELECT id, name, description, runner, model_id, memory_mode,
                    revision, created_at, updated_at
               FROM bundle WHERE id = ?1",
            [id],
            row_to_bundle_header,
        )
        .optional()?;
    let Some(mut bundle) = header else {
        return Ok(None);
    };
    bundle.members = read_members(connection, &bundle.id)?;
    Ok(Some(bundle))
}

pub fn list() -> Result<Vec<Bundle>, BundleError> {
    list_on(&super::data())
}

pub fn list_on(connection: &Connection) -> Result<Vec<Bundle>, BundleError> {
    let mut statement =
        connection.prepare("SELECT id FROM bundle ORDER BY updated_at DESC, lower(name), id")?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter()
        .map(|id| get_on(connection, &id)?.ok_or_else(|| BundleError::NotFound { id: id.clone() }))
        .collect()
}

/// Resolves the exact execution fields for a chat attachment or terminal
/// launch. Every member must still resolve; missing identities are preserved
/// in the editor, but they never become a guessed execution plan.
pub fn resolve_for_attachment(
    id: &str,
    expected_revision: i64,
    catalog: &impl TargetCatalog,
) -> Result<PreparedBundleAttachment, BundleError> {
    resolve_for_attachment_on(&super::data(), id, expected_revision, catalog)
}

pub fn resolve_for_attachment_on(
    connection: &Connection,
    id: &str,
    expected_revision: i64,
    catalog: &impl TargetCatalog,
) -> Result<PreparedBundleAttachment, BundleError> {
    let bundle =
        get_on(connection, id)?.ok_or_else(|| BundleError::NotFound { id: id.to_string() })?;
    if bundle.revision != expected_revision {
        return Err(BundleError::RevisionConflict {
            expected: expected_revision,
            actual: bundle.revision,
        });
    }
    prepared_attachment_from_bundle(bundle, catalog)
}

/// Existing in-app runner adapters can faithfully apply the locked runner,
/// cwd and model, while prompt members remain an explicit UI prefill and
/// already-installed skills/agents remain available. Components that require
/// a generated runner config or an unproven protocol field are rejected before
/// the durable session is created; silently ignoring them would make the
/// attachment snapshot a false claim.
pub fn validate_chat_support(prepared: &PreparedBundleAttachment) -> Result<(), BundleError> {
    for member in &prepared.snapshot.members {
        let unsupported = match (member.kind, member.role) {
            (MemberKind::Project, MemberRole::WorkingDirectory)
            | (MemberKind::Prompt, MemberRole::Prefill)
            | (MemberKind::Skill, MemberRole::Available)
            | (MemberKind::Agent, MemberRole::Available) => None,
            (MemberKind::Skill, MemberRole::InvokeFirstTurn) => {
                Some("first-turn skill invocation is not supported by both runner protocols")
            }
            (MemberKind::Agent, MemberRole::Primary) => {
                Some("primary-agent selection is not supported by both runner protocols")
            }
            (MemberKind::Memory, _) => {
                Some("supplemental memory requires a proven append-only runner channel")
            }
            (MemberKind::Mcp, _) => {
                Some("isolated MCP selection requires generated runner configuration")
            }
            (MemberKind::MediaCollection, _) => {
                Some("scoped media retrieval requires generated runner configuration")
            }
            _ => Some("the member role cannot be represented by the selected runner"),
        };
        if let Some(reason) = unsupported {
            return invalid(format!("{}: {reason}", member.snapshot_label));
        }
    }
    Ok(())
}

fn prepared_attachment_from_bundle(
    bundle: Bundle,
    catalog: &impl TargetCatalog,
) -> Result<PreparedBundleAttachment, BundleError> {
    let resolved = resolve_bundle(bundle, catalog)?;
    for member in &resolved.members {
        if member.resolution.status != ResolutionStatus::Ready {
            return invalid(format!(
                "{} is {}: {}",
                member.member.snapshot_label,
                match member.resolution.status {
                    ResolutionStatus::Ready => "ready",
                    ResolutionStatus::Missing => "missing",
                    ResolutionStatus::Incompatible => "incompatible",
                },
                member
                    .resolution
                    .reason
                    .as_deref()
                    .unwrap_or("the target is unavailable")
            ));
        }
    }

    let projects = resolved
        .bundle
        .members
        .iter()
        .filter_map(|member| match &member.target {
            MemberTarget::Project { path } => Some(path),
            _ => None,
        })
        .collect::<Vec<_>>();
    if projects.len() != 1 {
        return invalid("an executable bundle must contain exactly one working directory");
    }
    let cwd = PathBuf::from(projects[0])
        .canonicalize()
        .map_err(|_| BundleError::Invalid {
            message: "the bundle working directory is no longer available".into(),
        })?;
    if !cwd.is_dir() {
        return invalid("the bundle working directory is not a directory");
    }
    let cwd = cwd.to_string_lossy().into_owned();

    let snapshot = BundleAttachmentSnapshot {
        schema_version: ATTACHMENT_SCHEMA_VERSION,
        bundle_id: resolved.bundle.id.clone(),
        bundle_revision: resolved.bundle.revision,
        bundle_name: resolved.bundle.name.clone(),
        runner: resolved.bundle.runner,
        model_id: resolved.bundle.model_id.clone(),
        cwd: cwd.clone(),
        members: resolved
            .bundle
            .members
            .iter()
            .map(|member| SnapshotMember {
                member_id: member.id.clone(),
                ordinal: member.ordinal,
                kind: member.kind,
                role: member.role,
                target: member.target.clone(),
                snapshot_label: member.snapshot_label.clone(),
                disposition: member_disposition(member),
                note: None,
            })
            .collect(),
    };
    validate_attachment(&snapshot)?;
    Ok(PreparedBundleAttachment {
        runner: resolved.bundle.runner.provider(),
        model_id: resolved.bundle.model_id,
        cwd,
        snapshot,
    })
}

fn member_disposition(member: &BundleMember) -> SnapshotDisposition {
    match (member.kind, member.role) {
        (MemberKind::Skill | MemberKind::Agent, MemberRole::Available)
        | (MemberKind::Prompt, MemberRole::Prefill) => SnapshotDisposition::Available,
        _ => SnapshotDisposition::Apply,
    }
}

pub fn update(
    id: &str,
    expected_revision: i64,
    draft: BundleDraft,
    catalog: &impl TargetCatalog,
) -> Result<Bundle, BundleError> {
    let mut connection = super::data();
    update_on(&mut connection, id, expected_revision, draft, catalog)
}

pub fn update_on(
    connection: &mut Connection,
    id: &str,
    expected_revision: i64,
    draft: BundleDraft,
    catalog: &impl TargetCatalog,
) -> Result<Bundle, BundleError> {
    let existing =
        get_on(connection, id)?.ok_or_else(|| BundleError::NotFound { id: id.to_string() })?;
    if existing.revision != expected_revision {
        return Err(BundleError::RevisionConflict {
            expected: expected_revision,
            actual: existing.revision,
        });
    }
    let timestamp = super::now().max(existing.created_at);
    let prepared = prepare_draft(draft, catalog, Some(&existing), timestamp)?;
    let transaction = connection.transaction()?;
    let changed = transaction.execute(
        "UPDATE bundle
            SET name = ?1, description = ?2, runner = ?3, model_id = ?4,
                memory_mode = ?5, revision = revision + 1, updated_at = ?6
          WHERE id = ?7 AND revision = ?8",
        params![
            prepared.name,
            prepared.description,
            prepared.runner.as_str(),
            prepared.model_id,
            prepared.memory_mode.as_str(),
            timestamp,
            id,
            expected_revision,
        ],
    )?;
    if changed != 1 {
        let actual = transaction
            .query_row("SELECT revision FROM bundle WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?
            .ok_or_else(|| BundleError::NotFound { id: id.to_string() })?;
        return Err(BundleError::RevisionConflict {
            expected: expected_revision,
            actual,
        });
    }
    transaction.execute("DELETE FROM bundle_member WHERE bundle_id = ?1", [id])?;
    insert_members(&transaction, id, &prepared.members)?;
    transaction.commit()?;
    get_on(connection, id)?.ok_or_else(|| BundleError::NotFound { id: id.to_string() })
}

pub fn delete(id: &str, expected_revision: i64) -> Result<(), BundleError> {
    let mut connection = super::data();
    delete_on(&mut connection, id, expected_revision)
}

pub fn delete_on(
    connection: &mut Connection,
    id: &str,
    expected_revision: i64,
) -> Result<(), BundleError> {
    let transaction = connection.transaction()?;
    let actual = transaction
        .query_row("SELECT revision FROM bundle WHERE id = ?1", [id], |row| {
            row.get::<_, i64>(0)
        })
        .optional()?
        .ok_or_else(|| BundleError::NotFound { id: id.to_string() })?;
    if actual != expected_revision {
        return Err(BundleError::RevisionConflict {
            expected: expected_revision,
            actual,
        });
    }
    transaction.execute("DELETE FROM bundle WHERE id = ?1", [id])?;
    transaction.commit()?;
    Ok(())
}

/// Commits the session, immutable bundle snapshot and first queued turn as one
/// durable fact. A revision change or invalid/missing lock field rolls the
/// whole transaction back, so a runner can never spawn for a half-attached
/// session.
pub fn create_session_with_bundle_turn(
    session: super::sessions::NewSession,
    turn: super::sessions::NewTurn,
    prepared: PreparedBundleAttachment,
) -> Result<super::sessions::QueuedTurn, BundleError> {
    let mut connection = super::data();
    create_session_with_bundle_turn_on(&mut connection, session, turn, prepared)
}

pub fn create_session_with_bundle_turn_on(
    connection: &mut Connection,
    session: super::sessions::NewSession,
    turn: super::sessions::NewTurn,
    prepared: PreparedBundleAttachment,
) -> Result<super::sessions::QueuedTurn, BundleError> {
    validate_attachment(&prepared.snapshot)?;
    if session.runner != prepared.runner
        || session.cwd != prepared.cwd
        || turn.requested_model != prepared.model_id
        || prepared.snapshot.runner.provider() != prepared.runner
        || prepared.snapshot.cwd != prepared.cwd
        || prepared.snapshot.model_id != prepared.model_id
    {
        return invalid("the bundle locks the chat runner, working directory, and model");
    }
    super::sessions::validate_session(&session)
        .map_err(|message| BundleError::Invalid { message })?;
    super::sessions::validate_turn(&turn).map_err(|message| BundleError::Invalid { message })?;

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(BundleError::from)?;
    validate_snapshot_matches_current_bundle(&transaction, &prepared.snapshot)?;
    let timestamp = super::now();
    let (session_id, turn_id) =
        super::sessions::insert_session_with_turn(&transaction, session, turn, timestamp)
            .map_err(|message| BundleError::Invalid { message })?;
    insert_session_attachment(&transaction, &session_id, &prepared.snapshot, timestamp)?;
    transaction.commit()?;
    super::sessions::queued_turn_by_ids(connection, &session_id, &turn_id)
        .map_err(|message| BundleError::Database { message })
}

pub fn attach_session_on(
    connection: &mut Connection,
    session_id: &str,
    snapshot: &BundleAttachmentSnapshot,
) -> Result<SessionBundleAttachment, BundleError> {
    validate_attachment(snapshot)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(BundleError::from)?;
    let (session_runner, session_cwd): (String, String) = transaction
        .query_row(
            "SELECT runner, cwd FROM chat_session WHERE id = ?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| BundleError::Invalid {
            message: "the chat session does not exist".into(),
        })?;
    if session_runner != snapshot.runner.as_str() || session_cwd != snapshot.cwd {
        return Err(BundleError::Invalid {
            message: "the attachment runner and cwd must match the chat session".into(),
        });
    }
    let turns: i64 = transaction.query_row(
        "SELECT count(*) FROM chat_turn WHERE session_id = ?1",
        [session_id],
        |row| row.get(0),
    )?;
    if turns != 0 {
        return Err(BundleError::Invalid {
            message: "a bundle can only be attached before the first turn".into(),
        });
    }
    validate_snapshot_matches_current_bundle(&transaction, snapshot)?;
    let attached_at = super::now();
    let attachment = insert_session_attachment(&transaction, session_id, snapshot, attached_at)?;
    transaction.commit()?;
    Ok(attachment)
}

fn insert_session_attachment(
    connection: &Connection,
    session_id: &str,
    snapshot: &BundleAttachmentSnapshot,
    attached_at: i64,
) -> Result<SessionBundleAttachment, BundleError> {
    let json = serde_json::to_string(snapshot).map_err(|error| BundleError::Invalid {
        message: format!("attachment snapshot is not serializable: {error}"),
    })?;
    if json.len() > MAX_ATTACHMENT_BYTES {
        return Err(BundleError::Invalid {
            message: "attachment snapshot exceeds 256 KiB".into(),
        });
    }
    connection.execute(
        "INSERT INTO chat_session_bundle(
            session_id, source_bundle_id, source_bundle_revision,
            source_bundle_name, snapshot_schema_version, snapshot_json,
            attached_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            session_id,
            snapshot.bundle_id,
            snapshot.bundle_revision,
            snapshot.bundle_name,
            snapshot.schema_version,
            json,
            attached_at,
        ],
    )?;
    Ok(SessionBundleAttachment {
        session_id: session_id.to_string(),
        attached_at,
        snapshot: snapshot.clone(),
    })
}

fn validate_snapshot_matches_current_bundle(
    connection: &Connection,
    snapshot: &BundleAttachmentSnapshot,
) -> Result<(), BundleError> {
    let bundle = get_on(connection, &snapshot.bundle_id)?.ok_or_else(|| BundleError::NotFound {
        id: snapshot.bundle_id.clone(),
    })?;
    if bundle.revision != snapshot.bundle_revision {
        return Err(BundleError::RevisionConflict {
            expected: snapshot.bundle_revision,
            actual: bundle.revision,
        });
    }
    let project = bundle
        .members
        .iter()
        .find_map(|member| match &member.target {
            MemberTarget::Project { path } => Some(path),
            _ => None,
        })
        .ok_or_else(|| BundleError::Invalid {
            message: "the bundle no longer has a working directory".into(),
        })?;
    let canonical_cwd = PathBuf::from(project)
        .canonicalize()
        .map_err(|_| BundleError::Invalid {
            message: "the bundle working directory is no longer available".into(),
        })?
        .to_string_lossy()
        .into_owned();
    let members = bundle
        .members
        .iter()
        .map(|member| SnapshotMember {
            member_id: member.id.clone(),
            ordinal: member.ordinal,
            kind: member.kind,
            role: member.role,
            target: member.target.clone(),
            snapshot_label: member.snapshot_label.clone(),
            disposition: member_disposition(member),
            note: None,
        })
        .collect::<Vec<_>>();
    if snapshot.bundle_name != bundle.name
        || snapshot.runner != bundle.runner
        || snapshot.model_id != bundle.model_id
        || snapshot.cwd != canonical_cwd
        || snapshot.members != members
    {
        return invalid("the bundle attachment no longer matches its current revision");
    }
    Ok(())
}

pub fn get_session_attachment(
    session_id: &str,
) -> Result<Option<SessionBundleAttachment>, BundleError> {
    get_session_attachment_on(&super::data(), session_id)
}

pub fn get_session_attachment_on(
    connection: &Connection,
    session_id: &str,
) -> Result<Option<SessionBundleAttachment>, BundleError> {
    let row = connection
        .query_row(
            "SELECT snapshot_json, attached_at
               FROM chat_session_bundle WHERE session_id = ?1",
            [session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    row.map(|(json, attached_at)| {
        let snapshot = serde_json::from_str(&json).map_err(|error| BundleError::Database {
            message: format!("invalid stored bundle attachment: {error}"),
        })?;
        Ok(SessionBundleAttachment {
            session_id: session_id.to_string(),
            attached_at,
            snapshot,
        })
    })
    .transpose()
}

struct PreparedDraft {
    name: String,
    description: String,
    runner: BundleRunner,
    model_id: Option<String>,
    memory_mode: MemoryMode,
    members: Vec<BundleMember>,
}

fn prepare_draft(
    draft: BundleDraft,
    catalog: &impl TargetCatalog,
    existing: Option<&Bundle>,
    timestamp: i64,
) -> Result<PreparedDraft, BundleError> {
    let name = bounded_required("bundle name", &draft.name, MAX_NAME_CHARS)?;
    let description = bounded_optional(
        "bundle description",
        &draft.description,
        MAX_DESCRIPTION_CHARS,
    )?;
    let model_id = draft
        .model_id
        .as_deref()
        .map(|model| bounded_required("model id", model, MAX_MODEL_CHARS))
        .transpose()?;
    if draft.members.len() > MAX_MEMBERS {
        return invalid(format!(
            "a bundle can contain at most {MAX_MEMBERS} members"
        ));
    }

    let mut ordinals = draft
        .members
        .iter()
        .map(|member| member.ordinal)
        .collect::<Vec<_>>();
    ordinals.sort_unstable();
    if ordinals
        .iter()
        .enumerate()
        .any(|(expected, actual)| *actual as usize != expected)
    {
        return invalid("member ordinals must be unique and contiguous from zero");
    }

    let previous = existing
        .map(|bundle| {
            bundle
                .members
                .iter()
                .map(|member| (member.id.as_str(), member))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let mut ids = HashSet::new();
    let mut targets = HashSet::new();
    let mut project_count = 0;
    let mut prompt_count = 0;
    let mut primary_agent_count = 0;
    let mut memory_count = 0;
    let mut members = Vec::with_capacity(draft.members.len());

    for member in draft.members {
        validate_kind_role(member.kind, member.role)?;
        validate_kind_target(member.kind, &member.target)?;
        if let Some(text) = member.target.text() {
            bounded_required("member target", text, MAX_TARGET_CHARS)?;
        }
        if member.target.integer().is_some_and(|id| id <= 0) {
            return invalid("media collection ids must be positive");
        }
        let duplicate_key = (member.kind, member.target.identity_key());
        if !targets.insert(duplicate_key) {
            return invalid("the same target cannot appear twice in a bundle");
        }
        project_count += usize::from(member.kind == MemberKind::Project);
        prompt_count += usize::from(member.kind == MemberKind::Prompt);
        primary_agent_count +=
            usize::from(member.kind == MemberKind::Agent && member.role == MemberRole::Primary);
        memory_count += usize::from(member.kind == MemberKind::Memory);

        let (id, old) = match (&member.id, existing) {
            (Some(_), None) => return invalid("new bundle members cannot supply ids"),
            (Some(id), Some(_)) => {
                validate_uuid("bundle member id", id)?;
                if !ids.insert(id.clone()) {
                    return invalid("bundle member ids must be unique");
                }
                let old =
                    previous
                        .get(id.as_str())
                        .copied()
                        .ok_or_else(|| BundleError::Invalid {
                            message: "an updated member id does not belong to this bundle".into(),
                        })?;
                (id.clone(), Some(old))
            }
            (None, _) => {
                let id = Uuid::new_v4().to_string();
                ids.insert(id.clone());
                (id, None)
            }
        };

        let resolution = catalog
            .resolve(draft.runner, member.kind, &member.target)
            .map_err(|message| BundleError::Invalid { message })?;
        let unchanged = old.is_some_and(|old| {
            existing.is_some_and(|bundle| bundle.runner == draft.runner)
                && old.kind == member.kind
                && old.role == member.role
                && old.target == member.target
        });
        let snapshot_label = match resolution.status {
            ResolutionStatus::Ready => bounded_required(
                "resolved target label",
                resolution.current_label.as_deref().unwrap_or(""),
                MAX_LABEL_CHARS,
            )?,
            ResolutionStatus::Missing | ResolutionStatus::Incompatible if unchanged => old
                .expect("unchanged members have a previous row")
                .snapshot_label
                .clone(),
            _ => {
                return invalid(format!(
                    "{}: {}",
                    resolution
                        .current_label
                        .as_deref()
                        .unwrap_or("bundle member cannot be resolved"),
                    resolution.reason.as_deref().unwrap_or("target unavailable")
                ));
            }
        };
        members.push(BundleMember {
            id,
            ordinal: member.ordinal,
            kind: member.kind,
            role: member.role,
            target: member.target,
            snapshot_label,
            created_at: old.map(|old| old.created_at).unwrap_or(timestamp),
        });
    }
    if project_count > 1 || prompt_count > 1 || primary_agent_count > 1 {
        return invalid("a bundle allows one project, one prompt, and one primary agent");
    }
    if (draft.memory_mode == MemoryMode::Inherit && memory_count != 0)
        || (draft.memory_mode == MemoryMode::Supplement && memory_count == 0)
    {
        return invalid(
            "supplement memory mode requires at least one memory member; inherit mode allows none",
        );
    }
    members.sort_by_key(|member| member.ordinal);
    Ok(PreparedDraft {
        name,
        description,
        runner: draft.runner,
        model_id,
        memory_mode: draft.memory_mode,
        members,
    })
}

fn insert_members(
    connection: &Connection,
    bundle_id: &str,
    members: &[BundleMember],
) -> Result<(), BundleError> {
    for member in members {
        connection.execute(
            "INSERT INTO bundle_member(
                id, bundle_id, ordinal, kind, role, target_text,
                target_integer, snapshot_label, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                member.id,
                bundle_id,
                member.ordinal,
                member.kind.as_str(),
                member.role.as_str(),
                member.target.text(),
                member.target.integer(),
                member.snapshot_label,
                member.created_at,
            ],
        )?;
    }
    Ok(())
}

fn row_to_bundle_header(row: &Row<'_>) -> rusqlite::Result<Bundle> {
    let runner: String = row.get(3)?;
    let memory_mode: String = row.get(5)?;
    Ok(Bundle {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        runner: BundleRunner::from_db(&runner, 3)?,
        model_id: row.get(4)?,
        memory_mode: MemoryMode::from_db(&memory_mode, 5)?,
        revision: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        members: Vec::new(),
    })
}

fn read_members(
    connection: &Connection,
    bundle_id: &str,
) -> Result<Vec<BundleMember>, BundleError> {
    let mut statement = connection.prepare(
        "SELECT id, ordinal, kind, role, target_text, target_integer,
                snapshot_label, created_at
           FROM bundle_member WHERE bundle_id = ?1 ORDER BY ordinal",
    )?;
    let rows = statement.query_map([bundle_id], |row| {
        let kind_value: String = row.get(2)?;
        let role_value: String = row.get(3)?;
        let kind = MemberKind::from_db(&kind_value, 2)?;
        let role = MemberRole::from_db(&role_value, 3)?;
        let text: Option<String> = row.get(4)?;
        let integer: Option<i64> = row.get(5)?;
        let target = match kind {
            MemberKind::Project => MemberTarget::Project {
                path: text.ok_or_else(|| invalid_db_value(4, "missing project path"))?,
            },
            MemberKind::Mcp => MemberTarget::McpDeclaration {
                id: text.ok_or_else(|| invalid_db_value(4, "missing MCP declaration id"))?,
            },
            MemberKind::MediaCollection => MemberTarget::MediaCollection {
                id: integer.ok_or_else(|| invalid_db_value(5, "missing collection id"))?,
            },
            _ => MemberTarget::Entry {
                id: text.ok_or_else(|| invalid_db_value(4, "missing entry id"))?,
            },
        };
        Ok(BundleMember {
            id: row.get(0)?,
            ordinal: row.get(1)?,
            kind,
            role,
            target,
            snapshot_label: row.get(6)?,
            created_at: row.get(7)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn validate_attachment(snapshot: &BundleAttachmentSnapshot) -> Result<(), BundleError> {
    if snapshot.schema_version != ATTACHMENT_SCHEMA_VERSION {
        return invalid("unsupported bundle attachment schema version");
    }
    validate_uuid("bundle id", &snapshot.bundle_id)?;
    if snapshot.bundle_revision < 1 {
        return invalid("bundle attachment revision must be positive");
    }
    bounded_required("bundle name", &snapshot.bundle_name, MAX_NAME_CHARS)?;
    bounded_required("working directory", &snapshot.cwd, MAX_TARGET_CHARS)?;
    if let Some(model) = snapshot.model_id.as_deref() {
        bounded_required("model id", model, MAX_MODEL_CHARS)?;
    }
    if snapshot.members.len() > MAX_MEMBERS {
        return invalid(format!(
            "an attachment can contain at most {MAX_MEMBERS} members"
        ));
    }
    let mut ids = HashSet::new();
    let mut ordinals = HashSet::new();
    let mut targets = HashSet::new();
    let mut project_count = 0usize;
    let mut prompt_count = 0usize;
    let mut primary_agent_count = 0usize;
    for member in &snapshot.members {
        validate_uuid("bundle member id", &member.member_id)?;
        if !ids.insert(&member.member_id) {
            return invalid("attachment member ids must be unique");
        }
        if !ordinals.insert(member.ordinal) {
            return invalid("attachment member ordinals must be unique");
        }
        validate_kind_role(member.kind, member.role)?;
        validate_kind_target(member.kind, &member.target)?;
        if let Some(text) = member.target.text() {
            bounded_required("member target", text, MAX_TARGET_CHARS)?;
        }
        if member.target.integer().is_some_and(|id| id <= 0) {
            return invalid("media collection ids must be positive");
        }
        if !targets.insert((member.kind, member.target.identity_key())) {
            return invalid("the same target cannot appear twice in an attachment");
        }
        bounded_required("snapshot label", &member.snapshot_label, MAX_LABEL_CHARS)?;
        if let Some(note) = member.note.as_deref() {
            bounded_optional("attachment note", note, MAX_NOTE_CHARS)?;
        }
        project_count += usize::from(member.kind == MemberKind::Project);
        prompt_count += usize::from(member.kind == MemberKind::Prompt);
        primary_agent_count +=
            usize::from(member.kind == MemberKind::Agent && member.role == MemberRole::Primary);
    }
    if ordinals
        .iter()
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .max()
        .is_some_and(|max| max as usize + 1 != snapshot.members.len())
    {
        return invalid("attachment member ordinals must be contiguous from zero");
    }
    if project_count != 1 || prompt_count > 1 || primary_agent_count > 1 {
        return invalid(
            "an attachment requires one project and allows one prompt and one primary agent",
        );
    }
    Ok(())
}

fn validate_kind_role(kind: MemberKind, role: MemberRole) -> Result<(), BundleError> {
    let valid = match kind {
        MemberKind::Project => role == MemberRole::WorkingDirectory,
        MemberKind::Skill => matches!(role, MemberRole::Available | MemberRole::InvokeFirstTurn),
        MemberKind::Prompt => role == MemberRole::Prefill,
        MemberKind::Agent => matches!(role, MemberRole::Available | MemberRole::Primary),
        MemberKind::Memory => role == MemberRole::Supplement,
        MemberKind::Mcp => role == MemberRole::Enabled,
        MemberKind::MediaCollection => role == MemberRole::Retrieval,
    };
    if valid {
        Ok(())
    } else {
        invalid(format!(
            "role {} is invalid for {} members",
            role.as_str(),
            kind.as_str()
        ))
    }
}

fn validate_kind_target(kind: MemberKind, target: &MemberTarget) -> Result<(), BundleError> {
    let valid = matches!(
        (kind, target),
        (MemberKind::Project, MemberTarget::Project { .. })
            | (MemberKind::Skill, MemberTarget::Entry { .. })
            | (MemberKind::Prompt, MemberTarget::Entry { .. })
            | (MemberKind::Agent, MemberTarget::Entry { .. })
            | (MemberKind::Memory, MemberTarget::Entry { .. })
            | (MemberKind::Mcp, MemberTarget::McpDeclaration { .. })
            | (
                MemberKind::MediaCollection,
                MemberTarget::MediaCollection { .. }
            )
    );
    if valid {
        Ok(())
    } else {
        invalid(format!("target type does not match {}", kind.as_str()))
    }
}

fn bounded_required(field: &str, value: &str, max_chars: usize) -> Result<String, BundleError> {
    let trimmed = value.trim();
    let count = trimmed.chars().count();
    if count == 0 || count > max_chars {
        return invalid(format!("{field} must contain 1 to {max_chars} characters"));
    }
    Ok(trimmed.to_string())
}

fn bounded_optional(field: &str, value: &str, max_chars: usize) -> Result<String, BundleError> {
    let trimmed = value.trim();
    if trimmed.chars().count() > max_chars {
        return invalid(format!(
            "{field} must contain no more than {max_chars} characters"
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_uuid(field: &str, value: &str) -> Result<(), BundleError> {
    Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| BundleError::Invalid {
            message: format!("{field} must be a UUID"),
        })
}

fn invalid<T>(message: impl Into<String>) -> Result<T, BundleError> {
    Err(BundleError::Invalid {
        message: message.into(),
    })
}

fn invalid_db_value(column: usize, message: &'static str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeCatalog {
        missing: HashSet<String>,
        incompatible: HashSet<String>,
    }

    impl TargetCatalog for FakeCatalog {
        fn resolve(
            &self,
            _runner: BundleRunner,
            _kind: MemberKind,
            target: &MemberTarget,
        ) -> Result<TargetResolution, String> {
            let key = target.identity_key();
            if self.missing.contains(&key) {
                return Ok(TargetResolution::missing("fixture target is missing"));
            }
            if self.incompatible.contains(&key) {
                return Ok(TargetResolution::incompatible(
                    Some("Fixture".into()),
                    "fixture target is incompatible",
                ));
            }
            Ok(TargetResolution::ready(match target {
                MemberTarget::Project { path } => path.as_str(),
                MemberTarget::Entry { id } | MemberTarget::McpDeclaration { id } => id.as_str(),
                MemberTarget::MediaCollection { .. } => "References",
            }))
        }
    }

    fn connection() -> Connection {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        super::super::migrate_data(&mut connection).unwrap();
        connection
    }

    fn draft() -> BundleDraft {
        BundleDraft {
            name: "Launch kit".into(),
            description: "Real configuration".into(),
            runner: BundleRunner::ClaudeCode,
            model_id: Some("sonnet".into()),
            memory_mode: MemoryMode::Inherit,
            members: vec![
                BundleMemberDraft {
                    id: None,
                    ordinal: 0,
                    kind: MemberKind::Project,
                    role: MemberRole::WorkingDirectory,
                    target: MemberTarget::Project {
                        path: "/tmp/project".into(),
                    },
                },
                BundleMemberDraft {
                    id: None,
                    ordinal: 1,
                    kind: MemberKind::Skill,
                    role: MemberRole::Available,
                    target: MemberTarget::Entry {
                        id: "/tmp/SKILL.md".into(),
                    },
                },
                BundleMemberDraft {
                    id: None,
                    ordinal: 2,
                    kind: MemberKind::Mcp,
                    role: MemberRole::Enabled,
                    target: MemberTarget::McpDeclaration {
                        id: "mcp-declaration".into(),
                    },
                },
                BundleMemberDraft {
                    id: None,
                    ordinal: 3,
                    kind: MemberKind::MediaCollection,
                    role: MemberRole::Retrieval,
                    target: MemberTarget::MediaCollection { id: 7 },
                },
            ],
        }
    }

    fn update_draft(bundle: &Bundle) -> BundleDraft {
        BundleDraft {
            name: bundle.name.clone(),
            description: bundle.description.clone(),
            runner: bundle.runner,
            model_id: bundle.model_id.clone(),
            memory_mode: bundle.memory_mode,
            members: bundle
                .members
                .iter()
                .map(|member| BundleMemberDraft {
                    id: Some(member.id.clone()),
                    ordinal: member.ordinal,
                    kind: member.kind,
                    role: member.role,
                    target: member.target.clone(),
                })
                .collect(),
        }
    }

    #[test]
    fn aggregate_round_trips_with_server_owned_ids_and_labels() {
        let mut connection = connection();
        let bundle = create_on(&mut connection, draft(), &FakeCatalog::default()).unwrap();

        assert_eq!(bundle.revision, 1);
        assert_eq!(bundle.members.len(), 4);
        assert!(Uuid::parse_str(&bundle.id).is_ok());
        assert!(bundle
            .members
            .iter()
            .all(|member| Uuid::parse_str(&member.id).is_ok()));
        assert_eq!(bundle.members[3].snapshot_label, "References");
        assert_eq!(get_on(&connection, &bundle.id).unwrap(), Some(bundle));
    }

    #[test]
    fn optimistic_update_is_atomic_and_rejects_stale_revisions() {
        let mut connection = connection();
        let original = create_on(&mut connection, draft(), &FakeCatalog::default()).unwrap();
        let mut changed = update_draft(&original);
        changed.name = "Renamed".into();
        let updated = update_on(
            &mut connection,
            &original.id,
            original.revision,
            changed,
            &FakeCatalog::default(),
        )
        .unwrap();
        assert_eq!((updated.name.as_str(), updated.revision), ("Renamed", 2));

        let mut stale = update_draft(&updated);
        stale.name = "Must not win".into();
        let error = update_on(
            &mut connection,
            &updated.id,
            original.revision,
            stale,
            &FakeCatalog::default(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            BundleError::RevisionConflict {
                expected: 1,
                actual: 2
            }
        );
        assert_eq!(
            get_on(&connection, &updated.id).unwrap().unwrap().name,
            "Renamed"
        );
    }

    #[test]
    fn missing_existing_members_survive_but_new_missing_targets_are_rejected() {
        let mut connection = connection();
        let bundle = create_on(&mut connection, draft(), &FakeCatalog::default()).unwrap();
        let missing_key = bundle.members[1].target.identity_key();
        let catalog = FakeCatalog {
            missing: HashSet::from([missing_key]),
            incompatible: HashSet::new(),
        };

        let mut edit = update_draft(&bundle);
        edit.description = "The source disappeared after creation".into();
        let updated =
            update_on(&mut connection, &bundle.id, bundle.revision, edit, &catalog).unwrap();
        assert_eq!(updated.members[1].snapshot_label, "/tmp/SKILL.md");
        let resolved = resolve_bundle(updated.clone(), &catalog).unwrap();
        assert_eq!(
            resolved.members[1].resolution.status,
            ResolutionStatus::Missing
        );

        let mut new_missing = update_draft(&updated);
        new_missing.members.push(BundleMemberDraft {
            id: None,
            ordinal: 4,
            kind: MemberKind::Skill,
            role: MemberRole::Available,
            target: MemberTarget::Entry {
                id: "/tmp/missing.md".into(),
            },
        });
        let catalog = FakeCatalog {
            missing: HashSet::from(["entry:/tmp/SKILL.md".into(), "entry:/tmp/missing.md".into()]),
            incompatible: HashSet::new(),
        };
        assert!(matches!(
            update_on(
                &mut connection,
                &updated.id,
                updated.revision,
                new_missing,
                &catalog,
            ),
            Err(BundleError::Invalid { .. })
        ));
        assert_eq!(
            get_on(&connection, &updated.id).unwrap().unwrap().revision,
            2
        );
    }

    #[test]
    fn structural_validation_and_sql_constraints_fail_closed() {
        let mut connection = connection();
        let mut wrong_target = draft();
        wrong_target.members[0].target = MemberTarget::Entry {
            id: "/tmp/not-a-project".into(),
        };
        assert!(matches!(
            create_on(&mut connection, wrong_target, &FakeCatalog::default()),
            Err(BundleError::Invalid { .. })
        ));

        let bundle = create_on(&mut connection, draft(), &FakeCatalog::default()).unwrap();
        let sql_error = connection.execute(
            "INSERT INTO bundle_member(
                id, bundle_id, ordinal, kind, role, target_text,
                snapshot_label, created_at
             ) VALUES (?1, ?2, 9, 'project', 'prefill', '/tmp/other', 'Other', 1)",
            params![Uuid::new_v4().to_string(), bundle.id],
        );
        assert!(
            sql_error.is_err(),
            "role constraints must survive API bypass"
        );

        let duplicate_prompt = connection.execute(
            "INSERT INTO bundle_member(
                id, bundle_id, ordinal, kind, role, target_text,
                snapshot_label, created_at
             ) VALUES (?1, ?2, 9, 'prompt', 'prefill', '/tmp/a.md', 'A', 1),
                      (?3, ?2, 10, 'prompt', 'prefill', '/tmp/b.md', 'B', 1)",
            params![
                Uuid::new_v4().to_string(),
                bundle.id,
                Uuid::new_v4().to_string()
            ],
        );
        assert!(
            duplicate_prompt.is_err(),
            "partial cardinality indexes must hold"
        );
    }

    #[test]
    fn attachment_is_immutable_pre_turn_and_outlives_its_bundle() {
        let mut connection = connection();
        let project = tempfile::tempdir().unwrap();
        let mut draft = draft();
        draft.members[0].target = MemberTarget::Project {
            path: project.path().to_string_lossy().into_owned(),
        };
        let bundle = create_on(&mut connection, draft, &FakeCatalog::default()).unwrap();
        let prepared = resolve_for_attachment_on(
            &connection,
            &bundle.id,
            bundle.revision,
            &FakeCatalog::default(),
        )
        .unwrap();
        let queued = create_session_with_bundle_turn_on(
            &mut connection,
            super::super::sessions::NewSession {
                runner: prepared.runner,
                runner_session_id: Some(Uuid::new_v4().to_string()),
                cwd: prepared.cwd.clone(),
                title: "Chat".into(),
            },
            super::super::sessions::NewTurn {
                prompt: "Start".into(),
                requested_model: prepared.model_id.clone(),
                requested_effort: None,
                permission_mode: "plan".into(),
            },
            prepared.clone(),
        )
        .unwrap();
        assert!(
            attach_session_on(&mut connection, &queued.session.id, &prepared.snapshot).is_err()
        );

        delete_on(&mut connection, &bundle.id, bundle.revision).unwrap();
        let attached = get_session_attachment_on(&connection, &queued.session.id)
            .unwrap()
            .unwrap();
        assert_eq!(attached.snapshot, prepared.snapshot);
    }

    #[test]
    fn attachment_after_first_turn_is_rejected_without_writing() {
        let mut connection = connection();
        let project = tempfile::tempdir().unwrap();
        let mut draft = draft();
        draft.members[0].target = MemberTarget::Project {
            path: project.path().to_string_lossy().into_owned(),
        };
        let bundle = create_on(&mut connection, draft, &FakeCatalog::default()).unwrap();
        let prepared = resolve_for_attachment_on(
            &connection,
            &bundle.id,
            bundle.revision,
            &FakeCatalog::default(),
        )
        .unwrap();
        let session_id = Uuid::new_v4().to_string();
        connection
            .execute(
                "INSERT INTO chat_session(
                    id, runner, cwd, title, created_at, updated_at
                 ) VALUES (?1, 'claude-code', ?2, 'Chat', 1, 1)",
                params![&session_id, &prepared.cwd],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO chat_turn(
                    id, session_id, ordinal, prompt, permission_mode,
                    status, created_at
                 ) VALUES (?1, ?2, 1, 'hello', 'safe', 'queued', 1)",
                params![Uuid::new_v4().to_string(), session_id],
            )
            .unwrap();
        assert!(matches!(
            attach_session_on(&mut connection, &session_id, &prepared.snapshot),
            Err(BundleError::Invalid { .. })
        ));
        assert!(get_session_attachment_on(&connection, &session_id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn attachment_revision_conflict_rolls_back_session_and_first_turn() {
        let mut connection = connection();
        let project = tempfile::tempdir().unwrap();
        let mut draft = draft();
        draft.members[0].target = MemberTarget::Project {
            path: project.path().to_string_lossy().into_owned(),
        };
        let bundle = create_on(&mut connection, draft, &FakeCatalog::default()).unwrap();
        let prepared = resolve_for_attachment_on(
            &connection,
            &bundle.id,
            bundle.revision,
            &FakeCatalog::default(),
        )
        .unwrap();
        let mut edit = update_draft(&bundle);
        edit.name = "New revision".into();
        update_on(
            &mut connection,
            &bundle.id,
            bundle.revision,
            edit,
            &FakeCatalog::default(),
        )
        .unwrap();

        let result = create_session_with_bundle_turn_on(
            &mut connection,
            super::super::sessions::NewSession {
                runner: prepared.runner,
                runner_session_id: None,
                cwd: prepared.cwd.clone(),
                title: "Must not exist".into(),
            },
            super::super::sessions::NewTurn {
                prompt: "Must not run".into(),
                requested_model: prepared.model_id.clone(),
                requested_effort: None,
                permission_mode: "plan".into(),
            },
            prepared,
        );
        assert!(matches!(result, Err(BundleError::RevisionConflict { .. })));
        for table in ["chat_session", "chat_turn", "chat_session_bundle"] {
            let rows: i64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(rows, 0, "{table} must roll back with the attachment");
        }
    }

    #[test]
    fn attached_model_is_locked_for_later_turns() {
        let mut connection = connection();
        let project = tempfile::tempdir().unwrap();
        let mut draft = draft();
        draft.members[0].target = MemberTarget::Project {
            path: project.path().to_string_lossy().into_owned(),
        };
        let bundle = create_on(&mut connection, draft, &FakeCatalog::default()).unwrap();
        let prepared = resolve_for_attachment_on(
            &connection,
            &bundle.id,
            bundle.revision,
            &FakeCatalog::default(),
        )
        .unwrap();
        let queued = create_session_with_bundle_turn_on(
            &mut connection,
            super::super::sessions::NewSession {
                runner: prepared.runner,
                runner_session_id: None,
                cwd: prepared.cwd.clone(),
                title: "Locked".into(),
            },
            super::super::sessions::NewTurn {
                prompt: "First".into(),
                requested_model: prepared.model_id.clone(),
                requested_effort: None,
                permission_mode: "plan".into(),
            },
            prepared,
        )
        .unwrap();
        connection
            .execute(
                "UPDATE chat_turn SET status = 'completed', finished_at = 2 WHERE id = ?1",
                [&queued.turn.id],
            )
            .unwrap();
        let different = super::super::sessions::NewTurn {
            prompt: "Second".into(),
            requested_model: Some("another-model".into()),
            requested_effort: None,
            permission_mode: "plan".into(),
        };
        assert!(super::super::sessions::queue_turn_on(
            &mut connection,
            &queued.session.id,
            different,
            3
        )
        .is_err());
        let turns: i64 = connection
            .query_row(
                "SELECT count(*) FROM chat_turn WHERE session_id = ?1",
                [&queued.session.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(turns, 1);
    }
}
