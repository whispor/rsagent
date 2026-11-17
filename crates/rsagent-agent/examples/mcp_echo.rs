//! A minimal [Model Context Protocol] server, spoken over stdio.
//!
//! Point an `rsagent.toml` entry of kind `mcp-stdio` at this program and rsagent will
//! discover its `echo` tool:
//!
//! ```toml
//! [[tools.sources]]
//! kind = "mcp-stdio"
//! id = "echo"
//! command = "target/debug/examples/mcp_echo"
//! prefix = "echo"
//! ```
//!
//! It implements the small subset rsagent uses — `initialize`, `tools/list`, `tools/call` —
//! and is also the fixture for `tests/mcp_stdio.rs`. Run it with:
//!
//! ```bash
//! cargo run -p rsagent-agent --example mcp_echo
//! ```
//!
//! [Model Context Protocol]: https://modelcontextprotocol.io

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

/// The protocol revision this server reports.
const PROTOCOL_VERSION: &str = "2025-06-18";

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        // A message with no id is a notification (e.g. `notifications/initialized`):
        // it gets no response.
        if let Some(response) = respond(&message) {
            let _ = writeln!(stdout, "{response}");
            let _ = stdout.flush();
        }
    }
}

/// The JSON-RPC response to `message`, or `None` when it is a notification.
fn respond(message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let outcome = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "mcp_echo", "version": env!("CARGO_PKG_VERSION") },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": [tool()] })),
        "tools/call" => call(message),
        other => Err((-32601, format!("method not found: {other}"))),
    };
    Some(match outcome {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, text)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": text } })
        }
    })
}

/// Handle a `tools/call` request.
fn call(message: &Value) -> Result<Value, (i64, String)> {
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    if name != "echo" {
        return Err((-32602, format!("unknown tool: {name}")));
    }
    let text = params
        .get("arguments")
        .and_then(|arguments| arguments.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("");
    Ok(json!({ "content": [{ "type": "text", "text": text }] }))
}

/// The one tool this server exposes.
fn tool() -> Value {
    json!({
        "name": "echo",
        "description": "Echo the `text` argument back.",
        "inputSchema": {
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"],
        },
    })
}
