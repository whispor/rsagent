//! End-to-end tests for the agent loop, driven by the offline `FauxProvider`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream;
use rsagent_agent::{
    builtin_tools, memory_tools, Agent, AgentError, AgentEvent, Approver, Memory, PathFence,
    Permission, Tool, ToolContext, ToolResult, UnhealthySource,
};
use rsagent_ai::{
    AssistantMessage, ContentBlock, Context, EventStream, FauxProvider, FauxResponse, Message,
    Model, Models, Provider, StopReason, StreamEvent, StreamOptions, ToolSpec,
};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedReceiver;

/// Build an agent with the built-in tools and a scripted faux provider.
fn harness(responses: Vec<FauxResponse>) -> (Agent, FauxProvider, UnboundedReceiver<AgentEvent>) {
    harness_in(std::env::current_dir().expect("current dir"), responses)
}

/// Like [`harness`], but rooted at `cwd` so the read-tool fence admits paths under it.
fn harness_in(
    cwd: impl Into<PathBuf>,
    responses: Vec<FauxResponse>,
) -> (Agent, FauxProvider, UnboundedReceiver<AgentEvent>) {
    let faux = FauxProvider::new();
    faux.set_responses(responses);

    let mut models = Models::new();
    models.register(faux.clone());

    let mut agent = Agent::new(models, faux.model(), "system")
        .with_tools(builtin_tools())
        .with_cwd(cwd);
    let (tx, events) = tokio::sync::mpsc::unbounded_channel();
    agent.subscribe(tx);

    (agent, faux, events)
}

fn assistant_texts(agent: &Agent) -> Vec<String> {
    agent
        .messages()
        .iter()
        .filter_map(|m| match m {
            Message::Assistant(a) => Some(a.to_text()),
            _ => None,
        })
        .collect()
}

