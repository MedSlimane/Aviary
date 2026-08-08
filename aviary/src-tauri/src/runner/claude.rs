//! Claude Code's bidirectional stream-JSON adapter.

use super::{
    sanitize, FailureKind, Launch, PendingPayload, PermissionDecision, PermissionPrompt,
    PermissionReply, PermissionWireResponse, ProtocolAction, ProtocolTerminal, QueuedTurn, Runner,
    SafetyCapabilities, SafetyOption, SelectedSafety, SessionEvent,
};
use crate::store::sessions::ToolResultStatus;
use serde_json::{json, Value};
use std::process::Command;

const PROTOCOL_NAME: &str = "claude-stream-json";

#[derive(Debug, Default)]
pub(super) struct Protocol {
    started: bool,
}

pub(super) fn discover_safety() -> SafetyCapabilities {
    let output = match Command::new("claude").arg("--help").output() {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            return unavailable(format!(
                "Claude Code capability discovery exited with {}",
                output.status
            ))
        }
        Err(error) => return unavailable(format!("Claude Code is unavailable: {error}")),
    };
    let help = String::from_utf8_lossy(&output.stdout);
    let modes = permission_modes(&help);
    if modes.is_empty() {
        return unavailable(
            "Claude Code did not advertise a permission-mode list; interactive approvals are disabled"
                .to_string(),
        );
    }
    let options = modes
        .iter()
        .map(|mode| SafetyOption {
            id: format!("claude:{mode}"),
            label: mode_label(mode),
            description: mode_description(mode),
            interactive_approvals: matches!(mode.as_str(), "manual" | "acceptEdits" | "auto"),
            dangerous: matches!(mode.as_str(), "bypassPermissions" | "dontAsk"),
            sandbox: None,
            approval_policy: Some(mode.clone()),
        })
        .collect::<Vec<_>>();
    let default_option_id = modes
        .iter()
        .find(|mode| mode.as_str() == "plan")
        .or_else(|| modes.iter().find(|mode| mode.as_str() == "manual"))
        .map(|mode| format!("claude:{mode}"));
    SafetyCapabilities {
        runner: Runner::ClaudeCode,
        available: default_option_id.is_some(),
        protocol: PROTOCOL_NAME.to_string(),
        default_option_id,
        options,
        warning: None,
    }
}

pub(super) fn select_safety(id: &str) -> Result<SelectedSafety, String> {
    let mode = id
        .strip_prefix("claude:")
        .filter(|mode| !mode.is_empty())
        .ok_or_else(|| "invalid Claude Code safety option".to_string())?;
    Ok(SelectedSafety::Claude {
        option_id: id.to_string(),
        mode: mode.to_string(),
    })
}

pub(super) fn build(
    queued: &QueuedTurn,
    mode: &str,
    resume: bool,
) -> Result<(Protocol, Launch), (FailureKind, String)> {
    let mut command = Command::new("claude");
    command
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--input-format")
        .arg("stream-json")
        .arg("--permission-mode")
        .arg(mode)
        .current_dir(&queued.session.cwd)
        // Claude uses this marker to select the supported SDK stdio transport.
        .env("CLAUDE_CODE_ENTRYPOINT", "sdk-py")
        .env_remove("CLAUDECODE");
    if let Some(session_id) = queued.session.runner_session_id.as_deref() {
        if resume {
            command.arg(format!("--resume={session_id}"));
        } else {
            command.arg(format!("--session-id={session_id}"));
        }
    }
    if let Some(model) = queued.turn.requested_model.as_deref() {
        command.arg(format!("--model={model}"));
    }
    if let Some(effort) = queued.turn.requested_effort.as_deref() {
        command.arg(format!("--effort={effort}"));
    }
    let user = json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": queued.turn.prompt,
        },
        "parent_tool_use_id": null,
        "session_id": queued.session.runner_session_id.as_deref().unwrap_or(""),
    });
    let line =
        serde_json::to_string(&user).map_err(|error| (FailureKind::Input, error.to_string()))?;
    Ok((
        Protocol::default(),
        Launch {
            command,
            initial_lines: vec![line],
            close_stdin_after_initial: false,
            // SDK stream mode accepts subsequent user frames and therefore
            // stays alive after a turn result. Each supervised process owns one
            // turn, so the authoritative result closes its process group.
            kill_after_terminal: true,
        },
    ))
}

