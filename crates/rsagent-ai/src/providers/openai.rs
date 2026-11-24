//! `OpenAI` Chat Completions-compatible provider.
//!
//! One wire protocol (`openai-completions`) is shared by a large family of services:
//! `OpenAI` itself, `DeepSeek`, `Groq`, `Together`, `OpenRouter`, and local servers such as
//! `Ollama` and `vLLM`. This provider covers all of them by making the base URL configurable.

use std::collections::HashMap;
use std::time::Duration;

use async_stream::stream;
use futures::StreamExt;
use serde_json::{json, Value};

use super::core::ProviderCore;
use super::sse::{self, SseLine};
use crate::event::{ErrorReason, StreamEvent};
use crate::message::{AssistantMessage, ContentBlock, Context, Message};
use crate::provider::{Auth, EventStream, Provider, StreamOptions};
use crate::types::{now_millis, Model, StopReason, ThinkingLevel, Usage};

/// An `OpenAI` Chat Completions-compatible provider.
#[derive(Debug)]
pub struct OpenAiProvider {
    core: ProviderCore,
}

impl OpenAiProvider {
    /// Create a provider from its id, model catalog, and API-key environment variables.
    pub fn new(id: impl Into<String>, models: Vec<Model>, env_keys: Vec<String>) -> Self {
        OpenAiProvider {
            core: ProviderCore::new(id.into(), models, env_keys),
        }
    }

    /// Override the base URL for every model (e.g. point at Ollama or a proxy).
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.core.set_base_url(base_url.into());
        self
    }

    /// Override the per-read (idle) timeout for the streaming response.
    ///
    /// The default is generous; lower it to fail fast on a stalled connection, or raise it
    /// for an endpoint that is slow between tokens.
    #[must_use]
    pub fn with_read_timeout(mut self, read_timeout: Duration) -> Self {
        self.core = self.core.with_read_timeout(read_timeout);
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
}

impl Provider for OpenAiProvider {
    fn id(&self) -> &str {
        self.core.id()
    }

    fn models(&self) -> Vec<Model> {
        self.core.models()
    }

    fn resolve_auth(&self, _model: &Model) -> Auth {
        self.core.resolve_auth()
    }

    fn stream(&self, model: Model, context: Context, options: StreamOptions) -> EventStream {
        let api_key = options
            .api_key
            .clone()
            .or_else(|| self.core.resolve_api_key());
        let base_url = self.core.effective_base_url(&model);
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        let body = build_request_body(&model, &context, &options);
        let extra_headers = options.headers;
        let max_retries = options.max_retries;
        let client = self.core.client().clone();
        let missing_key = self.core.missing_key_message();

        Box::pin(stream! {
            let mut parser = OpenAiStreamParser::new(&model);
            yield StreamEvent::Start { partial: parser.partial.clone() };

            let Some(api_key) = api_key else {
                yield StreamEvent::Error {
                    reason: ErrorReason::Error,
                    error: AssistantMessage::error(&model, missing_key),
                };
                return;
            };

            let mut headers = vec![("authorization".to_string(), format!("Bearer {api_key}"))];
            headers.extend(extra_headers);

            let response =
                match super::http::send_with_retry(&client, &url, &headers, &body, max_retries).await
                {
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
            let mut buffer: Vec<u8> = Vec::new();

            'stream: while let Some(chunk) = byte_stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        buffer.extend_from_slice(&bytes);
                        while let Some(line) = sse::next_line(&mut buffer) {
                            let SseLine::Data(data) = sse::classify(&line) else {
                                continue;
                            };
                            // OpenAI terminates the stream with a `[DONE]` sentinel.
                            if data == "[DONE]" {
                                break 'stream;
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
                out.push(
                    json!({ "role": "system", "content": ContentBlock::concat_text(&m.content) }),
                );
            }
            Message::User(m) => out.push(user_message(&m.content)),
            Message::Assistant(m) => out.push(assistant_message(m)),
            Message::ToolResult(m) => {
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": m.tool_call_id,
                    "content": ContentBlock::concat_text(&m.content),
                }));
            }
        }
    }

