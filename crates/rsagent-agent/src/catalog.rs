//! Assembly: turn tool sources into the flat list the agent sees.
//!
//! This is the one place that knows sources exist. It expands them, applies name prefixes,
//! and refuses to build when two tools would end up with the same name — because a silent
//! override would let the model call "the other" tool, the hardest class of bug to find
//! (decision D5, `docs/EXTENSIBILITY.md` §3.4).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use async_trait::async_trait;
use rsagent_ai::ToolSpec;
use serde_json::Value;

use crate::source::{SourceHealth, SourceKind, ToolSource};
use crate::tool::{Tool, ToolContext, ToolResult};

/// The separator between a source prefix and a tool name.
///
/// Two underscores, because a single one is common inside `snake_case` names while a
/// double one is not.
pub const PREFIX_SEPARATOR: &str = "__";

/// A source plus the prefix its tool names are given, if any.
pub struct SourceEntry {
    source: Box<dyn ToolSource>,
    prefix: Option<String>,
}

impl SourceEntry {
    /// An entry whose tools keep their own names.
    pub fn new(source: Box<dyn ToolSource>) -> Self {
        SourceEntry {
            source,
            prefix: None,
        }
    }

    /// An entry whose tools are presented as `{prefix}__{name}`.
    pub fn with_prefix(source: Box<dyn ToolSource>, prefix: impl Into<String>) -> Self {
        SourceEntry {
            source,
            prefix: Some(prefix.into()),
        }
    }

    /// The source itself.
    pub fn source(&self) -> &dyn ToolSource {
        self.source.as_ref()
    }

    /// The prefix applied to this source's tool names, if any.
    pub fn prefix(&self) -> Option<&str> {
        self.prefix.as_deref()
    }
}

impl fmt::Debug for SourceEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceEntry")
            .field("id", &self.source.id())
            .field("kind", &self.source.kind())
            .field("prefix", &self.prefix)
            .finish()
    }
}

/// Why a set of sources could not be assembled.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CatalogError {
    /// Two sources offer the same tool name and neither has a prefix.
    #[error(
        "tool `{name}` is offered by both `{first}` and `{second}`; \
         give one of them a `prefix` in rsagent.toml"
    )]
    DuplicateTool {
        /// The clashing tool name.
        name: String,
        /// The id of the source that claimed it first.
        first: String,
        /// The id of the source that also wants it.
        second: String,
    },
    /// Two sources share an id, so config and events could not tell them apart.
    #[error("two tool sources share the id `{id}`")]
    DuplicateSource {
        /// The repeated id.
        id: String,
    },
}

/// The assembled tool set, and the sources it came from.
pub struct ToolCatalog {
    entries: Vec<SourceEntry>,
    tools: Vec<Box<dyn Tool>>,
}

impl fmt::Debug for ToolCatalog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolCatalog")
            .field("sources", &self.entries)
            .field("tools", &self.tools.len())
            .finish()
    }
}

impl ToolCatalog {
    /// Expand `entries` into a flat tool list.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError::DuplicateSource`] when two sources share an id, and
    /// [`CatalogError::DuplicateTool`] when two tools would share a name.
    pub fn build(entries: Vec<SourceEntry>) -> Result<Self, CatalogError> {
        let mut seen_ids: BTreeSet<String> = BTreeSet::new();
        // Name → the id of the source that claimed it, so a clash can name both sides.
        let mut claimed: BTreeMap<String, String> = BTreeMap::new();
        let mut tools: Vec<Box<dyn Tool>> = Vec::new();

        for entry in &entries {
            let id = entry.source.id();
            if !seen_ids.insert(id.to_string()) {
                return Err(CatalogError::DuplicateSource { id: id.to_string() });
            }
            for tool in entry.source.tools() {
                let name = qualified_name(&tool.spec().name, entry.prefix.as_deref());
                if let Some(first) = claimed.get(&name) {
                    return Err(CatalogError::DuplicateTool {
                        name,
                        first: first.clone(),
                        second: id.to_string(),
                    });
                }
                claimed.insert(name.clone(), id.to_string());
                tools.push(prefixed(tool, name));
            }
        }

        Ok(ToolCatalog { entries, tools })
    }

    /// The sources, in assembly order, for a status display.
    pub fn entries(&self) -> &[SourceEntry] {
        &self.entries
    }

    /// The number of tools across every source.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// The assembled tools, in order. Hand them to the agent with
    /// [`into_tools`](Self::into_tools).
    pub fn tools(&self) -> &[Box<dyn Tool>] {
        &self.tools
    }

    /// Whether no source offered any tool.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Consume the catalog and hand the flat list to the agent.
    pub fn into_tools(self) -> Vec<Box<dyn Tool>> {
        self.tools
    }

    /// Consume the catalog, yielding the tools *and* the sources behind them.
    ///
    /// A tool shares its source's circuit breaker, so a caller that keeps the sources can
    /// snapshot breaker state after a run even though the tools now belong to the agent
    /// (`docs/EXTENSIBILITY.md` §3.5).
    pub fn into_parts(self) -> (Vec<Box<dyn Tool>>, Vec<SourceEntry>) {
        (self.tools, self.entries)
    }

    /// Every source's health, paired with its id.
    pub fn health(&self) -> Vec<(String, SourceHealth)> {
        self.entries
            .iter()
            .map(|entry| (entry.source.id().to_string(), entry.source.health()))
            .collect()
    }

    /// The kind of each source, paired with its id.
    pub fn kinds(&self) -> Vec<(String, SourceKind)> {
        self.entries
            .iter()
            .map(|entry| (entry.source.id().to_string(), entry.source.kind()))
            .collect()
    }
}

