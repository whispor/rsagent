//! End-to-end test for the `mcp-stdio` tool source.
//!
//! It spawns the real `mcp_echo` example and talks to it over the actual protocol, so the
//! spawn / stdin / stdout path is exercised on every platform. The protocol logic itself is
//! covered by the unit tests in `src/sources/mcp_stdio.rs`, which drive it over an
//! in-memory pipe.

use std::path::PathBuf;
use std::time::Duration;

use rsagent_agent::{McpStdioSource, SourceHealth, SourceKind, ToolContext, ToolSource};
use serde_json::json;

/// The `mcp_echo` example binary, if it was built next to this test binary.
///
/// `cargo test` builds examples, so it normally exists; when it does not (e.g. a
/// `cargo test -p rsagent-agent --test mcp_stdio` that skipped examples), the test skips
/// rather than failing.
fn example_program() -> Option<PathBuf> {
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

#[tokio::test]
async fn an_mcp_stdio_source_talks_to_a_real_server() {
    let Some(program) = example_program() else {
        eprintln!("skipping: the `mcp_echo` example was not built");
        return;
    };

    let source = McpStdioSource::new(
        "echo",
        program.display().to_string(),
        Vec::new(),
        Some(Duration::from_secs(30)),
    )
    .await
    .expect("the server must handshake and list its tools");

    assert_eq!(source.kind(), SourceKind::McpStdio);
    assert_eq!(source.id(), "echo");
    assert_eq!(source.health(), SourceHealth::Healthy);

    let tools = source.tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].spec().name, "echo");
    assert!(
        tools[0].requires_approval(),
        "the server declares no read-only hint"
    );
    assert!(tools[0].is_sequential());

    let context = ToolContext::new(".");
    let result = tools[0]
        .execute("call-1", json!({ "text": "hello" }), &context)
        .await
        .expect("the call must succeed");
    assert_eq!(result.content[0].as_text(), Some("hello"));

    // A successful round-trip leaves the source healthy.
    assert_eq!(source.health(), SourceHealth::Healthy);
}

#[tokio::test]
async fn a_missing_mcp_server_fails_to_load() {
    let error = McpStdioSource::new(
        "missing",
        "definitely-not-a-real-mcp-server-xyz",
        Vec::new(),
        Some(Duration::from_secs(10)),
    )
    .await
    .expect_err("a missing program must fail");

    assert!(error.contains("cannot start MCP server"), "{error}");
}
