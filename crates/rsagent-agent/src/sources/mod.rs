//! The tool sources that ship with `rsagent`.
//!
//! `builtin` is the compiled-in set; `local` runs a program as a subprocess. Future kinds
//! (`mcp-stdio`, `mcp-http`, `skill`, `web`) implement the same
//! [`ToolSource`](crate::ToolSource) trait — see `docs/EXTENSIBILITY.md` §3.2.

mod builtin;
mod local;

pub use builtin::{BuiltinSource, BUILTIN_SOURCE_ID};
pub use local::{LocalSource, PROTOCOL_VERSION};
