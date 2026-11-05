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
