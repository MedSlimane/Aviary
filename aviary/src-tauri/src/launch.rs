//! Secure handoff from Aviary to an interactive runner in Terminal.
//!
//! Terminal necessarily crosses a shell boundary when it opens a `.command`
//! file. That file contains only two shell-quoted paths: this package's
//! `aviary-launch` helper and a private descriptor. Runner arguments, config,
//! memory and environment values never enter shell text. The helper validates
//! the descriptor's owner, modes, location, expiry and immutable artifacts,
//! then constructs `Command` from `OsString` values.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::providers::Runner;
use crate::store::bundles::{
    self, LiveTargetCatalog, MemberKind, MemberRole, PreparedBundleAttachment,
};

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::process::CommandExt;

const DESCRIPTOR_SCHEMA_VERSION: u32 = 1;
const STATUS_SCHEMA_VERSION: u32 = 1;
const MAX_LIFETIME: Duration = Duration::from_secs(5 * 60);
const MAX_CLOCK_SKEW_SECS: u64 = 5;
const MAX_DESCRIPTOR_BYTES: u64 = 256 * 1024;
const MAX_STATUS_BYTES: u64 = 16 * 1024;
const MAX_ARTIFACT_BYTES: usize = 1024 * 1024;
const MAX_ARTIFACT_TOTAL_BYTES: usize = 4 * 1024 * 1024;
const MAX_ARTIFACTS: usize = 16;
const MAX_ARGUMENTS: usize = 64;
const MAX_ENVIRONMENT: usize = 64;
const MAX_OS_VALUE_BYTES: usize = 64 * 1024;
const MAX_ARTIFACT_NAME_BYTES: usize = 96;
const TERMINAL_STATUS_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_PRUNE_SCAN: usize = 256;
const MAX_PRUNE_REMOVALS: usize = 32;
const CAPABILITY_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_CAPABILITY_OUTPUT_BYTES: usize = 256 * 1024;

const DESCRIPTOR_FILE: &str = "descriptor.json";
const STATUS_FILE: &str = "status.json";
const COMMAND_FILE: &str = "launch.command";
const CLAIM_FILE: &str = "claimed";

/// An argument can be literal metadata or a reference to a private artifact.
/// Artifact variants are resolved only after the random launch directory is
/// created, so no caller needs to interpolate a filesystem path into text.
pub enum LaunchValue {
    Literal(OsString),
    ArtifactPath(String),
    PrefixedArtifactPath { prefix: OsString, name: String },
}

pub struct LaunchEnvironment {
    pub key: OsString,
    pub value: LaunchValue,
}

/// Bytes may contain MCP credentials or instruction supplements, so this type
/// intentionally has no `Debug` implementation.
pub struct LaunchArtifact {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// There is deliberately no prompt field. Terminal launches begin as ordinary
/// interactive CLI sessions; a bundle prompt is only a UI prefill.
pub struct LaunchRequest {
    pub cwd: PathBuf,
    pub program: OsString,
    pub arguments: Vec<LaunchValue>,
    pub environment: Vec<LaunchEnvironment>,
    pub artifacts: Vec<LaunchArtifact>,
    pub lifetime: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedLaunch {
    pub launch_id: String,
    pub command_file: PathBuf,
    pub descriptor_file: PathBuf,
    pub status_file: PathBuf,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionOutcome {
    pub exit_code: Option<i32>,
}

impl ExecutionOutcome {
    pub fn process_exit_code(self) -> i32 {
        self.exit_code.unwrap_or(1).clamp(0, 255)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    InvalidLayout,
    InvalidDescriptor,
    InvalidPermissions,
    InvalidOwner,
    SymlinkRejected,
    Expired,
    AlreadyClaimed,
    WorkingDirectoryChanged,
    SpawnFailed,
    WaitFailed,
    Io,
    UnsupportedPlatform,
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidLayout => "the launch handoff is not in Aviary's private launch directory",
            Self::InvalidDescriptor => "the launch descriptor is invalid",
            Self::InvalidPermissions => "the launch handoff has unsafe permissions",
            Self::InvalidOwner => "the launch handoff belongs to another user",
            Self::SymlinkRejected => "symlinks are not allowed in a launch handoff",
            Self::Expired => "the launch request expired",
            Self::AlreadyClaimed => "the launch request was already claimed",
            Self::WorkingDirectoryChanged => {
                "the working directory no longer resolves to the attached directory"
            }
            Self::SpawnFailed => "the runner could not be started",
            Self::WaitFailed => "the runner exit status could not be observed",
            Self::Io => "the private launch handoff could not be read or written",
            Self::UnsupportedPlatform => "terminal launch is unavailable on this platform",
        })
    }
}

impl std::error::Error for LaunchError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum BundleLaunchError {
    Bundle { message: String },
    Unsupported { member: String, reason: String },
    RunnerUnavailable { runner: Runner },
    CapabilityUnavailable { runner: Runner, capability: String },
    Handoff { message: String },
}

impl std::fmt::Display for BundleLaunchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bundle { message } | Self::Handoff { message } => formatter.write_str(message),
            Self::Unsupported { member, reason } => write!(formatter, "{member}: {reason}"),
            Self::RunnerUnavailable { runner } => {
                write!(
                    formatter,
                    "{} is not available on this machine",
                    runner.label()
                )
            }
            Self::CapabilityUnavailable { runner, capability } => write!(
                formatter,
                "{} did not advertise terminal support for {capability}",
                runner.label()
            ),
        }
    }
}

impl std::error::Error for BundleLaunchError {}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EncodedOsString {
    hex: String,
}

#[derive(Serialize, Deserialize)]
struct LaunchDescriptor {
    schema_version: u32,
    launch_id: String,
    submitted_at: u64,
    expires_at: u64,
    cwd: EncodedOsString,
    program: EncodedOsString,
    arguments: Vec<EncodedOsString>,
    environment: Vec<DescriptorEnvironment>,
    artifacts: Vec<DescriptorArtifact>,
}

#[derive(Serialize, Deserialize)]
struct DescriptorEnvironment {
    key: EncodedOsString,
    value: EncodedOsString,
}

#[derive(Serialize, Deserialize)]
struct DescriptorArtifact {
    name: String,
    bytes: u64,
    sha256: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "state")]
