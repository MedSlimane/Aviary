//! Codex app-server JSONL adapter and its deliberately limited fallback.

use super::{
    configure_process_group, sanitize, terminate_process_group, CodexDecisionKind, FailureKind,
    Launch, PendingPayload, PermissionDecision, PermissionPrompt, PermissionReply,
    PermissionWireResponse, ProtocolAction, ProtocolTerminal, QueuedTurn, Runner,
    SafetyCapabilities, SafetyOption, SelectedSafety, SessionEvent,
};
use crate::store::sessions::{
    PermissionPromptData, PermissionQuestion, PermissionQuestionOption, ToolResultStatus,
};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const APP_SERVER_PROTOCOL: &str = "codex-app-server-v2";
const FALLBACK_PROTOCOL: &str = "codex-exec-read-only-fallback";
const INITIALIZE_ID: &str = "aviary-initialize";
const THREAD_ID: &str = "aviary-thread";
const TURN_ID: &str = "aviary-turn";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Initialize,
    Thread,
    Turn,
    Running,
}

#[derive(Debug)]
pub(super) struct Protocol {
    stage: Stage,
    resume_id: Option<String>,
    thread_id: Option<String>,
    turn_id: Option<String>,
    prompt: String,
    cwd: String,
    model: Option<String>,
    effort: Option<String>,
    sandbox: String,
    approval_policy: String,
}

#[derive(Debug, Default)]
pub(super) struct ExecProtocol {
    started: bool,
}

pub(super) fn discover_safety() -> SafetyCapabilities {
    let version = match Command::new("codex").arg("--version").output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).to_string()
        }
        Ok(output) => {
            return unavailable(format!(
                "Codex capability discovery exited with {}",
                output.status
            ))
        }
        Err(error) => return unavailable(format!("Codex is unavailable: {error}")),
    };
    match probe_app_server() {
        Ok(()) => SafetyCapabilities {
            runner: Runner::Codex,
            available: true,
            protocol: APP_SERVER_PROTOCOL.to_string(),
            default_option_id: Some("codex:read-only-never".to_string()),
            options: vec![
                safety_option(
                    "codex:read-only-never",
                    "Read-only",
                    "Read files without requesting elevated access.",
                    false,
                    "read-only",
                    "never",
                ),
                safety_option(
                    "codex:read-only-user",
                    "Read-only + approvals",
                    "Start read-only and route elevation requests to Aviary.",
                    true,
                    "read-only",
                    "on-request",
                ),
                safety_option(
                    "codex:workspace-user",
                    "Workspace + approvals",
                    "Allow workspace edits and ask before sandbox escapes.",
                    true,
                    "workspace-write",
                    "on-request",
                ),
            ],
            warning: None,
        },
        Err(error) => {
            let exec_available = Command::new("codex")
                .args(["exec", "--help"])
                .output()
                .is_ok_and(|output| output.status.success());
            SafetyCapabilities {
                runner: Runner::Codex,
                available: exec_available,
                protocol: FALLBACK_PROTOCOL.to_string(),
                default_option_id: exec_available
                    .then(|| "codex:read-only-never".to_string()),
                options: exec_available
                    .then(|| {
                        vec![safety_option(
                            "codex:read-only-never",
                            "Read-only",
                            "App-server approvals are unavailable; this fallback never elevates.",
                            false,
                            "read-only",
                            "never",
                        )]
                    })
                    .unwrap_or_default(),
                warning: Some(format!(
                    "{version} did not complete an app-server initialize probe: {error}. Manual approvals are unavailable."
                )),
            }
        }
    }
}

pub(super) fn select_safety(id: &str, protocol: &str) -> Result<SelectedSafety, String> {
    if protocol == FALLBACK_PROTOCOL {
        if id != "codex:read-only-never" {
            return Err(
                "Codex app-server is unavailable; only read-only + never is honest".to_string(),
            );
        }
        return Ok(SelectedSafety::CodexFallback {
            option_id: id.to_string(),
        });
    }
    let (sandbox, approval_policy) = match id {
        "codex:read-only-never" => ("read-only", "never"),
        "codex:read-only-user" => ("read-only", "on-request"),
        "codex:workspace-user" => ("workspace-write", "on-request"),
        _ => return Err("invalid Codex safety option".to_string()),
    };
    Ok(SelectedSafety::CodexAppServer {
        option_id: id.to_string(),
        sandbox: sandbox.to_string(),
        approval_policy: approval_policy.to_string(),
    })
}

pub(super) fn build_app_server(
    queued: &QueuedTurn,
    sandbox: &str,
    approval_policy: &str,
) -> Result<(Protocol, Launch), (FailureKind, String)> {
    let mut command = Command::new("codex");
    command
        .args(["app-server", "--stdio"])
        .current_dir(&queued.session.cwd);
    let protocol = Protocol {
        stage: Stage::Initialize,
        resume_id: queued.session.runner_session_id.clone(),
        thread_id: None,
        turn_id: None,
        prompt: queued.turn.prompt.clone(),
        cwd: queued.session.cwd.clone(),
        model: queued.turn.requested_model.clone(),
        effort: queued.turn.requested_effort.clone(),
        sandbox: sandbox.to_string(),
        approval_policy: approval_policy.to_string(),
    };
    let initialize = json!({
        "id": INITIALIZE_ID,
        "method": "initialize",
        "params": {
            "clientInfo": {
                "name": "aviary",
                "title": "Aviary",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": {
                "experimentalApi": true,
                "mcpServerOpenaiFormElicitation": true,
            }
        }
    });
    Ok((
        protocol,
        Launch {
            command,
            initial_lines: vec![serde_json::to_string(&initialize)
                .map_err(|error| (FailureKind::Input, error.to_string()))?],
            close_stdin_after_initial: false,
            kill_after_terminal: true,
        },
    ))
}