    out
}

fn user_message(blocks: &[ContentBlock]) -> Value {
    let has_image = blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. }));

    if !has_image {
        return json!({ "role": "user", "content": ContentBlock::concat_text(blocks) });
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
    if let Some(effort) = openai_reasoning_effort(options.thinking) {
        body["reasoning_effort"] = json!(effort);
    }

    body
}

/// Map a [`ThinkingLevel`] to an OpenAI `reasoning_effort` value, if any.
///
/// OpenAI has no `xhigh`/`max`, so those levels saturate at `high`.
fn openai_reasoning_effort(level: ThinkingLevel) -> Option<&'static str> {
    match level {
        ThinkingLevel::Off => None,
        ThinkingLevel::Minimal => Some("minimal"),
        ThinkingLevel::Low => Some("low"),
        ThinkingLevel::Medium => Some("medium"),
        ThinkingLevel::High | ThinkingLevel::XHigh | ThinkingLevel::Max => Some("high"),
    }
}

/// Incremental parser turning `OpenAI` streaming chunks into normalized [`StreamEvent`]s.
///
/// Kept as a standalone struct so its behavior is unit-testable without a network.
struct OpenAiStreamParser {
    model: Model,
    partial: AssistantMessage,
    /// openai tool-call index -> accumulated raw argument JSON string.
    tool_arg_buffers: HashMap<usize, String>,
    /// openai tool-call index -> index into `partial.content`.
    tool_content_index: HashMap<usize, usize>,
    finish_reason: Option<String>,
    usage: Usage,
    /// Set once a mid-stream `error` object arrives, so `finish` does not emit a success
    /// `Done` and nothing after the error is read.
    failed: bool,
}

impl OpenAiStreamParser {
    fn new(model: &Model) -> Self {
        OpenAiStreamParser {
            model: model.clone(),
            partial: AssistantMessage::partial(model),
            tool_arg_buffers: HashMap::new(),
            tool_content_index: HashMap::new(),
            finish_reason: None,
            usage: Usage::default(),
            failed: false,
        }
    }

