//! Durable chat sessions.
//!
//! The runner's transcript remains its own source of context, but Aviary needs
//! a durable, normalised copy of the conversation it displayed. This module
//! stores only typed events; raw runner JSON is deliberately not representable
//! because unknown payloads can contain prompts, tool arguments or file data.

use crate::providers::Runner;
use rusqlite::{params, types::Type, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const EVENT_SCHEMA_VERSION: i64 = 1;
pub const MAX_EVENT_PAYLOAD_BYTES: usize = 1024 * 1024;
const MAX_RUNNER_SESSION_ID_BYTES: usize = 512;
const MAX_CWD_BYTES: usize = 16 * 1024;
const MAX_TITLE_BYTES: usize = 1024;
const MAX_PROMPT_BYTES: usize = 256 * 1024;
const MAX_MODEL_BYTES: usize = 1024;
const MAX_EFFORT_BYTES: usize = 256;
const MAX_PERMISSION_MODE_BYTES: usize = 256;
const MAX_PERMISSION_ACTIONS: usize = 8;
const MAX_PERMISSION_QUESTIONS: usize = 8;
const MAX_QUESTION_OPTIONS: usize = 20;
const MAX_QUESTION_ID_BYTES: usize = 256;
const MAX_QUESTION_HEADER_BYTES: usize = 512;
const MAX_QUESTION_TEXT_BYTES: usize = 8 * 1024;
const MAX_QUESTION_OPTION_LABEL_BYTES: usize = 512;
const MAX_QUESTION_OPTION_DESCRIPTION_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TurnStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Interrupted,
}