impl Protocol {
    pub(super) fn handle_line(&mut self, line: &str) -> Result<Vec<ProtocolAction>, String> {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| format!("invalid Claude stream-JSON frame: {error}"))?;
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "system" if value.get("subtype").and_then(Value::as_str) == Some("init") => {
                self.init_actions(&value)
            }
            "assistant" => Ok(assistant_actions(&value)),
            "user" => Ok(tool_result_actions(&value)),
            "result" => Ok(result_actions(&value)),
            "control_request" => self.control_request(&value),
            "control_cancel_request" => Ok(value
                .get("request_id")
                .and_then(Value::as_str)
                .map(|id| vec![ProtocolAction::CancelPermission(id.to_string())])
                .unwrap_or_default()),
            // Partial stream events duplicate the authoritative assistant frame.
            "stream_event" | "rate_limit_event" | "prompt_suggestion" | "system" => Ok(Vec::new()),
            _ => Ok(Vec::new()),
        }
    }

    pub(super) fn permission_response(
        &mut self,
        payload: PendingPayload,
        reply: &PermissionReply,
    ) -> Result<PermissionWireResponse, String> {
        let PendingPayload::Claude { request_id, input } = payload else {
            return Err("Claude received a permission response for another protocol".to_string());
        };
        let response = match reply.decision {
            PermissionDecision::AllowOnce | PermissionDecision::Submit => json!({
                "behavior": "allow",
                "updatedInput": reply.updated_input.clone().unwrap_or(input),
            }),
            PermissionDecision::Deny => json!({
                "behavior": "deny",
                "message": reply.message.clone().unwrap_or_else(|| "Denied by user".to_string()),
            }),
            PermissionDecision::Cancel => json!({
                "behavior": "deny",
                "message": reply.message.clone().unwrap_or_else(|| "Cancelled by user".to_string()),
            }),
            PermissionDecision::AllowSession => {
                return Err(
                    "Claude Code does not advertise a session-scoped response for can_use_tool"
                        .to_string(),
                )
            }
        };
        Ok(PermissionWireResponse {
            line: json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": request_id,
                    "response": response,
                }
            }),
            interrupt: matches!(reply.decision, PermissionDecision::Cancel),
        })
    }

    fn init_actions(&mut self, value: &Value) -> Result<Vec<ProtocolAction>, String> {
        if self.started {
            return Ok(Vec::new());
        }
        let session_id = value
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "Claude init frame omitted its session id".to_string())?;
        self.started = true;
        Ok(vec![
            ProtocolAction::BindSession(session_id.to_string()),
            ProtocolAction::Event(SessionEvent::Started {
                model: string_field(value, "model"),
                cwd: string_field(value, "cwd"),
                tools: value
                    .get("tools")
                    .and_then(Value::as_array)
                    .and_then(|items| u64::try_from(items.len()).ok()),
                mcp_servers: value
                    .get("mcp_servers")
                    .and_then(Value::as_array)
                    .and_then(|items| u64::try_from(items.len()).ok()),
                permission_mode: value
                    .get("permissionMode")
                    .or_else(|| value.get("permission_mode"))
                    .and_then(Value::as_str)
                    .map(sanitize::text),
            }),
        ])
    }

    fn control_request(&self, value: &Value) -> Result<Vec<ProtocolAction>, String> {
        let request_id = value
            .get("request_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "Claude control request omitted request_id".to_string())?;
        let request = value
            .get("request")
            .and_then(Value::as_object)
            .ok_or_else(|| "Claude control request omitted request".to_string())?;
        if request.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
            return Ok(vec![ProtocolAction::Send(json!({
                "type": "control_response",
                "response": {
                    "subtype": "error",
                    "request_id": request_id,
                    "error": "Aviary does not implement this control request",
                }
            }))]);
        }
        let tool_name = request
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string();
        let input = request.get("input").cloned().unwrap_or_else(|| json!({}));
        let safe = sanitize::value(&input);
        Ok(vec![ProtocolAction::Permission(PermissionPrompt {
            wire_key: request_id.to_string(),
            tool_name,
            summary: summarise_tool_input(&safe),
            options: vec![
                "allow-once".to_string(),
                "deny".to_string(),
                "cancel".to_string(),
            ],
            prompt: None,
            payload: PendingPayload::Claude {
                request_id: request_id.to_string(),
                input,
            },
        })])
    }
}

fn assistant_actions(value: &Value) -> Vec<ProtocolAction> {
    let Some(blocks) = value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut actions = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    if !text.trim().is_empty() {
                        actions.push(ProtocolAction::Event(SessionEvent::Text {
                            text: sanitize::text(text),
                        }));
                    }
                }
            }
            Some("thinking") => {
                if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                    if !text.trim().is_empty() {
                        actions.push(ProtocolAction::Event(SessionEvent::Thinking {
                            text: sanitize::text(text),
                        }));
                    }
                }
            }
            Some("tool_use") => {
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                let safe = sanitize::value(&input);
                actions.push(ProtocolAction::Event(SessionEvent::ToolStarted {
                    call_id: block
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| format!("tool-{}", uuid::Uuid::new_v4())),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .map(sanitize::text)
                        .unwrap_or_else(|| "tool".to_string()),
                    summary: summarise_tool_input(&safe),
                    detail: Some(sanitize::compact(&safe)),
                }));
            }
            _ => {}
        }
    }
    actions
}

