//! The compiled-in tool set, presented as a [`ToolSource`].

use crate::source::{SourceKind, ToolSource};
use crate::tool::Tool;
use crate::tools::{memory_tools, select_builtin_tools};

/// The default id of the built-in source.
pub const BUILTIN_SOURCE_ID: &str = "core";

/// The built-in tools (`read_file`, `bash`, ...), optionally narrowed by name.
///
/// Narrowing exists so the same source serves both `rsagent --tools read_file` and a
/// `[[tools.sources]]` block with `only` / `exclude` in `rsagent.toml`.
#[derive(Debug)]
pub struct BuiltinSource {
    id: String,
    only: Option<Vec<String>>,
    exclude: Vec<String>,
    /// Whether the long-term-memory tools (`remember`, `recall`) are offered.
    memory: bool,
}

impl BuiltinSource {
    /// The full built-in set under the default id.
    pub fn new() -> Self {
        BuiltinSource {
            id: BUILTIN_SOURCE_ID.to_string(),
            only: None,
            exclude: Vec::new(),
            memory: false,
        }
    }

    /// Use a different id (a config file may name the built-in source anything).
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Keep only `only` (when set), then always drop `exclude`.
    #[must_use]
    pub fn with_selection(mut self, only: Option<Vec<String>>, exclude: Vec<String>) -> Self {
        self.only = only;
        self.exclude = exclude;
        self
    }

    /// Also offer the long-term-memory tools (`remember`, `recall`).
    ///
    /// They are added on top of `only`/`exclude`: those narrow the filesystem and shell
    /// primitives, while memory is a separate capability the caller enabled explicitly.
    #[must_use]
    pub fn with_memory(mut self) -> Self {
        self.memory = true;
        self
    }

    /// Names requested via `only` that matched no built-in tool, so a typo can be warned
    /// about instead of silently dropping a tool.
    pub fn unknown(&self) -> Vec<String> {
        self.selected().1
    }

    /// The tools this source offers, plus the unmatched names.
    fn selected(&self) -> (Vec<Box<dyn Tool>>, Vec<String>) {
        let exclude: Vec<&str> = self.exclude.iter().map(String::as_str).collect();
        let (mut tools, unknown) = select_builtin_tools(self.only.as_deref(), &exclude);
        if self.memory {
            tools.extend(memory_tools());
        }
        (tools, unknown)
    }
}

impl Default for BuiltinSource {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSource for BuiltinSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Builtin
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        self.selected().0
    }
}

#[cfg(test)]
mod tests {
    use super::BuiltinSource;
    use crate::source::{SourceKind, ToolSource};

    fn names(source: &BuiltinSource) -> Vec<String> {
        source.tools().iter().map(|tool| tool.spec().name).collect()
    }

    #[test]
    fn the_default_source_offers_every_builtin() {
        let source = BuiltinSource::new();
        assert_eq!(source.kind(), SourceKind::Builtin);
        assert_eq!(source.id(), "core");
        assert_eq!(names(&source).len(), 8);
    }

    #[test]
    fn selection_keeps_only_the_named_tools() {
        let source = BuiltinSource::new().with_selection(
            Some(vec!["read_file".to_string(), "glob".to_string()]),
            Vec::new(),
        );
        assert_eq!(names(&source), vec!["read_file", "glob"]);
        assert_eq!(source.unknown(), Vec::<String>::new());
    }

    #[test]
    fn exclude_wins_over_only() {
        let source = BuiltinSource::new().with_selection(None, vec!["bash".to_string()]);
        assert!(!names(&source).contains(&"bash".to_string()));
    }

    #[test]
    fn an_unmatched_name_is_reported() {
        let source = BuiltinSource::new().with_selection(
            Some(vec!["read_file".to_string(), "nope".to_string()]),
            Vec::new(),
        );
        assert_eq!(source.unknown(), vec!["nope".to_string()]);
    }

    #[test]
    fn memory_adds_the_remember_and_recall_tools() {
        let names = names(&BuiltinSource::new().with_memory());
        assert!(names.contains(&"remember".to_string()));
        assert!(names.contains(&"recall".to_string()));
        assert_eq!(names.len(), 10);
    }

    #[test]
    fn memory_tools_survive_a_narrow_selection() {
        let source = BuiltinSource::new()
            .with_selection(Some(vec!["read_file".to_string()]), Vec::new())
            .with_memory();
        let names = names(&source);
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&"remember".to_string()));
    }
}
