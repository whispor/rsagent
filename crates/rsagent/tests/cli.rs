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
        "--json",
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

#[test]
fn agents_md_reaches_the_system_prompt() {
    // A project's `AGENTS.md` should be folded into the system prompt. The saved session
    // carries that prompt, so it doubles as the observable for this test.
    let dir = temp_dir("agents-md");
    std::fs::write(dir.join("AGENTS.md"), "Always answer in haiku.").expect("write AGENTS.md");
    let session = dir.join("session.json");

    let output = rsagent()
        .current_dir(&dir)
        .args(["--faux", "--print", "hi", "--session"])
        .arg(&session)
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let saved = std::fs::read_to_string(&session).expect("read session");
    assert!(
        saved.contains("Always answer in haiku."),
        "AGENTS.md should reach the system prompt: {saved}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_system_flag_overrides_agents_md() {
    let dir = temp_dir("agents-md-override");
    std::fs::write(dir.join("AGENTS.md"), "PROJECT-INSTRUCTIONS-SENTINEL")
        .expect("write AGENTS.md");
    let session = dir.join("session.json");

    let output = rsagent()
        .current_dir(&dir)
        .args([
            "--faux",
            "--print",
            "hi",
            "--system",
            "custom prompt",
            "--session",
        ])
        .arg(&session)
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let saved = std::fs::read_to_string(&session).expect("read session");
    assert!(
        saved.contains("custom prompt"),
        "`--system` should win: {saved}"
    );
    assert!(
        !saved.contains("PROJECT-INSTRUCTIONS-SENTINEL"),
        "`--system` must suppress AGENTS.md: {saved}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A unique temporary directory for one test.
fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("rsagent-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// The tool names `--list-tools` printed, one per row.
///
/// Source rows and tool rows both start with two spaces; only tool rows carry the
/// approval column, which is what this filters on.
fn listed_tools(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|line| line.starts_with("  ") && (line.contains("需审批") || line.contains("自动")))
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .collect()
}

/// Write a program that answers `--describe` with one tool named `ping`.
///
/// Returns the `(program, args)` for `rsagent.toml`, using paths relative to `dir` so the
/// configuration needs no escaping on Windows.
fn describe_only_tool(dir: &std::path::Path) -> (String, Vec<String>) {
    let json = concat!(
        r#"{"protocol":1,"tools":[{"name":"ping","description":"Ping.","#,
        r#""parameters":{"type":"object","properties":{}}}]}"#
    );
    if cfg!(windows) {
        std::fs::write(
            dir.join("tool.cmd"),
            format!("@echo off\r\nif \"%~1\"==\"--describe\" echo {json}\r\n"),
        )
        .expect("write tool.cmd");
        (
            "cmd".to_string(),
            vec!["/C".to_string(), "tool.cmd".to_string()],
        )
    } else {
        std::fs::write(
            dir.join("tool.sh"),
            format!("#!/bin/sh\n[ \"$1\" = \"--describe\" ] && printf '%s\\n' '{json}'\n"),
        )
        .expect("write tool.sh");
        ("sh".to_string(), vec!["tool.sh".to_string()])
    }
}

/// Write a `rsagent.toml` with one local source pointing at a fresh fixture.
fn write_local_config(dir: &std::path::Path) {
    let (program, args) = describe_only_tool(dir);
    let args = args
        .iter()
        .map(|arg| format!("\"{arg}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        dir.join("rsagent.toml"),
        format!(
            "[[tools.sources]]\nkind = \"local\"\nid = \"demo\"\ncommand = \"{program}\"\nargs = [{args}]\n"
        ),
    )
    .expect("write rsagent.toml");
}

#[test]
fn rsagent_toml_narrows_the_builtin_source() {
    let dir = temp_dir("config-builtin");
    std::fs::write(
        dir.join("rsagent.toml"),
        "[[tools.sources]]\nkind = \"builtin\"\nid = \"core\"\nexclude = [\"bash\"]\n",
    )
    .expect("write rsagent.toml");

    let output = rsagent()
        .current_dir(&dir)
        .arg("--list-tools")
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let tools = listed_tools(&stdout);
    assert!(tools.contains(&"read_file".to_string()), "{stdout}");
    assert!(
        !tools.contains(&"bash".to_string()),
        "the config excludes bash: {stdout}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_tools_flag_overrides_the_config() {
    let dir = temp_dir("config-override");
    std::fs::write(
        dir.join("rsagent.toml"),
        "[[tools.sources]]\nkind = \"builtin\"\nid = \"core\"\nonly = [\"read_file\"]\n",
    )
    .expect("write rsagent.toml");

    let output = rsagent()
        .current_dir(&dir)
        .args(["--list-tools", "--tools", "glob"])
        .output()
        .expect("run rsagent");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        listed_tools(&stdout),
        vec!["glob"],
        "the CLI flag wins over the config: {stdout}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_broken_config_is_reported_and_the_run_continues() {
    let dir = temp_dir("config-broken");
    // `kind` is missing, so the file cannot be parsed.
    std::fs::write(
        dir.join("rsagent.toml"),
        "[[tools.sources]]\nid = \"core\"\n",
    )
    .expect("write rsagent.toml");

    let output = rsagent()
        .current_dir(&dir)
        .args(["--faux", "--print", "hi"])
        .output()
        .expect("run rsagent");

    assert!(
        output.status.success(),
        "a bad config must not fail the run: {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ignoring config"),
        "the problem must be reported: {stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_config_driven_local_source_is_listed() {
    let dir = temp_dir("config-local");
    write_local_config(&dir);

    let output = rsagent()
        .current_dir(&dir)
        .arg("--list-tools")
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("demo"),
        "the source id is missing: {stdout}"
    );
    assert!(
        stdout.contains("[local]"),
        "the source kind is missing: {stdout}"
    );
    assert!(
        listed_tools(&stdout).contains(&"ping".to_string()),
        "the local tool is missing: {stdout}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The `mcp_echo` example binary, if it was built next to this test binary.
fn mcp_echo_program() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let profile = exe.parent()?.parent()?;
    let name = if cfg!(windows) {
        "mcp_echo.exe"
    } else {
        "mcp_echo"
    };
    let path = profile.join("examples").join(name);
    path.exists().then_some(path)
}

#[test]
fn a_config_driven_mcp_stdio_source_is_listed() {
    let Some(program) = mcp_echo_program() else {
        eprintln!("skipping: the `mcp_echo` example was not built");
        return;
    };
    let dir = temp_dir("config-mcp");
    // Escape backslashes so the path is a valid TOML basic string on Windows.
    let command = program.display().to_string().replace('\\', "\\\\");
    std::fs::write(
        dir.join("rsagent.toml"),
        format!(
            "[[tools.sources]]\nkind = \"mcp-stdio\"\nid = \"echo\"\ncommand = \"{command}\"\nprefix = \"echo\"\n"
        ),
    )
    .expect("write rsagent.toml");

    let output = rsagent()
        .current_dir(&dir)
        .arg("--list-tools")
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[mcp-stdio]"),
        "the source kind is missing: {stdout}"
    );
    assert!(
        listed_tools(&stdout).contains(&"echo__echo".to_string()),
        "the prefixed MCP tool is missing: {stdout}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_config_driven_skill_source_is_listed() {
    let dir = temp_dir("config-skill");
    // A skill folder: the marker, a manifest, and the script each tool runs.
    let skill = dir.join("team");
    std::fs::create_dir_all(&skill).expect("create the skill dir");
    std::fs::write(skill.join("SKILL.md"), "# A test skill\n").expect("write SKILL.md");
    let script = if cfg!(windows) { "run.cmd" } else { "run.sh" };
    std::fs::write(skill.join(script), "exit 0\n").expect("write the script");
    std::fs::write(
        skill.join("skill.toml"),
        format!(
            "[[tool]]\nname = \"lint\"\ndescription = \"Lint a path.\"\ncommand = \"{script}\"\n"
        ),
    )
    .expect("write skill.toml");
    std::fs::write(
        dir.join("rsagent.toml"),
        "[[tools.sources]]\nkind = \"skill\"\nid = \"team\"\npath = \"team\"\nprefix = \"team\"\n",
    )
    .expect("write rsagent.toml");

    let output = rsagent()
        .current_dir(&dir)
        .arg("--list-tools")
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[skill]"),
        "the source kind is missing: {stdout}"
    );
    assert!(
        listed_tools(&stdout).contains(&"team__lint".to_string()),
        "the prefixed skill tool is missing: {stdout}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_config_driven_web_source_is_listed() {
    let dir = temp_dir("config-web");
    // No connection is made at load time, so the URL need not be live.
    std::fs::write(
        dir.join("rsagent.toml"),
        "[[tools.sources]]\nkind = \"web\"\nid = \"weather\"\nurl = \"https://api.example.com/weather\"\nname = \"get_weather\"\nprefix = \"wx\"\n",
    )
    .expect("write rsagent.toml");

    let output = rsagent()
        .current_dir(&dir)
        .arg("--list-tools")
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[web]"),
        "the source kind is missing: {stdout}"
    );
    assert!(
        listed_tools(&stdout).contains(&"wx__get_weather".to_string()),
        "the prefixed web tool is missing: {stdout}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_unreachable_mcp_http_source_is_reported_and_skipped() {
    let dir = temp_dir("config-mcp-http");
    // Port 1 on loopback is never listening, so the handshake fails at load time.
    std::fs::write(
        dir.join("rsagent.toml"),
        "[[tools.sources]]\nkind = \"mcp-http\"\nid = \"remote\"\nurl = \"http://127.0.0.1:1/mcp\"\n",
    )
    .expect("write rsagent.toml");

    let output = rsagent()
        .current_dir(&dir)
        .arg("--list-tools")
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("skipping tool source `remote`"),
        "the unreachable source must be reported and skipped: {stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_unknown_source_kind_is_reported_and_skipped() {
    let dir = temp_dir("config-unknown-kind");
    std::fs::write(
        dir.join("rsagent.toml"),
        "[[tools.sources]]\nkind = \"future\"\nid = \"future\"\n",
    )
    .expect("write rsagent.toml");

    let output = rsagent()
        .current_dir(&dir)
        .arg("--list-tools")
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown source kind `future`"),
        "the unsupported kind must be reported: {stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn json_emits_the_event_stream_as_json_lines() {
    // `--json` is the machine-readable counterpart of `--print`: stdout carries one JSON
    // object per event, so a script can follow the run without parsing terminal output.
    let output = rsagent()
        .args(["--faux", "--print", "hi", "--json"])
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut kinds = Vec::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let value: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("line is not JSON: {line:?} ({error})"));
        let kind = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("event has no `type`: {line:?}"));
        kinds.push(kind.to_string());
    }

    assert!(kinds.iter().any(|k| k == "agentStart"), "kinds: {kinds:?}");
    assert!(kinds.iter().any(|k| k == "turnEnd"), "kinds: {kinds:?}");
    assert!(kinds.iter().any(|k| k == "agentEnd"), "kinds: {kinds:?}");
}

#[test]
fn json_without_print_is_rejected() {
    let output = rsagent()
        .args(["--faux", "--json"])
        .output()
        .expect("run rsagent");

    assert!(
        !output.status.success(),
        "`--json` without `--print` must be refused"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--json requires --print"),
        "the reason must be explained: {stderr}"
    );
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