pub(super) fn build_fallback(
    queued: &QueuedTurn,
) -> Result<(ExecProtocol, Launch), (FailureKind, String)> {
    let mut command = Command::new("codex");
    if let Some(session_id) = queued.session.runner_session_id.as_deref() {
        command
            .args(["exec", "resume"])
            .arg("-c")
            .arg("approval_policy=\"never\"")
            .arg("-c")
            .arg("sandbox_mode=\"read-only\"")
            .arg("--json");
        if let Some(model) = queued.turn.requested_model.as_deref() {
            command.arg(format!("--model={model}"));
        }
        if let Some(effort) = queued.turn.requested_effort.as_deref() {
            command
                .arg("-c")
                .arg(toml_string_override("model_reasoning_effort", effort));
        }
        command.arg("--").arg(session_id).arg("-");
    } else {
        command
            .args(["exec", "--json", "--sandbox", "read-only"])
            .arg("-c")
            .arg("approval_policy=\"never\"");
        if let Some(model) = queued.turn.requested_model.as_deref() {
            command.arg(format!("--model={model}"));
        }
        if let Some(effort) = queued.turn.requested_effort.as_deref() {
            command
                .arg("-c")
                .arg(toml_string_override("model_reasoning_effort", effort));
        }
        command.arg("-");
    }
    command.current_dir(&queued.session.cwd);
    Ok((
        ExecProtocol::default(),
        Launch {
            command,
            initial_lines: vec![queued.turn.prompt.clone()],
            close_stdin_after_initial: true,
            kill_after_terminal: false,
        },
    ))
}

fn toml_string_override(key: &str, value: &str) -> String {
    // JSON string escaping is a strict subset of TOML basic-string escaping.
    // Keeping the entire override in one argv entry prevents both shell and
    // config-key injection from manually entered model settings.
    let encoded = serde_json::to_string(value).expect("serializing a string cannot fail");
    format!("{key}={encoded}")
}