enum LaunchStatus {
    Submitted {
        schema_version: u32,
        launch_id: String,
        submitted_at: u64,
    },
    Started {
        schema_version: u32,
        launch_id: String,
        submitted_at: u64,
        started_at: u64,
        pid: u32,
    },
    Exited {
        schema_version: u32,
        launch_id: String,
        submitted_at: u64,
        started_at: u64,
        finished_at: u64,
        exit_code: Option<i32>,
    },
    Failed {
        schema_version: u32,
        launch_id: String,
        submitted_at: u64,
        failed_at: u64,
        reason: FailureReason,
    },
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum FailureReason {
    Expired,
    InvalidPayload,
    WorkingDirectoryChanged,
    Spawn,
    Wait,
    Status,
}

struct SecureLayout {
    launch_dir: PathBuf,
    descriptor: PathBuf,
    status: PathBuf,
    claim: PathBuf,
}

/// Builds a private launch handoff below an injected Aviary directory. The
/// production caller passes `~/.aviary`; tests use an isolated directory.
pub fn prepare_at(
    aviary_dir: &Path,
    helper: &Path,
    request: LaunchRequest,
    now: SystemTime,
) -> Result<PreparedLaunch, LaunchError> {
    #[cfg(not(unix))]
    {
        let _ = (aviary_dir, helper, request, now);
        return Err(LaunchError::UnsupportedPlatform);
    }

    #[cfg(unix)]
    {
        validate_request(&request)?;
        let canonical_cwd = request
            .cwd
            .canonicalize()
            .map_err(|_| LaunchError::WorkingDirectoryChanged)?;
        let cwd_metadata = fs::symlink_metadata(&canonical_cwd)
            .map_err(|_| LaunchError::WorkingDirectoryChanged)?;
        if cwd_metadata.file_type().is_symlink() || !cwd_metadata.is_dir() {
            return Err(LaunchError::WorkingDirectoryChanged);
        }

        ensure_private_directory(aviary_dir)?;
        let launches = aviary_dir.join("launches");
        ensure_private_directory(&launches)?;
        prune_launches_at(aviary_dir, now)?;

        let launch_id = Uuid::new_v4().to_string();
        let launch_dir = launches.join(&launch_id);
        create_private_directory(&launch_dir)?;
        let prepared = (|| {
            let submitted_at = unix_seconds(now)?;
            let lifetime = request.lifetime.as_secs();
            let expires_at = submitted_at
                .checked_add(lifetime)
                .ok_or(LaunchError::InvalidDescriptor)?;

            let artifact_names = request
                .artifacts
                .iter()
                .map(|artifact| artifact.name.clone())
                .collect::<HashSet<_>>();
            let mut descriptor_artifacts = Vec::with_capacity(request.artifacts.len());
            for artifact in &request.artifacts {
                let path = launch_dir.join(&artifact.name);
                write_new_private_file(&path, &artifact.bytes, 0o600)?;
                descriptor_artifacts.push(DescriptorArtifact {
                    name: artifact.name.clone(),
                    bytes: u64::try_from(artifact.bytes.len())
                        .map_err(|_| LaunchError::InvalidDescriptor)?,
                    sha256: sha256(&artifact.bytes),
                });
            }

            let arguments = request
                .arguments
                .iter()
                .map(|value| resolve_value(value, &launch_dir, &artifact_names))
                .collect::<Result<Vec<_>, _>>()?;
            let environment = request
                .environment
                .iter()
                .map(|entry| {
                    Ok(DescriptorEnvironment {
                        key: EncodedOsString::encode(&entry.key)?,
                        value: resolve_value(&entry.value, &launch_dir, &artifact_names)?,
                    })
                })
                .collect::<Result<Vec<_>, LaunchError>>()?;
            let descriptor = LaunchDescriptor {
                schema_version: DESCRIPTOR_SCHEMA_VERSION,
                launch_id: launch_id.clone(),
                submitted_at,
                expires_at,
                cwd: EncodedOsString::encode(canonical_cwd.as_os_str())?,
                program: EncodedOsString::encode(&request.program)?,
                arguments,
                environment,
                artifacts: descriptor_artifacts,
            };
            validate_descriptor(&descriptor, now)?;

            let descriptor_file = launch_dir.join(DESCRIPTOR_FILE);
            let descriptor_bytes =
                serde_json::to_vec(&descriptor).map_err(|_| LaunchError::InvalidDescriptor)?;
            if descriptor_bytes.len() as u64 > MAX_DESCRIPTOR_BYTES {
                return Err(LaunchError::InvalidDescriptor);
            }
            write_new_private_file(&descriptor_file, &descriptor_bytes, 0o600)?;

            let status = LaunchStatus::Submitted {
                schema_version: STATUS_SCHEMA_VERSION,
                launch_id: launch_id.clone(),
                submitted_at,
            };
            let status_file = launch_dir.join(STATUS_FILE);
            write_new_private_file(
                &status_file,
                &serde_json::to_vec(&status).map_err(|_| LaunchError::InvalidDescriptor)?,
                0o600,
            )?;

            let command_file = launch_dir.join(COMMAND_FILE);
            let command_bytes =
                command_file_bytes(helper.as_os_str(), descriptor_file.as_os_str())?;
            write_new_private_file(&command_file, &command_bytes, 0o700)?;
            sync_directory(&launch_dir)?;
            sync_directory(&launches)?;

            Ok(PreparedLaunch {
                launch_id,
                command_file,
                descriptor_file,
                status_file,
                expires_at,
            })
        })();
        if prepared.is_err() {
            let _ = remove_validated_launch_dir(&launches, &launch_dir);
        }
        prepared
    }
}

pub fn prepare(helper: &Path, request: LaunchRequest) -> Result<PreparedLaunch, LaunchError> {
    let home = dirs::home_dir().ok_or(LaunchError::InvalidLayout)?;
    prepare_at(&home.join(".aviary"), helper, request, SystemTime::now())
}

/// Locates the private launch helper from the running app bundle. Deriving the
/// path here keeps installation-location guesses out of IPC and the frontend.
pub fn bundled_launch_helper() -> Result<PathBuf, LaunchError> {
    let executable = std::env::current_exe().map_err(|_| LaunchError::InvalidLayout)?;
    launch_helper_beside(&executable)
}

fn launch_helper_beside(executable: &Path) -> Result<PathBuf, LaunchError> {
    if !executable.is_absolute() {
        return Err(LaunchError::InvalidLayout);
    }
    let helper = executable
        .parent()
        .ok_or(LaunchError::InvalidLayout)?
        .join("aviary-launch");
    validate_helper(&helper)?;
    Ok(helper)
}

/// Resolves a bundle again immediately before preparing its terminal handoff.
/// Terminal starts promptless and interactive: a prompt member remains a UI
/// prefill. Components whose exact CLI semantics cannot be represented without
/// generated runner configuration fail closed instead of silently inheriting
/// a broader setup.
pub fn prepare_bundle_terminal(
    bundle_id: &str,
    expected_revision: i64,
    helper: &Path,
) -> Result<PreparedLaunch, BundleLaunchError> {
    validate_helper(helper).map_err(|error| BundleLaunchError::Handoff {
        message: error.to_string(),
    })?;
    let catalog = LiveTargetCatalog::scan();
    let prepared = bundles::resolve_for_attachment(bundle_id, expected_revision, &catalog)
        .map_err(|error| BundleLaunchError::Bundle {
            message: error.to_string(),
        })?;
    let request = terminal_request(&prepared)?;
    prepare(helper, request).map_err(|error| BundleLaunchError::Handoff {
        message: error.to_string(),
    })
}

fn terminal_request(
    prepared: &PreparedBundleAttachment,
) -> Result<LaunchRequest, BundleLaunchError> {
    for member in &prepared.snapshot.members {
        let reason = match (member.kind, member.role) {
            (MemberKind::Project, MemberRole::WorkingDirectory)
            | (MemberKind::Prompt, MemberRole::Prefill)
            | (MemberKind::Skill, MemberRole::Available)
            | (MemberKind::Agent, MemberRole::Available) => None,
            (MemberKind::Skill, MemberRole::InvokeFirstTurn) => {
                Some("interactive terminal launch has no first prompt in which to invoke a skill")
            }
            (MemberKind::Agent, MemberRole::Primary) => Some(
                "the installed runner's exact primary-agent CLI semantics have not been proven",
            ),
            (MemberKind::Memory, _) => Some(
                "supplemental memory cannot be appended without generating runner configuration",
            ),
            (MemberKind::Mcp, _) => {
                Some("an isolated MCP selection requires generated runner configuration")
            }
            (MemberKind::MediaCollection, _) => {
                Some("a scoped media collection requires generated runner configuration")
            }
            _ => Some("this member role cannot be represented by an interactive runner"),
        };
        if let Some(reason) = reason {
            return Err(BundleLaunchError::Unsupported {
                member: member.snapshot_label.clone(),
                reason: reason.into(),
            });
        }
    }

    let executable_name = match prepared.runner {
        Runner::ClaudeCode => "claude",
        Runner::Codex => "codex",
    };
    let program =
        resolve_executable(executable_name).ok_or(BundleLaunchError::RunnerUnavailable {
            runner: prepared.runner,
        })?;
    let mut arguments = Vec::new();
    if let Some(model) = prepared.model_id.as_deref() {
        if !advertises_flag(&program, "--model", prepared.runner)? {
            return Err(BundleLaunchError::CapabilityUnavailable {
                runner: prepared.runner,
                capability: "model selection".into(),
            });
        }
        arguments.push(LaunchValue::Literal(OsString::from(format!(
            "--model={model}"
        ))));
    }
    Ok(LaunchRequest {
        cwd: PathBuf::from(&prepared.cwd),
        program: program.into_os_string(),
        arguments,
        environment: Vec::new(),
        artifacts: Vec::new(),
        lifetime: Duration::from_secs(2 * 60),
    })
}

/// Removes a bounded number of expired submissions and old terminal statuses.
/// Only flat, owner-controlled UUID directories immediately below the exact
/// launch root are eligible; symlinks and unexpected directory shapes are
/// skipped and never traversed.
pub fn prune_launches_at(aviary_dir: &Path, now: SystemTime) -> Result<usize, LaunchError> {
    #[cfg(not(unix))]
    {
        let _ = (aviary_dir, now);
        return Err(LaunchError::UnsupportedPlatform);
    }

    #[cfg(unix)]
    {
        validate_private_directory(aviary_dir)?;
        let launches = aviary_dir.join("launches");
        validate_private_directory(&launches)?;
        let now = unix_seconds(now)?;
        let entries = fs::read_dir(&launches).map_err(|_| LaunchError::Io)?;
        let mut removed = 0usize;
        for entry in entries.take(MAX_PRUNE_SCAN) {
            if removed >= MAX_PRUNE_REMOVALS {
                break;
            }
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.parent() != Some(launches.as_path()) || launch_id_from_dir(&path).is_err() {
                continue;
            }
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || validate_owner(&metadata).is_err()
                || metadata.mode() & 0o7777 != 0o700
            {
                continue;
            }
            let status_path = path.join(STATUS_FILE);
            let Ok(bytes) = read_secure_file(&status_path, 0o600, MAX_STATUS_BYTES) else {
                continue;
            };
            let Ok(status) = serde_json::from_slice::<LaunchStatus>(&bytes) else {
                continue;
            };
            let should_remove = match status {
                LaunchStatus::Submitted {
                    schema_version,
                    submitted_at,
                    ..
                } => {
                    schema_version == STATUS_SCHEMA_VERSION
                        && now
                            > submitted_at
                                .saturating_add(MAX_LIFETIME.as_secs())
                                .saturating_add(MAX_CLOCK_SKEW_SECS)
                }
                LaunchStatus::Exited {
                    schema_version,
                    finished_at,
                    ..
                } => {
                    schema_version == STATUS_SCHEMA_VERSION
                        && now > finished_at.saturating_add(TERMINAL_STATUS_RETENTION.as_secs())
                }
                LaunchStatus::Failed {
                    schema_version,
                    failed_at,
                    ..
                } => {
                    schema_version == STATUS_SCHEMA_VERSION
                        && now > failed_at.saturating_add(TERMINAL_STATUS_RETENTION.as_secs())
                }
                LaunchStatus::Started {
                    schema_version,
                    started_at,
                    pid,
                    ..
                } => {
                    schema_version == STATUS_SCHEMA_VERSION
                        && now > started_at.saturating_add(TERMINAL_STATUS_RETENTION.as_secs())
                        && !process_exists(pid)
                }
            };
            if should_remove && remove_validated_launch_dir(&launches, &path).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: signal 0 performs an existence/permission check and does not
    // deliver a signal. A reused live pid makes cleanup conservatively skip.
    let result = unsafe { libc::kill(pid as i32, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub fn prune_launches() -> Result<usize, LaunchError> {
    let home = dirs::home_dir().ok_or(LaunchError::InvalidLayout)?;
    prune_launches_at(&home.join(".aviary"), SystemTime::now())
}

/// Opens the generated `.command` as a path argument. No shell command string
/// is constructed here; `open` receives one argument per semantic value.
#[cfg(target_os = "macos")]
pub fn open_terminal(prepared: &PreparedLaunch) -> Result<(), LaunchError> {
    let status =
        match terminal_open_command(Path::new("/usr/bin/open"), &prepared.command_file).status() {
            Ok(status) => status,
            Err(_) => {
                let _ = cancel_prepared(prepared);
                return Err(LaunchError::SpawnFailed);
            }
        };
    if status.success() {
        Ok(())
    } else {
        let _ = cancel_prepared(prepared);
        Err(LaunchError::SpawnFailed)
    }
}

#[cfg(not(target_os = "macos"))]
pub fn open_terminal(_prepared: &PreparedLaunch) -> Result<(), LaunchError> {
    Err(LaunchError::UnsupportedPlatform)
}

/// Cancels an unclaimed handoff and scrubs every payload byte immediately.
/// Once a helper has claimed the launch, only that helper may clean it up.
pub fn cancel_prepared(prepared: &PreparedLaunch) -> Result<(), LaunchError> {
    let home = dirs::home_dir().ok_or(LaunchError::InvalidLayout)?;
    cancel_prepared_at(&home.join(".aviary"), prepared)
}

pub fn cancel_prepared_at(aviary_dir: &Path, prepared: &PreparedLaunch) -> Result<(), LaunchError> {
    #[cfg(not(unix))]
    {
        let _ = (aviary_dir, prepared);
        return Err(LaunchError::UnsupportedPlatform);
    }
    #[cfg(unix)]
    {
        let layout = validate_layout(aviary_dir, &prepared.descriptor_file)?;
        if launch_id_from_dir(&layout.launch_dir)? != prepared.launch_id
            || layout.status != prepared.status_file
            || layout.launch_dir.join(COMMAND_FILE) != prepared.command_file
        {
            return Err(LaunchError::InvalidLayout);
        }
        if fs::symlink_metadata(&layout.claim).is_ok() {
            return Err(LaunchError::AlreadyClaimed);
        }
        let bytes = read_secure_file(&layout.status, 0o600, MAX_STATUS_BYTES)?;
        let status: LaunchStatus =
            serde_json::from_slice(&bytes).map_err(|_| LaunchError::InvalidDescriptor)?;
        if !matches!(
            status,
            LaunchStatus::Submitted {
                schema_version: STATUS_SCHEMA_VERSION,
                ref launch_id,
                ..
            } if launch_id == &prepared.launch_id
        ) {
            return Err(LaunchError::AlreadyClaimed);
        }
        let launches = aviary_dir.join("launches");
        remove_validated_launch_dir(&launches, &layout.launch_dir)
    }
}

fn terminal_open_command(open: &Path, command_file: &Path) -> Command {
    let mut command = Command::new(open);
    command
        .arg("-a")
        .arg("Terminal")
        .arg(command_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn resolve_executable(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(name))
        .find_map(|candidate| {
            let metadata = fs::symlink_metadata(&candidate).ok()?;
            if metadata.file_type().is_symlink() {
                let canonical = candidate.canonicalize().ok()?;
                let canonical_metadata = fs::metadata(&canonical).ok()?;
                executable_file(&canonical_metadata).then_some(canonical)
            } else {
                executable_file(&metadata).then_some(candidate)
            }
        })
}

fn executable_file(metadata: &fs::Metadata) -> bool {
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        metadata.mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn validate_helper(helper: &Path) -> Result<(), LaunchError> {
    if !helper.is_absolute() {
        return Err(LaunchError::InvalidLayout);
    }
    let metadata = fs::symlink_metadata(helper).map_err(|_| LaunchError::InvalidLayout)?;
    if metadata.file_type().is_symlink() || !executable_file(&metadata) {
        return Err(LaunchError::InvalidLayout);
    }
    #[cfg(unix)]
    validate_owner(&metadata)?;
    Ok(())
}

fn advertises_flag(program: &Path, flag: &str, runner: Runner) -> Result<bool, BundleLaunchError> {
    let output = bounded_capability_output(program, &["--help"]).ok_or_else(|| {
        BundleLaunchError::CapabilityUnavailable {
            runner,
            capability: flag.to_string(),
        }
    })?;
    let output = String::from_utf8_lossy(&output);
    Ok(output.split_whitespace().any(|token| {
        token == flag
            || token
                .strip_prefix(flag)
                .is_some_and(|suffix| suffix.starts_with('=') || suffix.starts_with('['))
    }))
}

/// Capability discovery is bounded even if a runner or one of its descendants
/// misbehaves. Both pipes are drained while the process runs; the isolated
/// process group is killed on timeout and again after the leader exits so a
/// descendant cannot keep a reader thread alive by inheriting stdout.
fn bounded_capability_output(program: &Path, arguments: &[&str]) -> Option<Vec<u8>> {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let mut child = command.spawn().ok()?;
    let pid = child.id();
    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let (sender, receiver) = mpsc::channel();
    for mut stream in [Box::new(stdout) as Box<dyn Read + Send>, Box::new(stderr)] {
        let sender = sender.clone();
        thread::spawn(move || {
            let mut kept = Vec::new();
            let mut buffer = [0u8; 8192];
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        let remaining = MAX_CAPABILITY_OUTPUT_BYTES.saturating_sub(kept.len());
                        kept.extend_from_slice(&buffer[..count.min(remaining)]);
                    }
                    Err(_) => break,
                }
            }
            let _ = sender.send(kept);
        });
    }
    drop(sender);

    let deadline = std::time::Instant::now() + CAPABILITY_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if std::time::Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            _ => break None,
        }
    };
    #[cfg(unix)]
    unsafe {
        let _ = libc::kill(-(pid as i32), libc::SIGKILL);
    }
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    let mut output = Vec::new();
    for _ in 0..2 {
        let bytes = receiver.recv_timeout(Duration::from_millis(250)).ok()?;
        let remaining = MAX_CAPABILITY_OUTPUT_BYTES.saturating_sub(output.len());
        output.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
    }
    status?.success().then_some(output)
}

/// Entry point used by the bundled helper binary.
pub fn execute_descriptor(path: &OsStr) -> Result<ExecutionOutcome, LaunchError> {
    let home = dirs::home_dir().ok_or(LaunchError::InvalidLayout)?;
    execute_descriptor_at(&home.join(".aviary"), Path::new(path), SystemTime::now())
}

pub fn execute_descriptor_at(
    aviary_dir: &Path,
    descriptor_path: &Path,
    now: SystemTime,
) -> Result<ExecutionOutcome, LaunchError> {
    #[cfg(not(unix))]
    {
        let _ = (aviary_dir, descriptor_path, now);
        return Err(LaunchError::UnsupportedPlatform);
    }

    #[cfg(unix)]
    {
        let layout = validate_layout(aviary_dir, descriptor_path)?;
        let directory_launch_id = launch_id_from_dir(&layout.launch_dir)?;
        let status_bytes = read_secure_file(&layout.status, 0o600, MAX_STATUS_BYTES)?;
        let status: LaunchStatus =
            serde_json::from_slice(&status_bytes).map_err(|_| LaunchError::InvalidDescriptor)?;
        let submitted_at = match status {
            LaunchStatus::Submitted {
                schema_version,
                launch_id,
                submitted_at,
            } if schema_version == STATUS_SCHEMA_VERSION && launch_id == directory_launch_id => {
                submitted_at
            }
            _ => return Err(LaunchError::AlreadyClaimed),
        };
        let now_seconds = unix_seconds(now)?;
        claim(&layout.claim)?;

        let payload = (|| {
            let descriptor_bytes =
                read_secure_file(&layout.descriptor, 0o600, MAX_DESCRIPTOR_BYTES)?;
            let descriptor: LaunchDescriptor = serde_json::from_slice(&descriptor_bytes)
                .map_err(|_| LaunchError::InvalidDescriptor)?;
            validate_descriptor(&descriptor, now)?;
            if descriptor.launch_id != directory_launch_id
                || descriptor.submitted_at != submitted_at
            {
                return Err(LaunchError::InvalidDescriptor);
            }
            let cwd = descriptor.cwd.decode_path()?;
            let program = descriptor.program.decode()?;
            let arguments = descriptor
                .arguments
                .iter()
                .map(EncodedOsString::decode)
                .collect::<Result<Vec<_>, _>>()?;
            let environment = descriptor
                .environment
                .iter()
                .map(|entry| Ok((entry.key.decode()?, entry.value.decode()?)))
                .collect::<Result<Vec<_>, LaunchError>>()?;
            Ok((descriptor, cwd, program, arguments, environment))
        })();
        let (descriptor, cwd, program, arguments, environment) = match payload {
            Ok(payload) => payload,
            Err(error) => {
                return Err(fail_claimed_launch_identity(
                    &layout,
                    &directory_launch_id,
                    submitted_at,
                    now_seconds,
                    FailureReason::InvalidPayload,
                    error,
                ))
            }
        };

        if let Err(error) = validate_artifacts(&layout.launch_dir, &descriptor.artifacts) {
            return Err(fail_claimed_launch(
                &layout,
                &descriptor,
                now_seconds,
                FailureReason::InvalidPayload,
                error,
            ));
        }
        if now_seconds > descriptor.expires_at {
            return Err(fail_claimed_launch(
                &layout,
                &descriptor,
                now_seconds,
                FailureReason::Expired,
                LaunchError::Expired,
            ));
        }

        let canonical = match cwd.canonicalize() {
            Ok(canonical) => canonical,
            Err(_) => {
                return Err(fail_claimed_launch(
                    &layout,
                    &descriptor,
                    now_seconds,
                    FailureReason::WorkingDirectoryChanged,
                    LaunchError::WorkingDirectoryChanged,
                ))
            }
        };
        let metadata = match fs::symlink_metadata(&cwd) {
            Ok(metadata) => metadata,
            Err(_) => {
                return Err(fail_claimed_launch(
                    &layout,
                    &descriptor,
                    now_seconds,
                    FailureReason::WorkingDirectoryChanged,
                    LaunchError::WorkingDirectoryChanged,
                ))
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() || canonical != cwd {
            return Err(fail_claimed_launch(
                &layout,
                &descriptor,
                now_seconds,
                FailureReason::WorkingDirectoryChanged,
                LaunchError::WorkingDirectoryChanged,
            ));
        }

        let mut command = Command::new(program);
        command.current_dir(&cwd);
        for argument in arguments {
            command.arg(argument);
        }
        for (key, value) in environment {
            command.env(key, value);
        }
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                return Err(fail_claimed_launch(
                    &layout,
                    &descriptor,
                    unix_seconds(SystemTime::now()).unwrap_or(now_seconds),
                    FailureReason::Spawn,
                    LaunchError::SpawnFailed,
                ));
            }
        };
        let started_at = unix_seconds(SystemTime::now()).unwrap_or(now_seconds);
        if write_status(
            &layout.status,
            &LaunchStatus::Started {
                schema_version: STATUS_SCHEMA_VERSION,
                launch_id: descriptor.launch_id.clone(),
                submitted_at: descriptor.submitted_at,
                started_at,
                pid: child.id(),
            },
        )
        .is_err()
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(fail_claimed_launch(
                &layout,
                &descriptor,
                unix_seconds(SystemTime::now()).unwrap_or(started_at),
                FailureReason::Status,
                LaunchError::Io,
            ));
        }

        let exit_status = match child.wait() {
            Ok(status) => status,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail_claimed_launch(
                    &layout,
                    &descriptor,
                    unix_seconds(SystemTime::now()).unwrap_or(started_at),
                    FailureReason::Wait,
                    LaunchError::WaitFailed,
                ));
            }
        };
        let finished_at = unix_seconds(SystemTime::now()).unwrap_or(started_at);
        let status_result = write_status(
            &layout.status,
            &LaunchStatus::Exited {
                schema_version: STATUS_SCHEMA_VERSION,
                launch_id: descriptor.launch_id.clone(),
                submitted_at: descriptor.submitted_at,
                started_at,
                finished_at,
                exit_code: exit_status.code(),
            },
        );
        let scrub_result = scrub_claimed_payload(&layout);
        status_result?;
        scrub_result?;
        Ok(ExecutionOutcome {
            exit_code: exit_status.code(),
        })
    }
}