fn tool_result_actions(value: &Value) -> Vec<ProtocolAction> {
    let Some(blocks) = value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        .map(|block| {
            let content = block.get("content").cloned().unwrap_or(Value::Null);
            let safe = sanitize::value(&content);
            ProtocolAction::Event(SessionEvent::ToolFinished {
                call_id: block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| format!("tool-{}", uuid::Uuid::new_v4())),
                name: "tool".to_string(),
                status: if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                    ToolResultStatus::Failed
                } else {
                    ToolResultStatus::Succeeded
                },
                summary: content_summary(&safe),
                detail: Some(sanitize::compact(&safe)),
            })
        })
        .collect()
}

fn result_actions(value: &Value) -> Vec<ProtocolAction> {
    let mut actions = Vec::new();
    if let Some(usage) = value.get("usage") {
        let input = u64_field(usage, "input_tokens");
        let cached = u64_field(usage, "cache_read_input_tokens");
        let output = u64_field(usage, "output_tokens");
        let total = input
            .zip(output)
            .map(|(input, output)| input.saturating_add(output));
        actions.push(ProtocolAction::Event(SessionEvent::TokenUsage {
            input_tokens: input,
            cached_input_tokens: cached,
            output_tokens: output,
            reasoning_output_tokens: None,
            total_tokens: total,
        }));
    }
    let duration_ms = u64_field(value, "duration_ms");
    if value.get("is_error").and_then(Value::as_bool) == Some(true) {
        let message = value
            .get("result")
            .and_then(Value::as_str)
            .map(sanitize::text)
            .unwrap_or_else(|| "Claude Code reported a failed turn".to_string());
        actions.push(ProtocolAction::Terminal(ProtocolTerminal::Failed {
            message,
            duration_ms,
        }));
    } else {
        actions.push(ProtocolAction::Terminal(ProtocolTerminal::Completed {
            duration_ms,
        }));
    }
    actions
}

fn permission_modes(help: &str) -> Vec<String> {
    let Some(position) = help.find("--permission-mode") else {
        return Vec::new();
    };
    let tail = &help[position..help.len().min(position + 700)];
    let Some(choices) = tail.find("choices:") else {
        return Vec::new();
    };
    let choices = &tail[choices + "choices:".len()..];
    let choices = choices.split(')').next().unwrap_or(choices);
    choices
        .split(',')
        .map(|value| value.trim().trim_matches('"'))
        .filter(|value| !value.is_empty() && value.chars().all(|ch| ch.is_ascii_alphanumeric()))
        .map(ToOwned::to_owned)
        .collect()
}

fn mode_label(mode: &str) -> String {
    match mode {
        "plan" => "Plan (read-only)".to_string(),
        "manual" => "Ask before changes".to_string(),
        "acceptEdits" => "Accept file edits".to_string(),
        "auto" => "Automatic".to_string(),
        "dontAsk" => "Never ask".to_string(),
        "bypassPermissions" => "Bypass permissions".to_string(),
        other => other.to_string(),
    }
}

fn mode_description(mode: &str) -> String {
    match mode {
        "plan" => "Explore and plan without changing files.".to_string(),
        "manual" => "Route tool permission requests to Aviary.".to_string(),
        "acceptEdits" => "Allow edits while retaining other permission prompts.".to_string(),
        "auto" => "Use Claude Code's installed automatic permission policy.".to_string(),
        "dontAsk" => "Deny operations that require a prompt.".to_string(),
        "bypassPermissions" => "Run without Claude Code permission checks.".to_string(),
        other => format!("Claude Code permission mode {other}."),
    }
}

fn unavailable(message: String) -> SafetyCapabilities {
    SafetyCapabilities {
        runner: Runner::ClaudeCode,
        available: false,
        protocol: PROTOCOL_NAME.to_string(),
        default_option_id: None,
        options: Vec::new(),
        warning: Some(message),
    }
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(sanitize::text)
}

