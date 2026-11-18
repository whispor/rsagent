//! Tool-call approval: a policy hook the agent consults before running tools
//! that mutate state.

use async_trait::async_trait;
use serde_json::Value;

/// The kind of side effect a tool has.
///
/// A tool declares its category with [`Tool::permission`], and the agent passes it
/// to the [`Approver`] so a policy can be finer than "ask for every mutating tool":
/// for example, auto-approve reads and network calls while still prompting for writes
/// and shell commands. The command-line `--allow` flag selects categories to allow.
///
/// [`Tool::permission`]: crate::tool::Tool::permission
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Reads files or state; no side effects.
    Read,
    /// Writes or edits files.
    Write,
    /// Runs a command or process.
    Exec,
    /// Makes a network request.
    Network,
}

impl Permission {
    /// Every category, in a stable order. The single source of truth for "which
    /// categories exist" — used by the CLI to parse `--allow` and to print usage.
    pub const ALL: [Permission; 4] = [
        Permission::Read,
        Permission::Write,
        Permission::Exec,
        Permission::Network,
    ];

    /// The wire name, as accepted by `--allow` and shown to the user.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Permission::Read => "read",
            Permission::Write => "write",
            Permission::Exec => "exec",
            Permission::Network => "network",
        }
    }

    /// Parse a category from its wire name (case-insensitive).
    ///
    /// Returns `None` for an unknown name; callers report the error and list
    /// [`Permission::ALL`].
    #[must_use]
    pub fn parse(name: &str) -> Option<Permission> {
        let name = name.trim();
        Permission::ALL
            .into_iter()
            .find(|permission| permission.as_str().eq_ignore_ascii_case(name))
    }
}

impl std::fmt::Display for Permission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Decides whether a tool call may proceed.
///
/// The agent consults an `Approver` only for tools whose [`Tool::requires_approval`]
/// returns `true`; everything else runs unconditionally. The tool's [`Permission`]
/// category is passed alongside the name and arguments so the policy can vary by kind.
///
/// [`Tool::requires_approval`]: crate::tool::Tool::requires_approval
#[async_trait]
pub trait Approver: Send + Sync {
    /// Return `true` to allow the call, `false` to deny it.
    async fn approve(&self, tool_name: &str, permission: Permission, args: &Value) -> bool;

    /// Whether the agent should announce a pending approval request for this call.
    ///
    /// Return `false` when the decision needs no user input — for example an approver
    /// that allows or denies everything, one the user has told to stop asking, or one
    /// that auto-approves this category. The agent then skips the
    /// [`ToolApprovalRequest`] event, so the UI shows no prompt.
    ///
    /// [`ToolApprovalRequest`]: crate::event::AgentEvent::ToolApprovalRequest
    fn needs_prompt(&self, _tool_name: &str, _permission: Permission) -> bool {
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
    async fn approve(&self, _tool_name: &str, _permission: Permission, _args: &Value) -> bool {
        true
    }

    fn needs_prompt(&self, _tool_name: &str, _permission: Permission) -> bool {
        false
    }
}

/// An [`Approver`] that denies every call.
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyAll;

#[async_trait]
impl Approver for DenyAll {
    async fn approve(&self, _tool_name: &str, _permission: Permission, _args: &Value) -> bool {
        false
    }

    fn needs_prompt(&self, _tool_name: &str, _permission: Permission) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::Permission;

    #[test]
    fn parse_accepts_wire_names_case_insensitively() {
        assert_eq!(Permission::parse("read"), Some(Permission::Read));
        assert_eq!(Permission::parse("WRITE"), Some(Permission::Write));
        assert_eq!(Permission::parse(" Exec "), Some(Permission::Exec));
        assert_eq!(Permission::parse("Network"), Some(Permission::Network));
    }

    #[test]
    fn parse_rejects_unknown_names() {
        assert_eq!(Permission::parse("delete"), None);
        assert_eq!(Permission::parse(""), None);
    }

    #[test]
    fn all_and_as_str_round_trip() {
        for permission in Permission::ALL {
            assert_eq!(Permission::parse(permission.as_str()), Some(permission));
            assert_eq!(permission.to_string(), permission.as_str());
        }
    }
}
