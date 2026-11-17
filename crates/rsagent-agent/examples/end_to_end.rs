//! The whole picture in one file. · 一页看全。
//!
//! `custom_tool.rs` shows a tool, `custom_provider.rs` shows a provider — this example
//! shows both working **together** through the loop, which is the part that is hard to
//! see from either half alone:
//!
//! 1. a [`Provider`] that streams a real tool call (the granular
//!    `ToolCallStart` / `ToolCallDelta` / `ToolCallEnd` vocabulary), then a closing reply;
//! 2. a [`Tool`] the agent actually runs, returning model-visible `content` plus
//!    model-invisible `details`;
//! 3. the two channels — the **events** narrate the run, the **transcript** holds the answer.
//!
//! `custom_tool.rs` 展示了工具，`custom_provider.rs` 展示了提供商——本例展示二者在循环里
//! **协同**工作，这是单看任何一半都难以看清的部分：一个会流式发出真实工具调用的提供商、
//! 一个真正被执行的工具、以及两条通道（事件叙述过程，对话记录承载结果）。
//!
//! ```bash
//! cargo run -p rsagent-agent --example end_to_end
//! ```

#![expect(
    clippy::print_stdout,
    reason = "an example's whole job is to print what it did to the terminal"
)]

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use futures::stream;
use rsagent_agent::{Agent, AgentEvent, Tool, ToolContext, ToolResult};
use rsagent_ai::{
    AssistantMessage, ContentBlock, Context, EventStream, Message, Modality, Model, Models,
    Provider, StopReason, StreamEvent, StreamOptions, ToolSpec,
};
use serde_json::{json, Value};

/// One scripted model turn. · 一段预置的模型回合。
#[derive(Debug)]
enum Step {
    /// The model asks for a tool call.
    Call { name: String, args: Value },
    /// The model replies with plain text.
    Say(String),
}

/// A provider that replays a fixed script of turns.
///
/// It emits the *same* event shapes a real provider does, so the loop — and anything
/// rendering the events — cannot tell it apart from a network-backed one.
///
/// 按预置脚本回放回合的提供商。它发出与真实提供商**相同**的事件形态，
/// 因此循环（以及任何渲染事件的代码）无法把它与联网实现区分开。
struct ScriptedProvider {
    model: Model,
    script: Mutex<VecDeque<Step>>,
}

impl ScriptedProvider {
    fn new(script: Vec<Step>) -> Self {
        ScriptedProvider {
            model: Model {
                id: "scripted-1".to_string(),
                name: "Scripted 1".to_string(),
                provider: "scripted".to_string(),
                api: "scripted".to_string(),
                base_url: String::new(),
                context_window: 8_192,
                max_tokens: 256,
                reasoning: false,
                input: vec![Modality::Text],
            },
            script: Mutex::new(script.into()),
        }
    }
}

impl Provider for ScriptedProvider {
    fn id(&self) -> &'static str {
        "scripted"
    }

    fn models(&self) -> Vec<Model> {
        vec![self.model.clone()]
    }

    fn stream(&self, model: Model, _context: Context, _options: StreamOptions) -> EventStream {
        // One `stream()` call == one model turn, so pop the next scripted step.
        // 一次 `stream()` 调用 == 一个模型回合，因此弹出脚本里的下一步。
        let next = self
            .script
            .lock()
            .expect("the script mutex is never poisoned")
            .pop_front()
            .unwrap_or_else(|| Step::Say("(script exhausted)".to_string()));

        let start = AssistantMessage::partial(&model);
        match next {
            Step::Say(text) => {
                let mut message = AssistantMessage::partial(&model);
                message.content.push(ContentBlock::text(text.clone()));
                message.stop_reason = StopReason::Stop;
                Box::pin(stream::iter(vec![
                    StreamEvent::Start { partial: start },
                    StreamEvent::TextStart { content_index: 0 },
                    StreamEvent::TextDelta {
                        content_index: 0,
                        delta: text.clone(),
                    },
                    StreamEvent::TextEnd {
                        content_index: 0,
                        content: text,
                    },
                    StreamEvent::Done {
                        reason: StopReason::Stop,
                        message,
                    },
                ]))
            }
            Step::Call { name, args } => {
                let raw = serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string());
                let call = ContentBlock::ToolCall {
                    id: "call_1".to_string(),
                    name,
                    arguments: args,
                };
                let mut message = AssistantMessage::partial(&model);
                message.content.push(call.clone());
                // `ToolUse` is what tells the loop to execute the call.
                // `ToolUse` 正是告诉循环去执行工具调用的信号。
                message.stop_reason = StopReason::ToolUse;
                Box::pin(stream::iter(vec![
                    StreamEvent::Start { partial: start },
                    StreamEvent::ToolCallStart { content_index: 0 },
                    StreamEvent::ToolCallDelta {
                        content_index: 0,
                        delta: raw,
                    },
                    StreamEvent::ToolCallEnd {
                        content_index: 0,
                        tool_call: call,
                    },
                    StreamEvent::Done {
                        reason: StopReason::ToolUse,
                        message,
                    },
                ]))
            }
        }
    }
}

