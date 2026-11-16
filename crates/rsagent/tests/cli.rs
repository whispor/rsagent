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
