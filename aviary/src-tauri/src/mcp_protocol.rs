//! Bounded MCP JSON-RPC lifecycle shared by Aviary's local servers.
//!
//! Stdout is protocol-only and every request is a single UTF-8 JSON line. The
//! runtime owns framing, initialization, ids, pagination and error semantics so
//! individual servers only describe and execute their read-only tools.

use serde::Serialize;
use serde_json::{json, Map, Value};
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub const PROTOCOL_VERSION: &str = "2025-11-25";
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const TOOL_PAGE_SIZE: usize = 50;
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &["2024-11-05", "2025-03-26", "2025-06-18", PROTOCOL_VERSION];

/// Exact process metadata for registering one of Aviary's bundled MCP
/// servers. The frontend receives semantic arguments rather than guessing an
/// installation path or constructing a shell command in the backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRegistration {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

pub fn media_registration(collection_id: Option<i64>) -> Result<McpRegistration, String> {
    let args = match collection_id {
        Some(id) if id > 0 => vec!["--collection".to_string(), id.to_string()],
        Some(_) => return Err("collection id must be a positive integer".into()),
        None => Vec::new(),
    };
    registration_for_current_executable("aviary-media", args)
}

pub fn library_registration() -> Result<McpRegistration, String> {
    registration_for_current_executable("aviary-library", Vec::new())
}

fn registration_for_current_executable(
    binary_name: &str,
    args: Vec<String>,
) -> Result<McpRegistration, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate the Aviary executable: {error}"))?;
    registration_beside(&executable, binary_name, args)
}

fn registration_beside(
    executable: &Path,
    binary_name: &str,
    args: Vec<String>,
) -> Result<McpRegistration, String> {
    if !executable.is_absolute() || !matches!(binary_name, "aviary-media" | "aviary-library") {
        return Err("invalid bundled MCP server location".into());
    }
    let parent = executable
        .parent()
        .ok_or("the Aviary executable has no parent directory")?;
    let command = parent.join(binary_name);
    let metadata = fs::symlink_metadata(&command)
        .map_err(|_| format!("the bundled {binary_name} server is unavailable"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "the bundled {binary_name} server is not a regular file"
        ));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(format!(
            "the bundled {binary_name} server is not executable"
        ));
    }
    let command = command
        .to_str()
        .ok_or("the bundled MCP server path is not valid UTF-8")?
        .to_string();
    Ok(McpRegistration {
        name: binary_name.to_string(),
        command,
        args,
    })
}

pub trait ToolServer {
    fn name(&self) -> &'static str;

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn instructions(&self) -> Option<&'static str> {
        None
    }

    fn tools(&self) -> Vec<Value>;

    fn call(&self, name: &str, arguments: &Map<String, Value>) -> Result<ToolResponse, ToolError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResponse {
    pub structured: Value,
    pub text: String,
}

