//! Drives the runners' own CLIs and streams their events to the UI.
//!
//! Chat does not reimplement the agent loop. It spawns
//! `claude -p --output-format stream-json` (or `codex exec --json`) and renders
//! the NDJSON they emit, which is why tools, MCP, skills, permissions and
//! session resume all work without being rebuilt here — and why a skill edited
//! in the Library applies on the very next turn.
//!
//! Permissions: this build of the Claude CLI exposes no per-call approval hook
//! a host can drive, only `--permission-mode`. So the mode is the control. It
//! is passed explicitly on every run, defaults to `plan` (read-only), and is
//! never remembered — a permissive choice cannot silently persist into a later
//! session.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use tauri::ipc::Channel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Runner {
    ClaudeCode,
    Codex,
}

/// Mirrors `--permission-mode`. Deliberately explicit: there is no `Default`
/// impl, so a caller cannot omit it and get whatever the CLI happens to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    Plan,
    Manual,
    AcceptEdits,
    Auto,
    DontAsk,
    BypassPermissions,
}

impl PermissionMode {
    fn as_flag(&self) -> &'static str {
        match self {
            PermissionMode::Plan => "plan",
            PermissionMode::Manual => "manual",
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Auto => "auto",
            PermissionMode::DontAsk => "dontAsk",
            PermissionMode::BypassPermissions => "bypassPermissions",
        }
    }
}

/// One line of the stream, normalised so the UI never branches on runner.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Event {
    /// Session metadata from the CLI's own init line.
    Started {
        session_id: String,
        model: String,
        cwd: String,
        tools: usize,
        mcp_servers: usize,
        permission_mode: String,
    },
    /// Assistant prose.
    Text { text: String },
    /// A tool the agent decided to call.
    ToolCall { name: String, summary: String },
    /// Anything structured we do not model yet, kept so nothing is silently lost.
    Raw { line_type: String, json: String },
    Finished {
        is_error: bool,
        duration_ms: u64,
    },
    Failed { message: String },
}

fn summarise_tool_input(input: &serde_json::Value) -> String {
    for key in ["file_path", "path", "command", "pattern", "url", "query"] {
        if let Some(v) = input.get(key).and_then(|v| v.as_str()) {
            let v = v.replace(
                &dirs::home_dir()
                    .map(|h| h.to_string_lossy().to_string())
                    .unwrap_or_default(),
                "~",
            );
            return if v.len() > 90 {
                format!("{}…", &v[..90])
            } else {
                v
            };
        }
    }
    String::new()
}

/// Translates one NDJSON line into zero or more UI events.
fn translate(line: &str) -> Vec<Event> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match ty {
        "system" if v.get("subtype").and_then(|s| s.as_str()) == Some("init") => {
            vec![Event::Started {
                session_id: v["session_id"].as_str().unwrap_or_default().to_string(),
                model: v["model"].as_str().unwrap_or_default().to_string(),
                cwd: v["cwd"].as_str().unwrap_or_default().to_string(),
                tools: v["tools"].as_array().map(|a| a.len()).unwrap_or(0),
                mcp_servers: v["mcp_servers"].as_array().map(|a| a.len()).unwrap_or(0),
                permission_mode: v["permissionMode"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            }]
        }
        "assistant" => {
            let mut out = Vec::new();
            if let Some(blocks) = v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                for b in blocks {
                    match b.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                                if !t.trim().is_empty() {
                                    out.push(Event::Text { text: t.to_string() });
                                }
                            }
                        }
                        Some("tool_use") => out.push(Event::ToolCall {
                            name: b
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("tool")
                                .to_string(),
                            summary: b
                                .get("input")
                                .map(summarise_tool_input)
                                .unwrap_or_default(),
                        }),
                        _ => {}
                    }
                }
            }
            out
        }
        "result" => vec![Event::Finished {
            is_error: v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false),
            duration_ms: v.get("duration_ms").and_then(|d| d.as_u64()).unwrap_or(0),
        }],
        // Hook noise and rate-limit notices are not worth surfacing as chat.
        "system" | "rate_limit_event" => Vec::new(),
        other => vec![Event::Raw {
            line_type: other.to_string(),
            json: line.chars().take(400).collect(),
        }],
    }
}

