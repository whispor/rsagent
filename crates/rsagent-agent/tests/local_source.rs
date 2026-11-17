//! End-to-end tests for the `local` tool source.
//!
//! Each test writes a tiny tool program to a temporary directory and talks to it over the
//! real protocol. The program is a shell script, so the tests exercise the same spawn /
//! stdin / stdout path a real source would, on every platform.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rsagent_agent::{
    BreakerPolicy, BuiltinSource, CatalogError, LocalSource, SourceEntry, SourceHealth, SourceKind,
    ToolCatalog, ToolContext, ToolSource,
};

/// The `--describe` response for a tool named `name`.
fn describe_json(name: &str) -> String {
    serde_json::json!({
        "protocol": 1,
        "tools": [{
            "name": name,
            "description": "Ping the source.",
            "parameters": { "type": "object", "properties": {} },
            "requires_approval": false,
        }]
    })
    .to_string()
}

/// A directory unique to one test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rsagent-local-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the temp dir");
    dir
}

/// Write a tool program and return the `(program, args)` a `rsagent.toml` would use.
///
/// `call_json` is the `--call` response and `call_exit` its exit code: exit `0` is a
/// healthy transport, anything else fails the way a broken one does. Both phases read
/// stdin first, so a program that never receives a request exits with code 3 — which is
/// what makes these tests prove the request is actually written.
fn tool_program(dir: &Path, name: &str, call_json: &str, call_exit: u8) -> (String, Vec<String>) {
    let describe = describe_json(name);
    let exit = call_exit.to_string();
    if cfg!(windows) {
        let script = r#"@echo off
if "%~1"=="--describe" goto describe
if "%~1"=="--call" goto call
exit /b 2
:describe
echo __DESCRIBE__
exit /b 0
:call
set /p request=
if not defined request exit /b 3
echo __CALL__
exit /b __EXIT__
"#
        .replace("__DESCRIBE__", &describe)
        .replace("__CALL__", call_json)
        .replace("__EXIT__", &exit)
        .replace('\n', "\r\n");
        let path = dir.join("tool.cmd");
        std::fs::write(&path, script).expect("write tool.cmd");
        (
            "cmd".to_string(),
            vec!["/C".to_string(), path.display().to_string()],
        )
    } else {
        let script = r#"#!/bin/sh
case "$1" in
  --describe) printf '%s\n' '__DESCRIBE__' ;;
  --call)
    read -r request || exit 3
    printf '%s\n' '__CALL__'
    exit __EXIT__ ;;
  *) exit 2 ;;
esac
"#
        .replace("__DESCRIBE__", &describe)
        .replace("__CALL__", call_json)
        .replace("__EXIT__", &exit);
        let path = dir.join("tool.sh");
        std::fs::write(&path, script).expect("write tool.sh");
        ("sh".to_string(), vec![path.display().to_string()])
    }
}

/// Build a source from a fresh fixture program.
async fn source(dir: &Path, id: &str, name: &str, call_json: &str, call_exit: u8) -> LocalSource {
    let (program, args) = tool_program(dir, name, call_json, call_exit);
    LocalSource::new(id, program, args, Some(Duration::from_secs(30)))
        .await
        .expect("the fixture must describe itself")
}

const PONG: &str = r#"{"content":"pong"}"#;

#[tokio::test]
async fn a_local_source_describes_and_calls_its_tool() {
    let dir = temp_dir("round-trip");
    let source = source(&dir, "tool", "ping", PONG, 0).await;

    assert_eq!(source.kind(), SourceKind::Local);
    assert_eq!(source.id(), "tool");
    assert_eq!(source.health(), SourceHealth::Healthy);

    let tools = source.tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].spec().name, "ping");
    assert!(!tools[0].requires_approval(), "the program opted out");
    assert!(
        tools[0].is_sequential(),
        "external programs never run in parallel"
    );

    let context = ToolContext::new(&dir);
    let result = tools[0]
        .execute("call-1", serde_json::json!({}), &context)
        .await
        .expect("the call must succeed");
    assert_eq!(result.content[0].as_text(), Some("pong"));

    // A successful round-trip leaves the source healthy.
    assert_eq!(source.health(), SourceHealth::Healthy);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_program_that_cannot_be_run_fails_to_load() {
    let error = LocalSource::new(
        "missing",
        "definitely-not-a-real-program-xyz",
        Vec::new(),
        Some(Duration::from_secs(10)),
    )
    .await
    .expect_err("a missing program must fail");

    assert!(error.contains("cannot run"), "{error}");
}