impl Protocol {
    pub(super) fn handle_line(&mut self, line: &str) -> Result<Vec<ProtocolAction>, String> {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| format!("invalid Codex app-server frame: {error}"))?;
        if value.get("id").is_some() && value.get("method").is_none() {
            return self.response(&value);
        }
        let Some(method) = value.get("method").and_then(Value::as_str) else {
            return Ok(Vec::new());
        };
        if value.get("id").is_some() {
            return self.server_request(method, &value);
        }
        self.notification(method, value.get("params").unwrap_or(&Value::Null))
    }

    pub(super) fn permission_response(
        &mut self,
        payload: PendingPayload,
        reply: &PermissionReply,
    ) -> Result<PermissionWireResponse, String> {
        match payload {
            PendingPayload::CodexDecision { rpc_id, kind } => {
                let decision = match reply.decision {
                    PermissionDecision::AllowOnce | PermissionDecision::Submit => "accept",
                    PermissionDecision::AllowSession => "acceptForSession",
                    PermissionDecision::Deny => "decline",
                    PermissionDecision::Cancel => "cancel",
                };
                let _ = kind;
                Ok(PermissionWireResponse {
                    line: json!({"id": rpc_id, "result": {"decision": decision}}),
                    interrupt: false,
                })
            }
            PendingPayload::CodexPermissions {
                rpc_id,
                permissions,
            } => {
                let allow = matches!(
                    reply.decision,
                    PermissionDecision::AllowOnce
                        | PermissionDecision::AllowSession
                        | PermissionDecision::Submit
                );
                let scope = if matches!(reply.decision, PermissionDecision::AllowSession) {
                    "session"
                } else {
                    "turn"
                };
                Ok(PermissionWireResponse {
                    line: json!({
                        "id": rpc_id,
                        "result": {
                            "permissions": if allow { permissions } else { json!({}) },
                            "scope": scope,
                        }
                    }),
                    interrupt: matches!(reply.decision, PermissionDecision::Cancel),
                })
            }
            PendingPayload::CodexUserInput { rpc_id, questions } => {
                if !matches!(
                    reply.decision,
                    PermissionDecision::Submit | PermissionDecision::Cancel
                ) {
                    return Err(
                        "Codex user-input requests accept only submit or cancel".to_string()
                    );
                }
                let answers = if matches!(reply.decision, PermissionDecision::Cancel) {
                    json!({})
                } else {
                    validate_answers(reply.answers.as_ref(), &questions)?
                };
                Ok(PermissionWireResponse {
                    line: json!({"id": rpc_id, "result": {"answers": answers}}),
                    interrupt: matches!(reply.decision, PermissionDecision::Cancel),
                })
            }
            PendingPayload::CodexElicitation { rpc_id } => {
                let action = match reply.decision {
                    PermissionDecision::Deny => "decline",
                    PermissionDecision::Cancel => "cancel",
                    PermissionDecision::AllowOnce
                    | PermissionDecision::AllowSession
                    | PermissionDecision::Submit => return Err(
                        "this Codex elicitation cannot be represented safely; decline or cancel it"
                            .to_string(),
                    ),
                };
                Ok(PermissionWireResponse {
                    line: json!({
                        "id": rpc_id,
                        "result": {"action": action, "content": reply.content.clone()}
                    }),
                    interrupt: false,
                })
            }
            PendingPayload::Claude { .. } => {
                Err("Codex received a Claude permission response".to_string())
            }
        }
    }

    pub(super) fn interrupt_message(&mut self) -> Option<Value> {
        Some(json!({
            "id": format!("aviary-interrupt-{}", uuid::Uuid::new_v4()),
            "method": "turn/interrupt",
            "params": {
                "threadId": self.thread_id.as_ref()?,
                "turnId": self.turn_id.as_ref()?,
            }
        }))
    }

    fn response(&mut self, value: &Value) -> Result<Vec<ProtocolAction>, String> {
        let id = value.get("id");
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Codex app-server returned an error");
            return Err(sanitize::text(message));
        }
        let result = value
            .get("result")
            .ok_or_else(|| "Codex response omitted result".to_string())?;
        match self.stage {
            Stage::Initialize if id == Some(&Value::String(INITIALIZE_ID.to_string())) => {
                self.stage = Stage::Thread;
                Ok(vec![
                    ProtocolAction::Send(json!({"method": "initialized"})),
                    ProtocolAction::Send(self.thread_request()),
                ])
            }
            Stage::Thread if id == Some(&Value::String(THREAD_ID.to_string())) => {
                let runner_thread_id = result
                    .get("thread")
                    .and_then(|thread| thread.get("id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| "Codex thread response omitted thread.id".to_string())?;
                self.thread_id = Some(runner_thread_id.to_string());
                self.stage = Stage::Turn;
                Ok(vec![
                    ProtocolAction::BindSession(runner_thread_id.to_string()),
                    ProtocolAction::Event(SessionEvent::Started {
                        model: result
                            .get("model")
                            .and_then(Value::as_str)
                            .map(sanitize::text),
                        cwd: result
                            .get("cwd")
                            .and_then(Value::as_str)
                            .map(sanitize::text),
                        tools: None,
                        mcp_servers: None,
                        permission_mode: Some(format!(
                            "{} + {}",
                            self.sandbox, self.approval_policy
                        )),
                    }),
                    ProtocolAction::Send(self.turn_request(runner_thread_id)),
                ])
            }
            Stage::Turn if id == Some(&Value::String(TURN_ID.to_string())) => {
                self.turn_id = result
                    .get("turn")
                    .and_then(|turn| turn.get("id"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                if self.turn_id.is_none() {
                    return Err("Codex turn/start response omitted turn.id".to_string());
                }
                self.stage = Stage::Running;
                Ok(Vec::new())
            }
            _ => Ok(Vec::new()),
        }
    }

    fn thread_request(&self) -> Value {
        let common = json!({
            "cwd": self.cwd,
            "model": self.model,
            "approvalPolicy": self.approval_policy,
            "approvalsReviewer": "user",
            "sandbox": self.sandbox,
        });
        if let Some(thread_id) = self.resume_id.as_deref() {
            let mut params = common;
            params["threadId"] = Value::String(thread_id.to_string());
            json!({"id": THREAD_ID, "method": "thread/resume", "params": params})
        } else {
            let mut params = common;
            params["experimentalRawEvents"] = Value::Bool(false);
            params["ephemeral"] = Value::Bool(false);
            json!({"id": THREAD_ID, "method": "thread/start", "params": params})
        }
    }

    fn turn_request(&self, thread_id: &str) -> Value {
        json!({
            "id": TURN_ID,
            "method": "turn/start",
            "params": {
                "threadId": thread_id,
                "input": [{"type": "text", "text": self.prompt}],
                "cwd": self.cwd,
                "model": self.model,
                "effort": self.effort,
                "approvalPolicy": self.approval_policy,
                "approvalsReviewer": "user",
                "sandboxPolicy": sandbox_policy(&self.sandbox, &self.cwd),
            }
        })
    }

    fn notification(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<Vec<ProtocolAction>, String> {
        match method {
            "turn/started" => {
                self.turn_id = params
                    .get("turn")
                    .and_then(|turn| turn.get("id"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .or_else(|| self.turn_id.clone());
                Ok(Vec::new())
            }
            "item/agentMessage/delta" => Ok(text_delta(params, false)),
            "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
                Ok(text_delta(params, true))
            }
            "item/started" => Ok(params
                .get("item")
                .and_then(tool_event_started)
                .into_iter()
                .map(ProtocolAction::Event)
                .collect()),
            "item/completed" => Ok(params
                .get("item")
                .and_then(tool_event_finished)
                .into_iter()
                .map(ProtocolAction::Event)
                .collect()),
            "item/commandExecution/outputDelta" => Ok(tool_delta(params, "Command")),
            "item/fileChange/outputDelta" => Ok(tool_delta(params, "File change")),
            "thread/tokenUsage/updated" => Ok(token_usage(params)),
            "turn/completed" => Ok(vec![ProtocolAction::Terminal(turn_terminal(params))]),
            _ => Ok(Vec::new()),
        }
    }

    fn server_request(&self, method: &str, value: &Value) -> Result<Vec<ProtocolAction>, String> {
        let rpc_id = value
            .get("id")
            .cloned()
            .ok_or_else(|| "Codex server request omitted id".to_string())?;
        let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
        let wire_key = rpc_key(&rpc_id);
        let prompt = match method {
            "item/commandExecution/requestApproval" => PermissionPrompt {
                wire_key,
                tool_name: "Command".to_string(),
                summary: first_string(&params, &["reason", "command"])
                    .unwrap_or_else(|| "Run command".to_string()),
                options: decision_options(params.get("availableDecisions")),
                prompt: None,
                payload: PendingPayload::CodexDecision {
                    rpc_id,
                    kind: CodexDecisionKind::Command,
                },
            },
            "item/fileChange/requestApproval" => PermissionPrompt {
                wire_key,
                tool_name: "File change".to_string(),
                summary: first_string(&params, &["reason", "grantRoot"])
                    .unwrap_or_else(|| "Apply file changes".to_string()),
                options: vec![
                    "allow-once".to_string(),
                    "allow-session".to_string(),
                    "deny".to_string(),
                    "cancel".to_string(),
                ],
                prompt: None,
                payload: PendingPayload::CodexDecision {
                    rpc_id,
                    kind: CodexDecisionKind::FileChange,
                },
            },
            "item/permissions/requestApproval" => {
                let permissions = params
                    .get("permissions")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                PermissionPrompt {
                    wire_key,
                    tool_name: "Additional permissions".to_string(),
                    summary: first_string(&params, &["reason"])
                        .unwrap_or_else(|| short(&sanitize::compact(&permissions), 240)),
                    options: vec![
                        "allow-once".to_string(),
                        "allow-session".to_string(),
                        "deny".to_string(),
                        "cancel".to_string(),
                    ],
                    prompt: None,
                    payload: PendingPayload::CodexPermissions {
                        rpc_id,
                        permissions,
                    },
                }
            }
            "item/tool/requestUserInput" => {
                let questions = parse_questions(&params)?;
                PermissionPrompt {
                    wire_key,
                    tool_name: "Question".to_string(),
                    summary: questions
                        .first()
                        .map(|question| question.question.clone())
                        .unwrap_or_else(|| "Codex requests input".to_string()),
                    options: vec!["submit".to_string(), "cancel".to_string()],
                    prompt: Some(PermissionPromptData::Questions {
                        questions: questions.clone(),
                    }),
                    payload: PendingPayload::CodexUserInput { rpc_id, questions },
                }
            }
            "mcpServer/elicitation/request" => PermissionPrompt {
                wire_key,
                tool_name: "MCP elicitation".to_string(),
                summary: first_string(&params, &["serverName"])
                    .unwrap_or_else(|| "MCP server requests input".to_string()),
                options: vec!["deny".to_string(), "cancel".to_string()],
                prompt: Some(PermissionPromptData::Unsupported {
                    message:
                        "This Codex version does not expose a bounded elicitation form to Aviary."
                            .to_string(),
                }),
                payload: PendingPayload::CodexElicitation { rpc_id },
            },
            _ => {
                return Ok(vec![ProtocolAction::Send(json!({
                    "id": rpc_id,
                    "error": {"code": -32601, "message": "Aviary does not implement this server request"}
                }))])
            }
        };
        Ok(vec![ProtocolAction::Permission(prompt)])
    }
}

impl ExecProtocol {
    pub(super) fn handle_line(&mut self, line: &str) -> Result<Vec<ProtocolAction>, String> {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| format!("invalid Codex exec JSON frame: {error}"))?;
        match value.get("type").and_then(Value::as_str).unwrap_or("") {
            "thread.started" => {
                let id = value
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "Codex thread.started omitted thread_id".to_string())?;
                self.started = true;
                Ok(vec![
                    ProtocolAction::BindSession(id.to_string()),
                    ProtocolAction::Event(SessionEvent::Started {
                        model: None,
                        cwd: None,
                        tools: None,
                        mcp_servers: None,
                        permission_mode: Some("read-only + never".to_string()),
                    }),
                ])
            }
            "item.started" => Ok(value
                .get("item")
                .and_then(tool_event_started)
                .into_iter()
                .map(ProtocolAction::Event)
                .collect()),
            "item.completed" => Ok(value
                .get("item")
                .map(exec_completed_actions)
                .unwrap_or_default()),
            "turn.completed" => Ok(vec![ProtocolAction::Terminal(
                ProtocolTerminal::Completed {
                    duration_ms: value.get("duration_ms").and_then(Value::as_u64),
                },
            )]),
            "turn.failed" | "error" => {
                Ok(vec![ProtocolAction::Terminal(ProtocolTerminal::Failed {
                    message: value
                        .get("message")
                        .or_else(|| value.get("error").and_then(|error| error.get("message")))
                        .and_then(Value::as_str)
                        .map(sanitize::text)
                        .unwrap_or_else(|| "Codex reported a failed turn".to_string()),
                    duration_ms: None,
                })])
            }
            _ => Ok(Vec::new()),
        }
    }
}

fn exec_completed_actions(item: &Value) -> Vec<ProtocolAction> {
    match item.get("type").and_then(Value::as_str).unwrap_or("") {
        "agent_message" | "agentMessage" => item
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(|text| {
                vec![ProtocolAction::Event(SessionEvent::Text {
                    text: sanitize::text(text),
                })]
            })
            .unwrap_or_default(),
        "reasoning" => {
            let summary = item
                .get("summary")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|part| {
                            part.as_str()
                                .or_else(|| part.get("text").and_then(Value::as_str))
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|text| !text.trim().is_empty());
            summary
                .map(|text| {
                    vec![ProtocolAction::Event(SessionEvent::Thinking {
                        text: sanitize::text(&text),
                    })]
                })
                .unwrap_or_default()
        }
        _ => tool_event_finished(item)
            .into_iter()
            .map(ProtocolAction::Event)
            .collect(),
    }
}

fn probe_app_server() -> Result<(), String> {
    let mut command = Command::new("codex");
    command.args(["app-server", "--stdio"]);
    configure_process_group(&mut command);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let mut stdin = child
        .stdin
        .take()
        .expect("piped Codex probe stdin must exist");
    let stdout = child
        .stdout
        .take()
        .expect("piped Codex probe stdout must exist");
    let request = json!({
        "id": "aviary-capability-probe",
        "method": "initialize",
        "params": {
            "clientInfo": {"name": "aviary", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"experimentalApi": true}
        }
    });
    let request = serde_json::to_string(&request).map_err(|error| error.to_string())?;
    if let Err(error) = writeln!(stdin, "{request}").and_then(|_| stdin.flush()) {
        terminate_process_group(&mut child, true);
        let _ = child.wait();
        return Err(error.to_string());
    }
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let result = reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())
            .and_then(|read| {
                if read == 0 {
                    Err("Codex app-server closed during initialize".to_string())
                } else {
                    serde_json::from_str::<Value>(&line).map_err(|error| error.to_string())
                }
            });
        let _ = tx.send(result);
    });
    let response = rx.recv_timeout(Duration::from_secs(3));
    terminate_process_group(&mut child, true);
    let _ = child.wait();
    let response = response.map_err(|_| "initialize timed out".to_string())?;
    let response = response?;
    if response.get("id") != Some(&Value::String("aviary-capability-probe".to_string()))
        || response.get("result").is_none()
    {
        return Err("initialize returned an unexpected response".to_string());
    }
    Ok(())
}