#[cfg(unix)]
fn fail_claimed_launch(
    layout: &SecureLayout,
    descriptor: &LaunchDescriptor,
    failed_at: u64,
    reason: FailureReason,
    original: LaunchError,
) -> LaunchError {
    fail_claimed_launch_identity(
        layout,
        &descriptor.launch_id,
        descriptor.submitted_at,
        failed_at,
        reason,
        original,
    )
}

#[cfg(unix)]
fn fail_claimed_launch_identity(
    layout: &SecureLayout,
    launch_id: &str,
    submitted_at: u64,
    failed_at: u64,
    reason: FailureReason,
    original: LaunchError,
) -> LaunchError {
    let status_result = write_status(
        &layout.status,
        &LaunchStatus::Failed {
            schema_version: STATUS_SCHEMA_VERSION,
            launch_id: launch_id.to_string(),
            submitted_at,
            failed_at,
            reason,
        },
    );
    let scrub_result = scrub_claimed_payload(layout);
    if status_result.is_err() || scrub_result.is_err() {
        LaunchError::Io
    } else {
        original
    }
}

fn validate_request(request: &LaunchRequest) -> Result<(), LaunchError> {
    if request.arguments.len() > MAX_ARGUMENTS
        || request.environment.len() > MAX_ENVIRONMENT
        || request.artifacts.len() > MAX_ARTIFACTS
        || request.lifetime.is_zero()
        || request.lifetime > MAX_LIFETIME
    {
        return Err(LaunchError::InvalidDescriptor);
    }
    validate_os_value(&request.program, false)?;
    let mut names = HashSet::new();
    let mut total = 0usize;
    for artifact in &request.artifacts {
        validate_artifact_name(&artifact.name)?;
        if !names.insert(artifact.name.as_str()) || artifact.bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(LaunchError::InvalidDescriptor);
        }
        total = total
            .checked_add(artifact.bytes.len())
            .ok_or(LaunchError::InvalidDescriptor)?;
    }
    if total > MAX_ARTIFACT_TOTAL_BYTES {
        return Err(LaunchError::InvalidDescriptor);
    }
    for value in &request.arguments {
        validate_launch_value(value)?;
        validate_artifact_reference(value, &names)?;
    }
    let mut environment_keys = HashSet::new();
    for entry in &request.environment {
        validate_os_value(&entry.key, false)?;
        if os_bytes(&entry.key).contains(&b'=') || !environment_keys.insert(os_bytes(&entry.key)) {
            return Err(LaunchError::InvalidDescriptor);
        }
        validate_launch_value(&entry.value)?;
        validate_artifact_reference(&entry.value, &names)?;
    }
    Ok(())
}

