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
    // The default provider needs a key. With none, the run fails fast — before any request
    // — with an actionable message, and `--print` reports that with a non-zero exit code so
    // scripts can detect it. A temp working directory keeps any nearby `.env` from supplying
    // a key.
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
    assert_eq!(
        output.status.code(),
        Some(2),
        "a missing key is a usage error, so exit 2"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing API key"),
        "stderr should explain the failure: {stderr}"
    );
    // The message must be actionable, not just a diagnosis.
    assert!(
        stderr.contains("OPENAI_API_KEY"),
        "stderr should name the env var to set: {stderr}"
    );
    assert!(
        stderr.contains("--api-key"),
        "stderr should offer the --api-key flag: {stderr}"
    );
    assert!(
        stderr.contains("--faux"),
        "stderr should mention the offline option: {stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_custom_base_url_without_a_key_only_warns() {
    // A custom `--base-url` often points at a local endpoint that needs no key, so the
    // preflight must warn and continue rather than refusing to start. Pointing at a
    // guaranteed-closed port makes the turn fail fast, but the warning must still appear.
    let dir = temp_dir("base-url-no-key");
    let output = rsagent()
        .current_dir(&dir)
        .env_remove("OPENAI_API_KEY")
        .env_remove("RSAGENT_API_KEY")
        .args(["--print", "hi", "--base-url", "http://127.0.0.1:1/v1"])
        .output()
        .expect("run rsagent");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("continuing because --base-url was set"),
        "a custom base URL should only warn about the missing key: {stderr}"
    );
    assert!(
        !stderr.contains("missing API key: no"),
        "the custom base URL must not trigger the fatal preflight: {stderr}"
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
fn a_session_write_is_atomic_and_leaves_no_temp_file() {
    // The session is written to a sibling temp file and renamed into place, so a crash
    // can never leave a half-written file behind, and no temp file survives a clean run.
    let dir = temp_dir("session-atomic");
    let session = dir.join("work.json");

    let output = rsagent()
        .current_dir(&dir)
        .args(["--faux", "--print", "hi", "--session"])
        .arg(&session)
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let saved = std::fs::read_to_string(&session).expect("read session");
    let value: serde_json::Value = serde_json::from_str(&saved).expect("the session is valid JSON");
    assert_eq!(value["version"], serde_json::json!(1));

    let leftovers: Vec<String> = std::fs::read_dir(&dir)
        .expect("read the working directory")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            std::path::Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("tmp"))
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "the atomic write left temp files behind: {leftovers:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_jsonl_session_is_appended_and_resumed() {
    // A `.jsonl` session is a header line plus one message per line, appended to across
    // runs rather than rewritten. The second run must resume it and grow the file.
    let dir = temp_dir("session-jsonl");
    let session = dir.join("work.jsonl");

    let first = rsagent()
        .current_dir(&dir)
        .args(["--faux", "--print", "one", "--session"])
        .arg(&session)
        .output()
        .expect("run rsagent");
    assert!(first.status.success(), "exit status: {:?}", first.status);

    let text = std::fs::read_to_string(&session).expect("read session");
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert!(
        lines[0].contains("\"type\":\"header\""),
        "the first line is the header: {}",
        lines[0]
    );
    assert!(lines.len() >= 3, "expected a header and messages: {text}");
    for line in &lines[1..] {
        let value: serde_json::Value = serde_json::from_str(line).expect("a message line is JSON");
        assert!(
            value.get("role").is_some(),
            "message line has a role: {line}"
        );
    }

    let second = rsagent()
        .current_dir(&dir)
        .args(["--faux", "--print", "two", "--session"])
        .arg(&session)
        .output()
        .expect("run rsagent");
    assert!(second.status.success(), "exit status: {:?}", second.status);
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("resumed"), "stderr: {stderr}");

    let after = std::fs::read_to_string(&session).expect("read session");
    let grown = after.lines().filter(|line| !line.trim().is_empty()).count();
    assert!(
        grown > lines.len(),
        "the JSONL session should have grown: {after}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_truncated_jsonl_tail_is_tolerated() {
    // A crash can leave a half-written last line. Loading must keep the valid messages
    // before it, and the next save must rewrite (not append) so the bad bytes are gone.
    let dir = temp_dir("session-jsonl-truncated");
    let session = dir.join("work.jsonl");
    let text = concat!(
        "{\"type\":\"header\",\"version\":1,\"systemPrompt\":\"saved\"}\n",
        "{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}],\"timestamp\":1}\n",
        "{\"role\":\"assistant\",\"content\":[{\"type\":\"te"
    );
    std::fs::write(&session, text).expect("write session");

    let output = rsagent()
        .current_dir(&dir)
        .args(["--faux", "--print", "next", "--session"])
        .arg(&session)
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("resumed 1 message(s)"), "stderr: {stderr}");
    assert!(!stderr.contains("could not read"), "stderr: {stderr}");
    // Dropping the tail is reported, not silent.
    assert!(
        stderr.contains("truncated line"),
        "stderr should say the tail was dropped: {stderr}"
    );

    // Every line is now well-formed JSON: the truncated tail was cleaned up by the rewrite.
    let after = std::fs::read_to_string(&session).expect("read session");
    for line in after.lines().filter(|line| !line.trim().is_empty()) {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|error| panic!("line is not JSON: {line:?} ({error})"));
    }

    std::fs::remove_dir_all(&dir).ok();
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