fn safety_option(
    id: &str,
    label: &str,
    description: &str,
    interactive: bool,
    sandbox: &str,
    approval_policy: &str,
) -> SafetyOption {
    SafetyOption {
        id: id.to_string(),
        label: label.to_string(),
        description: description.to_string(),
        interactive_approvals: interactive,
        dangerous: false,
        sandbox: Some(sandbox.to_string()),
        approval_policy: Some(approval_policy.to_string()),
    }
}

fn unavailable(message: String) -> SafetyCapabilities {
    SafetyCapabilities {
        runner: Runner::Codex,
        available: false,
        protocol: APP_SERVER_PROTOCOL.to_string(),
        default_option_id: None,
        options: Vec::new(),
        warning: Some(message),
    }
}

fn sandbox_policy(sandbox: &str, cwd: &str) -> Value {
    match sandbox {
        "workspace-write" => json!({
            "type": "workspaceWrite",
            "writableRoots": [cwd],
            "networkAccess": false,
            "excludeTmpdirEnvVar": false,
            "excludeSlashTmp": false,
        }),
        "danger-full-access" => json!({"type": "dangerFullAccess"}),
        _ => json!({"type": "readOnly", "networkAccess": false}),
    }
}

fn text_delta(params: &Value, thinking: bool) -> Vec<ProtocolAction> {
    params
        .get("delta")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(|text| {
            vec![ProtocolAction::Event(if thinking {
                SessionEvent::Thinking {
                    text: sanitize::text(text),
                }
            } else {
                SessionEvent::Text {
                    text: sanitize::text(text),
                }
            })]
        })
        .unwrap_or_default()
}

