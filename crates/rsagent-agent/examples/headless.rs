//! Drive the agent loop without the CLI. · 无头驱动 Agent 循环。
//!
//! The smallest useful embedding of `rsagent-agent`: register an offline provider,
//! subscribe to events, run one prompt, then read the transcript.
//!
//! `rsagent-agent` 最小可用的嵌入方式：注册一个离线提供商、订阅事件、运行一次提示词，
//! 然后读取对话记录。
//!
//! ```bash
//! cargo run -p rsagent-agent --example headless
//! ```

#![expect(
    clippy::print_stdout,
    reason = "an example's whole job is to print what it did to the terminal"
)]

use rsagent_agent::{builtin_tools, Agent, AgentEvent};
use rsagent_ai::{FauxProvider, FauxResponse, Message, Models};

#[tokio::main]
async fn main() {
    // 1. A provider. `FauxProvider` replays scripted replies — no network, no key.
    //    一个提供商。`FauxProvider` 回放预置回复——无需网络与密钥。
    let faux = FauxProvider::new();
    faux.set_responses(vec![FauxResponse::text("Hello from a headless agent!")]);
    let model = faux.model();

    // 2. Register it, then point the agent at one of its models.
    //    注册它，然后让 Agent 指向它的某个模型。
    let mut models = Models::new();
    models.register(faux);

    let mut agent =
        Agent::new(models, model, "You are a terse assistant.").with_tools(builtin_tools());

    // 3. Subscribe to the event stream (channel #2 — see docs/ARCHITECTURE.md §4).
    //    订阅事件流（第 2 条通道——见 docs/ARCHITECTURE.md §4）。
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    agent.subscribe(tx);

    // 4. Run one prompt. The return value is the outcome; the events are the narrative.
    //    运行一次提示词。返回值是结局，事件是过程叙事。
    agent.prompt("say hi").await.expect("the run completes");

    // 5. Print the narrative, then the answer read back from the transcript.
    //    打印过程叙事，再从对话记录里取回答案。
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::ToolExecutionStart { tool_name, .. } = event {
            println!("· tool: {tool_name}");
        }
    }

    let answer = agent
        .messages()
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::Assistant(assistant) if !assistant.to_text().is_empty() => {
                Some(assistant.to_text())
            }
            _ => None,
        })
        .unwrap_or_default();
    println!("answer: {answer}");
}