#[test]
fn tools_help_lists_the_subcommands() {
    let output = rsagent()
        .args(["tools", "--help"])
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    for sub in ["list", "doctor", "add", "remove", "reset"] {
        assert!(
            stdout.contains(sub),
            "`{sub}` is missing from `tools --help`: {stdout}"
        );
    }
}

#[test]
fn tools_list_prints_the_builtins() {
    // `tools list` and the `--list-tools` flag share one catalog builder, so the same
    // builtin tools must appear either way.
    let output = rsagent()
        .args(["tools", "list"])
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    for name in ["read_file", "write_file", "bash", "webfetch"] {
        assert!(
            stdout.contains(name),
            "`{name}` is missing from `tools list`: {stdout}"
        );
    }
    assert!(stdout.contains("core [builtin]"), "stdout: {stdout}");
}

#[test]
fn tools_add_writes_a_valid_entry() {
    let dir = temp_dir("tools-add");
    let output = rsagent()
        .current_dir(&dir)
        .args([
            "tools",
            "add",
            "weather",
            "--kind",
            "web",
            "--url",
            "https://example.com/api",
            "--name",
            "get_weather",
        ])
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let text = std::fs::read_to_string(dir.join("rsagent.toml")).expect("read rsagent.toml");
    assert!(text.contains("[[tools.sources]]"), "{text}");
    assert!(text.contains("id = \"weather\""), "{text}");
    assert!(text.contains("kind = \"web\""), "{text}");
    assert!(text.contains("url = \"https://example.com/api\""), "{text}");
    assert!(text.contains("name = \"get_weather\""), "{text}");
    // The `[tools]` table is implicit, so the file reads as a plain list of
    // `[[tools.sources]]` entries with no bare `[tools]` header line.
    assert!(
        !text.lines().any(|line| line.trim() == "[tools]"),
        "unexpected bare [tools] header: {text}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tools_add_rejects_a_duplicate_id() {
    let dir = temp_dir("tools-add-duplicate");
    let first = rsagent()
        .current_dir(&dir)
        .args([
            "tools",
            "add",
            "weather",
            "--kind",
            "web",
            "--url",
            "https://example.com/api",
        ])
        .output()
        .expect("run rsagent");
    assert!(first.status.success(), "exit status: {:?}", first.status);

    let second = rsagent()
        .current_dir(&dir)
        .args([
            "tools",
            "add",
            "weather",
            "--kind",
            "web",
            "--url",
            "https://other.example/api",
        ])
        .output()
        .expect("run rsagent");
    assert!(
        !second.status.success(),
        "a duplicate id must be refused, not overwritten"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("already exists"), "stderr: {stderr}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tools_add_then_remove_round_trips() {
    let dir = temp_dir("tools-roundtrip");
    let added = rsagent()
        .current_dir(&dir)
        .args([
            "tools",
            "add",
            "weather",
            "--kind",
            "web",
            "--url",
            "https://example.com/api",
        ])
        .output()
        .expect("run rsagent");
    assert!(added.status.success(), "exit status: {:?}", added.status);

    let removed = rsagent()
        .current_dir(&dir)
        .args(["tools", "remove", "weather"])
        .output()
        .expect("run rsagent");
    assert!(
        removed.status.success(),
        "exit status: {:?}",
        removed.status
    );

    let text = std::fs::read_to_string(dir.join("rsagent.toml")).expect("read rsagent.toml");
    assert!(
        !text.contains("weather"),
        "the entry should be gone: {text}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tools_remove_unknown_id_is_an_error() {
    let dir = temp_dir("tools-remove-missing");
    std::fs::write(
        dir.join("rsagent.toml"),
        "[[tools.sources]]\nkind = \"builtin\"\nid = \"core\"\n",
    )
    .expect("write rsagent.toml");

    let output = rsagent()
        .current_dir(&dir)
        .args(["tools", "remove", "ghost"])
        .output()
        .expect("run rsagent");
    assert!(!output.status.success(), "removing an unknown id must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no source with id `ghost`"),
        "stderr: {stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tools_doctor_reports_a_healthy_builtin() {
    let dir = temp_dir("tools-doctor-ok");
    let output = rsagent()
        .current_dir(&dir)
        .args(["tools", "doctor"])
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("core [builtin]"), "stdout: {stdout}");
    assert!(stdout.contains("正常"), "stdout: {stdout}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tools_doctor_fails_on_an_unloadable_source() {
    // `doctor` is usable as a CI gate: an unloadable source makes it exit non-zero and
    // prints a kind-specific fix hint.
    let dir = temp_dir("tools-doctor-bad");
    std::fs::write(
        dir.join("rsagent.toml"),
        "[[tools.sources]]\nkind = \"local\"\nid = \"broken\"\ncommand = \"rsagent-no-such-program-xyz\"\n",
    )
    .expect("write rsagent.toml");

    let output = rsagent()
        .current_dir(&dir)
        .args(["tools", "doctor"])
        .output()
        .expect("run rsagent");

    assert!(
        !output.status.success(),
        "doctor must fail when a source will not load"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("broken"), "stdout: {stdout}");
    assert!(stdout.contains("检查"), "a fix hint is expected: {stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("个来源有问题"), "stderr: {stderr}");

    std::fs::remove_dir_all(&dir).ok();
}

/// Write a breaker snapshot with one tripped source, at `dir`.
///
/// `opened_at` is far in the future so the cooldown can never have elapsed: the source
/// stays `Open` no matter when the test runs.
fn write_open_breaker(dir: &std::path::Path, id: &str) {
    let snapshot = format!(
        "{{\n  \"version\": 1,\n  \"breakers\": {{\n    \"{id}\": {{ \
         \"state\": \"open\", \"failures\": 3, \"openedAtUnix\": 9999999999, \
         \"cooldownSecs\": 60 }}\n  }}\n}}\n"
    );
    std::fs::write(dir.join(".rsagent-breakers.json"), snapshot).expect("write breaker file");
}

#[test]
fn tools_reset_clears_a_persisted_breaker() {
    let dir = temp_dir("tools-reset");
    write_open_breaker(&dir, "api");

    let output = rsagent()
        .current_dir(&dir)
        .args(["tools", "reset", "api"])
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("api"), "stdout: {stdout}");
    // The file is gone once nothing is tripped, so the next run starts clean.
    assert!(
        !dir.join(".rsagent-breakers.json").exists(),
        "the snapshot must be removed when empty"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tools_reset_with_nothing_to_clear_is_a_no_op() {
    let dir = temp_dir("tools-reset-empty");
    let output = rsagent()
        .current_dir(&dir)
        .args(["tools", "reset"])
        .output()
        .expect("run rsagent");

    assert!(
        output.status.success(),
        "resetting nothing must still succeed: {:?}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("没有熔断状态"), "stdout: {stdout}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tools_list_shows_a_persisted_breaker() {
    // A configured source whose breaker is stored as open must show as unhealthy, and
    // the snapshot must be listed explicitly.
    let dir = temp_dir("tools-list-breaker");
    std::fs::write(
        dir.join("rsagent.toml"),
        "[[tools.sources]]\nkind = \"web\"\nid = \"api\"\nurl = \"https://example.com/api\"\nname = \"call_api\"\n",
    )
    .expect("write rsagent.toml");
    write_open_breaker(&dir, "api");

    let output = rsagent()
        .current_dir(&dir)
        .args(["tools", "list"])
        .output()
        .expect("run rsagent");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("熔断器状态"),
        "the snapshot must be listed: {stdout}"
    );
    assert!(
        stdout.contains("circuit open"),
        "the seeded source must read as unhealthy: {stdout}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
