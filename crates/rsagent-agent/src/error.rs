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
}
