//! Normalized streaming events emitted while a model response is generated.
//!
//! Successful generation follows `Start → updates* → Done`. A failure after generation
//! starts follows `Start → updates* → Error`. Setup failures may emit `Error` alone.

use crate::message::{AssistantMessage, ContentBlock};
use crate::types::StopReason;

/// Distinguishes a genuine error from a caller-initiated abort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorReason {
    /// A real failure (network, protocol, or provider error).
    Error,
    /// The caller cancelled the request.
    Aborted,
}

/// A single normalized event from a provider stream.
///
/// Blocks are addressed by `content_index`; a block's `*_start`/`*_delta`/`*_end`
/// sequence is not guaranteed to be contiguous across different blocks.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum StreamEvent {
    /// Stream begins; carries the initial (empty) assistant message structure.
    Start {
        /// The empty partial message that later events fill in.
        partial: AssistantMessage,
    },
    /// A text block begins.
    TextStart {
        /// Index of the block within the assistant message.
        content_index: usize,
    },
    /// A chunk of text was appended to the block.
    TextDelta {
        /// Index of the block within the assistant message.
        content_index: usize,
        /// The appended text.
        delta: String,
    },
    /// A text block ends.
    TextEnd {
        /// Index of the block within the assistant message.
        content_index: usize,
        /// The block's full text.
        content: String,
    },
    /// A thinking block begins.
    ThinkingStart {
        /// Index of the block within the assistant message.
        content_index: usize,
    },
    /// A chunk of reasoning text was appended to the block.
    ThinkingDelta {
        /// Index of the block within the assistant message.
        content_index: usize,
        /// The appended reasoning text.
        delta: String,
    },
    /// A thinking block ends.
    ThinkingEnd {
        /// Index of the block within the assistant message.
        content_index: usize,
        /// The block's full reasoning text.
        content: String,
    },
    /// A tool-call block begins.
    ToolCallStart {
        /// Index of the block within the assistant message.
        content_index: usize,
    },
    /// A chunk of the tool-call arguments was appended.
    ToolCallDelta {
        /// Index of the block within the assistant message.
        content_index: usize,
        /// The appended raw argument text.
        delta: String,
    },
    /// A tool-call block ends.
    ToolCallEnd {
        /// Index of the block within the assistant message.
        content_index: usize,
        /// The finalized tool-call block.
        tool_call: ContentBlock,
    },
    /// Terminal success. `message` is the authoritative final assistant message.
    Done {
        /// Why generation stopped.
        reason: StopReason,
        /// The final assistant message.
        message: AssistantMessage,
    },
    /// Terminal failure. `error` is an assistant message with partial content.
    Error {
        /// Whether this was a real error or a caller abort.
        reason: ErrorReason,
        /// The partial assistant message carrying the failure detail.
        error: AssistantMessage,
    },
}

impl StreamEvent {
    /// Extract the final assistant message from a terminal event, if any.
    pub fn terminal_message(&self) -> Option<AssistantMessage> {
        match self {
            StreamEvent::Done { message, .. } => Some(message.clone()),
            StreamEvent::Error { error, .. } => Some(error.clone()),
            _ => None,
        }
    }
}