#[tokio::test]
async fn a_program_that_does_not_speak_the_protocol_fails_to_load() {
    // `echo` exists on every platform, but its output is not a describe response.
    let error = LocalSource::new("chatty", "echo", vec!["hello".to_string()], None)
        .await
        .expect_err("non-JSON output must fail");

    assert!(error.contains("did not print JSON"), "{error}");
}

#[tokio::test]
async fn repeated_transport_failures_open_the_breaker() {
    let dir = temp_dir("breaker");
    let source = source(&dir, "flaky", "ping", PONG, 9)
        .await
        .with_breaker_policy(BreakerPolicy {
            threshold: 2,
            base_cooldown: Duration::from_secs(60),
            max_cooldown: Duration::from_secs(600),
        });
    let tools = source.tools();
    let tool = &tools[0];
    let context = ToolContext::new(&dir);

    // Two failing calls trip the breaker…
    for _ in 0..2 {
        let error = tool
            .execute("call", serde_json::json!({}), &context)
            .await
            .expect_err("the fixture exits non-zero");
        assert!(error.contains("exited with"), "{error}");
    }

    // …and the next call is refused without running the program at all.
    let error = tool
        .execute("call", serde_json::json!({}), &context)
        .await
        .expect_err("an open breaker refuses the call");
    assert!(error.contains("circuit open"), "{error}");

    match source.health() {
        SourceHealth::Unhealthy(reason) => assert!(reason.contains("circuit open"), "{reason}"),
        other => panic!("expected an unhealthy source, got {other:?}"),
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_business_error_does_not_trip_the_breaker() {
    // A well-formed `{"error": ...}` means the program is working; it must not be counted
    // against the source, or one always-failing tool would hide the real problem.
    let dir = temp_dir("business");
    let source = source(&dir, "honest", "ping", r#"{"error":"nope"}"#, 0).await;
    let tools = source.tools();
    let context = ToolContext::new(&dir);

    for _ in 0..3 {
        let error = tools[0]
            .execute("call", serde_json::json!({}), &context)
            .await
            .expect_err("the fixture reports an error");
        assert_eq!(error, "nope");
    }

    assert_eq!(
        source.health(),
        SourceHealth::Healthy,
        "a working program stays healthy"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn the_catalog_expands_and_prefixes_local_tools() {
    let dir = temp_dir("catalog");
    let local = source(&dir, "remote", "ping", PONG, 0).await;

    let catalog = ToolCatalog::build(vec![
        SourceEntry::new(Box::new(
            BuiltinSource::new().with_selection(Some(vec!["read_file".to_string()]), Vec::new()),
        )),
        SourceEntry::with_prefix(Box::new(local), "r"),
    ])
    .expect("build");

    let names: Vec<String> = catalog
        .tools()
        .iter()
        .map(|tool| tool.spec().name)
        .collect();
    assert_eq!(names, vec!["read_file", "r__ping"]);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn an_unprefixed_local_tool_collides_with_a_builtin() {
    let dir = temp_dir("collision");
    // Name the local tool after a built-in, with no prefix: the catalog must refuse.
    let local = source(&dir, "clash", "read_file", PONG, 0).await;

    let error = ToolCatalog::build(vec![
        SourceEntry::new(Box::new(BuiltinSource::new())),
        SourceEntry::new(Box::new(local)),
    ])
    .expect_err("the collision must be refused");

    match error {
        CatalogError::DuplicateTool {
            name,
            first,
            second,
        } => {
            assert_eq!(name, "read_file");
            assert_eq!(first, "core");
            assert_eq!(second, "clash");
        }
        other => panic!("unexpected error: {other:?}"),
    }

    std::fs::remove_dir_all(&dir).ok();
}
