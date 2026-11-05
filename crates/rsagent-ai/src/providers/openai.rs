//! `OpenAI` Chat Completions-compatible provider.
//!
//! One wire protocol (`openai-completions`) is shared by a large family of services:
//! `OpenAI` itself, `DeepSeek`, `Groq`, `Together`, `OpenRouter`, and local servers such as
//! `Ollama` and `vLLM`. This provider covers all of them by making the base URL configurable.

use std::collections::HashMap;

use async_stream::stream;
use futures::StreamExt;
use serde_json::{json, Value};

use crate::event::{ErrorReason, StreamEvent};
use crate::message::{AssistantMessage, ContentBlock, Context, Message};
use crate::provider::{Auth, EventStream, Provider, StreamOptions};
use crate::types::{now_millis, Model, StopReason, Usage};

/// An `OpenAI` Chat Completions-compatible provider.
#[derive(Debug)]
pub struct OpenAiProvider {
    id: String,
    models: Vec<Model>,
    env_keys: Vec<String>,
    base_url_override: Option<String>,
}

impl OpenAiProvider {
    /// Create a provider from its id, model catalog, and API-key environment variables.
    pub fn new(id: impl Into<String>, models: Vec<Model>, env_keys: Vec<String>) -> Self {
        OpenAiProvider {
            id: id.into(),
            models,
            env_keys,
            base_url_override: None,
        }
    }

    /// Override the base URL for every model (e.g. point at Ollama or a proxy).
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url_override = Some(base_url.into());
        self
    }

    /// The stock `OpenAI` provider.
    pub fn openai() -> Self {
        let models = vec![
            Model::openai_compatible("openai", "gpt-4o-mini", "https://api.openai.com/v1"),
            Model::openai_compatible("openai", "gpt-4o", "https://api.openai.com/v1"),
        ];
        OpenAiProvider::new("openai", models, vec!["OPENAI_API_KEY".to_string()])
    }

    /// A single-model provider for an OpenAI-compatible endpoint (Ollama, vLLM, ...).
    pub fn custom(
        id: impl Into<String>,
        base_url: impl Into<String>,
        model_id: impl Into<String>,
        env_keys: Vec<String>,
    ) -> Self {
        let id = id.into();
        let base_url = base_url.into();
        let model = Model::openai_compatible(id.clone(), model_id, base_url);
        OpenAiProvider::new(id, vec![model], env_keys)
    }

    fn resolve_api_key(&self) -> Option<String> {
        for key in &self.env_keys {
            if let Ok(value) = std::env::var(key) {
                if !value.is_empty() {
                    return Some(value);
                }
            }
        }
        None
    }

    fn effective_base_url(&self, model: &Model) -> String {
        self.base_url_override
            .clone()
            .unwrap_or_else(|| model.base_url.clone())
    }
}

