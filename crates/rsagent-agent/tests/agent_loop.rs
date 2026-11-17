//! End-to-end tests for the agent loop, driven by the offline `FauxProvider`.

use std::path::PathBuf;

use rsagent_agent::{builtin_tools, Agent, AgentError, AgentEvent, Approver};
use rsagent_ai::{FauxProvider, FauxResponse, Message, Models};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedReceiver;

/// Build an agent with the built-in tools and a scripted faux provider.
fn harness(responses: Vec<FauxResponse>) -> (Agent, FauxProvider, UnboundedReceiver<AgentEvent>) {
    let faux = FauxProvider::new();
    faux.set_responses(responses);

    let mut models = Models::new();
    models.register(faux.clone());

    let mut agent = Agent::new(models, faux.model(), "system").with_tools(builtin_tools());
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
async fn a_model_error_ends_the_run_without_failing() {
    let (mut agent, _faux, _events) = harness(Vec::new());

    agent
        .prompt("hi")
        .await
        .expect("a model error is not an agent failure");

    let last = agent.messages().last().expect("an assistant message");
    assert_eq!(last.role_str(), "assistant");
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
    async fn approve(&self, _tool_name: &str, args: &Value) -> bool {
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

#[tokio::test]
async fn search_reports_matching_lines_with_paths_and_numbers() {
    let dir = std::env::temp_dir().join(format!("rsagent-search-it-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    std::fs::write(dir.join("a.txt"), "alpha\nneedle here\n").expect("write a");
    std::fs::write(dir.join("b.txt"), "no match\n").expect("write b");

    let (mut agent, _faux, _events) = harness(vec![
        FauxResponse::tool_calls(vec![(
            "search",
            json!({ "pattern": "needle", "path": dir.to_string_lossy() }),
        )]),
        FauxResponse::text("done"),
    ]);

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

    let (mut agent, _faux, _events) = harness(vec![
        FauxResponse::tool_calls(vec![(
            "glob",
            json!({ "pattern": "**/*.rs", "path": dir.to_string_lossy() }),
        )]),
        FauxResponse::text("done"),
    ]);

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

    let (mut agent, _faux, _events) = harness(vec![
        FauxResponse::tool_calls(vec![(
            "search",
            json!({ "pattern": "needle", "path": dir.to_string_lossy() }),
        )]),
        FauxResponse::text("done"),
    ]);

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