fn validate_artifact_reference(
    value: &LaunchValue,
    names: &HashSet<&str>,
) -> Result<(), LaunchError> {
    match value {
        LaunchValue::Literal(_) => Ok(()),
        LaunchValue::ArtifactPath(name) | LaunchValue::PrefixedArtifactPath { name, .. }
            if names.contains(name.as_str()) =>
        {
            Ok(())
        }
        LaunchValue::ArtifactPath(_) | LaunchValue::PrefixedArtifactPath { .. } => {
            Err(LaunchError::InvalidDescriptor)
        }
    }
}

fn validate_launch_value(value: &LaunchValue) -> Result<(), LaunchError> {
    match value {
        LaunchValue::Literal(value) => validate_os_value(value, true),
        LaunchValue::ArtifactPath(name) => validate_artifact_name(name),
        LaunchValue::PrefixedArtifactPath { prefix, name } => {
            validate_os_value(prefix, true)?;
            validate_artifact_name(name)
        }
    }
}

fn validate_descriptor(descriptor: &LaunchDescriptor, now: SystemTime) -> Result<(), LaunchError> {
    if descriptor.schema_version != DESCRIPTOR_SCHEMA_VERSION
        || Uuid::parse_str(&descriptor.launch_id).is_err()
        || descriptor.expires_at <= descriptor.submitted_at
        || descriptor.expires_at - descriptor.submitted_at > MAX_LIFETIME.as_secs()
        || descriptor.arguments.len() > MAX_ARGUMENTS
        || descriptor.environment.len() > MAX_ENVIRONMENT
        || descriptor.artifacts.len() > MAX_ARTIFACTS
    {
        return Err(LaunchError::InvalidDescriptor);
    }
    let now = unix_seconds(now)?;
    if descriptor.submitted_at > now.saturating_add(MAX_CLOCK_SKEW_SECS) {
        return Err(LaunchError::InvalidDescriptor);
    }
    descriptor.cwd.validate(true)?;
    descriptor.program.validate(false)?;
    for argument in &descriptor.arguments {
        argument.validate(true)?;
    }
    let mut environment_keys = HashSet::new();
    for entry in &descriptor.environment {
        entry.key.validate(false)?;
        entry.value.validate(true)?;
        let key = entry.key.bytes()?;
        if key.contains(&b'=') || !environment_keys.insert(key) {
            return Err(LaunchError::InvalidDescriptor);
        }
    }
    let mut names = HashSet::new();
    let mut total = 0usize;
    for artifact in &descriptor.artifacts {
        validate_artifact_name(&artifact.name)?;
        if artifact.sha256.len() != 64
            || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || artifact.bytes > MAX_ARTIFACT_BYTES as u64
            || !names.insert(artifact.name.as_str())
        {
            return Err(LaunchError::InvalidDescriptor);
        }
        total = total
            .checked_add(artifact.bytes as usize)
            .ok_or(LaunchError::InvalidDescriptor)?;
    }
    if total > MAX_ARTIFACT_TOTAL_BYTES {
        return Err(LaunchError::InvalidDescriptor);
    }
    Ok(())
}