impl Provider for OpenAiProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn models(&self) -> Vec<Model> {
        self.models.clone()
    }

    fn resolve_auth(&self, _model: &Model) -> Auth {
        Auth {
            api_key: self.resolve_api_key(),
            headers: Vec::new(),
            base_url: self.base_url_override.clone(),
        }
    }

    fn stream(&self, model: Model, context: Context, options: StreamOptions) -> EventStream {
        let api_key = options.api_key.clone().or_else(|| self.resolve_api_key());
        let base_url = self.effective_base_url(&model);
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        let body = build_request_body(&model, &context, &options);
        let extra_headers = options.headers;
        let max_retries = options.max_retries;

        Box::pin(stream! {
            let mut parser = OpenAiStreamParser::new(&model);
            yield StreamEvent::Start { partial: parser.partial.clone() };

            let Some(api_key) = api_key else {
                yield StreamEvent::Error {
                    reason: ErrorReason::Error,
                    error: AssistantMessage::error(&model, "missing API key"),
                };
                return;
            };

            let mut headers = vec![("authorization".to_string(), format!("Bearer {api_key}"))];
            headers.extend(extra_headers);

            let response =
                match super::http::send_with_retry(&url, &headers, &body, max_retries).await {
                    Ok(response) => response,
                    Err(error) => {
                        yield StreamEvent::Error {
                            reason: ErrorReason::Error,
                            error: AssistantMessage::error(&model, error),
                        };
                        return;
                    }
                };

            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();

            'stream: while let Some(chunk) = byte_stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(newline) = buffer.find('\n') {
                            let line = buffer[..newline].trim_end_matches('\r').to_string();
                            buffer.drain(..=newline);

                            let Some(data) = line.strip_prefix("data:") else {
                                continue;
                            };
                            let data = data.trim();
                            if data == "[DONE]" {
                                break 'stream;
                            }
                            if data.is_empty() {
                                continue;
                            }
                            if let Ok(value) = serde_json::from_str::<Value>(data) {
                                for event in parser.handle_chunk(&value) {
                                    yield event;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        yield StreamEvent::Error {
                            reason: ErrorReason::Error,
                            error: AssistantMessage::error(&model, format!("stream error: {error}")),
                        };
                        return;
                    }
                }
            }

            for event in parser.finish() {
                yield event;
            }
        })
    }
}

/// Converts a [`Context`] into the `OpenAI` `messages` array.
fn to_openai_messages(context: &Context) -> Vec<Value> {
    let mut out = Vec::new();

    if let Some(system_prompt) = &context.system_prompt {
        if !system_prompt.is_empty() {
            out.push(json!({ "role": "system", "content": system_prompt }));
        }
    }

    for message in &context.messages {
        match message {
            Message::System(m) => {
                out.push(json!({ "role": "system", "content": blocks_to_text(&m.content) }));
            }
            Message::User(m) => out.push(user_message(&m.content)),
            Message::Assistant(m) => out.push(assistant_message(m)),
            Message::ToolResult(m) => {
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": m.tool_call_id,
                    "content": blocks_to_text(&m.content),
                }));
            }
        }
    }

    out
}

fn blocks_to_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(ContentBlock::as_text)
        .collect::<Vec<_>>()
        .join("")
}

fn user_message(blocks: &[ContentBlock]) -> Value {
    let has_image = blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. }));

    if !has_image {
        return json!({ "role": "user", "content": blocks_to_text(blocks) });
    }

    let parts: Vec<Value> = blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => json!({ "type": "text", "text": text }),
            ContentBlock::Image { data, mime_type } => json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{mime_type};base64,{data}") },
            }),
            _ => json!({ "type": "text", "text": "" }),
        })
        .collect();

    json!({ "role": "user", "content": parts })
}

fn assistant_message(message: &AssistantMessage) -> Value {
    let text = message.to_text();
    let tool_calls: Vec<Value> = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall {
                id,
                name,
                arguments,
            } => Some(json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": serde_json::to_string(arguments).unwrap_or_else(|_| "{}".to_string()),
                },
            })),
            _ => None,
        })
        .collect();

    let mut value = json!({
        "role": "assistant",
        "content": if text.is_empty() { Value::Null } else { Value::String(text) },
    });
    if !tool_calls.is_empty() {
        value["tool_calls"] = Value::Array(tool_calls);
    }
    value
}

fn build_request_body(model: &Model, context: &Context, options: &StreamOptions) -> Value {
    let mut body = json!({
        "model": model.id,
        "messages": to_openai_messages(context),
        "stream": true,
        "stream_options": { "include_usage": true },
    });

    if !context.tools.is_empty() {
        let tools: Vec<Value> = context
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    },
                })
            })
            .collect();
        body["tools"] = Value::Array(tools);
    }

    if let Some(temperature) = options.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(max_tokens) = options.max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }

    body
}

