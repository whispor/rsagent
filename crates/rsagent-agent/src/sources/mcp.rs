//! The Model Context Protocol payloads shared by every MCP transport.
//!
//! `mcp-stdio` and `mcp-http` differ only in how a message reaches the server — a
//! newline-delimited pipe versus an HTTP POST. *What* is on the wire, and what the
//! responses mean, is identical: the same `initialize` / `tools/list` / `tools/call`
//! payloads, the same tool descriptors, the same `content` blocks. That shared half lives
//! here so neither transport owns it.
//!
//! [Model Context Protocol]: https://modelcontextprotocol.io

use rsagent_ai::ToolSpec;
use serde_json::{json, Value};

use crate::tool::ToolResult;

/// The MCP revision this client asks for.
///
/// The server may answer with a different revision it supports; the subset used here
/// (`initialize` / `tools/list` / `tools/call`) is stable across revisions, so any answer
/// is accepted rather than failing the connection.
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// The `initialize` parameters, identical whichever transport carries them.
pub(crate) fn initialize_params() -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "rsagent", "version": env!("CARGO_PKG_VERSION") },
    })
}

/// One tool as declared by the server.
#[derive(Debug, Clone)]
pub(crate) struct McpToolSpec {
    /// The tool name as the server knows it (before any source prefix is applied).
    pub(crate) name: String,
    /// A one-line description for the model.
    pub(crate) description: String,
    /// The JSON Schema the model must satisfy.
    pub(crate) parameters: Value,
    /// Whether a call needs the user's approval first.
    pub(crate) requires_approval: bool,
}

impl McpToolSpec {
    /// The [`ToolSpec`] handed to the model.
    pub(crate) fn tool_spec(&self) -> ToolSpec {
        ToolSpec::new(
            self.name.clone(),
            self.description.clone(),
            self.parameters.clone(),
        )
    }
}

/// Why a call to the server failed, from the caller's point of view.
#[derive(Debug)]
pub(crate) enum CallError {
    /// Connection-class: the transport failed (or could not be (re-)established).
    Transport(String),
    /// The server is reachable but rejected the request.
    Protocol(String),
}

/// Read the `tools` array of a `tools/list` result.
pub(crate) fn parse_tools(result: &Value) -> Result<Vec<McpToolSpec>, String> {
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| "`tools/list` returned no `tools` array".to_string())?;
    tools.iter().map(parse_tool).collect()
}

/// Read one entry of a `tools/list` result.
pub(crate) fn parse_tool(value: &Value) -> Result<McpToolSpec, String> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "a listed tool is missing `name`".to_string())?
        .to_string();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let parameters = value
        .get("inputSchema")
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    // Approval is the default: an arbitrary server may do anything. A tool that declares
    // itself read-only (`annotations.readOnlyHint`) opts out.
    let requires_approval = !value
        .get("annotations")
        .and_then(|annotations| annotations.get("readOnlyHint"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    Ok(McpToolSpec {
        name,
        description,
        parameters,
        requires_approval,
    })
}

/// Turn a `tools/call` result into a tool result.
pub(crate) fn parse_call_result(value: &Value) -> Result<ToolResult, String> {
    let text = render_content(value.get("content"));
    let is_error = value
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if is_error {
        return Err(if text.is_empty() {
            "the tool reported an error".to_string()
        } else {
            text
        });
    }
    let mut result = ToolResult::text(text);
    if let Some(details) = value.get("structuredContent") {
        result.details = Some(details.clone());
    }
    Ok(result)
}

/// Render an MCP `content` array as plain text.
fn render_content(content: Option<&Value>) -> String {
    let Some(blocks) = content.and_then(Value::as_array) else {
        return String::new();
    };
    blocks
        .iter()
        .map(render_block)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render one content block. Non-text blocks get a short placeholder, since a terminal
/// tool result is text.
fn render_block(block: &Value) -> String {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => block
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        Some(kind @ ("image" | "audio")) => {
            let mime = block
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("[{kind}: {mime}]")
        }
        Some("resource" | "resource_link") => {
            let uri = block
                .get("resource")
                .and_then(|resource| resource.get("uri"))
                .and_then(Value::as_str)
                .or_else(|| block.get("uri").and_then(Value::as_str))
                .unwrap_or("");
            format!("[resource: {uri}]")
        }
        Some(other) => format!("[{other}]"),
        None => block.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_call_result, parse_tool, parse_tools, render_content};
    use serde_json::json;

    #[test]
    fn a_listed_tool_defaults_its_schema_and_approval() {
        let spec = parse_tool(&json!({ "name": "bare" })).expect("parse");
        assert_eq!(spec.name, "bare");
        assert_eq!(
            spec.parameters,
            json!({ "type": "object", "properties": {} })
        );
        assert!(spec.requires_approval, "approval is the default");
    }

    #[test]
    fn a_read_only_annotation_opts_out_of_approval() {
        let spec = parse_tool(&json!({
            "name": "peek",
            "annotations": { "readOnlyHint": true },
        }))
        .expect("parse");
        assert!(!spec.requires_approval);
    }

    #[test]
    fn a_listed_tool_without_a_name_is_refused() {
        assert!(parse_tool(&json!({ "description": "anonymous" })).is_err());
        assert!(parse_tools(&json!({ "noTools": [] })).is_err());
    }

    #[test]
    fn a_call_error_result_becomes_err() {
        let error = parse_call_result(&json!({
            "content": [{ "type": "text", "text": "nope" }],
            "isError": true
        }))
        .expect_err("isError becomes Err");
        assert_eq!(error, "nope");
    }

    #[test]
    fn a_call_success_renders_text_and_keeps_details() {
        let result = parse_call_result(&json!({
            "content": [{ "type": "text", "text": "ok" }],
            "structuredContent": { "n": 1 }
        }))
        .expect("parse");
        assert_eq!(result.content[0].as_text(), Some("ok"));
        assert_eq!(result.details, Some(json!({ "n": 1 })));
    }

    #[test]
    fn non_text_blocks_get_a_placeholder() {
        let text = render_content(Some(&json!([
            { "type": "text", "text": "look:" },
            { "type": "image", "mimeType": "image/png" },
            { "type": "resource", "resource": { "uri": "file:///x" } }
        ])));
        assert_eq!(text, "look:\n[image: image/png]\n[resource: file:///x]");
    }
}