/// A tool that reports a word count, with structured `details` for the caller.
/// 统计单词数的工具，并附带结构化的 `details` 供调用方使用。
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
        let words = text.split_whitespace().count();
        Ok(ToolResult {
            // `content` goes back to the model; `details` stays with the caller.
            // `content` 会回传给模型；`details` 只留在调用方。
            content: vec![ContentBlock::text(format!("{words} words"))],
            details: Some(json!({ "words": words })),
            terminate: false,
        })
    }
}

#[tokio::main]
async fn main() {
    // 1. The provider: turn one calls the tool, turn two answers in prose.
    //    提供商：第一回合调用工具，第二回合用文字作答。
    let provider = ScriptedProvider::new(vec![
        Step::Call {
            name: "word_count".to_string(),
            args: json!({ "text": "the quick brown fox" }),
        },
        Step::Say("That sentence has 4 words.".to_string()),
    ]);
    let model = provider.model.clone();

    let mut models = Models::new();
    models.register(provider);

    // 2. The agent: our provider, our tool. Nothing built-in is involved.
    //    Agent：我们的提供商、我们的工具，不涉及任何内置件。
    let mut agent = Agent::new(models, model, "You are a terse assistant.")
        .with_tools(vec![Box::new(WordCount)]);

    // 3. Subscribe *before* running, so no event is missed.
    //    运行**之前**订阅，才不会漏掉事件。
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    agent.subscribe(tx);

    // 4. Run. The return value is the outcome; the events are the narrative.
    //    运行。返回值是结局，事件是过程叙事。
    agent
        .prompt("how many words are in 'the quick brown fox'?")
        .await
        .expect("the run completes");

    // 5. Channel #2 — replay the narrative.
    //    第 2 条通道——回放过程叙事。
    while let Ok(event) = rx.try_recv() {
        if let Some(line) = narrate(&event) {
            println!("{line}");
        }
    }

    // 6. Channel #1 — read the answer back out of the transcript.
    //    第 1 条通道——从对话记录里取回答案。
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

/// A one-line summary of the events worth showing.
///
/// Note `details`: it reaches us through the event but never enters the transcript,
/// so the model never sees it.
///
/// 只挑值得展示的事件，压成一行。注意 `details`：它通过事件抵达我们，却从不进入对话
/// 记录，因此模型永远看不到它。
fn narrate(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::AgentStart => Some("run   : start".to_string()),
        AgentEvent::AgentEnd { .. } => Some("run   : end".to_string()),
        AgentEvent::TurnStart => Some("turn  : start".to_string()),
        AgentEvent::ToolExecutionStart { tool_name, .. } => Some(format!("  call   {tool_name}")),
        AgentEvent::ToolExecutionEnd {
            tool_name,
            content,
            is_error,
            details,
            ..
        } => {
            let verdict = if *is_error { "fail" } else { "ok" };
            let extra = details
                .as_ref()
                .map_or_else(String::new, |value| format!("  details={value}"));
            Some(format!("  {verdict}   {tool_name}: {content}{extra}"))
        }
        AgentEvent::ContextCompacted { dropped_messages } => {
            Some(format!("  compact: dropped {dropped_messages} messages"))
        }
        _ => None,
    }
}
