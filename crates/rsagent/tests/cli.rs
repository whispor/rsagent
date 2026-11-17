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
fn print_stdout_is_only_the_answer() {
    // In `--print` mode stdout carries the answer and nothing else, so
    // `rsagent --print ... > answer.txt` yields a clean file: no progress chatter and
    // no trailing blank line. Progress goes to stderr instead.
    let output = rsagent()
        .args(["--faux", "--print", "hi"])
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, "(faux) you said: hi\n", "stdout: {stdout:?}");
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
        "--continue",
        "--no-color",
        "--allow-outside-cwd",
        "--tools",
        "--no-bash",
        "--thinking",
        "--list-tools",
    ] {
        assert!(
            stdout.contains(flag),
            "`{flag}` is missing from help: {stdout}"
        );
    }
}

#[test]
fn unknown_tool_names_are_reported() {
    let output = rsagent()
        .args(["--faux", "--print", "hi", "--tools", "read_file,nope"])
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown tool in --tools: nope"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn an_invalid_thinking_level_is_rejected() {
    let output = rsagent()
        .args(["--faux", "--print", "hi", "--thinking", "bogus"])
        .output()
        .expect("run rsagent");

    assert!(!output.status.success(), "an invalid level must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown thinking level"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn list_tools_prints_the_builtins_and_exits() {
    let output = rsagent().arg("--list-tools").output().expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    for name in [
        "read_file",
        "write_file",
        "edit_file",
        "list_dir",
        "search",
        "glob",
        "webfetch",
        "bash",
    ] {
        assert!(
            stdout.contains(name),
            "`{name}` is missing from --list-tools: {stdout}"
        );
    }
    // The approval requirement is the point of the listing.
    assert!(
        stdout.contains("需审批"),
        "the approval column is missing: {stdout}"
    );
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
fn a_print_run_without_an_api_key_exits_nonzero() {
    // The default provider needs a key; with none it fails the turn, and `--print`
    // must report that with a non-zero exit code so scripts can detect it. A temp
    // working directory keeps any nearby `.env` from supplying a key.
    let dir = temp_dir("no-key");
    let output = rsagent()
        .current_dir(&dir)
        .env_remove("OPENAI_API_KEY")
        .env_remove("RSAGENT_API_KEY")
        .args(["--print", "hi"])
        .output()
        .expect("run rsagent");

    assert!(
        !output.status.success(),
        "a missing API key must fail the run: {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing API key"),
        "stderr should explain the failure: {stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
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

/// A unique temporary directory for one test.
fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("rsagent-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn no_color_output_has_no_escapes() {
    let output = rsagent()
        .args(["--faux", "--print", "hi", "--no-color"])
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains('\u{1b}'),
        "no ANSI escapes expected: {stdout:?}"
    );
}

#[test]
fn a_versioned_session_is_resumed() {
    let dir = temp_dir("session-resume");
    let session = dir.join("work.json");
    std::fs::write(
        &session,
        r#"{"version":1,"context":{"systemPrompt":"saved prompt","messages":[]}}"#,
    )
    .expect("write session");

    let output = rsagent()
        .args(["--faux", "--print", "hi", "--session"])
        .arg(&session)
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("resumed"), "stderr: {stderr}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_legacy_session_without_a_version_is_still_read() {
    let dir = temp_dir("session-legacy");
    let session = dir.join("old.json");
    // The old format: a bare `Context` with no version envelope.
    std::fs::write(&session, r#"{"systemPrompt":"legacy","messages":[]}"#).expect("write session");

    let output = rsagent()
        .args(["--faux", "--print", "hi", "--session"])
        .arg(&session)
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("resumed"), "stderr: {stderr}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_newer_session_format_is_rejected_but_the_run_continues() {
    let dir = temp_dir("session-newer");
    let session = dir.join("future.json");
    std::fs::write(&session, r#"{"version":99,"context":{"messages":[]}}"#).expect("write session");

    let output = rsagent()
        .args(["--faux", "--print", "hi", "--session"])
        .arg(&session)
        .output()
        .expect("run rsagent");

    assert!(
        output.status.success(),
        "the run still succeeds: {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("could not read"), "stderr: {stderr}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn continue_creates_and_then_resumes_the_default_session() {
    let dir = temp_dir("session-continue");

    let first = rsagent()
        .current_dir(&dir)
        .args(["--faux", "--print", "hi", "--continue"])
        .output()
        .expect("run rsagent");
    assert!(first.status.success(), "exit status: {:?}", first.status);
    assert!(
        dir.join(".rsagent-session.json").exists(),
        "the default session file should be created"
    );

    let second = rsagent()
        .current_dir(&dir)
        .args(["--faux", "--print", "hi", "--continue"])
        .output()
        .expect("run rsagent");
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("resumed"), "stderr: {stderr}");

    std::fs::remove_dir_all(&dir).ok();
}