impl ToolResponse {
    pub fn new(structured: Value, text: impl Into<String>) -> Self {
        Self {
            structured,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    UnknownTool(String),
    InvalidArguments(String),
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Uninitialized,
    AwaitingInitialized,
    Ready,
}

pub struct Runtime<S> {
    server: S,
    phase: Phase,
}

impl<S: ToolServer> Runtime<S> {
    pub fn new(server: S) -> Self {
        Self {
            server,
            phase: Phase::Uninitialized,
        }
    }

    /// Handles one line. Notifications deliberately return no response.
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        if line.len() > MAX_REQUEST_BYTES {
            return Some(error_response(
                Value::Null,
                -32600,
                "request exceeds the 1 MiB limit",
            ));
        }
        let request: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                return Some(error_response(
                    Value::Null,
                    -32700,
                    &format!("parse error: {error}"),
                ))
            }
        };
        let Some(object) = request.as_object() else {
            return Some(error_response(
                Value::Null,
                -32600,
                "JSON-RPC batches and non-object requests are not supported",
            ));
        };
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(error_response(
                valid_id(object.get("id")).unwrap_or(Value::Null),
                -32600,
                "jsonrpc must be \"2.0\"",
            ));
        }
        let method = match object.get("method").and_then(Value::as_str) {
            Some(method) if !method.is_empty() => method,
            _ => {
                return Some(error_response(
                    valid_id(object.get("id")).unwrap_or(Value::Null),
                    -32600,
                    "method must be a non-empty string",
                ))
            }
        };

        let id = match object.get("id") {
            None => {
                self.handle_notification(method);
                return None;
            }
            Some(Value::Null) => {
                return Some(error_response(
                    Value::Null,
                    -32600,
                    "request id cannot be null",
                ))
            }
            Some(value) => match valid_id(Some(value)) {
                Some(id) => id,
                None => {
                    return Some(error_response(
                        Value::Null,
                        -32600,
                        "request id must be a string or integer",
                    ))
                }
            },
        };

        let response = match method {
            "initialize" => self.initialize(object, id.clone()),
            "tools/list" => {
                self.require_ready(id.clone(), |runtime| runtime.list_tools(object, id.clone()))
            }
            "tools/call" => {
                self.require_ready(id.clone(), |runtime| runtime.call_tool(object, id.clone()))
            }
            "ping" => self.require_ready(id.clone(), |_| success_response(id.clone(), json!({}))),
            _ => error_response(id, -32601, &format!("unknown method: {method}")),
        };
        Some(bound_response(response))
    }

    fn handle_notification(&mut self, method: &str) {
        if method == "notifications/initialized" && self.phase == Phase::AwaitingInitialized {
            self.phase = Phase::Ready;
        }
    }

    fn initialize(&mut self, request: &Map<String, Value>, id: Value) -> String {
        if self.phase != Phase::Uninitialized {
            return error_response(id, -32600, "server is already initialized");
        }
        let params = match request.get("params").and_then(Value::as_object) {
            Some(params) => params,
            None => return error_response(id, -32602, "initialize params are required"),
        };
        let requested = match params.get("protocolVersion").and_then(Value::as_str) {
            Some(version) => version,
            None => return error_response(id, -32602, "protocolVersion is required"),
        };
        let negotiated = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
            requested
        } else {
            PROTOCOL_VERSION
        };
        self.phase = Phase::AwaitingInitialized;
        let mut result = json!({
            "protocolVersion": negotiated,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": self.server.name(),
                "version": self.server.version()
            }
        });
        if let Some(instructions) = self.server.instructions() {
            result["instructions"] = Value::String(instructions.to_string());
        }
        success_response(id, result)
    }

    fn require_ready(&mut self, id: Value, operation: impl FnOnce(&mut Self) -> String) -> String {
        if self.phase != Phase::Ready {
            return error_response(id, -32002, "server initialization is not complete");
        }
        operation(self)
    }

    fn list_tools(&self, request: &Map<String, Value>, id: Value) -> String {
        let params = match optional_object(request.get("params")) {
            Ok(params) => params,
            Err(message) => return error_response(id, -32602, &message),
        };
        if let Some(extra) = first_extra_key(params, &["cursor", "_meta"]) {
            return error_response(id, -32602, &format!("unknown tools/list field: {extra}"));
        }
        let offset = match params.get("cursor") {
            None => 0,
            Some(Value::String(cursor)) => match cursor.parse::<usize>() {
                Ok(offset) => offset,
                Err(_) => return error_response(id, -32602, "cursor is invalid"),
            },
            Some(_) => return error_response(id, -32602, "cursor must be a string"),
        };
        let tools = self.server.tools();
        if offset > tools.len() {
            return error_response(id, -32602, "cursor is out of range");
        }
        let end = (offset + TOOL_PAGE_SIZE).min(tools.len());
        let mut result = json!({ "tools": tools[offset..end] });
        if end < tools.len() {
            result["nextCursor"] = Value::String(end.to_string());
        }
        success_response(id, result)
    }

    fn call_tool(&self, request: &Map<String, Value>, id: Value) -> String {
        let params = match request.get("params").and_then(Value::as_object) {
            Some(params) => params,
            None => return error_response(id, -32602, "tools/call params are required"),
        };
        if let Some(extra) = first_extra_key(params, &["name", "arguments", "_meta"]) {
            return error_response(id, -32602, &format!("unknown tools/call field: {extra}"));
        }
        let name = match params.get("name").and_then(Value::as_str) {
            Some(name) if !name.is_empty() => name,
            _ => return error_response(id, -32602, "tool name is required"),
        };
        let empty = Map::new();
        let arguments = match params.get("arguments") {
            None => &empty,
            Some(Value::Object(arguments)) => arguments,
            Some(_) => return error_response(id, -32602, "tool arguments must be an object"),
        };
        match self.server.call(name, arguments) {
            Ok(output) => success_response(
                id,
                json!({
                    "content": [{ "type": "text", "text": output.text }],
                    "structuredContent": output.structured,
                    "isError": false
                }),
            ),
            Err(ToolError::UnknownTool(tool)) => {
                error_response(id, -32602, &format!("unknown tool: {tool}"))
            }
            Err(ToolError::InvalidArguments(message) | ToolError::Failed(message)) => {
                success_response(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": message }],
                        "isError": true
                    }),
                )
            }
        }
    }
}

