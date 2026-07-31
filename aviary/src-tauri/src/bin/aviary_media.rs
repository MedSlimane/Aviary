//! `aviary-media` — an MCP server over the media board.
//!
//! This is the inversion the design spec describes: the board is not a gallery
//! you scroll, it is a retrieval surface your agents call into. Ask any agent
//! for "that grainy teal gradient from my references" and it gets the actual
//! file path.
//!
//! It is a **separate binary** because MCP speaks JSON-RPC over stdio — the
//! desktop app cannot be the thing a bare `claude` session spawns. Both read
//! the same `~/.aviary/data.db`, and this side only ever reads, so running it
//! while Aviary is open is safe (SQLite is in WAL mode).
//!
//! Register it with:
//!   claude mcp add aviary-media -- /path/to/aviary-media

use aviary_lib::mcp_media;
use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }

        let Some(response) = mcp_media::handle(&line) else {
            // Notifications have no id and take no reply.
            continue;
        };

        if writeln!(stdout, "{response}").is_err() {
            break;
        }
        let _ = stdout.flush();
    }
}