/// The text of every user message in the transcript, in order.
fn user_texts(agent: &Agent) -> Vec<String> {
    agent
        .messages()
        .iter()
        .filter_map(|m| match m {
            Message::User(u) => Some(
                u.content
                    .iter()
                    .filter_map(ContentBlock::as_text)
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn completes_a_text_turn() {
    let (mut agent, _faux, mut events) = harness(vec![FauxResponse::text("hello there")]);

    agent.prompt("hi").await.expect("prompt completes");

    assert_eq!(assistant_texts(&agent), vec!["hello there".to_string()]);

    let mut saw_tool = false;
    while let Ok(event) = events.try_recv() {
        saw_tool |= matches!(event, AgentEvent::ToolExecutionStart { .. });
    }
    assert!(!saw_tool, "a plain text turn must not run tools");
}

#[tokio::test]
async fn executes_a_write_file_tool_call() {
    let dir = std::env::temp_dir().join(format!("rsagent-agent-it-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("note.txt");
    let path = file.to_string_lossy().into_owned();

    let (mut agent, _faux, _events) = harness(vec![
        FauxResponse::tool_calls(vec![(
            "write_file",
            json!({ "path": path, "content": "abc" }),
        )]),
        FauxResponse::text("done"),
    ]);

    agent
        .prompt("write the file")
        .await
        .expect("prompt completes");

    assert_eq!(
        std::fs::read_to_string(&file).expect("file was written"),
        "abc"
    );
    let tool_results = agent
        .messages()
        .iter()
        .filter(|m| matches!(m, Message::ToolResult(_)))
        .count();
    assert_eq!(tool_results, 1);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn unknown_tools_are_reported_as_errors() {
    let (mut agent, _faux, _events) = harness(vec![
        FauxResponse::tool_calls(vec![("does_not_exist", json!({}))]),
        FauxResponse::text("ok"),
    ]);

    agent.prompt("go").await.expect("prompt completes");

    let failed = agent.messages().iter().find_map(|m| match m {
        Message::ToolResult(r) if r.is_error => Some(r.tool_name.clone()),
        _ => None,
    });
    assert_eq!(failed.as_deref(), Some("does_not_exist"));
}

#[tokio::test]
async fn a_newly_unhealthy_source_is_reported_once() {
    // A tool call then a text answer is two turns; a source that stays unhealthy across
    // both must be announced exactly once, not once per turn (§3.5).
    let (agent, _faux, mut events) = harness(vec![
        FauxResponse::tool_calls(vec![("does_not_exist", json!({}))]),
        FauxResponse::text("done"),
    ]);
    let mut agent = agent.with_health_probe(Arc::new(|| {
        vec![UnhealthySource {
            id: "local-x".to_string(),
            kind: "local".to_string(),
            reason: "circuit open".to_string(),
        }]
    }));

    agent.prompt("go").await.expect("prompt completes");

    let mut reported = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::SourceUnhealthy { source_id, .. } = event {
            reported.push(source_id);
        }
    }
    assert_eq!(
        reported,
        vec!["local-x".to_string()],
        "an unhealthy source is announced once per trip"
    );
}

#[tokio::test]
async fn a_model_error_surfaces_as_a_failure() {
    let (mut agent, _faux, _events) = harness(Vec::new());

    let error = agent
        .prompt("hi")
        .await
        .expect_err("a model error must surface, not be swallowed");

    assert!(
        matches!(error, AgentError::ModelFailed { .. }),
        "expected ModelFailed, got {error:?}"
    );
    // The failed turn is still recorded, and the reason is readable off the agent.
    let last = agent.messages().last().expect("an assistant message");
    assert_eq!(last.role_str(), "assistant");
    assert!(agent.error_message().is_some());
}

#[tokio::test]
async fn stops_once_the_turn_limit_is_reached() {
    let tool_turn = || FauxResponse::tool_calls(vec![("list_dir", json!({}))]);
    let faux = FauxProvider::new();
    faux.set_responses(vec![tool_turn(), tool_turn(), tool_turn()]);

    let mut models = Models::new();
    models.register(faux.clone());
    let mut agent = Agent::new(models, faux.model(), "system")
        .with_tools(builtin_tools())
        .with_max_turns(2);

    let error = agent
        .prompt("loop")
        .await
        .expect_err("turn limit is enforced");

    assert!(matches!(
        error,
        AgentError::MaxTurnsExceeded { max_turns: 2 }
    ));
}

#[tokio::test]
async fn transcript_round_trips_through_json() {
    let (mut agent, _faux, _events) = harness(vec![FauxResponse::text("first reply")]);
    agent.prompt("hello").await.expect("prompt completes");

    let context = agent.transcript();
    assert_eq!(context.messages.len(), 2);
    assert_eq!(context.system_prompt.as_deref(), Some("system"));

    let json = serde_json::to_string(&context).expect("serialize transcript");
    let restored: rsagent_ai::Context =
        serde_json::from_str(&json).expect("deserialize transcript");

    // A fresh agent that loads the transcript sees the same history.
    let (mut resumed, _faux2, _events2) = harness(vec![]);
    resumed.load_messages(restored.messages);

    assert_eq!(resumed.messages().len(), agent.messages().len());
    assert_eq!(assistant_texts(&resumed), vec!["first reply".to_string()]);
}

#[tokio::test]
async fn resuming_restores_the_saved_system_prompt() {
    let (mut agent, _faux, _events) = harness(vec![FauxResponse::text("hi")]);
    agent.prompt("hello").await.expect("prompt completes");

    let context = agent.transcript();
    assert_eq!(context.system_prompt.as_deref(), Some("system"));

    let (mut resumed, _faux2, _events2) = harness(vec![]);
    resumed.load_messages(context.messages);
    if let Some(prompt) = context.system_prompt {
        resumed.set_system_prompt(prompt);
    }

    assert_eq!(
        resumed.transcript().system_prompt.as_deref(),
        Some("system")
    );
}

#[tokio::test]
async fn compaction_drops_old_turns_when_over_budget() {
    let faux = FauxProvider::new();
    faux.set_responses(vec![FauxResponse::text("ok")]);
    let mut models = Models::new();
    models.register(faux.clone());

    // A tiny context window: budget is 3/4 of it, so a long transcript must be trimmed.
    let model = faux.model().with_context_window(400);
    let mut agent = Agent::new(models, model, "system");

    // Ten long user messages => ten turns, comfortably over the budget.
    let long = "x".repeat(200);
    let transcript: Vec<Message> = (0..10).map(|_| Message::user(long.clone())).collect();
    agent.load_messages(transcript);

    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    agent.subscribe(tx);

    agent.prompt("final").await.expect("prompt completes");

    let users = agent
        .messages()
        .iter()
        .filter(|m| matches!(m, Message::User(_)))
        .count();
    assert!(users < 11, "older turns must be dropped, kept {users}");

    let mut compacted = false;
    while let Ok(event) = events.try_recv() {
        compacted |= matches!(event, AgentEvent::ContextCompacted { .. });
    }
    assert!(compacted, "a compaction event must be emitted");
}

#[tokio::test]
async fn allow_all_approver_never_announces_a_prompt() {
    let dir = std::env::temp_dir().join(format!("rsagent-allow-it-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("approved.txt");
    let path = file.to_string_lossy().into_owned();

    let faux = FauxProvider::new();
    faux.set_responses(vec![
        FauxResponse::tool_calls(vec![(
            "write_file",
            json!({ "path": path, "content": "x" }),
        )]),
        FauxResponse::text("done"),
    ]);
    let mut models = Models::new();
    models.register(faux.clone());
    let mut agent = Agent::new(models, faux.model(), "system")
        .with_tools(builtin_tools())
        .with_approver(rsagent_agent::AllowAll);
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    agent.subscribe(tx);

    agent.prompt("write").await.expect("prompt completes");

    let mut prompted = false;
    while let Ok(event) = events.try_recv() {
        prompted |= matches!(event, AgentEvent::ToolApprovalRequest { .. });
    }
    assert!(
        !prompted,
        "an allow-all approver must not announce a prompt"
    );
    assert!(file.exists(), "the approved write must have run");
}

#[tokio::test]
async fn deny_all_approver_blocks_without_announcing_a_prompt() {
    let dir = std::env::temp_dir().join(format!("rsagent-deny-it-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("denied.txt");
    let path = file.to_string_lossy().into_owned();

    let faux = FauxProvider::new();
    faux.set_responses(vec![
        FauxResponse::tool_calls(vec![(
            "write_file",
            json!({ "path": path, "content": "x" }),
        )]),
        FauxResponse::text("done"),
    ]);
    let mut models = Models::new();
    models.register(faux.clone());
    let mut agent = Agent::new(models, faux.model(), "system")
        .with_tools(builtin_tools())
        .with_approver(rsagent_agent::DenyAll);
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    agent.subscribe(tx);

    agent.prompt("write").await.expect("prompt completes");

    let mut prompted = false;
    while let Ok(event) = events.try_recv() {
        prompted |= matches!(event, AgentEvent::ToolApprovalRequest { .. });
    }
    assert!(!prompted, "a deny-all approver must not announce a prompt");
    assert!(!file.exists(), "the denied write must not have run");
}

/// A custom approval policy: only allow writes whose target path lives under `root`.
///
/// This lives entirely outside the agent crate — the loop never needs to know about it.
struct ScopedApprover {
    root: PathBuf,
}

#[async_trait::async_trait]
impl Approver for ScopedApprover {
    async fn approve(&self, _tool_name: &str, _permission: Permission, args: &Value) -> bool {
        args.get("path")
            .and_then(Value::as_str)
            .is_some_and(|path| PathBuf::from(path).starts_with(&self.root))
    }
}

#[tokio::test]
async fn a_custom_approver_can_scope_writes_to_a_directory() {
    let root = std::env::temp_dir().join(format!("rsagent-scope-it-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create root");
    let inside = root.join("ok.txt");
    let outside = std::env::temp_dir().join(format!("rsagent-outside-{}.txt", std::process::id()));
    std::fs::remove_file(&outside).ok();

    let faux = FauxProvider::new();
    faux.set_responses(vec![
        FauxResponse::tool_calls(vec![(
            "write_file",
            json!({ "path": inside.to_string_lossy(), "content": "x" }),
        )]),
        FauxResponse::tool_calls(vec![(
            "write_file",
            json!({ "path": outside.to_string_lossy(), "content": "x" }),
        )]),
        FauxResponse::text("done"),
    ]);
    let mut models = Models::new();
    models.register(faux.clone());
    let mut agent = Agent::new(models, faux.model(), "system")
        .with_tools(builtin_tools())
        .with_approver(ScopedApprover { root: root.clone() });

    agent.prompt("write twice").await.expect("prompt completes");

    assert!(inside.exists(), "a write inside the root must be allowed");
    assert!(!outside.exists(), "a write outside the root must be denied");

    std::fs::remove_dir_all(&root).ok();
}

/// A policy that decides purely by category: allow writes, deny everything else.
struct CategoryApprover;

#[async_trait::async_trait]
impl Approver for CategoryApprover {
    async fn approve(&self, _tool_name: &str, permission: Permission, _args: &Value) -> bool {
        permission == Permission::Write
    }

    fn needs_prompt(&self, _tool_name: &str, _permission: Permission) -> bool {
        false
    }
}

#[tokio::test]
async fn a_category_approver_sees_each_tools_permission() {
    let dir = std::env::temp_dir().join(format!("rsagent-cat-it-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create dir");
    let target = dir.join("ok.txt");
    let marker = dir.join("ran.txt");

    let (agent, _faux, _events) = harness_in(
        &dir,
        vec![
            FauxResponse::tool_calls(vec![(
                "write_file",
                json!({ "path": target.to_string_lossy(), "content": "x" }),
            )]),
            FauxResponse::tool_calls(vec![(
                "bash",
                json!({ "command": format!("echo hi > {}", marker.display()) }),
            )]),
            FauxResponse::text("done"),
        ],
    );
    let mut agent = agent.with_approver(CategoryApprover);

    agent.prompt("go").await.expect("prompt completes");

    assert!(
        target.exists(),
        "the write category must be allowed by the write-only policy"
    );
    assert!(
        !marker.exists(),
        "the exec category must be denied by the write-only policy"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn search_reports_matching_lines_with_paths_and_numbers() {
    let dir = std::env::temp_dir().join(format!("rsagent-search-it-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    std::fs::write(dir.join("a.txt"), "alpha\nneedle here\n").expect("write a");
    std::fs::write(dir.join("b.txt"), "no match\n").expect("write b");

    let (mut agent, _faux, _events) = harness_in(
        &dir,
        vec![
            FauxResponse::tool_calls(vec![(
                "search",
                json!({ "pattern": "needle", "path": dir.to_string_lossy() }),
            )]),
            FauxResponse::text("done"),
        ],
    );

    agent.prompt("search").await.expect("prompt completes");

    let result = agent
        .messages()
        .iter()
        .find_map(|m| match m {
            Message::ToolResult(r) if r.tool_name == "search" => Some(
                r.content
                    .iter()
                    .filter_map(|block| block.as_text())
                    .collect::<String>(),
            ),
            _ => None,
        })
        .expect("a search result");

    assert!(result.contains("a.txt:2: needle here"), "got: {result}");
    assert!(!result.contains("b.txt"), "non-matching files are omitted");

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn search_skips_files_larger_than_the_read_cap() {
    use std::io::Write;

    // Mirrors `MAX_SEARCH_FILE_BYTES` in `tools.rs`, which is private to that module.
    const CAP: u64 = 8 * 1024 * 1024;

    let dir = temp_dir("search-cap-it");
    std::fs::write(dir.join("small.txt"), "needle\n").expect("write small");

    // A file one byte over the cap, seeded with a match *at the top*: if the tool buffered
    // it whole the match would be found, so its absence proves the read was skipped. The
    // sparse tail (`set_len`) costs no real disk space.
    let big = dir.join("big.txt");
    let mut file = std::fs::File::create(&big).expect("create big");
    file.write_all(b"needle\n").expect("seed big");
    file.set_len(CAP + 1).expect("grow big");
    drop(file);

    let (mut agent, _faux, _events) = harness_in(
        &dir,
        vec![
            FauxResponse::tool_calls(vec![(
                "search",
                json!({ "pattern": "needle", "path": dir.to_string_lossy() }),
            )]),
            FauxResponse::text("done"),
        ],
    );

    agent.prompt("search").await.expect("prompt completes");

    let result = tool_result_text(&agent, "search");
    assert!(result.contains("small.txt:1: needle"), "got: {result}");
    assert!(
        !result.contains("big.txt:1: needle"),
        "an over-cap file must not be read: {result}"
    );
    assert!(
        result.contains("skipped") && result.contains("big.txt"),
        "the skipped file must be named: {result}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A unique scratch directory for one test.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rsagent-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// The concatenated text of the first tool result for `name`.
fn tool_result_text(agent: &Agent, name: &str) -> String {
    agent
        .messages()
        .iter()
        .find_map(|m| match m {
            Message::ToolResult(r) if r.tool_name == name => Some(
                r.content
                    .iter()
                    .filter_map(|block| block.as_text())
                    .collect::<String>(),
            ),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no `{name}` tool result"))
}

#[tokio::test]
async fn glob_finds_files_by_pattern() {
    let dir = temp_dir("glob-it");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::write(dir.join("src/main.rs"), "").expect("write main");
    std::fs::write(dir.join("src/lib.rs"), "").expect("write lib");
    std::fs::write(dir.join("README.md"), "").expect("write readme");

    let (mut agent, _faux, _events) = harness_in(
        &dir,
        vec![
            FauxResponse::tool_calls(vec![(
                "glob",
                json!({ "pattern": "**/*.rs", "path": dir.to_string_lossy() }),
            )]),
            FauxResponse::text("done"),
        ],
    );

    agent
        .prompt("find rust files")
        .await
        .expect("prompt completes");

    let result = tool_result_text(&agent, "glob");
    assert!(result.contains("main.rs"), "got: {result}");
    assert!(result.contains("lib.rs"), "got: {result}");
    assert!(
        !result.contains("README.md"),
        "non-matching files are omitted"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn search_skips_build_and_vendor_directories() {
    let dir = temp_dir("search-skip-it");
    std::fs::create_dir_all(dir.join("target")).expect("create target");
    std::fs::write(dir.join("target/buried.txt"), "needle in target\n").expect("write buried");
    std::fs::write(dir.join("src.txt"), "needle in src\n").expect("write src");

    let (mut agent, _faux, _events) = harness_in(
        &dir,
        vec![
            FauxResponse::tool_calls(vec![(
                "search",
                json!({ "pattern": "needle", "path": dir.to_string_lossy() }),
            )]),
            FauxResponse::text("done"),
        ],
    );

    agent.prompt("search").await.expect("prompt completes");

    let result = tool_result_text(&agent, "search");
    assert!(result.contains("needle in src"), "got: {result}");
    assert!(
        !result.contains("needle in target"),
        "noise directories must be skipped, got: {result}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn webfetch_rejects_non_http_schemes() {
    let (mut agent, _faux, _events) = harness(vec![
        FauxResponse::tool_calls(vec![("webfetch", json!({ "url": "file:///etc/passwd" }))]),
        FauxResponse::text("done"),
    ]);

    agent
        .prompt("fetch a local file")
        .await
        .expect("prompt completes");

    let failed = agent.messages().iter().find_map(|m| match m {
        Message::ToolResult(r) if r.tool_name == "webfetch" => Some((
            r.is_error,
            r.content
                .iter()
                .filter_map(|block| block.as_text())
                .collect::<String>(),
        )),
        _ => None,
    });
    let (is_error, text) = failed.expect("a webfetch result");
    assert!(is_error, "a non-http scheme must fail");
    assert!(text.contains("unsupported URL scheme"), "got: {text}");
}

#[tokio::test]
async fn webfetch_returns_page_text_without_markup() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // A one-shot local HTTP server, so the test needs no external network.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");

    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let mut request = [0u8; 1024];
        let _ = socket.read(&mut request).await;
        let body = "<html><body><h1>Hello</h1><script>secret()</script><p>World</p></body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.flush().await;
    });

    let (mut agent, _faux, _events) = harness(vec![
        FauxResponse::tool_calls(vec![(
            "webfetch",
            json!({ "url": format!("http://{addr}/") }),
        )]),
        FauxResponse::text("done"),
    ]);

    agent
        .prompt("fetch the page")
        .await
        .expect("prompt completes");

    let result = tool_result_text(&agent, "webfetch");
    assert!(result.contains("Hello"), "got: {result}");
    assert!(result.contains("World"), "got: {result}");
    assert!(
        !result.contains("secret()"),
        "script content must be dropped: {result}"
    );
}

#[tokio::test]
async fn read_file_numbers_lines_and_honors_a_range() {
    let dir = temp_dir("read-file-range-it");
    std::fs::write(dir.join("poem.txt"), "alpha\nbeta\ngamma\ndelta\n").expect("write poem");
    let path = dir.join("poem.txt").to_string_lossy().into_owned();

    let (mut agent, _faux, _events) = harness_in(
        &dir,
        vec![
            FauxResponse::tool_calls(vec![(
                "read_file",
                json!({ "path": path, "offset": 2, "limit": 2 }),
            )]),
            FauxResponse::text("done"),
        ],
    );

    agent
        .prompt("read a slice")
        .await
        .expect("prompt completes");

    let result = tool_result_text(&agent, "read_file");
    assert!(
        result.contains("2\tbeta"),
        "line numbers are shown: {result}"
    );
    assert!(result.contains("3\tgamma"), "got: {result}");
    assert!(
        !result.contains("alpha"),
        "lines before the range: {result}"
    );
    assert!(!result.contains("delta"), "lines after the range: {result}");
    assert!(
        result.contains("showing lines 2-3 of 4"),
        "range footer: {result}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn read_file_reads_the_whole_file_by_default() {
    let dir = temp_dir("read-file-full-it");
    std::fs::write(dir.join("two.txt"), "one\ntwo\n").expect("write file");
    let path = dir.join("two.txt").to_string_lossy().into_owned();

    let (mut agent, _faux, _events) = harness_in(
        &dir,
        vec![
            FauxResponse::tool_calls(vec![("read_file", json!({ "path": path }))]),
            FauxResponse::text("done"),
        ],
    );

    agent.prompt("read it").await.expect("prompt completes");

    let result = tool_result_text(&agent, "read_file");
    assert!(result.contains("1\tone"), "got: {result}");
    assert!(result.contains("2\ttwo"), "got: {result}");
    assert!(
        !result.contains("showing lines"),
        "no footer for a full read: {result}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn read_file_rejects_paths_outside_the_working_directory() {
    let root = temp_dir("fence-it");
    let inside = root.join("inside");
    std::fs::create_dir_all(&inside).expect("create inside");
    std::fs::write(root.join("secret.txt"), "top secret\n").expect("write secret");
    let outside = root.join("secret.txt").to_string_lossy().into_owned();

    let (mut agent, _faux, _events) = harness_in(
        &inside,
        vec![
            FauxResponse::tool_calls(vec![("read_file", json!({ "path": outside }))]),
            FauxResponse::text("done"),
        ],
    );

    agent
        .prompt("read outside")
        .await
        .expect("prompt completes");

    let failed = agent.messages().iter().find_map(|m| match m {
        Message::ToolResult(r) if r.tool_name == "read_file" => Some((
            r.is_error,
            r.content
                .iter()
                .filter_map(|block| block.as_text())
                .collect::<String>(),
        )),
        _ => None,
    });
    let (is_error, text) = failed.expect("a read_file result");
    assert!(is_error, "reading outside the fence must fail");
    assert!(
        text.contains("outside the working directory"),
        "got: {text}"
    );

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn read_file_reaches_outside_when_the_fence_is_off() {
    let root = temp_dir("no-fence-it");
    let inside = root.join("inside");
    std::fs::create_dir_all(&inside).expect("create inside");
    std::fs::write(root.join("secret.txt"), "top secret\n").expect("write secret");
    let outside = root.join("secret.txt").to_string_lossy().into_owned();

    let (agent, _faux, _events) = harness_in(
        &inside,
        vec![
            FauxResponse::tool_calls(vec![("read_file", json!({ "path": outside }))]),
            FauxResponse::text("done"),
        ],
    );
    let mut agent = agent.with_path_fence(PathFence::Unrestricted);

    agent
        .prompt("read outside")
        .await
        .expect("prompt completes");

    let result = tool_result_text(&agent, "read_file");
    assert!(result.contains("top secret"), "got: {result}");

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn edit_file_rejects_an_ambiguous_match() {
    let dir = temp_dir("edit-ambiguous-it");
    std::fs::write(dir.join("f.txt"), "foo\nbar\nfoo\n").expect("write file");
    let path = dir.join("f.txt").to_string_lossy().into_owned();

    let (mut agent, _faux, _events) = harness(vec![
        FauxResponse::tool_calls(vec![(
            "edit_file",
            json!({ "path": path, "old_string": "foo", "new_string": "baz" }),
        )]),
        FauxResponse::text("done"),
    ]);

    agent.prompt("edit").await.expect("prompt completes");

    let failed = agent.messages().iter().find_map(|m| match m {
        Message::ToolResult(r) if r.tool_name == "edit_file" => Some((
            r.is_error,
            r.content
                .iter()
                .filter_map(|block| block.as_text())
                .collect::<String>(),
        )),
        _ => None,
    });
    let (is_error, text) = failed.expect("an edit_file result");
    assert!(is_error, "an ambiguous match must fail");
    assert!(text.contains("occurs 2 times"), "got: {text}");
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt")).expect("read file"),
        "foo\nbar\nfoo\n",
        "the file must be left untouched"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn edit_file_replaces_every_occurrence_when_asked() {
    let dir = temp_dir("edit-all-it");
    std::fs::write(dir.join("f.txt"), "foo\nbar\nfoo\n").expect("write file");
    let path = dir.join("f.txt").to_string_lossy().into_owned();

    let (mut agent, _faux, _events) = harness(vec![
        FauxResponse::tool_calls(vec![(
            "edit_file",
            json!({ "path": path, "old_string": "foo", "new_string": "baz", "replace_all": true }),
        )]),
        FauxResponse::text("done"),
    ]);

    agent.prompt("edit").await.expect("prompt completes");

    let result = tool_result_text(&agent, "edit_file");
    assert!(result.contains("2 occurrence"), "got: {result}");
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt")).expect("read file"),
        "baz\nbar\nbaz\n"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn edit_file_replaces_a_unique_match() {
    let dir = temp_dir("edit-unique-it");
    std::fs::write(dir.join("f.txt"), "alpha\nbeta\ngamma\n").expect("write file");
    let path = dir.join("f.txt").to_string_lossy().into_owned();

    let (mut agent, _faux, _events) = harness(vec![
        FauxResponse::tool_calls(vec![(
            "edit_file",
            json!({ "path": path, "old_string": "beta", "new_string": "BETA" }),
        )]),
        FauxResponse::text("done"),
    ]);

    agent.prompt("edit").await.expect("prompt completes");

    let result = tool_result_text(&agent, "edit_file");
    assert!(result.contains("Replaced 1 occurrence"), "got: {result}");
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt")).expect("read file"),
        "alpha\nBETA\ngamma\n"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn read_file_output_edits_a_crlf_file() {
    // On a Windows checkout the file has `\r\n` endings. A model that reads a line and
    // pastes it back into `edit_file` must not be defeated by the `\r` `read_file` used
    // to strip (ACTIONS.md A3).
    let dir = temp_dir("crlf-edit-it");
    std::fs::write(dir.join("f.txt"), "alpha\r\nbeta\r\ngamma\r\n").expect("write file");
    let path = dir.join("f.txt").to_string_lossy().into_owned();

    // Read the file and take the `beta` line's text exactly as `read_file` rendered it.
    let (mut reader, _faux, _events) = harness_in(
        &dir,
        vec![
            FauxResponse::tool_calls(vec![("read_file", json!({ "path": path }))]),
            FauxResponse::text("done"),
        ],
    );
    reader.prompt("read").await.expect("prompt completes");
    let rendered = tool_result_text(&reader, "read_file");
    let fragment = rendered
        .split('\n')
        .find_map(|line| line.strip_prefix("2\t"))
        .expect("the beta line is rendered");

    // Feed that fragment straight back as `old_string`: the edit must succeed.
    let (mut editor, _faux, _events) = harness_in(
        &dir,
        vec![
            FauxResponse::tool_calls(vec![(
                "edit_file",
                json!({ "path": path, "old_string": fragment, "new_string": "BETA\r" }),
            )]),
            FauxResponse::text("done"),
        ],
    );
    editor.prompt("edit").await.expect("prompt completes");

    let result = tool_result_text(&editor, "edit_file");
    assert!(result.contains("Replaced 1 occurrence"), "got: {result}");
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt")).expect("read file"),
        "alpha\r\nBETA\r\ngamma\r\n",
        "the CRLF endings survive the edit"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A provider that records the last request's [`Context`] and replies with fixed text.
struct CapturingProvider {
    last: Arc<Mutex<Option<Context>>>,
}

impl Provider for CapturingProvider {
    fn id(&self) -> &'static str {
        "capture"
    }

    fn models(&self) -> Vec<Model> {
        vec![Model::openai_compatible("capture", "capture-1", "")]
    }

    fn stream(&self, model: Model, context: Context, _options: StreamOptions) -> EventStream {
        *self.last.lock().expect("capture lock") = Some(context);
        let mut message = AssistantMessage::partial(&model);
        message.content.push(ContentBlock::text("ok"));
        message.stop_reason = StopReason::Stop;
        Box::pin(stream::iter(vec![StreamEvent::Done {
            reason: StopReason::Stop,
            message,
        }]))
    }
}

/// A faux provider that records the system prompt of every request, in order, then replays
/// the scripted responses. Used to observe how many times the memory is read per turn.
struct RecordingFaux {
    faux: FauxProvider,
    prompts: Arc<Mutex<Vec<Option<String>>>>,
}

impl Provider for RecordingFaux {
    fn id(&self) -> &str {
        self.faux.id()
    }

    fn models(&self) -> Vec<Model> {
        self.faux.models()
    }

    fn stream(&self, model: Model, context: Context, options: StreamOptions) -> EventStream {
        self.prompts
            .lock()
            .expect("prompt lock")
            .push(context.system_prompt.clone());
        self.faux.stream(model, context, options)
    }
}

#[tokio::test]
async fn memory_notes_are_folded_into_the_system_prompt() {
    let dir = temp_dir("memory-prompt-it");
    let memory = Arc::new(Memory::new(dir.join("notes.jsonl"), 10));
    memory
        .append("the project uses cargo nextest")
        .expect("append");

    let last = Arc::new(Mutex::new(None));
    let mut models = Models::new();
    models.register(CapturingProvider {
        last: Arc::clone(&last),
    });

    let mut agent = Agent::new(
        models,
        Model::openai_compatible("capture", "capture-1", ""),
        "base system",
    )
    .with_memory(Arc::clone(&memory));

    agent.prompt("hi").await.expect("prompt completes");

    let system = last
        .lock()
        .expect("capture lock")
        .as_ref()
        .expect("a request was made")
        .system_prompt
        .clone()
        .expect("a system prompt was sent");
    assert!(
        system.starts_with("base system"),
        "the base prompt is kept: {system}"
    );
    assert!(
        system.contains("cargo nextest"),
        "the memory note is folded in: {system}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn the_remember_tool_writes_to_memory() {
    let dir = temp_dir("memory-remember-it");
    let memory = Arc::new(Memory::new(dir.join("notes.jsonl"), 10));

    let faux = FauxProvider::new();
    faux.set_responses(vec![
        FauxResponse::tool_calls(vec![("remember", json!({ "text": "prefer nextest" }))]),
        FauxResponse::text("done"),
    ]);
    let mut models = Models::new();
    models.register(faux.clone());

    let mut tools = builtin_tools();
    tools.extend(memory_tools());
    let mut agent = Agent::new(models, faux.model(), "system")
        .with_tools(tools)
        .with_memory(Arc::clone(&memory));

    agent
        .prompt("remember this")
        .await
        .expect("prompt completes");

    let notes = memory.recent(10).expect("read memory");
    assert!(
        notes
            .iter()
            .any(|note| note.text.contains("prefer nextest")),
        "the note was stored: {notes:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn compaction_distills_dropped_turns_into_memory() {
    let dir = temp_dir("memory-compact-it");
    let memory = Arc::new(Memory::new(dir.join("notes.jsonl"), 50));

    let faux = FauxProvider::new();
    faux.set_responses(vec![
        FauxResponse::text("first answer"),
        FauxResponse::text("second answer"),
        FauxResponse::text("third answer"),
    ]);
    let mut models = Models::new();
    models.register(faux.clone());

    // A tiny window forces compaction once there are three turns to choose from; the
    // middle turn is then dropped and distilled into memory.
    let model = faux.model().with_context_window(8);
    let mut agent = Agent::new(models, model, "system").with_memory(Arc::clone(&memory));

    agent.prompt("first question").await.expect("turn 1");
    agent.prompt("second question").await.expect("turn 2");
    agent.prompt("third question").await.expect("turn 3");

    let notes = memory.recent(50).expect("read memory");
    assert!(
        notes
            .iter()
            .any(|note| note.text.contains("[compacted]") && note.text.contains("second question")),
        "compaction recorded a digest of the dropped turn: {notes:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn compaction_notes_are_rate_limited_to_one_per_few_turns() {
    let dir = temp_dir("memory-compact-rate");
    let memory = Arc::new(Memory::new(dir.join("notes.jsonl"), 50));

    let faux = FauxProvider::new();
    // One text reply per prompt, so each prompt is a single turn.
    faux.set_responses((0..6).map(|_| FauxResponse::text("ok")).collect());
    let mut models = Models::new();
    models.register(faux.clone());

    // A tiny window with long turns, so every turn is over budget and compacts.
    let model = faux.model().with_context_window(40);
    let mut agent = Agent::new(models, model, "system").with_memory(Arc::clone(&memory));

    let long = "x".repeat(300);
    let transcript: Vec<Message> = (0..5).map(|_| Message::user(long.clone())).collect();
    agent.load_messages(transcript);

    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    agent.subscribe(tx);

    // Six turns, each of which compacts once. Without the rate gate each would append a
    // `[compacted]` note; with it, only the first of the six records.
    for _ in 0..6 {
        agent
            .prompt(long.clone())
            .await
            .expect("a prompt completes");
    }

    let mut compactions = 0;
    while let Ok(event) = events.try_recv() {
        if matches!(event, AgentEvent::ContextCompacted { .. }) {
            compactions += 1;
        }
    }
    let notes = memory
        .recent(50)
        .expect("read memory")
        .iter()
        .filter(|note| note.text.starts_with("[compacted]"))
        .count();

    assert!(
        compactions >= 6,
        "the test must exercise several compactions, saw {compactions}"
    );
    assert_eq!(
        notes, 1,
        "one distillation note per few turns, not one per compaction, saw {notes}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn steering_injects_a_message_mid_run() {
    let (mut agent, faux, _events) = harness(vec![
        FauxResponse::tool_calls(vec![("read_file", json!({ "path": "Cargo.toml" }))]),
        FauxResponse::text("done"),
    ]);
    // Queue a steering message. The public API exposes no in-flight handle (`steer` takes
    // `&mut self`, and unlike `abort` there is no clonable token), so it is queued up front
    // and injected at the first turn boundary — as a user message, distinct from the prompt.
    agent.steer("also check the tests");

    agent.prompt("start").await.expect("prompt completes");

    assert_eq!(
        user_texts(&agent),
        vec!["start".to_string(), "also check the tests".to_string()],
        "a steering message is injected into the transcript as a user message"
    );
    assert_eq!(
        faux.call_count(),
        2,
        "the tool call carried the run into a second request, so steering reached a turn"
    );
}

#[tokio::test]
async fn follow_up_keeps_the_run_going() {
    let (mut agent, faux, _events) = harness(vec![
        FauxResponse::text("first reply"),
        FauxResponse::text("second reply"),
    ]);
    // Queue follow-up work before the run: it is injected only once the agent would stop.
    agent.follow_up("keep going");

    agent.prompt("start").await.expect("prompt completes");

    assert_eq!(
        assistant_texts(&agent),
        vec!["first reply".to_string(), "second reply".to_string()],
        "a queued follow-up adds a turn instead of letting the run stop"
    );
    assert!(
        user_texts(&agent).contains(&"keep going".to_string()),
        "the follow-up is injected as a user message"
    );
    assert_eq!(
        faux.call_count(),
        2,
        "the follow-up forced a second request"
    );
}

#[tokio::test]
async fn aborting_a_run_ends_it_without_an_error() {
    let (mut agent, faux, mut events) = harness(vec![FauxResponse::text("y".repeat(200))]);
    // Pace the stream so the run is genuinely in flight when it is aborted.
    let _ = faux.with_chunk_delay(Duration::from_millis(20));

    let token = agent.cancel_token();
    tokio::spawn(async move {
        // Abort only once the turn is actually streaming, from a separate task — the same
        // way an embedder cancels a run it started elsewhere.
        while let Some(event) = events.recv().await {
            if matches!(event, AgentEvent::MessageUpdate { .. }) {
                token.cancel();
                break;
            }
        }
    });

    let result = agent.prompt("hi").await;

    assert!(
        result.is_ok(),
        "a user abort ends the run with Ok, not an error: {result:?}"
    );
    assert!(
        agent.error_message().is_none(),
        "an abort is not a model error"
    );
    assert!(
        assistant_texts(&agent).is_empty(),
        "the aborted turn must not produce a completed assistant message"
    );
}

/// A shared counter that records the peak number of tool executions in flight at once.
#[derive(Default)]
struct ConcurrencyProbe {
    active: AtomicUsize,
    peak: AtomicUsize,
    runs: AtomicUsize,
}

impl ConcurrencyProbe {
    fn enter(&self) {
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
    }

    fn leave(&self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A tool that records concurrency around a short await, and declares whether the batch
/// containing it must run sequentially.
struct ProbeTool {
    name: &'static str,
    sequential: bool,
    probe: Arc<ConcurrencyProbe>,
}

#[async_trait]
impl Tool for ProbeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            self.name,
            "Records execution concurrency.",
            json!({ "type": "object" }),
        )
    }

    fn is_sequential(&self) -> bool {
        self.sequential
    }

    async fn execute(
        &self,
        _call_id: &str,
        _args: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, String> {
        self.probe.enter();
        tokio::time::sleep(Duration::from_millis(20)).await;
        self.probe.leave();
        self.probe.runs.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::text(format!("{} ran", self.name)))
    }
}

#[tokio::test]
async fn a_sequential_tool_is_not_run_in_parallel_with_others() {
    let probe = Arc::new(ConcurrencyProbe::default());
    let faux = FauxProvider::new();
    faux.set_responses(vec![
        FauxResponse::tool_calls(vec![
            ("probe_parallel", json!({})),
            ("probe_sequential", json!({})),
        ]),
        FauxResponse::text("done"),
    ]);
    let mut models = Models::new();
    models.register(faux.clone());

    let mut agent = Agent::new(models, faux.model(), "system").with_tools(vec![
        Box::new(ProbeTool {
            name: "probe_parallel",
            sequential: false,
            probe: Arc::clone(&probe),
        }),
        Box::new(ProbeTool {
            name: "probe_sequential",
            sequential: true,
            probe: Arc::clone(&probe),
        }),
    ]);
    let (tx, _events) = tokio::sync::mpsc::unbounded_channel();
    agent.subscribe(tx);

    agent.prompt("go").await.expect("prompt completes");

    assert_eq!(probe.runs.load(Ordering::SeqCst), 2, "both tools ran");
    assert_eq!(
        probe.peak.load(Ordering::SeqCst),
        1,
        "a batch containing a sequential tool must run its calls one at a time"
    );
}

#[tokio::test]
async fn a_turn_reads_memory_once() {
    let dir = temp_dir("memory-read-once");
    let memory = Arc::new(Memory::new(dir.join("notes.jsonl"), 50));
    memory.append("seeded note").expect("append");

    let faux = FauxProvider::new();
    faux.set_responses(vec![
        FauxResponse::tool_calls(vec![("read_file", json!({ "path": "Cargo.toml" }))]),
        FauxResponse::text("done"),
    ]);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let mut models = Models::new();
    models.register(RecordingFaux {
        faux: faux.clone(),
        prompts: Arc::clone(&prompts),
    });

    // A tiny window with long seeded turns, so turn 1 compacts and distils a note.
    let model = faux.model().with_context_window(40);
    let mut agent = Agent::new(models, model, "system").with_memory(Arc::clone(&memory));
    let long = "x".repeat(300);
    agent.load_messages((0..5).map(|_| Message::user(long.clone())).collect());

    agent.prompt("go").await.expect("prompt completes");

    let notes = memory.recent(50).expect("read memory");
    assert!(
        notes
            .iter()
            .any(|note| note.text.starts_with("[compacted]")),
        "turn 1 distilled a note: {notes:?}"
    );

    let prompts = prompts.lock().expect("prompt lock");
    assert!(prompts.len() >= 2, "the run made at least two requests");
    let first = prompts[0].as_deref().expect("a system prompt");
    let second = prompts[1].as_deref().expect("a system prompt");
    assert!(
        first.contains("seeded note"),
        "the existing memory is folded in: {first}"
    );
    // Rendered once, before this turn's distill: the note just written is not yet in the
    // prompt that was already built — so the file was read a single time this turn.
    assert!(
        !first.contains("[compacted]"),
        "the prompt is rendered before the distill, from one read: {first}"
    );
    assert!(
        second.contains("[compacted]"),
        "the following turn re-renders and sees the new note: {second}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