/// Runs one turn, streaming events to the frontend as they arrive.
pub fn run_turn(
    runner: Runner,
    prompt: String,
    cwd: Option<String>,
    mode: PermissionMode,
    model: Option<String>,
    effort: Option<String>,
    channel: Channel<Event>,
) -> Result<(), String> {
    let mut cmd = match runner {
        Runner::ClaudeCode => {
            let mut c = Command::new("claude");
            c.arg("-p")
                .arg(&prompt)
                .arg("--output-format")
                .arg("stream-json")
                .arg("--verbose")
                .arg("--permission-mode")
                .arg(mode.as_flag());
            // Omitted entirely when unset, so the CLI keeps the user's default
            // rather than Aviary silently pinning one.
            if let Some(m) = model.as_deref() {
                c.arg("--model").arg(m);
            }
            if let Some(e) = effort.as_deref() {
                c.arg("--effort").arg(e);
            }
            c
        }
        Runner::Codex => {
            let mut c = Command::new("codex");
            c.arg("exec").arg("--json");
            if let Some(m) = model.as_deref() {
                c.arg("-m").arg(m);
            }
            // Codex takes effort as a config override rather than a flag.
            if let Some(e) = effort.as_deref() {
                c.arg("-c").arg(format!("model_reasoning_effort=\"{e}\""));
            }
            // Codex has no equivalent mode flag; the read-only sandbox is the
            // closest analogue to plan.
            if matches!(mode, PermissionMode::Plan) {
                c.arg("-s").arg("read-only");
            }
            // Prompt is positional, so every flag must precede it.
            c.arg(&prompt);
            c
        }
    };

    if let Some(dir) = cwd.as_ref().filter(|d| std::path::Path::new(d).is_dir()) {
        cmd.current_dir(dir);
    }

    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", match runner {
            Runner::ClaudeCode => "claude",
            Runner::Codex => "codex",
        }))?;

    let stdout = child.stdout.take().ok_or("no stdout")?;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        for ev in translate(&line) {
            let _ = channel.send(ev);
        }
    }

    match child.wait() {
        Ok(status) if !status.success() => {
            let msg = child
                .stderr
                .take()
                .map(|e| {
                    BufReader::new(e)
                        .lines()
                        .map_while(Result::ok)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            let _ = channel.send(Event::Failed {
                message: if msg.trim().is_empty() {
                    format!("exited with {status}")
                } else {
                    msg.chars().take(500).collect()
                },
            });
        }
        Err(e) => {
            let _ = channel.send(Event::Failed {
                message: e.to_string(),
            });
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_the_real_schema() {
        // Shapes captured from an actual `claude -p --output-format stream-json` run.
        let init = r#"{"type":"system","subtype":"init","cwd":"/tmp","session_id":"abc","tools":["Read","Bash"],"mcp_servers":[{"name":"figma"}],"model":"claude-opus-4","permissionMode":"plan"}"#;
        match &translate(init)[0] {
            Event::Started { model, tools, mcp_servers, permission_mode, .. } => {
                assert_eq!(model, "claude-opus-4");
                assert_eq!(*tools, 2);
                assert_eq!(*mcp_servers, 1);
                assert_eq!(permission_mode, "plan");
            }
            e => panic!("expected Started, got {e:?}"),
        }

        let assistant = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking now."},{"type":"tool_use","name":"Read","input":{"file_path":"/tmp/a.md"}}]}}"#;
        let evs = translate(assistant);
        assert!(matches!(evs[0], Event::Text { .. }));
        match &evs[1] {
            Event::ToolCall { name, summary } => {
                assert_eq!(name, "Read");
                assert_eq!(summary, "/tmp/a.md");
            }
            e => panic!("expected ToolCall, got {e:?}"),
        }

        let result = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1234}"#;
        assert!(matches!(translate(result)[0], Event::Finished { is_error: false, duration_ms: 1234 }));

        // Hook chatter must not reach the transcript.
        assert!(translate(r#"{"type":"system","subtype":"hook_started"}"#).is_empty());
        assert!(translate(r#"{"type":"rate_limit_event"}"#).is_empty());

        // Malformed lines are dropped, not fatal.
        assert!(translate("not json").is_empty());
    }
}