pub fn serve_stdio<S: ToolServer>(server: S) -> io::Result<()> {
    serve(
        server,
        io::BufReader::new(io::stdin().lock()),
        io::BufWriter::new(io::stdout().lock()),
    )
}

pub fn serve<S: ToolServer, R: BufRead, W: Write>(
    server: S,
    mut reader: R,
    mut writer: W,
) -> io::Result<()> {
    let mut runtime = Runtime::new(server);
    loop {
        match read_bounded_line(&mut reader)? {
            BoundedLine::Eof => return Ok(()),
            BoundedLine::TooLong => {
                writeln!(
                    writer,
                    "{}",
                    error_response(Value::Null, -32600, "request exceeds the 1 MiB limit")
                )?;
                writer.flush()?;
            }
            BoundedLine::Bytes(bytes) => {
                if bytes.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                let response = match std::str::from_utf8(&bytes) {
                    Ok(line) => runtime.handle_line(line),
                    Err(_) => Some(error_response(
                        Value::Null,
                        -32700,
                        "request is not valid UTF-8",
                    )),
                };
                if let Some(response) = response {
                    writeln!(writer, "{response}")?;
                    writer.flush()?;
                }
            }
        }
    }
}

enum BoundedLine {
    Eof,
    TooLong,
    Bytes(Vec<u8>),
}

fn read_bounded_line(reader: &mut impl BufRead) -> io::Result<BoundedLine> {
    let mut bytes = Vec::new();
    let mut too_long = false;
    let mut saw_data = false;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            if !saw_data {
                return Ok(BoundedLine::Eof);
            }
            break;
        }
        saw_data = true;
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(buffer.len(), |position| position + 1);
        let content_len = newline.unwrap_or(buffer.len());
        if !too_long {
            if bytes.len() + content_len > MAX_REQUEST_BYTES {
                too_long = true;
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
    if too_long {
        return Ok(BoundedLine::TooLong);
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    Ok(BoundedLine::Bytes(bytes))
}

fn valid_id(id: Option<&Value>) -> Option<Value> {
    match id? {
        Value::String(value) => Some(Value::String(value.clone())),
        Value::Number(value) if value.as_i64().is_some() || value.as_u64().is_some() => {
            Some(Value::Number(value.clone()))
        }
        _ => None,
    }
}

fn optional_object(value: Option<&Value>) -> Result<&Map<String, Value>, String> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    match value {
        None => Ok(EMPTY.get_or_init(Map::new)),
        Some(Value::Object(object)) => Ok(object),
        Some(_) => Err("params must be an object".into()),
    }
}

pub fn first_extra_key<'a>(object: &'a Map<String, Value>, allowed: &[&str]) -> Option<&'a str> {
    object
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
        .map(String::as_str)
}

