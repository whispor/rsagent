//! End-to-end tests that run the compiled `rsagent` binary.
//!
//! These exercise the CLI itself — argument parsing, provider wiring, and the terminal
//! renderer — which the crate-level tests do not cover.

use std::process::Command;

/// A command that runs the binary built for this test run.
fn rsagent() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rsagent"))
}

#[test]
fn faux_print_runs_a_turn_end_to_end() {
    let output = rsagent()
        .args(["--faux", "--print", "hello"])
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("(faux) you said: hello"),
        "unexpected stdout: {stdout}"
    );
}

#[test]
fn version_flag_prints_the_crate_version() {
    let output = rsagent().arg("--version").output().expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "unexpected version output: {stdout}"
    );
}

#[test]
fn help_lists_the_flags() {
    let output = rsagent().arg("--help").output().expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    for flag in [
        "--max-turns",
        "--temperature",
        "--max-tokens",
        "--anthropic",
    ] {
        assert!(
            stdout.contains(flag),
            "`{flag}` is missing from help: {stdout}"
        );
    }
}

#[test]
fn help_does_not_leak_env_values() {
    // The env var name should show up, but never its value.
    let output = rsagent()
        .env("RSAGENT_API_KEY", "sentinel-secret-value")
        .arg("--help")
        .output()
        .expect("run rsagent");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("RSAGENT_API_KEY"),
        "the env var name should appear in help: {stdout}"
    );
    assert!(
        !stdout.contains("sentinel-secret-value"),
        "the env value must never be printed: {stdout}"
    );
}

#[test]
fn tuning_flags_are_accepted() {
    let output = rsagent()
        .args([
            "--faux",
            "--print",
            "hi",
            "--max-turns",
            "5",
            "--temperature",
            "0.2",
            "--max-tokens",
            "512",
        ])
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("(faux) you said: hi"),
        "unexpected stdout: {stdout}"
    );
}

#[test]
fn a_zero_turn_limit_is_rejected() {
    let output = rsagent()
        .args(["--faux", "--print", "hi", "--max-turns", "0"])
        .output()
        .expect("run rsagent");

    assert!(
        !output.status.success(),
        "a zero turn limit must be rejected"
    );
}

#[test]
fn repl_handles_slash_commands() {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = rsagent()
        .arg("--faux")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rsagent");

    child
        .stdin
        .as_mut()
        .expect("stdin is piped")
        .write_all(b"/help\nhello\n/clear\nexit\n")
        .expect("write to stdin");

    let output = child.wait_with_output().expect("wait for rsagent");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("可用命令"), "help output missing: {stdout}");
    assert!(
        stdout.contains("(faux) you said: hello"),
        "the prompt was not answered: {stdout}"
    );
    assert!(
        stdout.contains("已清空会话记录"),
        "/clear output missing: {stdout}"
    );
}