/// Incremental parser turning `OpenAI` streaming chunks into normalized [`StreamEvent`]s.
///
/// Kept as a standalone struct so its behavior is unit-testable without a network.
struct OpenAiStreamParser {
    partial: AssistantMessage,
    /// openai tool-call index -> accumulated raw argument JSON string.
    tool_arg_buffers: HashMap<usize, String>,
    /// openai tool-call index -> index into `partial.content`.
    tool_content_index: HashMap<usize, usize>,
    finish_reason: Option<String>,
    usage: Usage,
}

impl OpenAiStreamParser {
    fn new(model: &Model) -> Self {
        OpenAiStreamParser {
            partial: AssistantMessage::partial(model),
            tool_arg_buffers: HashMap::new(),
            tool_content_index: HashMap::new(),
            finish_reason: None,
            usage: Usage::default(),
        }
    }

    fn handle_chunk(&mut self, value: &Value) -> Vec<StreamEvent> {
        let mut events = Vec::new();

        if let Some(usage) = value.get("usage").and_then(Value::as_object) {
            self.usage = Usage {
                input: usage
                    .get("prompt_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                output: usage
                    .get("completion_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cache_read: usage
                    .get("prompt_tokens_details")
                    .and_then(|d| d.get("cached_tokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cache_write: 0,
            };
        }

        let Some(choices) = value.get("choices").and_then(Value::as_array) else {
            return events;
        };

        for choice in choices {
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                self.finish_reason = Some(reason.to_string());
            }

            let Some(delta) = choice.get("delta") else {
                continue;
            };

            if let Some(reasoning) = delta.get("reasoning_content").and_then(Value::as_str) {
                self.push_thinking(reasoning, &mut events);
            }
            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                self.push_text(text, &mut events);
            }
            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for call in tool_calls {
                    self.push_tool_call(call, &mut events);
                }
            }
        }

        events
    }

    fn push_thinking(&mut self, delta: &str, events: &mut Vec<StreamEvent>) {
        let index = if let Some(ContentBlock::Thinking { .. }) = self.partial.content.last() {
            self.partial.content.len() - 1
        } else {
            let index = self.partial.content.len();
            self.partial.content.push(ContentBlock::Thinking {
                thinking: String::new(),
                signature: None,
            });
            events.push(StreamEvent::ThinkingStart {
                content_index: index,
            });
            index
        };
        if let Some(ContentBlock::Thinking { thinking, .. }) = self.partial.content.get_mut(index) {
            thinking.push_str(delta);
        }
        events.push(StreamEvent::ThinkingDelta {
            content_index: index,
            delta: delta.to_string(),
        });
    }

    fn push_text(&mut self, delta: &str, events: &mut Vec<StreamEvent>) {
        let index = if let Some(ContentBlock::Text { .. }) = self.partial.content.last() {
            self.partial.content.len() - 1
        } else {
            let index = self.partial.content.len();
            self.partial.content.push(ContentBlock::Text {
                text: String::new(),
            });
            events.push(StreamEvent::TextStart {
                content_index: index,
            });
            index
        };
        if let Some(ContentBlock::Text { text }) = self.partial.content.get_mut(index) {
            text.push_str(delta);
        }
        events.push(StreamEvent::TextDelta {
            content_index: index,
            delta: delta.to_string(),
        });
    }

    fn push_tool_call(&mut self, call: &Value, events: &mut Vec<StreamEvent>) {
        let openai_index =
            usize::try_from(call.get("index").and_then(Value::as_u64).unwrap_or(0)).unwrap_or(0);

        let content_index = if let Some(index) = self.tool_content_index.get(&openai_index) {
            *index
        } else {
            let index = self.partial.content.len();
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .map_or_else(|| format!("call_{index}"), str::to_string);
            let name = call
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            self.partial.content.push(ContentBlock::ToolCall {
                id,
                name,
                arguments: json!({}),
            });
            self.tool_content_index.insert(openai_index, index);
            self.tool_arg_buffers.insert(openai_index, String::new());
            events.push(StreamEvent::ToolCallStart {
                content_index: index,
            });
            index
        };

        // A late chunk may carry the function name or id that the first chunk omitted.
        if let Some(name) = call
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
        {
            if let Some(ContentBlock::ToolCall { name: slot, .. }) =
                self.partial.content.get_mut(content_index)
            {
                if slot.is_empty() {
                    *slot = name.to_string();
                }
            }
        }

        if let Some(arguments) = call
            .get("function")
            .and_then(|f| f.get("arguments"))
            .and_then(Value::as_str)
        {
            if let Some(buffer) = self.tool_arg_buffers.get_mut(&openai_index) {
                buffer.push_str(arguments);
            }
            events.push(StreamEvent::ToolCallDelta {
                content_index,
                delta: arguments.to_string(),
            });
        }
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();

        // Finalize tool-call arguments now that the full JSON has arrived.
        for (openai_index, content_index) in self.tool_content_index.clone() {
            let raw = self
                .tool_arg_buffers
                .get(&openai_index)
                .cloned()
                .unwrap_or_default();
            let arguments = serde_json::from_str::<Value>(&raw).unwrap_or_else(|_| json!({}));
            if let Some(ContentBlock::ToolCall {
                arguments: slot, ..
            }) = self.partial.content.get_mut(content_index)
            {
                *slot = arguments;
            }
        }

        // Emit the matching `*_end` events for every block, in order.
        for (index, block) in self.partial.content.iter().enumerate() {
            match block {
                ContentBlock::Text { text } => events.push(StreamEvent::TextEnd {
                    content_index: index,
                    content: text.clone(),
                }),
                ContentBlock::Thinking { thinking, .. } => events.push(StreamEvent::ThinkingEnd {
                    content_index: index,
                    content: thinking.clone(),
                }),
                ContentBlock::ToolCall { .. } => events.push(StreamEvent::ToolCallEnd {
                    content_index: index,
                    tool_call: block.clone(),
                }),
                ContentBlock::Image { .. } => {}
            }
        }

        self.partial.stop_reason = stop_reason_from_finish(self.finish_reason.as_deref());
        self.partial.usage = self.usage.clone();
        self.partial.timestamp = now_millis();

        events.push(StreamEvent::Done {
            reason: self.partial.stop_reason,
            message: self.partial.clone(),
        });

        events
    }
}

fn stop_reason_from_finish(finish_reason: Option<&str>) -> StopReason {
    match finish_reason {
        Some("tool_calls" | "function_call") => StopReason::ToolUse,
        Some("length") => StopReason::Length,
        _ => StopReason::Stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Modality;

    fn test_model() -> Model {
        Model {
            id: "test".to_string(),
            name: "Test".to_string(),
            provider: "test".to_string(),
            api: "openai-completions".to_string(),
            base_url: "http://localhost".to_string(),
            context_window: 1000,
            max_tokens: 100,
            reasoning: true,
            input: vec![Modality::Text],
        }
    }

    #[test]
    fn parses_streamed_text_and_tool_call() {
        let model = test_model();
        let mut parser = OpenAiStreamParser::new(&model);

        parser.handle_chunk(&json!({
            "choices": [{ "delta": { "content": "Hel" } }]
        }));
        parser.handle_chunk(&json!({
            "choices": [{ "delta": { "content": "lo" } }]
        }));
        parser.handle_chunk(&json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": { "name": "read_file", "arguments": "{\"path\":" }
                    }]
                }
            }]
        }));
        parser.handle_chunk(&json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "\"a.txt\"}" }
                    }]
                }
            }]
        }));
        parser.handle_chunk(&json!({
            "choices": [{ "delta": {}, "finish_reason": "tool_calls" }]
        }));

        let events = parser.finish();
        let done = events
            .iter()
            .find_map(|e| match e {
                StreamEvent::Done { message, .. } => Some(message.clone()),
                _ => None,
            })
            .expect("a Done event");

        assert_eq!(done.to_text(), "Hello");
        assert_eq!(done.stop_reason, StopReason::ToolUse);
        let calls = done.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "read_file");
        assert_eq!(calls[0].2["path"], "a.txt");
    }
}
