//! Add your own provider. · 添加你自己的提供商。
//!
//! A provider owns a model catalog and turns a [`Context`] into a stream of normalized
//! [`StreamEvent`]s. Every wire protocol (OpenAI, Anthropic, ...) is just one
//! implementation of this trait, so a custom provider is a first-class citizen.
//!
//! 提供商拥有模型目录，并把 [`Context`] 变成一串规范化的 [`StreamEvent`]。每种线协议
//! （OpenAI、Anthropic……）都只是该 trait 的一个实现，因此自定义提供商是一等公民。
//!
//! This example is an offline "echo" provider that ignores the request and always
//! replies with a fixed line.
//! 本例是一个离线「回声」提供商：忽略请求，永远回复固定的一句话。
//!
//! ```bash
//! cargo run -p rsagent-ai --example custom_provider
//! ```

#![expect(
    clippy::print_stdout,
    reason = "an example's whole job is to print what it did to the terminal"
)]

use futures::stream;
use rsagent_ai::{
    AssistantMessage, ContentBlock, Context, EventStream, Modality, Model, Models, Provider,
    StopReason, StreamEvent, StreamOptions, Usage,
};

/// A provider that always replies with the same text. · 永远回复同一句话的提供商。
struct EchoProvider {
    model: Model,
}

impl EchoProvider {
    fn new() -> Self {
        EchoProvider {
            model: Model {
                id: "echo-1".to_string(),
                name: "Echo 1".to_string(),
                provider: "echo".to_string(),
                api: "echo".to_string(),
                base_url: String::new(),
                context_window: 8_192,
                max_tokens: 256,
                reasoning: false,
                input: vec![Modality::Text],
            },
        }
    }
}

impl Provider for EchoProvider {
    fn id(&self) -> &'static str {
        "echo"
    }

    fn models(&self) -> Vec<Model> {
        vec![self.model.clone()]
    }

    fn stream(&self, model: Model, _context: Context, _options: StreamOptions) -> EventStream {
        let reply = "pong".to_string();

        // The final message is authoritative; the intermediate events are for a UI.
        // 最终消息是权威结果；中间事件是给 UI 用的。
        let mut message = AssistantMessage::partial(&model);
        message.content.push(ContentBlock::text(reply.clone()));
        message.stop_reason = StopReason::Stop;
        message.usage = Usage {
            input: 1,
            output: 1,
            ..Usage::default()
        };

        let events = vec![
            StreamEvent::Start {
                partial: AssistantMessage::partial(&model),
            },
            StreamEvent::TextStart { content_index: 0 },
            StreamEvent::TextDelta {
                content_index: 0,
                delta: reply.clone(),
            },
            StreamEvent::TextEnd {
                content_index: 0,
                content: reply,
            },
            StreamEvent::Done {
                reason: StopReason::Stop,
                message,
            },
        ];
        Box::pin(stream::iter(events))
    }
}

#[tokio::main]
async fn main() {
    let provider = EchoProvider::new();
    let model = provider.model.clone();

    let mut models = Models::new();
    models.register(provider);

    // `complete` drains the stream and hands back the final assistant message.
    // `complete` 会把事件流读尽，并返回最终的 assistant 消息。
    let reply = models
        .complete(&model, Context::default(), StreamOptions::default())
        .await;
    println!("{}: {}", reply.provider, reply.to_text());
}
