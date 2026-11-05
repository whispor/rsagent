//! Events emitted by the agent loop, for UI and observability.
//!
//! The sequence mirrors pi's event flow so a UI written against these events is easy to
//! reason about: `AgentStart → (TurnStart → messages/tools → TurnEnd)+ → AgentEnd`.

use rsagent_ai::{Message, StreamEvent};
use serde_json::Value;

/// An event emitted by the agent loop.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum AgentEvent {
    /// The run begins.
    AgentStart,
    /// A new turn begins (one model call plus its tool executions).
    TurnStart,
    /// Any message begins.
    MessageStart {
        /// The message that started.
        message: Message,
    },
    /// Streaming update for the in-progress assistant message.
    MessageUpdate {
        /// The normalized stream event.
        event: StreamEvent,
    },
    /// A message completes.
    MessageEnd {
        /// The completed message.
        message: Message,
    },
    /// A tool begins executing.
    ToolExecutionStart {
        /// Id of the tool call being executed.
        tool_call_id: String,
        /// Name of the tool being executed.
        tool_name: String,
        /// Human-readable label for display.
        label: String,
        /// The parsed tool arguments.
        args: Value,
    },
    /// A tool call is awaiting an approval decision.
    ToolApprovalRequest {
        /// Id of the tool call awaiting approval.
        tool_call_id: String,
        /// Name of the tool.
        tool_name: String,
        /// The parsed tool arguments.
        args: Value,
    },
    /// The approval decision for a pending tool call.
    ToolApprovalResponse {
        /// Id of the tool call.
        tool_call_id: String,
        /// Whether the call was approved.
        approved: bool,
    },
    /// A tool streams progress.
    ToolExecutionUpdate {
        /// Id of the tool call.
        tool_call_id: String,
        /// The progress content.
        content: String,
    },
    /// A tool finishes.
    ToolExecutionEnd {
        /// Id of the tool call.
        tool_call_id: String,
        /// Name of the tool.
        tool_name: String,
        /// Whether the tool failed.
        is_error: bool,
        /// The tool's combined text output.
        content: String,
        /// Model-invisible structured details, when the tool provided any.
        details: Option<Value>,
    },
    /// A turn completes with its assistant message and tool results.
    TurnEnd {
        /// The assistant message that ended the turn.
        message: Message,
        /// Tool-result messages produced during the turn.
        tool_results: Vec<Message>,
    },
    /// The final event of a run.
    AgentEnd {
        /// The full transcript at the end of the run.
        messages: Vec<Message>,
    },
}