fn resolve_value(
    value: &LaunchValue,
    launch_dir: &Path,
    artifact_names: &HashSet<String>,
) -> Result<EncodedOsString, LaunchError> {
    match value {
        LaunchValue::Literal(value) => EncodedOsString::encode(value),
        LaunchValue::ArtifactPath(name) => {
            if !artifact_names.contains(name) {
                return Err(LaunchError::InvalidDescriptor);
            }
            EncodedOsString::encode(launch_dir.join(name).as_os_str())
        }
        LaunchValue::PrefixedArtifactPath { prefix, name } => {
            if !artifact_names.contains(name) {
                return Err(LaunchError::InvalidDescriptor);
            }
            let mut bytes = os_bytes(prefix);
            bytes.extend_from_slice(&os_bytes(launch_dir.join(name).as_os_str()));
            EncodedOsString::from_bytes(bytes)
        }
    }
}

fn validate_artifacts(
    launch_dir: &Path,
    artifacts: &[DescriptorArtifact],
) -> Result<(), LaunchError> {
    for artifact in artifacts {
        let bytes = read_secure_file(
            &launch_dir.join(&artifact.name),
            0o600,
            MAX_ARTIFACT_BYTES as u64,
        )?;
        if bytes.len() as u64 != artifact.bytes || sha256(&bytes) != artifact.sha256 {
            return Err(LaunchError::InvalidDescriptor);
        }
    }
    Ok(())
}

fn validate_layout(aviary_dir: &Path, descriptor_path: &Path) -> Result<SecureLayout, LaunchError> {
    if !aviary_dir.is_absolute() || !descriptor_path.is_absolute() {
        return Err(LaunchError::InvalidLayout);
    }
    let launches = aviary_dir.join("launches");
    let launch_dir = descriptor_path
        .parent()
        .ok_or(LaunchError::InvalidLayout)?
        .to_path_buf();
    if descriptor_path.file_name() != Some(OsStr::new(DESCRIPTOR_FILE))
        || launch_dir.parent() != Some(launches.as_path())
    {
        return Err(LaunchError::InvalidLayout);
    }
    launch_id_from_dir(&launch_dir)?;
    validate_private_directory(aviary_dir)?;
    validate_private_directory(&launches)?;
    validate_private_directory(&launch_dir)?;
    validate_regular_file(descriptor_path, 0o600, MAX_DESCRIPTOR_BYTES)?;
    let status = launch_dir.join(STATUS_FILE);
    validate_regular_file(&status, 0o600, MAX_STATUS_BYTES)?;
    Ok(SecureLayout {
        descriptor: descriptor_path.to_path_buf(),
        status,
        claim: launch_dir.join(CLAIM_FILE),
        launch_dir,
    })
}

