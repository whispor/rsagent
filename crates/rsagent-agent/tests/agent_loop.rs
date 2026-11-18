//! End-to-end tests for the agent loop, driven by the offline `FauxProvider`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures::stream;
use rsagent_agent::{
    builtin_tools, memory_tools, Agent, AgentError, AgentEvent, Approver, Memory, PathFence,
    Permission, UnhealthySource,
};
use rsagent_ai::{
    AssistantMessage, ContentBlock, Context, EventStream, FauxProvider, FauxResponse, Message,
    Model, Models, Provider, StopReason, StreamEvent, StreamOptions,
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
