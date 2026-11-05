//! The LLM-facing tool declaration.

use serde::{Deserialize, Serialize};

/// A tool as the model sees it: a name, a description, and a JSON Schema for arguments.
///
/// This is deliberately decoupled from execution. `rsagent-agent` pairs a `ToolSpec`
/// with an executable implementation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    /// The tool name the model calls.
    pub name: String,
    /// A description that tells the model when to use the tool.
    pub description: String,
    /// A JSON Schema object describing the tool's parameters.
    pub parameters: serde_json::Value,
}

impl ToolSpec {
    /// Build a tool declaration from its parts.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        ToolSpec {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}
