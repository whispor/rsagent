//! The executable side of a tool.

use std::path::PathBuf;

use async_trait::async_trait;
use rsagent_ai::{ContentBlock, ToolSpec};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::cancel::CancelToken;

/// Ambient context handed to a tool on each execution.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Working directory used to resolve relative paths.
    pub cwd: PathBuf,
    progress: Option<UnboundedSender<String>>,
    cancel: CancelToken,
}

impl ToolContext {
    /// Create a context rooted at `cwd`, with no progress sink and a fresh cancel token.
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        ToolContext {
            cwd: cwd.into(),
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
