//! The executable side of a tool.

use std::path::PathBuf;

use async_trait::async_trait;
use rsagent_ai::{ContentBlock, ToolSpec};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::cancel::CancelToken;

/// Whether filesystem tools may reach outside the working directory.
///
/// Read tools honor this by default: a path that resolves (through `..` or a symlink)
/// to somewhere outside the working directory is rejected with a clear error. The
/// command-line `--allow-outside-cwd` flag selects [`Unrestricted`](PathFence::Unrestricted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathFence {
    /// Read tools must stay within the working directory.
    WorkingDirectory,
    /// Read tools may reach anywhere the process can read.
    Unrestricted,
}

/// Ambient context handed to a tool on each execution.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Working directory used to resolve relative paths.
    pub cwd: PathBuf,
    /// When set, read tools must resolve within this directory.
    fence: Option<PathBuf>,
    progress: Option<UnboundedSender<String>>,
    cancel: CancelToken,
}

impl ToolContext {
    /// Create a context rooted at `cwd`, with no progress sink and a fresh cancel token.
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        ToolContext {
            cwd: cwd.into(),
            fence: None,
            progress: None,
            cancel: CancelToken::new(),
        }
    }

    /// Attach a progress sink. Each string is surfaced as a tool-progress event.
    #[must_use]
    pub fn with_progress(mut self, progress: UnboundedSender<String>) -> Self {
        self.progress = Some(progress);
        self
    }

    /// Attach a cancellation token.
    #[must_use]
    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// Constrain read tools to `root` (see [`resolve_within`](Self::resolve_within)).
    #[must_use]
    pub fn with_fence(mut self, root: impl Into<PathBuf>) -> Self {
        self.fence = Some(root.into());
        self
    }

    /// The progress sink, if one was attached.
    pub fn progress(&self) -> Option<&UnboundedSender<String>> {
        self.progress.as_ref()
    }

    /// The cancellation token for this execution.
    pub fn cancel(&self) -> &CancelToken {
        &self.cancel
    }

    /// Resolve a possibly-relative path against the working directory.
    pub fn resolve(&self, path: &str) -> PathBuf {
        let candidate = PathBuf::from(path);
        if candidate.is_absolute() {
            candidate
        } else {
            self.cwd.join(candidate)
        }
    }

    /// Resolve a path for reading, requiring it to stay within the fence when one is set.
    ///
    /// Symlinks are resolved before the check, so a link that points outside the fence
    /// is rejected just like a `..` escape. A path that does not exist is reported as an
    /// error rather than silently accepted.
    ///
    /// # Errors
    ///
    /// Returns an error when the path cannot be canonicalized, when the fence root cannot
    /// be canonicalized, or when the resolved path lies outside the fence.
    pub async fn resolve_within(&self, path: &str) -> Result<PathBuf, String> {
        let resolved = self.resolve(path);
        let Some(fence) = &self.fence else {
            return Ok(resolved);
        };
        let real = tokio::fs::canonicalize(&resolved)
            .await
            .map_err(|error| format!("cannot access {}: {error}", resolved.display()))?;
        let boundary = tokio::fs::canonicalize(fence).await.map_err(|error| {
            format!(
                "cannot access working directory {}: {error}",
                fence.display()
            )
        })?;
        if real.starts_with(&boundary) {
            Ok(real)
        } else {
            Err(format!(
                "path {} is outside the working directory {} (rerun with --allow-outside-cwd to permit)",
                resolved.display(),
                boundary.display()
            ))
        }
    }
}

/// The successful outcome of a tool call.
///
/// Mirrors pi's `AgentToolResult`: `content` is shown to the model, `details` is
/// structured data for the UI or logs, and `terminate` hints that the agent may
/// stop after the current batch.
#[derive(Debug, Clone, Default)]
pub struct ToolResult {
    /// Content blocks returned to the model.
    pub content: Vec<ContentBlock>,
    /// Model-invisible structured details.
    pub details: Option<Value>,
    /// Hint that the agent should skip the automatic follow-up model call.
    pub terminate: bool,
}

impl ToolResult {
    /// A result carrying a single text block.
    pub fn text(text: impl Into<String>) -> Self {
        ToolResult {
            content: vec![ContentBlock::text(text)],
            details: None,
            terminate: false,
        }
    }
}

/// A tool the agent can execute.
///
/// Following pi's convention, a failed tool must return `Err(message)` rather than
/// encoding the failure in its content. The agent reports `Err` results to the model
/// with `is_error: true`.
///
/// # Why the error is a `String`
///
/// A tool's failure is ultimately *shown to the model* as text, so the message is the
/// payload, not a detail alongside it. A structured error enum would have to be
/// stringified for the model anyway, and every tool author would pay the cost of
/// declaring and converting into it. The kinds that genuinely need to be told apart —
/// "the source is unreachable" versus "the source answered with a business error" — are
/// decided **structurally**, by the code path, and fed to the source's circuit breaker
/// there (`sources/local.rs`, `sources/web.rs`, …), never by matching on the message.
/// So the message is free-form on purpose; a caller that needs more can inspect the
/// tool's own [`ToolResult::details`] on success, or the [`ToolExecutionEnd`] event.
///
/// [`ToolExecutionEnd`]: crate::AgentEvent::ToolExecutionEnd
///
/// # Examples
///
/// A tool is a [`ToolSpec`] (what the model sees) plus an [`execute`](Tool::execute)
/// implementation (what runs):
///
/// ```
/// use async_trait::async_trait;
/// use rsagent_agent::{Tool, ToolContext, ToolResult};
/// use rsagent_ai::ToolSpec;
/// use serde_json::{json, Value};
///
/// struct WordCount;
///
/// #[async_trait]
/// impl Tool for WordCount {
///     fn spec(&self) -> ToolSpec {
///         ToolSpec::new(
///             "word_count",
///             "Count the words in `text`.",
///             json!({ "type": "object", "properties": { "text": { "type": "string" } } }),
///         )
///     }
///
///     async fn execute(
///         &self,
///         _call_id: &str,
///         args: Value,
///         _context: &ToolContext,
///     ) -> Result<ToolResult, String> {
///         let text = args.get("text").and_then(Value::as_str).ok_or("missing `text`")?;
///         Ok(ToolResult::text(text.split_whitespace().count().to_string()))
///     }
/// }
///
/// let tool = WordCount;
/// assert_eq!(tool.spec().name, "word_count");
/// assert!(!tool.requires_approval());
/// ```
#[async_trait]
pub trait Tool: Send + Sync {
    /// The declaration the model sees.
    fn spec(&self) -> ToolSpec;

    /// A human-readable label for UI display. Defaults to the tool's wire name.
    fn label(&self) -> String {
        self.spec().name
    }

    /// Execute the tool. `args` is the parsed JSON arguments object.
    async fn execute(
        &self,
        call_id: &str,
        args: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, String>;

    /// Whether the whole tool batch must run sequentially because of this tool.
    fn is_sequential(&self) -> bool {
        false
    }

    /// Whether the agent must obtain user approval before running this tool.
    ///
    /// Tools that mutate the filesystem or run commands should return `true`.
    fn requires_approval(&self) -> bool {
        false
    }
}