impl TurnStatus {
    fn as_db(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    fn from_db(value: &str, column: usize) -> rusqlite::Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(invalid_value(column, "unknown chat turn status")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureKind {
    Spawn,
    Protocol,
    RunnerExit,
    Input,
    Internal,
}

impl FailureKind {
    fn as_db(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Protocol => "protocol",
            Self::RunnerExit => "runner-exit",
            Self::Input => "input",
            Self::Internal => "internal",
        }
    }

    fn from_db(value: &str, column: usize) -> rusqlite::Result<Self> {
        match value {
            "spawn" => Ok(Self::Spawn),
            "protocol" => Ok(Self::Protocol),
            "runner-exit" => Ok(Self::RunnerExit),
            "input" => Ok(Self::Input),
            "internal" => Ok(Self::Internal),
            _ => Err(invalid_value(column, "unknown chat failure kind")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolResultStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PermissionPromptData {
    Questions { questions: Vec<PermissionQuestion> },
    Unsupported { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    pub is_other: bool,
    pub is_secret: bool,
    pub options: Vec<PermissionQuestionOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionQuestionOption {
    pub label: String,
    pub description: String,
}

/// Version-one persisted event protocol.
///
/// Strings here are presentation data already selected by a runner adapter.
/// There is intentionally no `Raw` or arbitrary-JSON variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SessionEvent {
    Started {
        model: Option<String>,
        cwd: Option<String>,
        tools: Option<u64>,
        mcp_servers: Option<u64>,
        permission_mode: Option<String>,
    },
    Thinking {
        text: String,
    },
    Text {
        text: String,
    },
    ToolCall {
        call_id: Option<String>,
        name: String,
        summary: String,
        detail: Option<String>,
    },
    ToolResult {
        call_id: Option<String>,
        status: ToolResultStatus,
        summary: String,
        detail: Option<String>,
    },
    ToolStarted {
        call_id: String,
        name: String,
        summary: String,
        detail: Option<String>,
    },
    ToolUpdated {
        call_id: String,
        name: String,
        summary: String,
        detail: Option<String>,
    },
    ToolFinished {
        call_id: String,
        name: String,
        status: ToolResultStatus,
        summary: String,
        detail: Option<String>,
    },
    PermissionRequest {
        request_id: String,
        tool_name: String,
        summary: String,
        options: Vec<String>,
        #[serde(default)]
        prompt: Option<PermissionPromptData>,
        /// Rehydration must combine this with the owning turn status. A request
        /// in any non-running turn is display-only and cannot be answered.
        #[serde(default = "default_true")]
        expires_with_turn: bool,
    },
    PermissionResolved {
        request_id: String,
        decision: String,
    },
    TokenUsage {
        input_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        reasoning_output_tokens: Option<u64>,
        total_tokens: Option<u64>,
    },
    Finished {
        duration_ms: Option<u64>,
    },
    Interrupted {
        duration_ms: Option<u64>,
    },
    Failed {
        failure: FailureKind,
        display_message: String,
        duration_ms: Option<u64>,
    },
}

fn default_true() -> bool {
    true
}

impl SessionEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Started { .. } => "started",
            Self::Thinking { .. } => "thinking",
            Self::Text { .. } => "text",
            Self::ToolCall { .. } => "tool-call",
            Self::ToolResult { .. } => "tool-result",
            Self::ToolStarted { .. } => "tool-started",
            Self::ToolUpdated { .. } => "tool-updated",
            Self::ToolFinished { .. } => "tool-finished",
            Self::PermissionRequest { .. } => "permission-request",
            Self::PermissionResolved { .. } => "permission-resolved",
            Self::TokenUsage { .. } => "token-usage",
            Self::Finished { .. } => "finished",
            Self::Interrupted { .. } => "interrupted",
            Self::Failed { .. } => "failed",
        }
    }

    fn encode(&self) -> Result<String, String> {
        validate_event(self)?;
        let payload = serde_json::to_string(self).map_err(|e| e.to_string())?;
        if payload.len() > MAX_EVENT_PAYLOAD_BYTES {
            return Err(format!(
                "normalised chat event exceeds the {MAX_EVENT_PAYLOAD_BYTES}-byte persistence limit"
            ));
        }
        Ok(payload)
    }

    fn decode(schema_version: i64, kind: &str, payload: &str) -> Result<Self, String> {
        if schema_version != EVENT_SCHEMA_VERSION {
            return Err(format!(
                "unsupported chat event schema version {schema_version}"
            ));
        }
        if payload.len() > MAX_EVENT_PAYLOAD_BYTES {
            return Err("stored chat event exceeds the persistence limit".to_string());
        }
        let event: Self = serde_json::from_str(payload).map_err(|e| e.to_string())?;
        if event.kind() != kind {
            return Err("stored chat event kind does not match its payload".to_string());
        }
        Ok(event)
    }

    fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Finished { .. } | Self::Interrupted { .. } | Self::Failed { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSession {
    pub id: String,
    pub runner: Runner,
    pub runner_session_id: Option<String>,
    pub cwd: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTurn {
    pub id: String,
    pub session_id: String,
    pub ordinal: i64,
    pub prompt: String,
    pub requested_model: Option<String>,
    pub requested_effort: Option<String>,
    pub permission_mode: String,
    pub status: TurnStatus,
    pub failure_kind: Option<FailureKind>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredEvent {
    pub id: i64,
    pub turn_id: String,
    pub sequence: i64,
    pub schema_version: i64,
    pub created_at: i64,
    pub event: SessionEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnDetail {
    pub turn: ChatTurn,
    pub events: Vec<StoredEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDetail {
    pub session: ChatSession,
    pub turns: Vec<TurnDetail>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session: ChatSession,
    pub turn_count: i64,
    pub last_turn_status: Option<TurnStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedTurn {
    pub session: ChatSession,
    pub turn: ChatTurn,
}

#[derive(Debug, Clone)]
pub struct NewSession {
    pub runner: Runner,
    pub runner_session_id: Option<String>,
    pub cwd: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct NewTurn {
    pub prompt: String,
    pub requested_model: Option<String>,
    pub requested_effort: Option<String>,
    pub permission_mode: String,
}

pub fn create_session_with_turn(session: NewSession, turn: NewTurn) -> Result<QueuedTurn, String> {
    let mut conn = super::data();
    create_session_with_turn_on(&mut conn, session, turn, super::now())
}

pub fn queue_turn(session_id: &str, turn: NewTurn) -> Result<ChatTurn, String> {
    let mut conn = super::data();
    queue_turn_on(&mut conn, session_id, turn, super::now())
}

pub fn bind_runner_session_id(
    session_id: &str,
    runner_session_id: &str,
) -> Result<ChatSession, String> {
    let mut conn = super::data();
    bind_runner_session_id_on(&mut conn, session_id, runner_session_id, super::now())
}

pub fn mark_turn_running(turn_id: &str) -> Result<ChatTurn, String> {
    let mut conn = super::data();
    mark_turn_running_on(&mut conn, turn_id, super::now())
}

pub fn append_event(turn_id: &str, event: SessionEvent) -> Result<StoredEvent, String> {
    let mut conn = super::data();
    append_event_on(&mut conn, turn_id, event, super::now())
}

pub fn complete_turn(turn_id: &str, duration_ms: Option<u64>) -> Result<StoredEvent, String> {
    let mut conn = super::data();
    complete_turn_on(&mut conn, turn_id, duration_ms, super::now())
}

pub fn fail_turn(
    turn_id: &str,
    failure: FailureKind,
    display_message: String,
    duration_ms: Option<u64>,
) -> Result<StoredEvent, String> {
    let mut conn = super::data();
    fail_turn_on(
        &mut conn,
        turn_id,
        failure,
        display_message,
        duration_ms,
        super::now(),
    )
}

pub fn interrupt_turn(turn_id: &str) -> Result<ChatTurn, String> {
    let mut conn = super::data();
    interrupt_turn_on(&mut conn, turn_id, super::now())
}

pub fn interrupt_turn_with_event(
    turn_id: &str,
    duration_ms: Option<u64>,
) -> Result<StoredEvent, String> {
    let mut conn = super::data();
    interrupt_turn_with_event_on(&mut conn, turn_id, duration_ms, super::now())
}

/// Marks work abandoned by a previous Aviary process without replaying it.
pub fn reconcile_interrupted_turns() -> Result<usize, String> {
    let mut conn = super::data();
    reconcile_interrupted_turns_on(&mut conn, super::now())
}

pub fn load_session(session_id: &str) -> Result<Option<SessionDetail>, String> {
    load_session_on(&super::data(), session_id)
}

pub fn list_sessions(limit: usize) -> Result<Vec<SessionSummary>, String> {
    list_sessions_on(&super::data(), limit)
}

pub fn runner_session_started(session_id: &str) -> Result<bool, String> {
    runner_session_started_on(&super::data(), session_id)
}

fn create_session_with_turn_on(
    conn: &mut Connection,
    session: NewSession,
    turn: NewTurn,
    at: i64,
) -> Result<QueuedTurn, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let (session_id, turn_id) = insert_session_with_turn(&tx, session, turn, at)?;
    tx.commit().map_err(|e| e.to_string())?;

    queued_turn_by_ids(conn, &session_id, &turn_id)
}

/// Inserts a new session and its first queued turn into a transaction owned by
/// the caller. Bundle attachment uses this hook so the immutable attachment
/// snapshot and the work it configures become visible in the same commit.
pub(super) fn insert_session_with_turn(
    conn: &Connection,
    session: NewSession,
    turn: NewTurn,
    at: i64,
) -> Result<(String, String), String> {
    validate_session(&session)?;
    validate_turn(&turn)?;

    let session_id = Uuid::new_v4().to_string();
    let turn_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO chat_session(
             id, runner, runner_session_id, cwd, title, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![
            session_id,
            runner_to_db(session.runner),
            session.runner_session_id,
            session.cwd,
            session.title,
            at
        ],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO chat_turn(
             id, session_id, ordinal, prompt, requested_model,
             requested_effort, permission_mode, status, created_at
         ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, 'queued', ?7)",
        params![
            turn_id,
            session_id,
            turn.prompt,
            turn.requested_model,
            turn.requested_effort,
            turn.permission_mode,
            at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok((session_id, turn_id))
}

pub(super) fn queued_turn_by_ids(
    conn: &Connection,
    session_id: &str,
    turn_id: &str,
) -> Result<QueuedTurn, String> {
    let session = session_by_id(conn, session_id)?.ok_or("created chat session disappeared")?;
    let turn = turn_by_id(conn, turn_id)?.ok_or("created chat turn disappeared")?;
    Ok(QueuedTurn { session, turn })
}

pub(super) fn queue_turn_on(
    conn: &mut Connection,
    session_id: &str,
    turn: NewTurn,
    at: i64,
) -> Result<ChatTurn, String> {
    validate_turn(&turn)?;
    let turn_id = Uuid::new_v4().to_string();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    if session_by_id(&tx, session_id)?.is_none() {
        return Err("chat session not found".to_string());
    }
    enforce_attached_model(&tx, session_id, turn.requested_model.as_deref())?;
    let active: i64 = tx
        .query_row(
            "SELECT count(*) FROM chat_turn
              WHERE session_id = ?1 AND status IN ('queued', 'running')",
            [session_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if active != 0 {
        return Err("chat session already has an active turn".to_string());
    }
    let ordinal: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(ordinal), 0) + 1 FROM chat_turn WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO chat_turn(
             id, session_id, ordinal, prompt, requested_model,
             requested_effort, permission_mode, status, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8)",
        params![
            turn_id,
            session_id,
            ordinal,
            turn.prompt,
            turn.requested_model,
            turn.requested_effort,
            turn.permission_mode,
            at
        ],
    )
    .map_err(|e| e.to_string())?;
    touch_session(&tx, session_id, at)?;
    tx.commit().map_err(|e| e.to_string())?;
    turn_by_id(conn, &turn_id)?.ok_or_else(|| "created chat turn disappeared".to_string())
}

fn bind_runner_session_id_on(
    conn: &mut Connection,
    session_id: &str,
    runner_session_id: &str,
    at: i64,
) -> Result<ChatSession, String> {
    validate_runner_session_id(runner_session_id)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let session = session_by_id(&tx, session_id)?.ok_or("chat session not found")?;
    match session.runner_session_id.as_deref() {
        Some(existing) if existing != runner_session_id => {
            return Err("runner session id cannot change after it is bound".to_string());
        }
        Some(_) => return Ok(session),
        None => {}
    }
    tx.execute(
        "UPDATE chat_session
            SET runner_session_id = ?1, updated_at = ?2
          WHERE id = ?3 AND runner_session_id IS NULL",
        params![runner_session_id, at, session_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    session_by_id(conn, session_id)?.ok_or_else(|| "chat session not found".to_string())
}

fn mark_turn_running_on(conn: &mut Connection, turn_id: &str, at: i64) -> Result<ChatTurn, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let changed = tx
        .execute(
            "UPDATE chat_turn
                SET status = 'running', started_at = ?1
              WHERE id = ?2 AND status = 'queued'",
            params![at, turn_id],
        )
        .map_err(|e| e.to_string())?;
    if changed != 1 {
        return Err(transition_error(&tx, turn_id, "queued", "running"));
    }
    touch_session_for_turn(&tx, turn_id, at)?;
    tx.commit().map_err(|e| e.to_string())?;
    turn_by_id(conn, turn_id)?.ok_or_else(|| "chat turn not found".to_string())
}

fn append_event_on(
    conn: &mut Connection,
    turn_id: &str,
    event: SessionEvent,
    at: i64,
) -> Result<StoredEvent, String> {
    if event.is_terminal() {
        return Err("terminal chat events must use complete_turn or fail_turn".to_string());
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    require_turn_status(&tx, turn_id, &[TurnStatus::Running])?;
    let stored = insert_event(&tx, turn_id, event, at)?;
    touch_session_for_turn(&tx, turn_id, at)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(stored)
}

fn complete_turn_on(
    conn: &mut Connection,
    turn_id: &str,
    duration_ms: Option<u64>,
    at: i64,
) -> Result<StoredEvent, String> {
    let duration = duration_to_db(duration_ms)?;
    let event = SessionEvent::Finished { duration_ms };
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    require_turn_status(&tx, turn_id, &[TurnStatus::Running])?;
    let stored = insert_event(&tx, turn_id, event, at)?;
    tx.execute(
        "UPDATE chat_turn
            SET status = 'completed', finished_at = ?1, duration_ms = ?2
          WHERE id = ?3 AND status = 'running'",
        params![at, duration, turn_id],
    )
    .map_err(|e| e.to_string())?;
    touch_session_for_turn(&tx, turn_id, at)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(stored)
}

fn fail_turn_on(
    conn: &mut Connection,
    turn_id: &str,
    failure: FailureKind,
    display_message: String,
    duration_ms: Option<u64>,
    at: i64,
) -> Result<StoredEvent, String> {
    let duration = duration_to_db(duration_ms)?;
    let event = SessionEvent::Failed {
        failure,
        display_message,
        duration_ms,
    };
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    require_turn_status(&tx, turn_id, &[TurnStatus::Queued, TurnStatus::Running])?;
    let stored = insert_event(&tx, turn_id, event, at)?;
    tx.execute(
        "UPDATE chat_turn
            SET status = 'failed', failure_kind = ?1,
                finished_at = ?2, duration_ms = ?3
          WHERE id = ?4 AND status IN ('queued', 'running')",
        params![failure.as_db(), at, duration, turn_id],
    )
    .map_err(|e| e.to_string())?;
    touch_session_for_turn(&tx, turn_id, at)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(stored)
}

fn interrupt_turn_on(conn: &mut Connection, turn_id: &str, at: i64) -> Result<ChatTurn, String> {
    interrupt_turn_with_event_on(conn, turn_id, None, at)?;
    turn_by_id(conn, turn_id)?.ok_or_else(|| "chat turn not found".to_string())
}

fn interrupt_turn_with_event_on(
    conn: &mut Connection,
    turn_id: &str,
    duration_ms: Option<u64>,
    at: i64,
) -> Result<StoredEvent, String> {
    let duration = duration_to_db(duration_ms)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    require_turn_status(&tx, turn_id, &[TurnStatus::Queued, TurnStatus::Running])?;
    let stored = insert_event(&tx, turn_id, SessionEvent::Interrupted { duration_ms }, at)?;
    tx.execute(
        "UPDATE chat_turn
            SET status = 'interrupted', finished_at = ?1, duration_ms = ?2
          WHERE id = ?3 AND status IN ('queued', 'running')",
        params![at, duration, turn_id],
    )
    .map_err(|e| e.to_string())?;
    touch_session_for_turn(&tx, turn_id, at)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(stored)
}

fn reconcile_interrupted_turns_on(conn: &mut Connection, at: i64) -> Result<usize, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let active_turns = {
        let mut stmt = tx
            .prepare(
                "SELECT id
                   FROM chat_turn
                  WHERE status IN ('queued', 'running')
                  ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?
    };
    let unresolved = {
        let mut stmt = tx
            .prepare(
                "SELECT e.turn_id, json_extract(e.payload, '$.request_id')
                   FROM chat_event e
                   JOIN chat_turn t ON t.id = e.turn_id
                  WHERE t.status IN ('queued', 'running')
                    AND e.kind = 'permission-request'
                    AND NOT EXISTS (
                        SELECT 1
                          FROM chat_event r
                         WHERE r.turn_id = e.turn_id
                           AND r.kind = 'permission-resolved'
                           AND json_extract(r.payload, '$.request_id')
                               = json_extract(e.payload, '$.request_id')
                    )
                  ORDER BY e.turn_id, e.sequence",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?
    };
    for (turn_id, request_id) in unresolved {
        insert_event(
            &tx,
            &turn_id,
            SessionEvent::PermissionResolved {
                request_id,
                decision: "app-restarted".to_string(),
            },
            at,
        )?;
    }
    for turn_id in &active_turns {
        let already_terminal: bool = tx
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM chat_event
                      WHERE turn_id = ?1 AND kind = 'interrupted'
                 )",
                [turn_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !already_terminal {
            insert_event(
                &tx,
                turn_id,
                SessionEvent::Interrupted { duration_ms: None },
                at,
            )?;
        }
    }
    let changed = tx
        .execute(
            "UPDATE chat_turn
                SET status = 'interrupted', finished_at = ?1
              WHERE status IN ('queued', 'running')",
            [at],
        )
        .map_err(|e| e.to_string())?;
    if changed != 0 {
        tx.execute(
            "UPDATE chat_session
                SET updated_at = ?1
              WHERE id IN (
                    SELECT DISTINCT session_id FROM chat_turn
                     WHERE status = 'interrupted' AND finished_at = ?1
              )",
            [at],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    debug_assert_eq!(changed, active_turns.len());
    Ok(changed)
}

fn insert_event(
    conn: &Connection,
    turn_id: &str,
    event: SessionEvent,
    at: i64,
) -> Result<StoredEvent, String> {
    let payload = event.encode()?;
    let sequence: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM chat_event WHERE turn_id = ?1",
            [turn_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO chat_event(
             turn_id, sequence, schema_version, kind, payload, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            turn_id,
            sequence,
            EVENT_SCHEMA_VERSION,
            event.kind(),
            payload,
            at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(StoredEvent {
        id: conn.last_insert_rowid(),
        turn_id: turn_id.to_string(),
        sequence,
        schema_version: EVENT_SCHEMA_VERSION,
        created_at: at,
        event,
    })
}

fn load_session_on(conn: &Connection, session_id: &str) -> Result<Option<SessionDetail>, String> {
    let Some(session) = session_by_id(conn, session_id)? else {
        return Ok(None);
    };
    let turns = turns_for_session(conn, session_id)?;
    let mut details = Vec::with_capacity(turns.len());
    for turn in turns {
        let events = events_for_turn(conn, &turn.id)?;
        details.push(TurnDetail { turn, events });
    }
    Ok(Some(SessionDetail {
        session,
        turns: details,
    }))
}

fn list_sessions_on(conn: &Connection, limit: usize) -> Result<Vec<SessionSummary>, String> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let limit = i64::try_from(limit).map_err(|_| "session list limit is too large")?;
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.runner, s.runner_session_id, s.cwd, s.title,
                    s.created_at, s.updated_at,
                    (SELECT count(*) FROM chat_turn t WHERE t.session_id = s.id),
                    (SELECT status FROM chat_turn t
                      WHERE t.session_id = s.id
                      ORDER BY ordinal DESC LIMIT 1)
               FROM chat_session s
              ORDER BY s.updated_at DESC, s.id
              LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([limit], |row| {
            let status = row.get::<_, Option<String>>(8)?;
            Ok(SessionSummary {
                session: read_session(row)?,
                turn_count: row.get(7)?,
                last_turn_status: status
                    .as_deref()
                    .map(|value| TurnStatus::from_db(value, 8))
                    .transpose()?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}

fn runner_session_started_on(conn: &Connection, session_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1
               FROM chat_event e
               JOIN chat_turn t ON t.id = e.turn_id
              WHERE t.session_id = ?1 AND e.kind = 'started'
         )",
        [session_id],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

pub(super) fn session_by_id(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<ChatSession>, String> {
    conn.query_row(
        "SELECT id, runner, runner_session_id, cwd, title, created_at, updated_at
           FROM chat_session WHERE id = ?1",
        [session_id],
        read_session,
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub(super) fn turn_by_id(conn: &Connection, turn_id: &str) -> Result<Option<ChatTurn>, String> {
    conn.query_row(
        "SELECT id, session_id, ordinal, prompt, requested_model,
                requested_effort, permission_mode, status, failure_kind,
                created_at, started_at, finished_at, duration_ms
           FROM chat_turn WHERE id = ?1",
        [turn_id],
        read_turn,
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn turns_for_session(conn: &Connection, session_id: &str) -> Result<Vec<ChatTurn>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, session_id, ordinal, prompt, requested_model,
                    requested_effort, permission_mode, status, failure_kind,
                    created_at, started_at, finished_at, duration_ms
               FROM chat_turn WHERE session_id = ?1 ORDER BY ordinal",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([session_id], read_turn)
        .map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}

fn events_for_turn(conn: &Connection, turn_id: &str) -> Result<Vec<StoredEvent>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, turn_id, sequence, schema_version, kind, payload, created_at
               FROM chat_event WHERE turn_id = ?1 ORDER BY sequence",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([turn_id], |row| {
            let schema_version: i64 = row.get(3)?;
            let kind: String = row.get(4)?;
            let payload: String = row.get(5)?;
            let event = SessionEvent::decode(schema_version, &kind, &payload)
                .map_err(|message| invalid_value(5, message))?;
            Ok(StoredEvent {
                id: row.get(0)?,
                turn_id: row.get(1)?,
                sequence: row.get(2)?,
                schema_version,
                created_at: row.get(6)?,
                event,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}

fn read_session(row: &Row<'_>) -> rusqlite::Result<ChatSession> {
    let runner: String = row.get(1)?;
    Ok(ChatSession {
        id: row.get(0)?,
        runner: runner_from_db(&runner, 1)?,
        runner_session_id: row.get(2)?,
        cwd: row.get(3)?,
        title: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn read_turn(row: &Row<'_>) -> rusqlite::Result<ChatTurn> {
    let status: String = row.get(7)?;
    let failure: Option<String> = row.get(8)?;
    let duration: Option<i64> = row.get(12)?;
    Ok(ChatTurn {
        id: row.get(0)?,
        session_id: row.get(1)?,
        ordinal: row.get(2)?,
        prompt: row.get(3)?,
        requested_model: row.get(4)?,
        requested_effort: row.get(5)?,
        permission_mode: row.get(6)?,
        status: TurnStatus::from_db(&status, 7)?,
        failure_kind: failure
            .as_deref()
            .map(|value| FailureKind::from_db(value, 8))
            .transpose()?,
        created_at: row.get(9)?,
        started_at: row.get(10)?,
        finished_at: row.get(11)?,
        duration_ms: duration
            .map(|value| {
                u64::try_from(value).map_err(|_| invalid_value(12, "negative turn duration"))
            })
            .transpose()?,
    })
}

pub(super) fn validate_session(session: &NewSession) -> Result<(), String> {
    if session.cwd.trim().is_empty() {
        return Err("chat session cwd cannot be empty".to_string());
    }
    if session.title.trim().is_empty() {
        return Err("chat session title cannot be empty".to_string());
    }
    bounded("chat session cwd", &session.cwd, MAX_CWD_BYTES)?;
    bounded("chat session title", &session.title, MAX_TITLE_BYTES)?;
    if let Some(id) = session.runner_session_id.as_deref() {
        validate_runner_session_id(id)?;
    }
    Ok(())
}

pub(super) fn validate_turn(turn: &NewTurn) -> Result<(), String> {
    if turn.prompt.trim().is_empty() {
        return Err("chat turn prompt cannot be empty".to_string());
    }
    if turn.permission_mode.trim().is_empty() {
        return Err("chat turn permission mode cannot be empty".to_string());
    }
    bounded("chat turn prompt", &turn.prompt, MAX_PROMPT_BYTES)?;
    bounded(
        "chat turn permission mode",
        &turn.permission_mode,
        MAX_PERMISSION_MODE_BYTES,
    )?;
    if let Some(model) = turn.requested_model.as_deref() {
        bounded("chat turn model", model, MAX_MODEL_BYTES)?;
    }
    if let Some(effort) = turn.requested_effort.as_deref() {
        bounded("chat turn effort", effort, MAX_EFFORT_BYTES)?;
    }
    Ok(())
}

/// An attached bundle owns the model choice for the lifetime of its session.
/// Comparing the typed snapshot, rather than a denormalized column, keeps the
/// immutable attachment as the single source of execution intent.
fn enforce_attached_model(
    conn: &Connection,
    session_id: &str,
    requested_model: Option<&str>,
) -> Result<(), String> {
    let snapshot = conn
        .query_row(
            "SELECT snapshot_json FROM chat_session_bundle WHERE session_id = ?1",
            [session_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(snapshot) = snapshot else {
        return Ok(());
    };
    let snapshot: super::bundles::BundleAttachmentSnapshot = serde_json::from_str(&snapshot)
        .map_err(|_| {
            "the stored bundle attachment is invalid; no new turn was queued".to_string()
        })?;
    if snapshot.model_id.as_deref() != requested_model {
        return Err("the attached bundle locks the model for this chat session".to_string());
    }
    Ok(())
}

fn validate_event(event: &SessionEvent) -> Result<(), String> {
    let SessionEvent::PermissionRequest {
        options, prompt, ..
    } = event
    else {
        return Ok(());
    };
    if options.len() > MAX_PERMISSION_ACTIONS {
        return Err("permission request has too many actions".to_string());
    }
    for option in options {
        bounded("permission action", option, 128)?;
    }
    match prompt {
        Some(PermissionPromptData::Questions { questions }) => {
            if questions.is_empty() || questions.len() > MAX_PERMISSION_QUESTIONS {
                return Err(format!(
                    "permission request must contain 1 to {MAX_PERMISSION_QUESTIONS} questions"
                ));
            }
            for question in questions {
                bounded(
                    "permission question id",
                    &question.id,
                    MAX_QUESTION_ID_BYTES,
                )?;
                bounded(
                    "permission question header",
                    &question.header,
                    MAX_QUESTION_HEADER_BYTES,
                )?;
                bounded(
                    "permission question",
                    &question.question,
                    MAX_QUESTION_TEXT_BYTES,
                )?;
                if question.options.len() > MAX_QUESTION_OPTIONS {
                    return Err("permission question has too many options".to_string());
                }
                for option in &question.options {
                    bounded(
                        "permission question option label",
                        &option.label,
                        MAX_QUESTION_OPTION_LABEL_BYTES,
                    )?;
                    bounded(
                        "permission question option description",
                        &option.description,
                        MAX_QUESTION_OPTION_DESCRIPTION_BYTES,
                    )?;
                }
            }
        }
        Some(PermissionPromptData::Unsupported { message }) => {
            bounded(
                "unsupported permission message",
                message,
                MAX_QUESTION_TEXT_BYTES,
            )?;
        }
        None => {}
    }
    Ok(())
}

fn bounded(name: &str, value: &str, max_bytes: usize) -> Result<(), String> {
    if value.len() > max_bytes {
        Err(format!(
            "{name} exceeds the {max_bytes}-byte persistence limit"
        ))
    } else {
        Ok(())
    }
}

fn validate_runner_session_id(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("runner session id cannot be empty".to_string());
    }
    if value.len() > MAX_RUNNER_SESSION_ID_BYTES {
        return Err("runner session id exceeds the persistence limit".to_string());
    }
    Ok(())
}

fn duration_to_db(value: Option<u64>) -> Result<Option<i64>, String> {
    value
        .map(|duration| {
            i64::try_from(duration).map_err(|_| "turn duration is too large".to_string())
        })
        .transpose()
}

fn require_turn_status(
    conn: &Connection,
    turn_id: &str,
    allowed: &[TurnStatus],
) -> Result<TurnStatus, String> {
    let status = turn_by_id(conn, turn_id)?
        .map(|turn| turn.status)
        .ok_or("chat turn not found")?;
    if allowed.contains(&status) {
        Ok(status)
    } else {
        Err(format!(
            "chat turn is {}, so this transition is not allowed",
            status.as_db()
        ))
    }
}

fn transition_error(conn: &Connection, turn_id: &str, expected: &str, target: &str) -> String {
    match turn_by_id(conn, turn_id) {
        Ok(Some(turn)) => format!(
            "cannot move chat turn from {} to {target}; expected {expected}",
            turn.status.as_db()
        ),
        Ok(None) => "chat turn not found".to_string(),
        Err(error) => error,
    }
}

fn touch_session(conn: &Connection, session_id: &str, at: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE chat_session SET updated_at = ?1 WHERE id = ?2",
        params![at, session_id],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn touch_session_for_turn(conn: &Connection, turn_id: &str, at: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE chat_session
            SET updated_at = ?1
          WHERE id = (SELECT session_id FROM chat_turn WHERE id = ?2)",
        params![at, turn_id],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn runner_to_db(runner: Runner) -> &'static str {
    match runner {
        Runner::ClaudeCode => "claude-code",
        Runner::Codex => "codex",
    }
}

fn runner_from_db(value: &str, column: usize) -> rusqlite::Result<Runner> {
    match value {
        "claude-code" => Ok(Runner::ClaudeCode),
        "codex" => Ok(Runner::Codex),
        _ => Err(invalid_value(column, "unknown chat runner")),
    }
}

fn invalid_value(column: usize, message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message.into(),
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn database() -> (TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = Connection::open(dir.path().join("data.db")).unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        super::super::migrate_data(&mut conn).unwrap();
        (dir, conn)
    }

    fn session(runner_session_id: Option<&str>) -> NewSession {
        NewSession {
            runner: Runner::ClaudeCode,
            runner_session_id: runner_session_id.map(str::to_string),
            cwd: "/tmp/project".to_string(),
            title: "Inspect the project".to_string(),
        }
    }

    fn turn(prompt: &str) -> NewTurn {
        NewTurn {
            prompt: prompt.to_string(),
            requested_model: Some("discovered-model".to_string()),
            requested_effort: Some("high".to_string()),
            permission_mode: "plan".to_string(),
        }
    }

    #[test]
    fn creates_transitions_and_rehydrates_in_order() {
        let (_dir, mut conn) = database();
        let created =
            create_session_with_turn_on(&mut conn, session(Some("runner-a")), turn("First"), 10)
                .unwrap();
        Uuid::parse_str(&created.session.id).unwrap();
        Uuid::parse_str(&created.turn.id).unwrap();
        assert_eq!(created.turn.status, TurnStatus::Queued);

        mark_turn_running_on(&mut conn, &created.turn.id, 11).unwrap();
        let text = SessionEvent::Text {
            text: "A measured answer".to_string(),
        };
        let first_event = append_event_on(&mut conn, &created.turn.id, text.clone(), 12).unwrap();
        assert_eq!(first_event.sequence, 1);
        let finished = complete_turn_on(&mut conn, &created.turn.id, Some(25), 13).unwrap();
        assert_eq!(finished.sequence, 2);

        let second = queue_turn_on(&mut conn, &created.session.id, turn("Second"), 14).unwrap();
        assert_eq!(second.ordinal, 2);
        mark_turn_running_on(&mut conn, &second.id, 15).unwrap();
        fail_turn_on(
            &mut conn,
            &second.id,
            FailureKind::RunnerExit,
            "Runner exited".to_string(),
            Some(30),
            16,
        )
        .unwrap();

        let loaded = load_session_on(&conn, &created.session.id)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.turns.len(), 2);
        assert_eq!(loaded.turns[0].turn.status, TurnStatus::Completed);
        assert_eq!(loaded.turns[0].events[0].event, text);
        assert!(matches!(
            loaded.turns[0].events[1].event,
            SessionEvent::Finished {
                duration_ms: Some(25)
            }
        ));
        assert_eq!(loaded.turns[1].turn.status, TurnStatus::Failed);
        assert_eq!(
            loaded.turns[1].turn.failure_kind,
            Some(FailureKind::RunnerExit)
        );

        let summaries = list_sessions_on(&conn, 20).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].turn_count, 2);
        assert_eq!(summaries[0].last_turn_status, Some(TurnStatus::Failed));
    }

    #[test]
    fn rejects_invalid_transitions_and_two_active_turns() {
        let (_dir, mut conn) = database();
        let created =
            create_session_with_turn_on(&mut conn, session(None), turn("First"), 1).unwrap();
        assert!(queue_turn_on(&mut conn, &created.session.id, turn("Too soon"), 2).is_err());
        assert!(complete_turn_on(&mut conn, &created.turn.id, None, 2).is_err());

        mark_turn_running_on(&mut conn, &created.turn.id, 2).unwrap();
        assert!(mark_turn_running_on(&mut conn, &created.turn.id, 3).is_err());
        complete_turn_on(&mut conn, &created.turn.id, None, 4).unwrap();
        assert!(append_event_on(
            &mut conn,
            &created.turn.id,
            SessionEvent::Text {
                text: "late".to_string()
            },
            5
        )
        .is_err());
    }

    #[test]
    fn runner_identity_binds_once_and_is_unique_per_runner() {
        let (_dir, mut conn) = database();
        let first =
            create_session_with_turn_on(&mut conn, session(None), turn("First"), 1).unwrap();
        let bound = bind_runner_session_id_on(&mut conn, &first.session.id, "runner-a", 2).unwrap();
        assert_eq!(bound.runner_session_id.as_deref(), Some("runner-a"));
        bind_runner_session_id_on(&mut conn, &first.session.id, "runner-a", 3).unwrap();
        assert!(bind_runner_session_id_on(&mut conn, &first.session.id, "runner-b", 3).is_err());

        let second =
            create_session_with_turn_on(&mut conn, session(None), turn("Second"), 4).unwrap();
        assert!(bind_runner_session_id_on(&mut conn, &second.session.id, "runner-a", 5).is_err());
    }

    #[test]
    fn startup_reconciliation_never_replays_active_work() {
        let (_dir, mut conn) = database();
        let queued =
            create_session_with_turn_on(&mut conn, session(None), turn("Queued"), 1).unwrap();
        let running = create_session_with_turn_on(
            &mut conn,
            NewSession {
                title: "Running".to_string(),
                ..session(None)
            },
            turn("Running"),
            2,
        )
        .unwrap();
        mark_turn_running_on(&mut conn, &running.turn.id, 3).unwrap();

        assert_eq!(reconcile_interrupted_turns_on(&mut conn, 4).unwrap(), 2);
        assert_eq!(
            turn_by_id(&conn, &queued.turn.id).unwrap().unwrap().status,
            TurnStatus::Interrupted
        );
        assert_eq!(
            turn_by_id(&conn, &running.turn.id).unwrap().unwrap().status,
            TurnStatus::Interrupted
        );
        for session_id in [&queued.session.id, &running.session.id] {
            let detail = load_session_on(&conn, session_id).unwrap().unwrap();
            assert!(matches!(
                detail.turns[0].events.last().map(|event| &event.event),
                Some(SessionEvent::Interrupted { duration_ms: None })
            ));
        }
        let event_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_event", [], |row| row.get(0))
            .unwrap();
        assert_eq!(reconcile_interrupted_turns_on(&mut conn, 5).unwrap(), 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM chat_event", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            event_count
        );
        let resumed =
            queue_turn_on(&mut conn, &queued.session.id, turn("Explicit follow-up"), 6).unwrap();
        assert_eq!(resumed.ordinal, 2);
    }

    #[test]
    fn startup_reconciliation_expires_pending_permissions() {
        let (_dir, mut conn) = database();
        let created =
            create_session_with_turn_on(&mut conn, session(None), turn("First"), 1).unwrap();
        mark_turn_running_on(&mut conn, &created.turn.id, 2).unwrap();
        append_event_on(
            &mut conn,
            &created.turn.id,
            SessionEvent::PermissionRequest {
                request_id: "request".to_string(),
                tool_name: "Bash".to_string(),
                summary: "Run command".to_string(),
                options: vec!["allow-once".to_string(), "deny".to_string()],
                prompt: None,
                expires_with_turn: true,
            },
            3,
        )
        .unwrap();

        reconcile_interrupted_turns_on(&mut conn, 4).unwrap();
        let loaded = load_session_on(&conn, &created.session.id)
            .unwrap()
            .unwrap();
        assert!(matches!(
            loaded.turns[0].events[1].event,
            SessionEvent::PermissionResolved {
                ref request_id,
                ref decision,
            } if request_id == "request" && decision == "app-restarted"
        ));
    }

    #[test]
    fn preassigned_runner_id_resumes_only_after_a_started_event() {
        let (_dir, mut conn) = database();
        let created =
            create_session_with_turn_on(&mut conn, session(Some("preassigned")), turn("First"), 1)
                .unwrap();
        mark_turn_running_on(&mut conn, &created.turn.id, 2).unwrap();
        fail_turn_on(
            &mut conn,
            &created.turn.id,
            FailureKind::Spawn,
            "not started".to_string(),
            None,
            3,
        )
        .unwrap();
        assert!(!runner_session_started_on(&conn, &created.session.id).unwrap());

        let retry = queue_turn_on(&mut conn, &created.session.id, turn("Retry"), 4).unwrap();
        mark_turn_running_on(&mut conn, &retry.id, 5).unwrap();
        append_event_on(
            &mut conn,
            &retry.id,
            SessionEvent::Started {
                model: None,
                cwd: None,
                tools: None,
                mcp_servers: None,
                permission_mode: Some("manual".to_string()),
            },
            6,
        )
        .unwrap();
        assert!(runner_session_started_on(&conn, &created.session.id).unwrap());
    }

    #[test]
    fn rejects_oversized_turns_and_permission_forms_before_writing() {
        let (_dir, mut conn) = database();
        let oversized = NewTurn {
            prompt: "🪶".repeat(MAX_PROMPT_BYTES),
            ..turn("First")
        };
        assert!(create_session_with_turn_on(&mut conn, session(None), oversized, 1).is_err());
        let sessions: i64 = conn
            .query_row("SELECT count(*) FROM chat_session", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sessions, 0);

        let created =
            create_session_with_turn_on(&mut conn, session(None), turn("First"), 2).unwrap();
        mark_turn_running_on(&mut conn, &created.turn.id, 3).unwrap();
        let questions = (0..=MAX_PERMISSION_QUESTIONS)
            .map(|index| PermissionQuestion {
                id: format!("q{index}"),
                header: "Header".to_string(),
                question: "Question?".to_string(),
                is_other: false,
                is_secret: false,
                options: Vec::new(),
            })
            .collect();
        assert!(append_event_on(
            &mut conn,
            &created.turn.id,
            SessionEvent::PermissionRequest {
                request_id: "request".to_string(),
                tool_name: "Question".to_string(),
                summary: "Questions".to_string(),
                options: vec!["submit".to_string()],
                prompt: Some(PermissionPromptData::Questions { questions }),
                expires_with_turn: true,
            },
            4,
        )
        .is_err());
    }

    #[test]
    fn event_protocol_is_bounded_and_detects_kind_corruption() {
        let (_dir, mut conn) = database();
        let created =
            create_session_with_turn_on(&mut conn, session(None), turn("First"), 1).unwrap();
        mark_turn_running_on(&mut conn, &created.turn.id, 2).unwrap();
        let oversized = SessionEvent::Text {
            text: "x".repeat(MAX_EVENT_PAYLOAD_BYTES + 1),
        };
        assert!(append_event_on(&mut conn, &created.turn.id, oversized, 3).is_err());
        let count: i64 = conn
            .query_row("SELECT count(*) FROM chat_event", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);

        append_event_on(
            &mut conn,
            &created.turn.id,
            SessionEvent::Text {
                text: "safe".to_string(),
            },
            4,
        )
        .unwrap();
        conn.execute("UPDATE chat_event SET kind = 'tool-call'", [])
            .unwrap();
        assert!(load_session_on(&conn, &created.session.id).is_err());
    }

    #[test]
    fn deleting_an_aviary_session_cascades_its_local_transcript() {
        let (_dir, mut conn) = database();
        let created =
            create_session_with_turn_on(&mut conn, session(None), turn("First"), 1).unwrap();
        mark_turn_running_on(&mut conn, &created.turn.id, 2).unwrap();
        append_event_on(
            &mut conn,
            &created.turn.id,
            SessionEvent::Text {
                text: "answer".to_string(),
            },
            3,
        )
        .unwrap();
        conn.execute(
            "DELETE FROM chat_session WHERE id = ?1",
            [&created.session.id],
        )
        .unwrap();
        let turns: i64 = conn
            .query_row("SELECT count(*) FROM chat_turn", [], |row| row.get(0))
            .unwrap();
        let events: i64 = conn
            .query_row("SELECT count(*) FROM chat_event", [], |row| row.get(0))
            .unwrap();
        assert_eq!((turns, events), (0, 0));
    }
}
