//! JSON-RPC handling for the `aviary-media` MCP server.
//!
//! Kept in the library rather than the binary so it is testable without
//! spawning a process — the protocol handling is where the bugs live, and a
//! stdio round-trip is a poor test harness.
//!
//! Implements the minimum of MCP that a client needs: `initialize`,
//! `tools/list`, `tools/call`. Read-only by construction: no tool here mutates
//! the board, so an agent can never delete a designer's references.

use crate::media;
use serde_json::{json, Value};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Handles one JSON-RPC line. Returns `None` for notifications, which take no
/// reply — answering one is a protocol error that some clients treat as fatal.
pub fn handle(line: &str) -> Option<String> {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
    };

    let id = req.get("id").cloned();
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

    // No id means notification.
    let Some(id) = id else {
        return None;
    };

    let result = match method {
        "initialize" => json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "aviary-media", "version": env!("CARGO_PKG_VERSION") }
        }),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => match call(&req) {
            Ok(v) => v,
            Err(e) => return Some(error(id, -32602, &e)),
        },
        "ping" => json!({}),
        _ => return Some(error(id, -32601, &format!("unknown method: {method}"))),
    };

    Some(
        json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
    )
}

fn error(id: Value, code: i64, message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
    .to_string()
}

fn tools() -> Value {
    json!([
        {
            "name": "search_media",
            "description": "Search the Aviary media board by keyword. Matches titles, notes, \
                            original filenames and tags (including auto-derived colour and \
                            orientation tags such as 'teal', 'dark', 'landscape'). Returns \
                            absolute file paths that can be read directly.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Keywords, e.g. 'grainy teal gradient'" },
                    "limit": { "type": "integer", "description": "Max results (default 20)" }
                },
                "required": ["query"]
            }
        },
        {
            "name": "get_media",
            "description": "Fetch one media item by its hash, including its absolute path, \
                            dimensions, dominant colour and tags.",
            "inputSchema": {
                "type": "object",
                "properties": { "hash": { "type": "string" } },
                "required": ["hash"]
            }
        },
        {
            "name": "list_collections",
            "description": "List the collections on the media board with their item counts.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

fn call(req: &Value) -> Result<Value, String> {
    let params = req.get("params").ok_or("missing params")?;
    let name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or("missing tool name")?;
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

    let text = match name {
        "search_media" => {
            let query = args.get("query").and_then(|q| q.as_str()).unwrap_or("");
            let limit = args
                .get("limit")
                .and_then(|l| l.as_u64())
                .unwrap_or(20)
                .min(100) as usize;
            let hits = media::search(query, limit);
            if hits.is_empty() {
                format!("No media matched {query:?}.")
            } else {
                let lines: Vec<String> = hits.iter().map(describe).collect();
                format!("{} result(s):\n\n{}", hits.len(), lines.join("\n\n"))
            }
        }
        "get_media" => {
            let hash = args
                .get("hash")
                .and_then(|h| h.as_str())
                .ok_or("missing hash")?;
            match media::get(hash) {
                Some(item) => describe(&item),
                None => format!("No media with hash {hash}."),
            }
        }
        "list_collections" => {
            let cols = media::collections();
            if cols.is_empty() {
                "No collections yet.".to_string()
            } else {
                cols.iter()
                    .map(|c| format!("{} ({} items)", c.name, c.count))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        other => return Err(format!("unknown tool: {other}")),
    };

    Ok(json!({ "content": [{ "type": "text", "text": text }] }))
}

/// One item rendered for an agent: the path first, because that is what the
/// caller actually needs to act on.
fn describe(item: &media::MediaItem) -> String {
    let mut out = format!("path: {}\n", item.path);
    if let Some(t) = &item.title {
        out.push_str(&format!("title: {t}\n"));
    }
    if let (Some(w), Some(h)) = (item.width, item.height) {
        out.push_str(&format!("size: {w}x{h}\n"));
    }
    if let Some(c) = &item.dominant {
        out.push_str(&format!("colour: {c}\n"));
    }
    if !item.tags.is_empty() {
        out.push_str(&format!("tags: {}\n", item.tags.join(", ")));
    }
    out.push_str(&format!("hash: {}", item.hash));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_get_no_reply() {
        let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert!(handle(note).is_none());
    }

    #[test]
    fn initialize_reports_tools_capability() {
        let out = handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(v["result"]["serverInfo"]["name"], "aviary-media");
        assert!(v["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn lists_the_three_tools() {
        let out = handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let names: Vec<&str> = v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["search_media", "get_media", "list_collections"]);
    }

    #[test]
    fn unknown_method_is_an_error_not_a_panic() {
        let out = handle(r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], -32601);
    }

    #[test]
    fn malformed_json_reports_a_parse_error() {
        let out = handle("{not json").unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], -32700);
    }

    #[test]
    fn unknown_tool_is_rejected() {
        let out = handle(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"rm","arguments":{}}}"#,
        )
        .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], -32602);
    }
}
