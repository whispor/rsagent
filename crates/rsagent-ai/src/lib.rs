//! `rsagent-ai` — a unified, multi-provider LLM API with normalized streaming events.
//!
//! This crate mirrors the role of `@earendil-works/pi-ai` in the pi project: it is the
//! bottom layer of the stack and knows nothing about agents, tool execution, or UI.
//!
//! Design notes borrowed from pi:
//! * A [`Context`] is plain, serializable data (system prompt + messages + tool specs),
//!   so a conversation can be persisted or handed to another model/provider at will.
//! * Providers are the runtime unit: each owns a model catalog, auth resolution, and a
//!   [`Provider::stream`] implementation. Wire protocols (OpenAI Chat Completions,
//!   Anthropic Messages, ...) are shared "API" implementations behind providers.
//! * Streaming is exposed as a normalized [`StreamEvent`] sequence. Request failures
//!   after the stream starts never panic; they surface as a terminal `error` event and
//!   the final [`AssistantMessage`] carries the details.

pub mod event;
pub mod message;
pub mod provider;
pub mod providers;
pub mod tool;
pub mod types;

pub use event::{ErrorReason, StreamEvent};
pub use message::{
    AssistantMessage, ContentBlock, Context, Message, SystemMessage, ToolResultMessage, UserMessage,
};
pub use provider::{Auth, EventStream, Models, Provider, StreamOptions};
pub use providers::{AnthropicProvider, FauxBlock, FauxProvider, FauxResponse, OpenAiProvider};
pub use tool::ToolSpec;
pub use types::{now_millis, Modality, Model, StopReason, ThinkingLevel, Usage};