fn success_response(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error_response(id: Value, code: i64, message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
    .to_string()
}

fn bound_response(response: String) -> String {
    if response.len() <= MAX_RESPONSE_BYTES {
        response
    } else {
        error_response(Value::Null, -32603, "response exceeds the 2 MiB limit")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::{symlink, PermissionsExt};

    struct TestServer {
        tool_count: usize,
    }

    impl ToolServer for TestServer {
        fn name(&self) -> &'static str {
            "test-server"
        }

        fn tools(&self) -> Vec<Value> {
            (0..self.tool_count)
                .map(|index| {
                    json!({
                        "name": format!("tool-{index}"),
                        "description": "fixture",
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                            "additionalProperties": false
                        }
                    })
                })
                .collect()
        }

        fn call(
            &self,
            name: &str,
            arguments: &Map<String, Value>,
        ) -> Result<ToolResponse, ToolError> {
            match name {
                "tool-0" if arguments.contains_key("bad") => {
                    Err(ToolError::InvalidArguments("bad fixture argument".into()))
                }
                "tool-0" => Ok(ToolResponse::new(json!({ "ok": true }), "ok")),
                other => Err(ToolError::UnknownTool(other.into())),
            }
        }
    }

    fn request(runtime: &mut Runtime<TestServer>, value: Value) -> Value {
        serde_json::from_str(&runtime.handle_line(&value.to_string()).unwrap()).unwrap()
    }

    fn ready_runtime(tool_count: usize) -> Runtime<TestServer> {
        let mut runtime = Runtime::new(TestServer { tool_count });
        let init = request(
            &mut runtime,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": PROTOCOL_VERSION }
            }),
        );
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(runtime
            .handle_line(
                &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string()
            )
            .is_none());
        runtime
    }

    #[test]
    fn lifecycle_blocks_tools_until_initialized_notification() {
        let mut runtime = Runtime::new(TestServer { tool_count: 1 });
        let before = request(
            &mut runtime,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        );
        assert_eq!(before["error"]["code"], -32002);
        request(
            &mut runtime,
            json!({
                "jsonrpc": "2.0", "id": 2, "method": "initialize",
                "params": { "protocolVersion": "2025-06-18" }
            }),
        );
        let waiting = request(
            &mut runtime,
            json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
        );
        assert_eq!(waiting["error"]["code"], -32002);
        assert!(runtime
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none());
        let ready = request(
            &mut runtime,
            json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/list" }),
        );
        assert_eq!(ready["result"]["tools"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn supported_versions_echo_and_unknown_versions_offer_current() {
        let mut supported = Runtime::new(TestServer { tool_count: 0 });
        let response = request(
            &mut supported,
            json!({
                "jsonrpc": "2.0", "id": "old", "method": "initialize",
                "params": { "protocolVersion": "2024-11-05" }
            }),
        );
        assert_eq!(response["id"], "old");
        assert_eq!(response["result"]["protocolVersion"], "2024-11-05");

        let mut unknown = Runtime::new(TestServer { tool_count: 0 });
        let response = request(
            &mut unknown,
            json!({
                "jsonrpc": "2.0", "id": 8, "method": "initialize",
                "params": { "protocolVersion": "future-version" }
            }),
        );
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    #[test]
    fn ids_batches_and_malformed_requests_fail_closed() {
        let mut runtime = Runtime::new(TestServer { tool_count: 0 });
        let null_id: Value = serde_json::from_str(
            &runtime
                .handle_line(r#"{"jsonrpc":"2.0","id":null,"method":"initialize"}"#)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(null_id["error"]["code"], -32600);
        let batch: Value = serde_json::from_str(&runtime.handle_line("[]").unwrap()).unwrap();
        assert_eq!(batch["error"]["code"], -32600);
        let malformed: Value = serde_json::from_str(&runtime.handle_line("{bad").unwrap()).unwrap();
        assert_eq!(malformed["error"]["code"], -32700);
    }

    #[test]
    fn tool_results_have_structured_and_text_compatibility() {
        let mut runtime = ready_runtime(1);
        let success = request(
            &mut runtime,
            json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "tool-0", "arguments": {} }
            }),
        );
        assert_eq!(success["result"]["structuredContent"]["ok"], true);
        assert_eq!(success["result"]["content"][0]["text"], "ok");
        assert_eq!(success["result"]["isError"], false);

        let failure = request(
            &mut runtime,
            json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "tool-0", "arguments": { "bad": true } }
            }),
        );
        assert_eq!(failure["result"]["isError"], true);
        let unknown = request(
            &mut runtime,
            json!({
                "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                "params": { "name": "missing", "arguments": {} }
            }),
        );
        assert_eq!(unknown["error"]["code"], -32602);
    }

    #[test]
    fn tool_listing_is_cursor_paginated() {
        let mut runtime = ready_runtime(55);
        let first = request(
            &mut runtime,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        );
        assert_eq!(first["result"]["tools"].as_array().unwrap().len(), 50);
        assert_eq!(first["result"]["nextCursor"], "50");
        let second = request(
            &mut runtime,
            json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/list",
                "params": { "cursor": "50" }
            }),
        );
        assert_eq!(second["result"]["tools"].as_array().unwrap().len(), 5);
        assert!(second["result"].get("nextCursor").is_none());
    }

    #[test]
    fn framing_discards_oversized_lines_and_continues() {
        let oversized = "x".repeat(MAX_REQUEST_BYTES + 1);
        let input = format!(
            "{oversized}\n{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\"protocolVersion\":\"{PROTOCOL_VERSION}\"}}}}\n"
        );
        let mut output = Vec::new();
        serve(
            TestServer { tool_count: 0 },
            io::Cursor::new(input.into_bytes()),
            &mut output,
        )
        .unwrap();
        let lines = String::from_utf8(output).unwrap();
        let lines = lines.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        let first: Value = serde_json::from_str(lines[0]).unwrap();
        let second: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["error"]["code"], -32600);
        assert_eq!(second["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    #[cfg(unix)]
    #[test]
    fn registration_uses_only_verified_allowlisted_siblings() {
        let temporary = tempfile::tempdir().unwrap();
        let executable = temporary.path().join("aviary");
        fs::write(&executable, b"app").unwrap();
        for binary in ["aviary-media", "aviary-library"] {
            let path = temporary.path().join(binary);
            fs::write(&path, b"server").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let registration = registration_beside(
            &executable,
            "aviary-media",
            vec!["--collection".into(), "7".into()],
        )
        .unwrap();
        assert_eq!(registration.name, "aviary-media");
        assert_eq!(
            registration.command,
            temporary.path().join("aviary-media").to_str().unwrap()
        );
        assert_eq!(registration.args, ["--collection", "7"]);
        assert!(registration_beside(&executable, "arbitrary-program", Vec::new()).is_err());

        let target = temporary.path().join("target");
        fs::write(&target, b"server").unwrap();
        let library = temporary.path().join("aviary-library");
        fs::remove_file(&library).unwrap();
        symlink(&target, &library).unwrap();
        assert!(registration_beside(&executable, "aviary-library", Vec::new()).is_err());
    }

    #[test]
    fn media_registration_rejects_non_positive_collection_ids_before_lookup() {
        assert!(media_registration(Some(0)).is_err());
        assert!(media_registration(Some(-1)).is_err());
    }
}
