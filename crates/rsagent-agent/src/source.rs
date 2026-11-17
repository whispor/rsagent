//! Where a group of tools comes from.
//!
//! `ToolSource` is the single abstraction that makes a compiled-in tool set, a local
//! program, and (later) an MCP server the same shape. It is expanded at *assembly time*
//! by [`ToolCatalog`](crate::ToolCatalog) into a flat `Vec<Box<dyn Tool>>`; the agent loop
//! never learns that sources exist (decision D2, `docs/EXTENSIBILITY.md` §3.1).

use crate::tool::Tool;

/// What kind of source a [`ToolSource`] is.
///
/// `#[non_exhaustive]`: `docs/EXTENSIBILITY.md` §3.2 also names `mcp-stdio`, `mcp-http`,
/// `skill`, and `web`. They are added in P2 — declaring variants with no implementation
/// would be code without a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SourceKind {
    /// The compiled-in tool set: `read_file`, `bash`, and friends.
    Builtin,
    /// A program run as a subprocess, speaking the JSON-lines protocol.
    Local,
}

impl SourceKind {
    /// The stable name used in `rsagent.toml` and in `doctor` output.
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Builtin => "builtin",
            SourceKind::Local => "local",
        }
    }
}

/// Whether a source can currently be used.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SourceHealth {
    /// Usable.
    Healthy,
    /// Unusable right now, with a reason for `doctor` to print.
    Unhealthy(String),
}

/// A group of tools, plus how to keep it healthy.
///
/// Implementations must be cheap to call: [`tools`](ToolSource::tools) is invoked during
/// assembly, and [`health`](ToolSource::health) may be polled by a status display.
pub trait ToolSource: Send + Sync {
    /// What kind of source this is. Drives config, docs, and `doctor`.
    fn kind(&self) -> SourceKind;

    /// A stable id, unique among sources. Used for config and event attribution.
    fn id(&self) -> &str;

    /// The tools this source currently offers.
    fn tools(&self) -> Vec<Box<dyn Tool>>;

    /// Current health. Defaults to [`Healthy`](SourceHealth::Healthy), which is right for
    /// any source that cannot fail to load.
    fn health(&self) -> SourceHealth {
        SourceHealth::Healthy
    }

    /// Do whatever this source needs before use (e.g. check that a program exists).
    ///
    /// Default: nothing to do. Must be idempotent.
    ///
    /// # Errors
    ///
    /// Returns a message when the source cannot be prepared.
    fn install(&self) -> Result<(), String> {
        Ok(())
    }

    /// Undo [`install`](ToolSource::install). Must not delete user data.
    ///
    /// # Errors
    ///
    /// Returns a message when the source cannot be cleaned up.
    fn uninstall(&self) -> Result<(), String> {
        Ok(())
    }
}