    fn handle_chunk(&mut self, value: &Value) -> Vec<StreamEvent> {
        // A failed stream already yielded its terminal `Error`; ignore anything after it.
        if self.failed {
            return Vec::new();
        }

        // An `error` object mid-stream ends the turn. Ignoring it let the turn end as an
        // empty success, so the failure was invisible to the caller.
        if let Some(error) = value.get("error") {
            self.failed = true;
            let detail = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("provider error");
            return vec![StreamEvent::Error {
                reason: ErrorReason::Error,
                error: AssistantMessage::error(&self.model, detail),
            }];
        }

        let mut events = Vec::new();

        if let Some(usage) = value.get("usage").and_then(Value::as_object) {
            // OpenAI's `prompt_tokens` *includes* cache hits (they are a subset, in
            // `prompt_tokens_details.cached_tokens`), but `Usage::input` is cache-exclusive
            // — the Anthropic shape — so the cached tokens are subtracted back out.
            let cached = usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let prompt = usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            self.usage = Usage {
                input: prompt.saturating_sub(cached),
                output: usage
                    .get("completion_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cache_read: cached,
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
        // A failed stream already yielded its terminal `Error`; emitting a `Done` now would
        // let "last terminal event wins" consumers read success.
        if self.failed {
            return Vec::new();
        }

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

        // A stream that carried a tool call but no `finish_reason` was cut off: the call may
        // be incomplete, and defaulting to `Stop` would silently drop it. Fail instead.
        let has_tool_call = self
            .partial
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolCall { .. }));
        if self.finish_reason.is_none() && has_tool_call {
            return vec![StreamEvent::Error {
                reason: ErrorReason::Error,
                error: AssistantMessage::error(
                    &self.model,
                    "the stream ended before the tool call finished (no finish_reason)",
                ),
            }];
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

    #[test]
    fn usage_normalizes_prompt_tokens_to_a_cache_exclusive_input() {
        let model = test_model();
        let mut parser = OpenAiStreamParser::new(&model);

        // OpenAI's `prompt_tokens` (100) *includes* the 40 cached tokens. `Usage::input` is
        // cache-exclusive, so those must be subtracted out — and `input + cache_read` must
        // recover the full prompt size, matching how Anthropic already reports it.
        parser.handle_chunk(&json!({
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 20,
                "prompt_tokens_details": { "cached_tokens": 40 }
            },
            "choices": [{ "delta": {} }]
        }));

        let usage = parser
            .finish()
            .into_iter()
            .find_map(|event| match event {
                StreamEvent::Done { message, .. } => Some(message.usage),
                _ => None,
            })
            .expect("a Done event");

        assert_eq!(usage.input, 60);
        assert_eq!(usage.cache_read, 40);
        assert_eq!(usage.output, 20);
        assert_eq!(usage.input + usage.cache_read, 100);
    }

    #[test]
    fn maps_thinking_levels_to_reasoning_effort() {
        let model = test_model();
        let context = Context::default();
        let body_for = |level| {
            let options = StreamOptions {
                thinking: level,
                ..StreamOptions::default()
            };
            build_request_body(&model, &context, &options)
        };

        assert!(body_for(ThinkingLevel::Off)
            .get("reasoning_effort")
            .is_none());
        assert_eq!(
            body_for(ThinkingLevel::Minimal)["reasoning_effort"],
            json!("minimal")
        );
        assert_eq!(
            body_for(ThinkingLevel::Low)["reasoning_effort"],
            json!("low")
        );
        assert_eq!(
            body_for(ThinkingLevel::Medium)["reasoning_effort"],
            json!("medium")
        );
        assert_eq!(
            body_for(ThinkingLevel::High)["reasoning_effort"],
            json!("high")
        );
        assert_eq!(
            body_for(ThinkingLevel::Max)["reasoning_effort"],
            json!("high")
        );
    }

    #[test]
    fn an_error_object_in_the_stream_ends_it() {
        let model = test_model();
        let mut parser = OpenAiStreamParser::new(&model);

        parser.handle_chunk(&json!({ "choices": [{ "delta": { "content": "hi" } }] }));
        let events = parser.handle_chunk(&json!({ "error": { "message": "boom" } }));
        assert!(
            matches!(events.last(), Some(StreamEvent::Error { .. })),
            "expected a terminal error, got {events:?}"
        );

        // Nothing after the error is read, and `finish` must not append a success `Done`.
        assert!(parser
            .handle_chunk(&json!({ "choices": [{ "delta": { "content": "more" } }] }))
            .is_empty());
        assert!(parser.finish().is_empty(), "no Done after an error");
    }

    #[test]
    fn a_truncated_stream_with_a_tool_call_is_not_a_silent_success() {
        let model = test_model();
        let mut parser = OpenAiStreamParser::new(&model);

        // A tool call begins, but the stream ends with no `finish_reason`.
        parser.handle_chunk(&json!({
            "choices": [{ "delta": { "tool_calls": [{
                "index": 0,
                "id": "call_1",
                "function": { "name": "read_file", "arguments": "{\"path\":\"a" }
            }] } }]
        }));

        let events = parser.finish();
        assert!(
            matches!(events.last(), Some(StreamEvent::Error { .. })),
            "a truncated tool call must fail, not stop, got {events:?}"
        );
    }

    #[test]
    fn a_stream_with_no_finish_reason_and_no_tool_call_still_stops() {
        // Some servers omit `finish_reason` on a plain-text stream; that is a clean stop.
        let model = test_model();
        let mut parser = OpenAiStreamParser::new(&model);
        parser.handle_chunk(&json!({ "choices": [{ "delta": { "content": "hi" } }] }));

        let reason = parser.finish().iter().find_map(|event| match event {
            StreamEvent::Done { reason, .. } => Some(*reason),
            _ => None,
        });
        assert_eq!(reason, Some(StopReason::Stop));
    }
}
