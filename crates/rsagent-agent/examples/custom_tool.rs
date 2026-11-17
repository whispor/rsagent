//! Add your own tool. · 添加你自己的工具。
//!
//! A tool is two halves: a [`ToolSpec`] the model sees (name, description, JSON Schema)
//! and a [`Tool`] that actually runs. They are separate so the wire contract and the
//! execution logic can evolve independently.
//!
//! 一个工具分两半：给模型看的 [`ToolSpec`]（名称、描述、参数 JSON Schema），与真正
//! 执行的 [`Tool`]。二者分离，使协议契约与执行逻辑能各自演进。
//!
//! This example implements a `word_count` tool and drives the agent with a scripted call.
//! 本例实现一个 `word_count` 工具，并用一段预置的工具调用来驱动 Agent。
//!
//! ```bash
//! cargo run -p rsagent-agent --example custom_tool
//! ```

#![expect(
    clippy::print_stdout,
    reason = "an example's whole job is to print what it did to the terminal"
)]

use async_trait::async_trait;
use rsagent_agent::{Agent, Tool, ToolContext, ToolResult};
use rsagent_ai::{FauxProvider, FauxResponse, Message, Models, ToolSpec};
use serde_json::{json, Value};

/// Counts the words in a string. · 统计字符串里的单词数。
struct WordCount;

#[async_trait]
impl Tool for WordCount {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "word_count",
            "Count the words in `text`.",
            json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The text to count." }
                },
                "required": ["text"]
            }),
        )
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let text = args
            .get("text")
            .and_then(Value::as_str)
            .ok_or("missing `text` argument")?;
        Ok(ToolResult::text(
            text.split_whitespace().count().to_string(),
        ))
    }
}

#[tokio::main]
async fn main() {
    // Script one tool call, then a closing text reply.
    // 预置一次工具调用，再接一条收尾文本回复。
    let faux = FauxProvider::new();
    faux.set_responses(vec![
        FauxResponse::tool_calls(vec![(
            "word_count",
            json!({ "text": "the quick brown fox" }),
        )]),
        FauxResponse::text("done"),
    ]);
    let model = faux.model();

    let mut models = Models::new();
    models.register(faux);

    // Hand the agent *only* our tool, instead of the built-in set.
    // 只把我们的工具交给 Agent，而不是内置工具集。
    let mut agent = Agent::new(models, model, "You are a terse assistant.")
        .with_tools(vec![Box::new(WordCount)]);

    agent
        .prompt("how many words are in 'the quick brown fox'?")
        .await
        .expect("the run completes");

    // Read the tool result back out of the transcript.
    // 从对话记录里读回工具结果。
    for message in agent.messages() {
        if let Message::ToolResult(result) = message {
            let text: String = result.content.iter().filter_map(|b| b.as_text()).collect();
            println!("word_count -> {text}");
        }
    }
}