fn tool_delta(params: &Value, name: &str) -> Vec<ProtocolAction> {
    let Some(id) = params.get("itemId").and_then(Value::as_str) else {
        return Vec::new();
    };
    let detail = params
        .get("delta")
        .and_then(Value::as_str)
        .map(sanitize::text);
    vec![ProtocolAction::Event(SessionEvent::ToolUpdated {
        call_id: id.to_string(),
        name: name.to_string(),
        summary: detail
            .as_deref()
            .map(|value| short(value, 240))
            .unwrap_or_default(),
        detail,
    })]
}

fn tool_event_started(item: &Value) -> Option<SessionEvent> {
    let (id, name, summary, detail) = tool_identity(item)?;
    Some(SessionEvent::ToolStarted {
        call_id: id,
        name,
        summary,
        detail,
    })
}

fn tool_event_finished(item: &Value) -> Option<SessionEvent> {
    let (id, name, summary, detail) = tool_identity(item)?;
    let status = item.get("status").and_then(Value::as_str).unwrap_or("");
    let failed = matches!(status, "failed" | "declined" | "rejected" | "cancelled")
        || item.get("success").and_then(Value::as_bool) == Some(false)
        || item.get("error").is_some_and(|error| !error.is_null());
    Some(SessionEvent::ToolFinished {
        call_id: id,
        name,
        status: if failed {
            ToolResultStatus::Failed
        } else {
            ToolResultStatus::Succeeded
        },
        summary,
        detail,
    })
}

