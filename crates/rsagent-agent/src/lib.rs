//! `rsagent-agent` — a stateful agent loop with tool execution and event streaming.
//!
//! This mirrors the role of `@earendil-works/pi-agent-core`: it sits on top of
//! `rsagent-ai` and knows about tools and turns, but nothing about a specific UI.

pub mod agent;
pub mod approval;
pub mod breaker;
pub mod cancel;
mod catalog;
pub mod error;
pub mod event;
mod glob;
mod html;
mod platform;
pub mod source;
pub mod sources;
pub mod tool;
pub mod tools;

pub use agent::Agent;
pub use approval::{AllowAll, Approver, DenyAll};
pub use breaker::{BreakerPolicy, BreakerState, CircuitBreaker};
pub use cancel::CancelToken;
pub use catalog::{CatalogError, SourceEntry, ToolCatalog, PREFIX_SEPARATOR};
pub use error::AgentError;
pub use event::AgentEvent;
pub use source::{SourceHealth, SourceKind, ToolSource};
pub use sources::{
    BuiltinSource, LocalSource, McpStdioSource, BUILTIN_SOURCE_ID, MCP_PROTOCOL_VERSION,
    PROTOCOL_VERSION,
};
pub use tool::{PathFence, Tool, ToolContext, ToolResult};
pub use tools::{
    builtin_tools, select_builtin_tools, BashTool, EditFileTool, GlobTool, ListDirTool,
    ReadFileTool, SearchTool, WebFetchTool, WriteFileTool,
};