fn launch_id_from_dir(launch_dir: &Path) -> Result<String, LaunchError> {
    let value = launch_dir
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or(LaunchError::InvalidLayout)?;
    Uuid::parse_str(value).map_err(|_| LaunchError::InvalidLayout)?;
    Ok(value.to_string())
}

#[cfg(unix)]
fn ensure_private_directory(path: &Path) -> Result<(), LaunchError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(LaunchError::SymlinkRejected);
            }
            if !metadata.is_dir() {
                return Err(LaunchError::InvalidLayout);
            }
            validate_owner(&metadata)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .map_err(|_| LaunchError::Io)?;
            validate_private_directory(path)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_private_directory(path)
        }
        Err(_) => Err(LaunchError::Io),
    }
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> Result<(), LaunchError> {
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(path).map_err(|_| LaunchError::Io)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| LaunchError::Io)?;
    validate_private_directory(path)
}

#[cfg(unix)]
fn validate_private_directory(path: &Path) -> Result<(), LaunchError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| LaunchError::InvalidLayout)?;
    if metadata.file_type().is_symlink() {
        return Err(LaunchError::SymlinkRejected);
    }
    if !metadata.is_dir() {
        return Err(LaunchError::InvalidLayout);
    }
    validate_owner(&metadata)?;
    if metadata.mode() & 0o7777 != 0o700 {
        return Err(LaunchError::InvalidPermissions);
    }
    Ok(())
}

#[cfg(unix)]
fn validate_regular_file(path: &Path, mode: u32, max: u64) -> Result<(), LaunchError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| LaunchError::InvalidLayout)?;
    if metadata.file_type().is_symlink() {
        return Err(LaunchError::SymlinkRejected);
    }
    if !metadata.is_file() || metadata.len() > max {
        return Err(LaunchError::InvalidDescriptor);
    }
    validate_owner(&metadata)?;
    if metadata.mode() & 0o7777 != mode {
        return Err(LaunchError::InvalidPermissions);
    }
    Ok(())
}

#[cfg(unix)]
fn validate_owner(metadata: &fs::Metadata) -> Result<(), LaunchError> {
    // SAFETY: `geteuid` has no preconditions and does not access memory.
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() == effective_uid {
        Ok(())
    } else {
        Err(LaunchError::InvalidOwner)
    }
}

#[cfg(unix)]
fn write_new_private_file(path: &Path, bytes: &[u8], mode: u32) -> Result<(), LaunchError> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(libc::O_NOFOLLOW);
    let mut file = options.open(path).map_err(|_| LaunchError::Io)?;
    file.write_all(bytes).map_err(|_| LaunchError::Io)?;
    file.sync_all().map_err(|_| LaunchError::Io)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|_| LaunchError::Io)?;
    validate_regular_file(path, mode, bytes.len() as u64)
}

#[cfg(unix)]
fn read_secure_file(path: &Path, mode: u32, max: u64) -> Result<Vec<u8>, LaunchError> {
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW);
    let file = options.open(path).map_err(|error| {
        if error.raw_os_error() == Some(libc::ELOOP) {
            LaunchError::SymlinkRejected
        } else {
            LaunchError::Io
        }
    })?;
    let metadata = file.metadata().map_err(|_| LaunchError::Io)?;
    if !metadata.is_file() || metadata.len() > max {
        return Err(LaunchError::InvalidDescriptor);
    }
    validate_owner(&metadata)?;
    if metadata.mode() & 0o7777 != mode {
        return Err(LaunchError::InvalidPermissions);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| LaunchError::Io)?;
    if bytes.len() as u64 > max {
        return Err(LaunchError::InvalidDescriptor);
    }
    Ok(bytes)
}

#[cfg(unix)]
fn claim(path: &Path) -> Result<(), LaunchError> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    match options.open(path) {
        Ok(file) => {
            file.sync_all().map_err(|_| LaunchError::Io)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .map_err(|_| LaunchError::Io)?;
            validate_regular_file(path, 0o600, 0)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(LaunchError::AlreadyClaimed)
        }
        Err(_) => Err(LaunchError::Io),
    }
}

#[cfg(unix)]
fn write_status(path: &Path, status: &LaunchStatus) -> Result<(), LaunchError> {
    let launch_dir = path.parent().ok_or(LaunchError::InvalidLayout)?;
    validate_private_directory(launch_dir)?;
    validate_regular_file(path, 0o600, MAX_STATUS_BYTES)?;
    let bytes = serde_json::to_vec(status).map_err(|_| LaunchError::InvalidDescriptor)?;
    if bytes.len() as u64 > MAX_STATUS_BYTES {
        return Err(LaunchError::InvalidDescriptor);
    }
    let temporary = launch_dir.join(format!(".status-{}", Uuid::new_v4()));
    write_new_private_file(&temporary, &bytes, 0o600)?;
    fs::rename(&temporary, path).map_err(|_| LaunchError::Io)?;
    sync_directory(launch_dir)
}

#[cfg(unix)]
fn scrub_claimed_payload(layout: &SecureLayout) -> Result<(), LaunchError> {
    validate_private_directory(&layout.launch_dir)?;
    validate_regular_file(&layout.status, 0o600, MAX_STATUS_BYTES)?;
    // The empty claim marker is not secret and must survive with the status:
    // a concurrent helper may already have read the submitted descriptor when
    // this helper exits. Removing the marker would let that stale reader
    // recreate it and run the command twice.
    remove_flat_launch_children(
        &layout.launch_dir,
        &[OsStr::new(STATUS_FILE), OsStr::new(CLAIM_FILE)],
    )?;
    sync_directory(&layout.launch_dir)
}

#[cfg(unix)]
fn remove_validated_launch_dir(launches: &Path, launch_dir: &Path) -> Result<(), LaunchError> {
    validate_private_directory(launches)?;
    if launch_dir.parent() != Some(launches) || launch_id_from_dir(launch_dir).is_err() {
        return Err(LaunchError::InvalidLayout);
    }
    validate_private_directory(launch_dir)?;
    remove_flat_launch_children(launch_dir, &[])?;
    fs::remove_dir(launch_dir).map_err(|_| LaunchError::Io)?;
    sync_directory(launches)
}

#[cfg(unix)]
fn remove_flat_launch_children(launch_dir: &Path, preserved: &[&OsStr]) -> Result<(), LaunchError> {
    for entry in fs::read_dir(launch_dir).map_err(|_| LaunchError::Io)? {
        let entry = entry.map_err(|_| LaunchError::Io)?;
        let path = entry.path();
        if path.parent() != Some(launch_dir) {
            return Err(LaunchError::InvalidLayout);
        }
        if preserved.iter().any(|name| path.file_name() == Some(*name)) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|_| LaunchError::Io)?;
        validate_owner(&metadata)?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            // Launch handoffs are deliberately flat. Refusing nested content
            // keeps cleanup from ever becoming a recursive broad delete.
            return Err(LaunchError::InvalidLayout);
        }
        fs::remove_file(&path).map_err(|_| LaunchError::Io)?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), LaunchError> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| LaunchError::Io)
}

fn command_file_bytes(helper: &OsStr, descriptor: &OsStr) -> Result<Vec<u8>, LaunchError> {
    let mut bytes = b"#!/bin/sh\nexec ".to_vec();
    bytes.extend_from_slice(&shell_quote(helper)?);
    bytes.push(b' ');
    bytes.extend_from_slice(&shell_quote(descriptor)?);
    bytes.push(b'\n');
    Ok(bytes)
}

