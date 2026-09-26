//! MCP over stdio: one JSON-RPC message per line.
//!
//! **stdout is the protocol channel.** Every diagnostic goes to stderr, or the
//! first stray print desynchronises the session. The bridge is then a child
//! of the agent's client — which is fine: the application, the part that
//! restarts, is a separate process either way.

use std::io::{BufRead, Write};

use serde_json::Value;

use crate::server::Server;
use crate::wire::{Request, Response, PARSE_ERROR};

pub fn serve_stdio(server: &Server) {
    serve(server, std::io::stdin().lock(), std::io::stdout().lock());
}

/// The read → handle → write loop over any pair of streams, so it can be
/// tested without a process.
pub fn serve(server: &Server, input: impl BufRead, mut output: impl Write) {
    for line in input.lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(req) => server.handle(req),
            Err(e) => Some(Response::error(
                Value::Null,
                PARSE_ERROR,
                format!("parse error: {e}"),
            )),
        };
        if let Some(resp) = response {
            let Ok(json) = serde_json::to_string(&resp) else {
                continue;
            };
            if writeln!(output, "{json}").is_err() || output.flush().is_err() {
                return;
            }
        }
    }
}
