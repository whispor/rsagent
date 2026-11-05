//! Conversation messages and the serializable [`Context`].

use serde::{Deserialize, Serialize};

use crate::tool::ToolSpec;
use crate::types::{now_millis, StopReason, Usage};

/// A single block of content inside a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[non_exhaustive]
pub enum ContentBlock {
    /// Plain text.
    Text {
        /// The text content.
        text: String,
    },
    /// A reasoning/thinking segment emitted before the visible answer.
    Thinking {
        /// The reasoning text.
        thinking: String,
        /// Optional provider signature used to verify the thinking block.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// An inline image.
    Image {
        /// Base64-encoded image data.
        data: String,
        /// IANA media type, e.g. `image/png`.
        mime_type: String,
    },
    /// A request from the model to invoke a tool.
    ToolCall {
        /// Provider-assigned id, echoed back with the tool result.
        id: String,
        /// The tool name.
        name: String,
        /// Parsed arguments. During streaming this may be a partial best-effort object.
        arguments: serde_json::Value,
    },
}

impl ContentBlock {
    /// Build a text block from any string-like value.
    pub fn text(text: impl Into<String>) -> Self {
        ContentBlock::Text { text: text.into() }
    }

    /// Borrow the text if this block is [`ContentBlock::Text`].
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentBlock::Text { text } => Some(text),
            _ => None,
        }
    }

    /// Borrow the `(id, name, arguments)` triple if this is a [`ContentBlock::ToolCall`].
    pub fn as_tool_call(&self) -> Option<(&str, &str, &serde_json::Value)> {
        match self {
            ContentBlock::ToolCall {
                id,
                name,
                arguments,
            } => Some((id, name, arguments)),
            _ => None,
        }
    }
}

/// A system-role message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMessage {
    /// The message content blocks.
    pub content: Vec<ContentBlock>,
    /// Creation time, in milliseconds since the Unix epoch.
    pub timestamp: i64,
}

/// A user-role message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMessage {
    /// The message content blocks.
    pub content: Vec<ContentBlock>,
    /// Creation time, in milliseconds since the Unix epoch.
    pub timestamp: i64,
}

/// An assistant-role message produced by a model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    /// The message content blocks.
    pub content: Vec<ContentBlock>,
    /// Id of the model that produced the message.
    pub model: String,
    /// Id of the provider that produced the message.
    pub provider: String,
    /// Token accounting for the call.
    #[serde(default)]
    pub usage: Usage,
    /// Why generation stopped.
    pub stop_reason: StopReason,
    /// Human-readable failure detail, set when `stop_reason` is `Error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Creation time, in milliseconds since the Unix epoch.
    pub timestamp: i64,
}

impl AssistantMessage {
    /// An empty partial message, used to seed a stream.
    pub fn partial(model: &crate::types::Model) -> Self {
        AssistantMessage {
            content: Vec::new(),
            model: model.id.clone(),
            provider: model.provider.clone(),
            usage: Usage::default(),
            stop_reason: StopReason::Pending,
            error_message: None,
            timestamp: now_millis(),
        }
    }

    /// A terminal error message for the given model.
    pub fn error(model: &crate::types::Model, message: impl Into<String>) -> Self {
        AssistantMessage {
            content: Vec::new(),
            model: model.id.clone(),
            provider: model.provider.clone(),
            usage: Usage::default(),
            stop_reason: StopReason::Error,
            error_message: Some(message.into()),
            timestamp: now_millis(),
        }
    }

    /// Concatenate every text block into a single owned string.
    pub fn to_text(&self) -> String {
        self.content
            .iter()
            .filter_map(ContentBlock::as_text)
            .collect::<Vec<_>>()
            .join("")
    }

    /// All tool calls contained in this message.
    pub fn tool_calls(&self) -> Vec<(&str, &str, &serde_json::Value)> {
        self.content
            .iter()
            .filter_map(ContentBlock::as_tool_call)
            .collect()
    }
}

/// The result of executing one tool call, fed back to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultMessage {
    /// Id of the tool call this result answers.
    pub tool_call_id: String,
    /// Name of the tool that ran.
    pub tool_name: String,
    /// The result content blocks.
    pub content: Vec<ContentBlock>,
    /// Whether the tool failed.
    pub is_error: bool,
    /// Creation time, in milliseconds since the Unix epoch.
    pub timestamp: i64,
}

/// A transcript message: the discriminated union the LLM understands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
#[non_exhaustive]
pub enum Message {
    /// A system-role message.
    System(SystemMessage),
    /// A user-role message.
    User(UserMessage),
    /// An assistant-role message.
    Assistant(AssistantMessage),
    /// A tool result.
    ToolResult(ToolResultMessage),
}

impl Message {
    /// Build a user message with a single text block.
    pub fn user(content: impl Into<String>) -> Self {
        Message::User(UserMessage {
            content: vec![ContentBlock::text(content)],
            timestamp: now_millis(),
        })
    }

    /// Build a system message with a single text block.
    pub fn system(content: impl Into<String>) -> Self {
        Message::System(SystemMessage {
            content: vec![ContentBlock::text(content)],
            timestamp: now_millis(),
        })
    }

    /// The message's role as it appears on the wire.
    pub fn role_str(&self) -> &'static str {
        match self {
            Message::System(_) => "system",
            Message::User(_) => "user",
            Message::Assistant(_) => "assistant",
            Message::ToolResult(_) => "toolResult",
        }
    }
}

/// Everything needed to make one model call. Fully serializable so it can be
/// persisted or handed to a different provider mid-session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    /// Optional system prompt prepended to the transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// The conversation transcript.
    #[serde(default)]
    pub messages: Vec<Message>,
    /// Tool declarations offered to the model.
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
}
