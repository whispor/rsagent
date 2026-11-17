//! Platform differences, kept in exactly one place.
//!
//! Business code MUST NOT carry `#[cfg(...)]`: every OS-specific choice lives here, so a
//! platform bug has one file to look at and the three-OS CI matrix points straight at it.
//! See `docs/EXTENSIBILITY.md` §4.2.

use tokio::process::Command;

/// A shell command that runs `command` through the platform's shell.
///
/// Windows uses `cmd /C`; every other platform uses `sh -c`. The returned [`Command`] is
/// ready to be configured further (working directory, stdio) and spawned by the caller.
///
/// This is the single `#[cfg]` in the workspace; keeping it here is the point.
pub(crate) fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    let mut process = {
        let mut process = Command::new("cmd");
        process.arg("/C");
        process
    };
    #[cfg(not(windows))]
    let mut process = {
        let mut process = Command::new("sh");
        process.arg("-c");
        process
    };
    process.arg(command);
    process
}

#[cfg(test)]
mod tests {
    use super::shell_command;

    /// The one test that actually proves the platform shell is wired up — and it runs on
    /// all three CI operating systems.
    #[tokio::test]
    async fn runs_a_command_through_the_platform_shell() {
        let output = shell_command("echo hello")
            .output()
            .await
            .expect("spawn the platform shell");

        assert!(output.status.success(), "status: {:?}", output.status);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("hello"), "stdout: {stdout:?}");
    }
}