fn shell_quote(value: &OsStr) -> Result<Vec<u8>, LaunchError> {
    let raw = os_bytes(value);
    if raw.contains(&0) {
        return Err(LaunchError::InvalidDescriptor);
    }
    let mut quoted = Vec::with_capacity(raw.len() + 2);
    quoted.push(b'\'');
    for byte in raw {
        if byte == b'\'' {
            quoted.extend_from_slice(b"'\"'\"'");
        } else {
            quoted.push(byte);
        }
    }
    quoted.push(b'\'');
    Ok(quoted)
}

impl EncodedOsString {
    fn encode(value: &OsStr) -> Result<Self, LaunchError> {
        validate_os_value(value, true)?;
        Self::from_bytes(os_bytes(value))
    }

    fn from_bytes(bytes: Vec<u8>) -> Result<Self, LaunchError> {
        if bytes.len() > MAX_OS_VALUE_BYTES || bytes.contains(&0) {
            return Err(LaunchError::InvalidDescriptor);
        }
        let mut hex = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write as _;
            write!(&mut hex, "{byte:02x}").map_err(|_| LaunchError::InvalidDescriptor)?;
        }
        Ok(Self { hex })
    }

    fn bytes(&self) -> Result<Vec<u8>, LaunchError> {
        if self.hex.len() % 2 != 0 || self.hex.len() / 2 > MAX_OS_VALUE_BYTES {
            return Err(LaunchError::InvalidDescriptor);
        }
        let mut bytes = Vec::with_capacity(self.hex.len() / 2);
        for pair in self.hex.as_bytes().chunks_exact(2) {
            let text = std::str::from_utf8(pair).map_err(|_| LaunchError::InvalidDescriptor)?;
            bytes.push(u8::from_str_radix(text, 16).map_err(|_| LaunchError::InvalidDescriptor)?);
        }
        if bytes.contains(&0) {
            return Err(LaunchError::InvalidDescriptor);
        }
        Ok(bytes)
    }

    fn validate(&self, allow_empty: bool) -> Result<(), LaunchError> {
        let bytes = self.bytes()?;
        if !allow_empty && bytes.is_empty() {
            Err(LaunchError::InvalidDescriptor)
        } else {
            Ok(())
        }
    }

    fn decode(&self) -> Result<OsString, LaunchError> {
        let bytes = self.bytes()?;
        #[cfg(unix)]
        {
            Ok(OsString::from_vec(bytes))
        }
        #[cfg(not(unix))]
        {
            String::from_utf8(bytes)
                .map(OsString::from)
                .map_err(|_| LaunchError::InvalidDescriptor)
        }
    }

    fn decode_path(&self) -> Result<PathBuf, LaunchError> {
        self.decode().map(PathBuf::from)
    }
}

fn validate_os_value(value: &OsStr, allow_empty: bool) -> Result<(), LaunchError> {
    let bytes = os_bytes(value);
    if bytes.len() > MAX_OS_VALUE_BYTES || bytes.contains(&0) || (!allow_empty && bytes.is_empty())
    {
        Err(LaunchError::InvalidDescriptor)
    } else {
        Ok(())
    }
}

fn validate_artifact_name(name: &str) -> Result<(), LaunchError> {
    if name.is_empty()
        || name.len() > MAX_ARTIFACT_NAME_BYTES
        || !name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        || matches!(
            name,
            "." | ".." | DESCRIPTOR_FILE | STATUS_FILE | COMMAND_FILE | CLAIM_FILE
        )
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Err(LaunchError::InvalidDescriptor)
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    value.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    value.to_string_lossy().as_bytes().to_vec()
}