/// `name` as the model sees it: prefixed when the source declared one.
fn qualified_name(name: &str, prefix: Option<&str>) -> String {
    match prefix {
        Some(prefix) => format!("{prefix}{PREFIX_SEPARATOR}{name}"),
        None => name.to_string(),
    }
}

/// Wrap `tool` when its name was qualified; otherwise pass it through untouched.
fn prefixed(tool: Box<dyn Tool>, name: String) -> Box<dyn Tool> {
    if tool.spec().name == name {
        tool
    } else {
        Box::new(PrefixedTool { inner: tool, name })
    }
}

/// A tool presented under a qualified name, delegating everything else.
struct PrefixedTool {
    inner: Box<dyn Tool>,
    name: String,
}

#[async_trait]
impl Tool for PrefixedTool {
    fn spec(&self) -> ToolSpec {
        let mut spec = self.inner.spec();
        spec.name.clone_from(&self.name);
        spec
    }

    fn label(&self) -> String {
        self.inner.label()
    }

    async fn execute(
        &self,
        call_id: &str,
        args: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, String> {
        self.inner.execute(call_id, args, context).await
    }

    fn is_sequential(&self) -> bool {
        self.inner.is_sequential()
    }

    fn requires_approval(&self) -> bool {
        self.inner.requires_approval()
    }
}

#[cfg(test)]
mod tests {
    use super::{CatalogError, SourceEntry, ToolCatalog, PREFIX_SEPARATOR};
    use crate::source::{SourceHealth, SourceKind, ToolSource};
    use crate::tool::{Tool, ToolContext, ToolResult};
    use async_trait::async_trait;
    use rsagent_ai::ToolSpec;
    use serde_json::{json, Value};

    /// A source offering one trivial tool per name given.
    struct FakeSource {
        id: String,
        names: Vec<String>,
    }

    impl FakeSource {
        fn boxed(id: &str, names: &[&str]) -> Box<dyn ToolSource> {
            Box::new(FakeSource {
                id: id.to_string(),
                names: names.iter().map(|name| (*name).to_string()).collect(),
            })
        }
    }

    impl ToolSource for FakeSource {
        fn kind(&self) -> SourceKind {
            SourceKind::Local
        }

        fn id(&self) -> &str {
            &self.id
        }

        fn tools(&self) -> Vec<Box<dyn Tool>> {
            self.names
                .iter()
                .map(|name| -> Box<dyn Tool> { Box::new(FakeTool { name: name.clone() }) })
                .collect()
        }
    }

    struct FakeTool {
        name: String,
    }

    #[async_trait]
    impl Tool for FakeTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec::new(
                self.name.clone(),
                "a fake tool",
                json!({ "type": "object", "properties": {} }),
            )
        }

        async fn execute(
            &self,
            _call_id: &str,
            _args: Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, String> {
            Ok(ToolResult::text(self.name.clone()))
        }
    }

    fn names(tools: &[Box<dyn Tool>]) -> Vec<String> {
        tools.iter().map(|tool| tool.spec().name).collect()
    }

    #[test]
    fn expands_sources_into_one_flat_list_in_order() {
        let catalog = ToolCatalog::build(vec![
            SourceEntry::new(FakeSource::boxed("a", &["one", "two"])),
            SourceEntry::new(FakeSource::boxed("b", &["three"])),
        ])
        .expect("build");

        assert_eq!(names(&catalog.into_tools()), vec!["one", "two", "three"]);
    }

    #[test]
    fn a_prefix_qualifies_every_tool_of_that_source() {
        let catalog = ToolCatalog::build(vec![
            SourceEntry::new(FakeSource::boxed("a", &["read_file"])),
            SourceEntry::with_prefix(FakeSource::boxed("fs", &["read_file"]), "fs"),
        ])
        .expect("build");

        assert_eq!(
            names(&catalog.into_tools()),
            vec![
                "read_file".to_string(),
                format!("fs{PREFIX_SEPARATOR}read_file")
            ]
        );
    }

    #[test]
    fn a_prefix_resolves_what_would_otherwise_collide() {
        // The point of prefixes: the same tool name from two sources must coexist.
        let catalog = ToolCatalog::build(vec![
            SourceEntry::new(FakeSource::boxed("a", &["read_file"])),
            SourceEntry::with_prefix(FakeSource::boxed("fs", &["read_file"]), "fs"),
        ]);
        assert!(catalog.is_ok());
    }

    #[test]
    fn an_unprefixed_collision_names_both_sources() {
        let error = ToolCatalog::build(vec![
            SourceEntry::new(FakeSource::boxed("first", &["read_file"])),
            SourceEntry::new(FakeSource::boxed("second", &["read_file"])),
        ])
        .expect_err("a collision must be refused");

        match error {
            CatalogError::DuplicateTool {
                name,
                first,
                second,
            } => {
                assert_eq!(name, "read_file");
                assert_eq!(first, "first");
                assert_eq!(second, "second");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn a_repeated_source_id_is_refused() {
        let error = ToolCatalog::build(vec![
            SourceEntry::new(FakeSource::boxed("dup", &["one"])),
            SourceEntry::new(FakeSource::boxed("dup", &["two"])),
        ])
        .expect_err("a repeated id must be refused");

        assert!(matches!(error, CatalogError::DuplicateSource { id } if id == "dup"));
    }

    #[test]
    fn health_is_reported_per_source() {
        let catalog = ToolCatalog::build(vec![
            SourceEntry::new(FakeSource::boxed("a", &["one"])),
            SourceEntry::new(FakeSource::boxed("b", &["two"])),
        ])
        .expect("build");

        assert_eq!(
            catalog.health(),
            vec![
                ("a".to_string(), SourceHealth::Healthy),
                ("b".to_string(), SourceHealth::Healthy),
            ]
        );
    }
}
