//! The tool sources that ship with `rsagent`.
//!
//! `builtin` is the compiled-in set; `local` runs a program as a subprocess; `mcp_stdio`
//! and `mcp_http` speak the Model Context Protocol over stdio and HTTP. Future kinds
//! (`skill`, `web`) implement the same [`ToolSource`](crate::ToolSource) trait — see
//! `docs/EXTENSIBILITY.md` §3.2.

mod builtin;
mod local;
mod mcp;
mod mcp_http;
mod mcp_stdio;

pub use builtin::{BuiltinSource, BUILTIN_SOURCE_ID};
pub use local::{LocalSource, PROTOCOL_VERSION};
pub use mcp::MCP_PROTOCOL_VERSION;
pub use mcp_http::McpHttpSource;
pub use mcp_stdio::McpStdioSource;