fn unix_seconds(value: SystemTime) -> Result<u64, LaunchError> {
    value
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| LaunchError::InvalidDescriptor)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[cfg(unix)]
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, SystemTime) {
        let temporary = tempfile::tempdir().unwrap();
        let aviary = temporary.path().join(".aviary");
        let cwd = temporary.path().join("cwd");
        fs::create_dir(&cwd).unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        (temporary, aviary, cwd, now)
    }

    #[cfg(unix)]
    fn request(cwd: PathBuf, program: &str) -> LaunchRequest {
        LaunchRequest {
            cwd,
            program: OsString::from(program),
            arguments: Vec::new(),
            environment: Vec::new(),
            artifacts: Vec::new(),
            lifetime: Duration::from_secs(120),
        }
    }

    #[cfg(unix)]
    #[test]
    fn bundled_helper_is_an_owned_executable_sibling() {
        let temporary = tempfile::tempdir().unwrap();
        let executable = temporary.path().join("aviary");
        let helper = temporary.path().join("aviary-launch");
        fs::write(&executable, b"app").unwrap();
        fs::write(&helper, b"helper").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(launch_helper_beside(&executable).unwrap(), helper);

        fs::set_permissions(&helper, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            launch_helper_beside(&executable),
            Err(LaunchError::InvalidLayout)
        );
    }

    #[cfg(unix)]
    #[test]
    fn prepares_exact_private_layout_without_prompt_material_in_shell() {
        let (_temporary, aviary, cwd, now) = fixture();
        let secret = "prompt with 'quotes' and\nnewlines";
        let mut request = request(cwd, "/usr/bin/true");
        request.environment.push(LaunchEnvironment {
            key: OsString::from("PRIVATE_VALUE"),
            value: LaunchValue::Literal(OsString::from(secret)),
        });
        request.artifacts.push(LaunchArtifact {
            name: "mcp.json".to_string(),
            bytes: secret.as_bytes().to_vec(),
        });
        let helper = Path::new("/Applications/Aviary's\nPreview.app/Contents/MacOS/aviary-launch");
        let prepared = prepare_at(&aviary, helper, request, now).unwrap();
        let launch_dir = prepared.command_file.parent().unwrap();

        assert_eq!(mode(&aviary), 0o700);
        assert_eq!(mode(&aviary.join("launches")), 0o700);
        assert_eq!(mode(launch_dir), 0o700);
        assert_eq!(mode(&prepared.descriptor_file), 0o600);
        assert_eq!(mode(&launch_dir.join("status.json")), 0o600);
        assert_eq!(mode(&launch_dir.join("mcp.json")), 0o600);
        assert_eq!(mode(&prepared.command_file), 0o700);

        let shell = fs::read(&prepared.command_file).unwrap();
        assert!(!shell
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()));
        assert!(shell.starts_with(b"#!/bin/sh\nexec '"));
        assert!(shell.windows(5).any(|window| window == b"'\"'\"'"));
    }

    #[cfg(unix)]
    #[test]
    fn os_arguments_with_quotes_newlines_and_flag_names_round_trip_exactly() {
        let (_temporary, aviary, cwd, now) = fixture();
        let output = cwd.join("captured");
        let values = ["quote'value", "two\nlines", "--dangerously-not-a-flag"];
        let mut launch_request = request(cwd.clone(), "/bin/sh");
        launch_request.arguments = vec![
            LaunchValue::Literal(OsString::from("-c")),
            LaunchValue::Literal(OsString::from(
                "output=$1; shift; printf '%s\\0' \"$@\" > \"$output\"",
            )),
            LaunchValue::Literal(OsString::from("fixture")),
            LaunchValue::Literal(output.as_os_str().to_os_string()),
        ];
        launch_request.arguments.extend(
            values
                .iter()
                .map(|value| LaunchValue::Literal(OsString::from(value))),
        );
        let prepared = prepare_at(&aviary, Path::new("/helper"), launch_request, now).unwrap();
        let outcome = execute_descriptor_at(&aviary, &prepared.descriptor_file, now).unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        let captured = fs::read(output).unwrap();
        let mut expected = values.join("\0").into_bytes();
        expected.push(0);
        assert_eq!(captured, expected);
        let status: serde_json::Value =
            serde_json::from_slice(&fs::read(&prepared.status_file).unwrap()).unwrap();
        assert_eq!(status["state"], "exited");
        let remaining = fs::read_dir(prepared.status_file.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(
            remaining.into_iter().collect::<HashSet<_>>(),
            HashSet::from([OsString::from(STATUS_FILE), OsString::from(CLAIM_FILE)])
        );

        let (_temporary, aviary, cwd, now) = fixture();
        let prepared = prepare_at(
            &aviary,
            Path::new("/helper"),
            request(cwd, "/usr/bin/true"),
            now,
        )
        .unwrap();
        fs::write(&prepared.descriptor_file, b"not-json").unwrap();
        assert_eq!(
            execute_descriptor_at(&aviary, &prepared.descriptor_file, now),
            Err(LaunchError::InvalidDescriptor)
        );
        let status: serde_json::Value =
            serde_json::from_slice(&fs::read(&prepared.status_file).unwrap()).unwrap();
        assert_eq!(status["state"], "failed");
        assert_eq!(
            fs::read_dir(prepared.status_file.parent().unwrap())
                .unwrap()
                .count(),
            2
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_expiry_symlinks_weak_modes_and_changed_cwd() {
        let (_temporary, aviary, cwd, now) = fixture();
        let prepared = prepare_at(
            &aviary,
            Path::new("/helper"),
            request(cwd.clone(), "/usr/bin/true"),
            now,
        )
        .unwrap();
        assert_eq!(
            execute_descriptor_at(
                &aviary,
                &prepared.descriptor_file,
                now + Duration::from_secs(121)
            ),
            Err(LaunchError::Expired)
        );

        let (_temporary, aviary, cwd, now) = fixture();
        let prepared = prepare_at(
            &aviary,
            Path::new("/helper"),
            request(cwd, "/usr/bin/true"),
            now,
        )
        .unwrap();
        fs::set_permissions(&prepared.descriptor_file, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            execute_descriptor_at(&aviary, &prepared.descriptor_file, now),
            Err(LaunchError::InvalidPermissions)
        );

        let (_temporary, aviary, cwd, now) = fixture();
        let mut with_artifact = request(cwd, "/usr/bin/true");
        with_artifact.artifacts.push(LaunchArtifact {
            name: "config.json".to_string(),
            bytes: b"{}".to_vec(),
        });
        let prepared = prepare_at(&aviary, Path::new("/helper"), with_artifact, now).unwrap();
        let artifact = prepared
            .descriptor_file
            .parent()
            .unwrap()
            .join("config.json");
        fs::remove_file(&artifact).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", &artifact).unwrap();
        assert_eq!(
            execute_descriptor_at(&aviary, &prepared.descriptor_file, now),
            Err(LaunchError::SymlinkRejected)
        );

        let (temporary, aviary, cwd, now) = fixture();
        let prepared = prepare_at(
            &aviary,
            Path::new("/helper"),
            request(cwd.clone(), "/usr/bin/true"),
            now,
        )
        .unwrap();
        fs::remove_dir(&cwd).unwrap();
        let replacement = temporary.path().join("replacement");
        fs::create_dir(&replacement).unwrap();
        std::os::unix::fs::symlink(&replacement, &cwd).unwrap();
        assert_eq!(
            execute_descriptor_at(&aviary, &prepared.descriptor_file, now),
            Err(LaunchError::WorkingDirectoryChanged)
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_claim_allows_exactly_one_execution() {
        let (_temporary, aviary, cwd, now) = fixture();
        let prepared = prepare_at(
            &aviary,
            Path::new("/helper"),
            request(cwd, "/usr/bin/true"),
            now,
        )
        .unwrap();
        let aviary = Arc::new(aviary);
        let descriptor = Arc::new(prepared.descriptor_file);
        let barrier = Arc::new(Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let aviary = Arc::clone(&aviary);
                let descriptor = Arc::clone(&descriptor);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    execute_descriptor_at(&aviary, &descriptor, now)
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(LaunchError::AlreadyClaimed)))
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_traversal_missing_artifacts_and_oversized_inputs() {
        let (_temporary, aviary, cwd, now) = fixture();
        let mut traversal = request(cwd.clone(), "/usr/bin/true");
        traversal.artifacts.push(LaunchArtifact {
            name: "../secret".to_string(),
            bytes: Vec::new(),
        });
        assert!(matches!(
            prepare_at(&aviary, Path::new("/helper"), traversal, now),
            Err(LaunchError::InvalidDescriptor)
        ));

        let mut missing = request(cwd.clone(), "/usr/bin/true");
        missing
            .arguments
            .push(LaunchValue::ArtifactPath("missing.json".to_string()));
        assert!(matches!(
            prepare_at(&aviary, Path::new("/helper"), missing, now),
            Err(LaunchError::InvalidDescriptor)
        ));

        let mut oversized = request(cwd, "/usr/bin/true");
        oversized.artifacts.push(LaunchArtifact {
            name: "huge.bin".to_string(),
            bytes: vec![0; MAX_ARTIFACT_BYTES + 1],
        });
        assert!(matches!(
            prepare_at(&aviary, Path::new("/helper"), oversized, now),
            Err(LaunchError::InvalidDescriptor)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn partial_prepare_and_claimed_failures_scrub_secret_payloads() {
        let (_temporary, aviary, cwd, now) = fixture();
        let secret = b"PARTIAL_SECRET_CANARY".to_vec();
        let mut too_large = request(cwd.clone(), "/usr/bin/true");
        too_large.artifacts.push(LaunchArtifact {
            name: "secret.txt".into(),
            bytes: secret.clone(),
        });
        too_large.arguments = (0..MAX_ARGUMENTS)
            .map(|_| LaunchValue::Literal(OsString::from("x".repeat(MAX_OS_VALUE_BYTES))))
            .collect();
        assert_eq!(
            prepare_at(&aviary, Path::new("/helper"), too_large, now),
            Err(LaunchError::InvalidDescriptor)
        );
        assert_eq!(fs::read_dir(aviary.join("launches")).unwrap().count(), 0);

        let mut tampered = request(cwd, "/usr/bin/true");
        tampered.artifacts.push(LaunchArtifact {
            name: "secret.txt".into(),
            bytes: secret,
        });
        let prepared = prepare_at(&aviary, Path::new("/helper"), tampered, now).unwrap();
        fs::write(
            prepared
                .descriptor_file
                .parent()
                .unwrap()
                .join("secret.txt"),
            b"tampered",
        )
        .unwrap();
        assert_eq!(
            execute_descriptor_at(&aviary, &prepared.descriptor_file, now),
            Err(LaunchError::InvalidDescriptor)
        );
        let remaining = fs::read_dir(prepared.status_file.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(
            remaining.into_iter().collect::<HashSet<_>>(),
            HashSet::from([OsString::from(STATUS_FILE), OsString::from(CLAIM_FILE)])
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_and_pruning_remove_only_exact_unclaimed_uuid_layouts() {
        let (temporary, aviary, cwd, now) = fixture();
        let prepared = prepare_at(
            &aviary,
            Path::new("/helper"),
            request(cwd.clone(), "/usr/bin/true"),
            now,
        )
        .unwrap();
        let launch_dir = prepared.descriptor_file.parent().unwrap().to_path_buf();
        cancel_prepared_at(&aviary, &prepared).unwrap();
        assert!(!launch_dir.exists());

        let prepared = prepare_at(
            &aviary,
            Path::new("/helper"),
            request(cwd, "/usr/bin/true"),
            now,
        )
        .unwrap();
        write_status(
            &prepared.status_file,
            &LaunchStatus::Started {
                schema_version: STATUS_SCHEMA_VERSION,
                launch_id: prepared.launch_id.clone(),
                submitted_at: unix_seconds(now).unwrap(),
                started_at: unix_seconds(now).unwrap(),
                pid: i32::MAX as u32,
            },
        )
        .unwrap();

        let outside = temporary.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), b"keep").unwrap();
        let symlink_name = Uuid::new_v4().to_string();
        std::os::unix::fs::symlink(&outside, aviary.join("launches").join(symlink_name)).unwrap();

        let removed = prune_launches_at(
            &aviary,
            now + TERMINAL_STATUS_RETENTION + Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(removed, 1);
        assert!(!prepared.status_file.parent().unwrap().exists());
        assert_eq!(fs::read(outside.join("keep")).unwrap(), b"keep");
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0o7777
    }
}