fn tool_identity(item: &Value) -> Option<(String, String, String, Option<String>)> {
    let kind = item.get("type").and_then(Value::as_str)?;
    let id = item.get("id").and_then(Value::as_str)?.to_string();
    let safe = sanitize::value(item);
    let (name, summary) = match kind {
        "commandExecution" => (
            "Command".to_string(),
            first_string(&safe, &["command"]).unwrap_or_default(),
        ),
        "fileChange" => (
            "File change".to_string(),
            safe.get("changes")
                .and_then(Value::as_array)
                .map(|changes| format!("{} file change(s)", changes.len()))
                .unwrap_or_default(),
        ),
        "mcpToolCall" => {
            let server = first_string(&safe, &["server"]).unwrap_or_else(|| "MCP".to_string());
            let tool = first_string(&safe, &["tool"]).unwrap_or_else(|| "tool".to_string());
            (format!("{server} · {tool}"), format!("{server}/{tool}"))
        }
        "dynamicToolCall" => (
            first_string(&safe, &["tool"]).unwrap_or_else(|| "Dynamic tool".to_string()),
            first_string(&safe, &["tool"]).unwrap_or_default(),
        ),
        "webSearch" => (
            "Web search".to_string(),
            first_string(&safe, &["query"]).unwrap_or_default(),
        ),
        "collabAgentToolCall" => (
            "Agent".to_string(),
            first_string(&safe, &["tool"]).unwrap_or_default(),
        ),
        _ => return None,
    };
    Some((
        id,
        name,
        short(&summary, 240),
        Some(sanitize::compact(&safe)),
    ))
}

fn token_usage(params: &Value) -> Vec<ProtocolAction> {
    let Some(last) = params.get("tokenUsage").and_then(|usage| usage.get("last")) else {
        return Vec::new();
    };
    vec![ProtocolAction::Event(SessionEvent::TokenUsage {
        input_tokens: nonnegative(last, "inputTokens"),
        cached_input_tokens: nonnegative(last, "cachedInputTokens"),
        output_tokens: nonnegative(last, "outputTokens"),
        reasoning_output_tokens: nonnegative(last, "reasoningOutputTokens"),
        total_tokens: nonnegative(last, "totalTokens"),
    })]
}

fn turn_terminal(params: &Value) -> ProtocolTerminal {
    let turn = params.get("turn").unwrap_or(params);
    let duration_ms = turn
        .get("durationMs")
        .and_then(Value::as_i64)
        .and_then(|value| u64::try_from(value).ok());
    match turn.get("status").and_then(Value::as_str).unwrap_or("") {
        "completed" => ProtocolTerminal::Completed { duration_ms },
        "interrupted" => ProtocolTerminal::Interrupted,
        _ => ProtocolTerminal::Failed {
            message: turn
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(sanitize::text)
                .unwrap_or_else(|| "Codex reported a failed turn".to_string()),
            duration_ms,
        },
    }
}

fn rpc_key(value: &Value) -> String {
    match value {
        Value::String(value) => format!("string:{value}"),
        Value::Number(value) => format!("number:{value}"),
        _ => format!("other:{}", sanitize::compact(value)),
    }
}

