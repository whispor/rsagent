//! Errors returned by the agent loop.

/// An error that ended an [`Agent::prompt`](crate::Agent::prompt) run.
///
/// Implements [`std::error::Error`], `Display`, `Send`, `Sync`, and `'static`, so
/// callers can wrap it in an application error type such as `anyhow::Error`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentError {
    /// The model stream ended without producing a final assistant message.
    #[error("model stream ended without a result")]
    NoResult,
    /// The agent reached its configured turn limit before the run finished.
    #[error("agent exceeded the maximum number of turns ({max_turns})")]
    MaxTurnsExceeded {
        /// The configured turn limit that was hit.
        max_turns: usize,
    },
    /// The model reported a terminal error (a bad key, a refused request, a network
    /// failure) and the run stopped early.
    ///
    /// The error is also surfaced as a [`StreamEvent::Error`](rsagent_ai::StreamEvent)
    /// event and in [`Agent::error_message`](crate::Agent::error_message), so a UI that
    /// renders the event stream shows the same reason; returning it here as well means a
    /// caller writing `prompt(..).await?` cannot silently treat the failure as success.
    /// A user-initiated [`abort`](crate::Agent::abort) is *not* an error.
    #[error("model request failed: {message}")]
    ModelFailed {
        /// The model's own error text.
        message: String,
    },
}
