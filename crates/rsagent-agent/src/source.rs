//! Where a group of tools comes from.
//!
//! `ToolSource` is the single abstraction that makes a compiled-in tool set, a local
//! program, and an MCP server the same shape. It is expanded at *assembly time*
//! by [`ToolCatalog`](crate::ToolCatalog) into a flat `Vec<Box<dyn Tool>>`; the agent loop
//! never learns that sources exist (decision D2, `docs/EXTENSIBILITY.md` §3.1).

use crate::breaker::BreakerReport;
use crate::tool::Tool;

/// What kind of source a [`ToolSource`] is.
///
/// `#[non_exhaustive]`: `docs/EXTENSIBILITY.md` §3.2 names the kinds, and new ones are
/// added as variants so downstream `match`es keep compiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SourceKind {
    /// The compiled-in tool set: `read_file`, `bash`, and friends.
    Builtin,
    /// A program run as a subprocess, speaking the JSON-lines protocol.
    Local,
    /// A [Model Context Protocol] server, spoken over stdio.
    ///
    /// [Model Context Protocol]: https://modelcontextprotocol.io
    McpStdio,
    /// A [Model Context Protocol] server, reached over HTTP.
    ///
    /// [Model Context Protocol]: https://modelcontextprotocol.io
    McpHttp,
    /// A folder of scripts declared by a `skill.toml` manifest.
    Skill,
    /// A single HTTP endpoint wrapped as one tool.
    Web,
}

impl SourceKind {
    /// Every kind, in the order they are documented.
    ///
    /// The single list of "what kinds exist": [`as_str`](Self::as_str) names them,
    /// [`parse`](Self::parse) resolves a config string against it, and callers build their
    /// "expected one of …" messages from it — so adding a kind updates one place.
    pub const ALL: [SourceKind; 6] = [
        SourceKind::Builtin,
        SourceKind::Local,
        SourceKind::McpStdio,
        SourceKind::McpHttp,
        SourceKind::Skill,
        SourceKind::Web,
    ];

    /// The stable name used in `rsagent.toml` and in `doctor` output.
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Builtin => "builtin",
            SourceKind::Local => "local",
            SourceKind::McpStdio => "mcp-stdio",
            SourceKind::McpHttp => "mcp-http",
            SourceKind::Skill => "skill",
            SourceKind::Web => "web",
        }
    }

    /// Parse a config `kind` string, or `None` when it names no known kind.
    pub fn parse(name: &str) -> Option<SourceKind> {
        SourceKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == name)
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

/// A source that a health probe currently considers unhealthy.
///
/// The CLI builds these from its live sources and hands them to the agent's health probe
/// ([`Agent::with_health_probe`](crate::Agent::with_health_probe)); the loop turns each
/// *newly* unhealthy one into an [`AgentEvent::SourceUnhealthy`](crate::AgentEvent). The
/// loop itself never touches a [`ToolSource`], so it stays ignorant of where tools come
/// from (decision D2, `docs/EXTENSIBILITY.md` §3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnhealthySource {
    /// The source's stable id.
    pub id: String,
    /// The source's kind, as its short config name (`local`, `mcp-http`, ...).
    pub kind: String,
    /// Why the source is unhealthy.
    pub reason: String,
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

    /// The source's circuit-breaker state, if it has one.
    ///
    /// Sources that cannot fail to load (e.g. `builtin`) have no breaker and return
    /// `None`. The CLI persists these reports so `rsagent tools list` / `doctor` can show
    /// them and `rsagent tools reset` can clear them (`docs/EXTENSIBILITY.md` §3.5).
    fn breaker_report(&self) -> Option<BreakerReport> {
        None
    }

    /// Rebuild this source's circuit breaker from a persisted report.
    ///
    /// Default: nothing to do. Called once at startup, before the first call.
    fn restore_breaker(&self, _report: &BreakerReport) {}

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