fn u64_field(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

fn summarise_tool_input(input: &Value) -> String {
    for key in ["file_path", "path", "command", "pattern", "url", "query"] {
        if let Some(value) = input.get(key).and_then(Value::as_str) {
            return short(value, 240);
        }
    }
    if input.as_object().is_some_and(|map| map.is_empty()) {
        String::new()
    } else {
        short(&sanitize::compact(input), 240)
    }
}

fn content_summary(value: &Value) -> String {
    match value {
        Value::String(value) => short(value, 240),
        _ => short(&sanitize::compact(value), 240),
    }
}

fn short(value: &str, max_chars: usize) -> String {
    let home = dirs::home_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let value = if home.is_empty() {
        value.to_string()
    } else {
        value.replace(&home, "~")
    };
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

    fn queued(runner_session_id: Option<&str>, ordinal: i64) -> QueuedTurn {
        QueuedTurn {
            session: ChatSession {
                id: "aviary-session".to_string(),
                runner: Runner::ClaudeCode,
                runner_session_id: runner_session_id.map(ToOwned::to_owned),
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
                session_id: "aviary-session".to_string(),
                ordinal,
                prompt: "secret prompt passed only over stdin".to_string(),
                requested_model: None,
                requested_effort: None,
                permission_mode: "claude:manual".to_string(),
                status: TurnStatus::Queued,
                failure_kind: None,
                created_at: 0,
                started_at: None,
                finished_at: None,
                duration_ms: None,
            },
        }
    }

    #[test]
    fn launch_uses_stdin_and_resume_without_print_flag() {
        let (_, launch) = build(&queued(Some("runner-session"), 2), "manual", true).unwrap();
        let args = launch
            .command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(!args.iter().any(|arg| arg == "-p" || arg == "--print"));
        assert!(!args.iter().any(|arg| arg.contains("secret prompt")));
        assert!(args.iter().any(|arg| arg == "--resume=runner-session"));
        assert!(launch.initial_lines[0].contains("secret prompt"));

        let (_, launch) = build(
            &queued(Some("--dangerously-skip-permissions"), 2),
            "manual",
            true,
        )
        .unwrap();
        let args = launch
            .command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args
            .iter()
            .any(|arg| arg == "--resume=--dangerously-skip-permissions"));
        assert!(!args
            .iter()
            .any(|arg| arg == "--dangerously-skip-permissions"));

        let (_, launch) = build(
            &queued(Some("--dangerously-skip-permissions"), 1),
            "manual",
            false,
        )
        .unwrap();
        let args = launch
            .command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args
            .iter()
            .any(|arg| arg == "--session-id=--dangerously-skip-permissions"));

        let mut injected = queued(Some("runner-session"), 2);
        injected.turn.requested_model = Some("--dangerously-skip-permissions".to_string());
        injected.turn.requested_effort = Some("--dangerously-skip-permissions".to_string());
        let (_, launch) = build(&injected, "manual", true).unwrap();
        let args = launch
            .command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args
            .iter()
            .any(|arg| arg == "--model=--dangerously-skip-permissions"));
        assert!(args
            .iter()
            .any(|arg| arg == "--effort=--dangerously-skip-permissions"));
        assert!(!args
            .iter()
            .any(|arg| arg == "--dangerously-skip-permissions"));
    }

    #[test]
    fn approval_and_cancel_frames_round_trip_exactly() {
        let mut protocol = Protocol::default();
        let actions = protocol
            .handle_line(
                r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"pwd","api_token":"no"}}}"#,
            )
            .unwrap();
        let ProtocolAction::Permission(prompt) = actions.into_iter().next().unwrap() else {
            panic!("expected permission");
        };
        assert!(!prompt.summary.contains("no"));
        let response = protocol
            .permission_response(
                prompt.payload,
                &PermissionReply {
                    decision: PermissionDecision::AllowOnce,
                    updated_input: None,
                    message: None,
                    answers: None,
                    content: None,
                },
            )
            .unwrap();
        assert_eq!(response.line["type"], "control_response");
        assert_eq!(response.line["response"]["subtype"], "success");
        assert_eq!(response.line["response"]["request_id"], "req-1");
        assert_eq!(response.line["response"]["response"]["behavior"], "allow");

        let cancelled = protocol
            .handle_line(r#"{"type":"control_cancel_request","request_id":"req-1"}"#)
            .unwrap();
        assert!(matches!(
            cancelled.as_slice(),
            [ProtocolAction::CancelPermission(id)] if id == "req-1"
        ));
    }

    #[test]
    fn discovers_modes_from_help_instead_of_a_shared_list() {
        let modes = permission_modes(
            "--permission-mode <mode> text (choices: \"manual\", \"plan\", \"futureMode\")",
        );
        assert_eq!(modes, ["manual", "plan", "futureMode"]);
    }
}