fn decision_options(value: Option<&Value>) -> Vec<String> {
    let mut options = value
        .and_then(Value::as_array)
        .map(|decisions| {
            decisions
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|decision| match decision {
                    "accept" => Some("allow-once"),
                    "acceptForSession" => Some("allow-session"),
                    "decline" => Some("deny"),
                    "cancel" => Some("cancel"),
                    _ => None,
                })
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if options.is_empty() {
        options = vec![
            "allow-once".to_string(),
            "allow-session".to_string(),
            "deny".to_string(),
            "cancel".to_string(),
        ];
    }
    options
}

fn parse_questions(params: &Value) -> Result<Vec<PermissionQuestion>, String> {
    const MAX_QUESTIONS: usize = 8;
    const MAX_OPTIONS: usize = 20;

    let values = params
        .get("questions")
        .and_then(Value::as_array)
        .ok_or_else(|| "Codex user-input request omitted questions".to_string())?;
    if values.is_empty() || values.len() > MAX_QUESTIONS {
        return Err(format!(
            "Codex user-input request must contain 1 to {MAX_QUESTIONS} questions"
        ));
    }
    let mut questions = Vec::with_capacity(values.len());
    for value in values {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(sanitize::text)
            .ok_or_else(|| "Codex user-input question omitted id".to_string())?;
        if questions
            .iter()
            .any(|question: &PermissionQuestion| question.id == id)
        {
            return Err("Codex user-input question ids must be unique".to_string());
        }
        let raw_options = value
            .get("options")
            .filter(|options| !options.is_null())
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if raw_options.len() > MAX_OPTIONS {
            return Err(format!(
                "Codex user-input question exceeds {MAX_OPTIONS} options"
            ));
        }
        let options = raw_options
            .iter()
            .map(|option| {
                Ok(PermissionQuestionOption {
                    label: option
                        .get("label")
                        .and_then(Value::as_str)
                        .map(sanitize::text)
                        .ok_or_else(|| "Codex question option omitted label".to_string())?,
                    description: option
                        .get("description")
                        .and_then(Value::as_str)
                        .map(sanitize::text)
                        .ok_or_else(|| "Codex question option omitted description".to_string())?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        questions.push(PermissionQuestion {
            id,
            header: value
                .get("header")
                .and_then(Value::as_str)
                .map(sanitize::text)
                .ok_or_else(|| "Codex user-input question omitted header".to_string())?,
            question: value
                .get("question")
                .and_then(Value::as_str)
                .map(sanitize::text)
                .ok_or_else(|| "Codex user-input question omitted question".to_string())?,
            is_other: value
                .get("isOther")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            is_secret: value
                .get("isSecret")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            options,
        });
    }
    Ok(questions)
}

fn validate_answers(
    value: Option<&Value>,
    questions: &[PermissionQuestion],
) -> Result<Value, String> {
    const MAX_ANSWERS_PER_QUESTION: usize = 16;

    let supplied = value
        .and_then(Value::as_object)
        .ok_or_else(|| "Codex user-input response requires an answers object".to_string())?;
    if supplied.len() != questions.len() {
        return Err("Codex user-input response must answer every displayed question".to_string());
    }
    let mut output = serde_json::Map::new();
    for question in questions {
        let answers = supplied
            .get(&question.id)
            .and_then(|answer| answer.get("answers"))
            .and_then(Value::as_array)
            .ok_or_else(|| format!("answer for question {:?} is malformed", question.id))?;
        if answers.len() > MAX_ANSWERS_PER_QUESTION {
            return Err(format!(
                "answer for question {:?} exceeds {MAX_ANSWERS_PER_QUESTION} values",
                question.id
            ));
        }
        let answers = answers
            .iter()
            .map(|answer| {
                answer
                    .as_str()
                    .map(sanitize::text)
                    .map(Value::String)
                    .ok_or_else(|| "Codex user-input answers must be strings".to_string())
            })
            .collect::<Result<Vec<_>, String>>()?;
        output.insert(question.id.clone(), json!({"answers": answers}));
    }
    if supplied.keys().any(|id| !output.contains_key(id)) {
        return Err("Codex user-input response contains an unknown question id".to_string());
    }
    Ok(Value::Object(output))
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(sanitize::text)
}

fn nonnegative(value: &Value, key: &str) -> Option<u64> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .and_then(|value| u64::try_from(value).ok())
}

fn short(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sessions::{ChatSession, ChatTurn, TurnStatus};

    fn queued(resume: Option<&str>) -> QueuedTurn {
        QueuedTurn {
            session: ChatSession {
                id: "session".to_string(),
                runner: Runner::Codex,
                runner_session_id: resume.map(ToOwned::to_owned),
                cwd: std::env::current_dir()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                title: "test".to_string(),
                created_at: 0,
                updated_at: 0,
            },
            turn: ChatTurn {
                id: "turn".to_string(),
                session_id: "session".to_string(),
                ordinal: 1,
                prompt: "prompt only on stdin".to_string(),
                requested_model: Some("model".to_string()),
                requested_effort: Some("high".to_string()),
                permission_mode: "codex:workspace-user".to_string(),
                status: TurnStatus::Queued,
                failure_kind: None,
                created_at: 0,
                started_at: None,
                finished_at: None,
                duration_ms: None,
            },
        }
    }

    fn protocol(resume: Option<&str>) -> Protocol {
        build_app_server(&queued(resume), "workspace-write", "on-request")
            .unwrap()
            .0
    }

    #[test]
    fn resume_state_machine_keeps_prompt_off_argv_and_routes_user_reviews() {
        let (mut protocol, launch) = build_app_server(
            &queued(Some("runner-thread")),
            "workspace-write",
            "on-request",
        )
        .unwrap();
        let args = launch
            .command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(!args.iter().any(|arg| arg.contains("prompt only")));
        let actions = protocol
            .handle_line(r#"{"id":"aviary-initialize","result":{"userAgent":"x"}}"#)
            .unwrap();
        let ProtocolAction::Send(thread) = &actions[1] else {
            panic!("expected thread request");
        };
        assert_eq!(thread["method"], "thread/resume");
        assert_eq!(thread["params"]["threadId"], "runner-thread");
        assert_eq!(thread["params"]["approvalsReviewer"], "user");

        let actions = protocol
            .handle_line(
                r#"{"id":"aviary-thread","result":{"thread":{"id":"authoritative"},"model":"m","cwd":"/tmp"}}"#,
            )
            .unwrap();
        let ProtocolAction::Send(turn) = &actions[2] else {
            panic!("expected turn request");
        };
        assert_eq!(turn["method"], "turn/start");
        assert_eq!(turn["params"]["approvalsReviewer"], "user");
        assert_eq!(turn["params"]["input"][0]["text"], "prompt only on stdin");
    }

    #[test]
    fn command_approval_supports_once_session_decline_and_cancel() {
        let mut protocol = protocol(None);
        let actions = protocol
            .server_request(
                "item/commandExecution/requestApproval",
                &json!({
                    "id": 7,
                    "params": {"command": "git status", "availableDecisions": ["accept", "acceptForSession", "decline", "cancel"]}
                }),
            )
            .unwrap();
        let ProtocolAction::Permission(prompt) = actions.into_iter().next().unwrap() else {
            panic!("expected permission");
        };
        assert_eq!(
            prompt.options,
            ["allow-once", "allow-session", "deny", "cancel"]
        );
        let response = protocol
            .permission_response(
                prompt.payload,
                &PermissionReply {
                    decision: PermissionDecision::AllowSession,
                    updated_input: None,
                    message: None,
                    answers: None,
                    content: None,
                },
            )
            .unwrap();
        assert_eq!(
            response.line,
            json!({"id": 7, "result": {"decision": "acceptForSession"}})
        );
    }

    #[test]
    fn permissions_deny_is_an_empty_grant_and_cancel_interrupts() {
        let mut protocol = protocol(None);
        protocol.thread_id = Some("thread".to_string());
        protocol.turn_id = Some("turn".to_string());
        let reply = protocol
            .permission_response(
                PendingPayload::CodexPermissions {
                    rpc_id: json!("p"),
                    permissions: json!({"network": {"enabled": true}}),
                },
                &PermissionReply {
                    decision: PermissionDecision::Cancel,
                    updated_input: None,
                    message: None,
                    answers: None,
                    content: None,
                },
            )
            .unwrap();
        assert_eq!(reply.line["result"]["permissions"], json!({}));
        assert!(reply.interrupt);
        let interrupt = protocol.interrupt_message().unwrap();
        assert_eq!(interrupt["method"], "turn/interrupt");
        assert_eq!(interrupt["params"]["threadId"], "thread");
        assert_eq!(interrupt["params"]["turnId"], "turn");
    }

    #[test]
    fn user_input_persists_a_typed_form_and_validates_exact_answers() {
        let mut protocol = protocol(None);
        let actions = protocol
            .server_request(
                "item/tool/requestUserInput",
                &json!({
                    "id": "question-rpc",
                    "params": {
                        "questions": [{
                            "id": "choice",
                            "header": "Choose",
                            "question": "Which path?",
                            "isOther": true,
                            "isSecret": false,
                            "options": [{"label": "Safe", "description": "Stay read-only"}]
                        }]
                    }
                }),
            )
            .unwrap();
        let ProtocolAction::Permission(prompt) = actions.into_iter().next().unwrap() else {
            panic!("expected permission");
        };
        let Some(PermissionPromptData::Questions { questions }) = &prompt.prompt else {
            panic!("expected typed questions");
        };
        assert_eq!(questions[0].id, "choice");
        assert!(questions[0].is_other);
        let response = protocol
            .permission_response(
                prompt.payload,
                &PermissionReply {
                    decision: PermissionDecision::Submit,
                    updated_input: None,
                    message: None,
                    answers: Some(json!({"choice": {"answers": ["Safe"]}})),
                    content: None,
                },
            )
            .unwrap();
        assert_eq!(
            response.line,
            json!({
                "id": "question-rpc",
                "result": {"answers": {"choice": {"answers": ["Safe"]}}}
            })
        );
    }

    #[test]
    fn mcp_elicitation_is_explicitly_non_submittable() {
        let mut protocol = protocol(None);
        let actions = protocol
            .server_request(
                "mcpServer/elicitation/request",
                &json!({"id": 9, "params": {"serverName": "example"}}),
            )
            .unwrap();
        let ProtocolAction::Permission(prompt) = actions.into_iter().next().unwrap() else {
            panic!("expected permission");
        };
        assert_eq!(prompt.options, ["deny", "cancel"]);
        assert!(matches!(
            prompt.prompt,
            Some(PermissionPromptData::Unsupported { .. })
        ));
        assert!(protocol
            .permission_response(
                prompt.payload,
                &PermissionReply {
                    decision: PermissionDecision::Submit,
                    updated_input: None,
                    message: None,
                    answers: None,
                    content: Some(json!({})),
                },
            )
            .is_err());
    }

    #[test]
    fn fallback_prompt_is_stdin_and_safety_is_forced() {
        let (_, launch) = build_fallback(&queued(None)).unwrap();
        let args = launch
            .command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(!args.iter().any(|arg| arg.contains("prompt only")));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--sandbox", "read-only"]));
        assert!(args.iter().any(|arg| arg == "approval_policy=\"never\""));
        assert_eq!(launch.initial_lines, ["prompt only on stdin"]);

        let (_, resumed) = build_fallback(&queued(Some("--runner-thread"))).unwrap();
        let args = resumed
            .command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args
            .iter()
            .any(|arg| arg == "model_reasoning_effort=\"high\""));
        let marker = args.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(args[marker + 1], "--runner-thread");
    }

    #[test]
    fn fallback_model_and_effort_cannot_inject_options_or_toml_keys() {
        for resume in [None, Some("runner-thread")] {
            let mut queued = queued(resume);
            queued.turn.requested_model =
                Some("--dangerously-bypass-approvals-and-sandbox".to_string());
            let injected_effort = "high\"\napproval_policy=\"on-request";
            queued.turn.requested_effort = Some(injected_effort.to_string());
            let (_, launch) = build_fallback(&queued).unwrap();
            let args = launch
                .command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>();

            assert!(args
                .iter()
                .any(|arg| { arg == "--model=--dangerously-bypass-approvals-and-sandbox" }));
            assert!(!args
                .iter()
                .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox"));
            let override_arg = args
                .iter()
                .find(|arg| arg.starts_with("model_reasoning_effort="))
                .unwrap();
            assert!(!override_arg.contains('\n'));
            let parsed: toml::Value = toml::from_str(override_arg).unwrap();
            assert_eq!(
                parsed["model_reasoning_effort"].as_str(),
                Some(injected_effort)
            );
            assert!(parsed.get("approval_policy").is_none());
        }
    }

    #[test]
    fn fallback_completed_agent_message_is_not_dropped() {
        let mut protocol = ExecProtocol::default();
        let actions = protocol
            .handle_line(
                r#"{"type":"item.completed","item":{"id":"message","type":"agent_message","text":"Done"}}"#,
            )
            .unwrap();
        assert!(matches!(
            actions.as_slice(),
            [ProtocolAction::Event(SessionEvent::Text { text })] if text == "Done"
        ));
    }
}
