//! Tool-call approval: a policy hook the agent consults before running tools
//! that mutate state.

use async_trait::async_trait;
use serde_json::Value;

/// Decides whether a tool call may proceed.
///
/// The agent consults an `Approver` only for tools whose [`Tool::requires_approval`]
/// returns `true`; everything else runs unconditionally.
///
/// [`Tool::requires_approval`]: crate::tool::Tool::requires_approval
#[async_trait]
pub trait Approver: Send + Sync {
    /// Return `true` to allow the call, `false` to deny it.
    async fn approve(&self, tool_name: &str, args: &Value) -> bool;

    /// Whether the agent should announce a pending approval request for this call.
    ///
    /// Return `false` when the decision needs no user input — for example an approver
    /// that allows or denies everything, or one the user has told to stop asking. The
    /// agent then skips the [`ToolApprovalRequest`] event, so the UI shows no prompt.
    ///
    /// [`ToolApprovalRequest`]: crate::event::AgentEvent::ToolApprovalRequest
    fn needs_prompt(&self, _tool_name: &str) -> bool {
        true
    }
}

/// An [`Approver`] that allows every call.
///
/// This is the agent's default, so a library user gets the raw loop unless they
/// opt into a policy.
#[derive(Debug, Default, Clone, Copy)]
pub struct AllowAll;

#[async_trait]
impl Approver for AllowAll {
    async fn approve(&self, _tool_name: &str, _args: &Value) -> bool {
        true
    }

    fn needs_prompt(&self, _tool_name: &str) -> bool {
        false
    }
}

/// An [`Approver`] that denies every call.
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyAll;

#[async_trait]
impl Approver for DenyAll {
    async fn approve(&self, _tool_name: &str, _args: &Value) -> bool {
        false
    }

    fn needs_prompt(&self, _tool_name: &str) -> bool {
        false
    }
}
