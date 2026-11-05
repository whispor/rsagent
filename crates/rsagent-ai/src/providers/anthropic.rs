//! Anthropic Messages API-compatible provider.
//!
//! Anthropic's wire protocol differs from OpenAI's in three ways this module handles:
//! the system prompt is a top-level `system` field (not a message), tool results are
//! `tool_result` blocks inside a `user` message, and tool arguments arrive as a stream
//! of `input_json_delta` fragments. Everything is normalized into the same
//! [`StreamEvent`] sequence the rest of the stack already understands.

use std::collections::{HashMap, HashSet};

use async_stream::stream;
use futures::StreamExt;
use serde_json::{json, Value};

use crate::event::{ErrorReason, StreamEvent};
use crate::message::{AssistantMessage, ContentBlock, Context, Message};
use crate::provider::{Auth, EventStream, Provider, StreamOptions};
use crate::types::{now_millis, Modality, Model, StopReason, Usage};

/// The Anthropic API version header value this provider sends.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// An Anthropic Messages-compatible provider.
#[derive(Debug)]
pub struct AnthropicProvider {
    id: String,
    models: Vec<Model>,
    env_keys: Vec<String>,
    base_url_override: Option<String>,
}

/// Build an Anthropic-flavored [`Model`] with the Messages wire protocol.
fn messages_model(provider: &str, id: impl Into<String>, base_url: impl Into<String>) -> Model {
    let id = id.into();
    Model {
        name: id.clone(),
        id,
        provider: provider.to_string(),
        api: "anthropic-messages".to_string(),
        base_url: base_url.into(),
        context_window: 200_000,
        max_tokens: 8_192,
        reasoning: true,
        input: vec![Modality::Text, Modality::Image],
    }
}

impl AnthropicProvider {
    /// Create a provider from its id, model catalog, and API-key environment variables.
    pub fn new(id: impl Into<String>, models: Vec<Model>, env_keys: Vec<String>) -> Self {
        AnthropicProvider {
            id: id.into(),
            models,
            env_keys,
            base_url_override: None,
        }
    }

    /// Override the base URL for every model (e.g. point at a proxy).
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url_override = Some(base_url.into());
        self
    }

    /// The stock Anthropic provider.
    pub fn anthropic() -> Self {
        let models = vec![
            messages_model(
                "anthropic",
                "claude-3-5-sonnet-latest",
                "https://api.anthropic.com",
            ),
            messages_model(
                "anthropic",
                "claude-3-5-haiku-latest",
                "https://api.anthropic.com",
            ),
        ];
        AnthropicProvider::new("anthropic", models, vec!["ANTHROPIC_API_KEY".to_string()])
    }

    /// A single-model provider for an Anthropic-compatible endpoint.
    pub fn custom(
        id: impl Into<String>,
        base_url: impl Into<String>,
        model_id: impl Into<String>,
        env_keys: Vec<String>,
    ) -> Self {
        let id = id.into();
        let model = messages_model(&id, model_id, base_url);
        AnthropicProvider::new(id, vec![model], env_keys)
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

impl Provider for AnthropicProvider {
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
        let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
        let body = build_request_body(&model, &context, &options);
        let extra_headers = options.headers;
        let max_retries = options.max_retries;

        Box::pin(stream! {
            let mut parser = AnthropicStreamParser::new(&model);
            yield StreamEvent::Start { partial: parser.partial.clone() };

            let Some(api_key) = api_key else {
                yield StreamEvent::Error {
                    reason: ErrorReason::Error,
                    error: AssistantMessage::error(&model, "missing API key"),
                };
                return;
            };

            let mut headers = vec![
                ("x-api-key".to_string(), api_key),
                ("anthropic-version".to_string(), ANTHROPIC_VERSION.to_string()),
            ];
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

            while let Some(chunk) = byte_stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(newline) = buffer.find('\n') {
                            let line = buffer[..newline].trim_end_matches('\r').to_string();
                            buffer.drain(..=newline);

                            // Anthropic interleaves `event:` and `data:` lines; only the
                            // `data:` JSON is needed, and it carries its own `type` field.
                            let Some(data) = line.strip_prefix("data:") else {
                                continue;
                            };
                            let data = data.trim();
                            if data.is_empty() {
                                continue;
                            }
                            if let Ok(value) = serde_json::from_str::<Value>(data) {
                                for event in parser.handle_event(&value) {
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

/// Converts a [`Context`] into the Anthropic `messages` array.
///
/// The system prompt is dropped here (it is sent as a top-level field), and consecutive
/// tool results are merged into a single `user` message, which is what Anthropic expects.
fn to_anthropic_messages(context: &Context) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut pending_tool_results: Vec<Value> = Vec::new();

    for message in &context.messages {
        match message {
            Message::System(_) => {}
            Message::User(m) => {
                flush_tool_results(&mut out, &mut pending_tool_results);
                out.push(json!({ "role": "user", "content": user_blocks(&m.content) }));
            }
            Message::Assistant(m) => {
                flush_tool_results(&mut out, &mut pending_tool_results);
                out.push(json!({ "role": "assistant", "content": assistant_blocks(m) }));
            }
            Message::ToolResult(m) => {
                pending_tool_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id,
                    "content": blocks_to_text(&m.content),
                    "is_error": m.is_error,
                }));
            }
        }
    }
    flush_tool_results(&mut out, &mut pending_tool_results);
    out
}

/// Merge any accumulated tool results into one trailing `user` message.
fn flush_tool_results(out: &mut Vec<Value>, pending: &mut Vec<Value>) {
    if pending.is_empty() {
        return;
    }
    let content = std::mem::take(pending);
    out.push(json!({ "role": "user", "content": content }));
}

fn blocks_to_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(ContentBlock::as_text)
        .collect::<Vec<_>>()
        .join("")
}

fn user_blocks(blocks: &[ContentBlock]) -> Vec<Value> {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => json!({ "type": "text", "text": text }),
            ContentBlock::Image { data, mime_type } => json!({
                "type": "image",
                "source": { "type": "base64", "media_type": mime_type, "data": data },
            }),
            _ => json!({ "type": "text", "text": "" }),
        })
        .collect()
}

fn assistant_blocks(message: &AssistantMessage) -> Vec<Value> {
    let mut blocks: Vec<Value> = Vec::new();
    let text = message.to_text();
    if !text.is_empty() {
        blocks.push(json!({ "type": "text", "text": text }));
    }
    for (id, name, arguments) in message.tool_calls() {
        blocks.push(json!({ "type": "tool_use", "id": id, "name": name, "input": arguments }));
    }
    if blocks.is_empty() {
        blocks.push(json!({ "type": "text", "text": "" }));
    }
    blocks
}

fn build_request_body(model: &Model, context: &Context, options: &StreamOptions) -> Value {
    // Anthropic requires `max_tokens`; fall back to the model's ceiling when unset.
    let max_tokens = options
        .max_tokens
        .unwrap_or_else(|| u32::try_from(model.max_tokens).unwrap_or(4_096));
    let mut body = json!({
        "model": model.id,
        "max_tokens": max_tokens,
        "messages": to_anthropic_messages(context),
        "stream": true,
    });

    if let Some(system) = &context.system_prompt {
        if !system.is_empty() {
            body["system"] = json!(system);
        }
    }
    if !context.tools.is_empty() {
        let tools: Vec<Value> = context
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.parameters,
                })
            })
            .collect();
        body["tools"] = Value::Array(tools);
    }
    if let Some(temperature) = options.temperature {
        body["temperature"] = json!(temperature);
    }

    body
}

fn stop_reason_from_anthropic(reason: Option<&str>) -> StopReason {
    match reason {
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::Length,
        _ => StopReason::Stop,
    }
}

/// Incremental parser turning Anthropic SSE events into normalized [`StreamEvent`]s.
///
/// Kept as a standalone struct so its behavior is unit-testable without a network.
struct AnthropicStreamParser {
    model: Model,
    partial: AssistantMessage,
    /// anthropic content-block index -> index into `partial.content`.
    block_index: HashMap<usize, usize>,
    /// anthropic content-block index -> accumulated `partial_json` for a `tool_use` block.
    tool_json: HashMap<usize, String>,
    /// anthropic content-block indices whose `*_end` event has already been emitted.
    closed: HashSet<usize>,
    stop_reason: Option<String>,
    usage: Usage,
}

impl AnthropicStreamParser {
    fn new(model: &Model) -> Self {
        AnthropicStreamParser {
            model: model.clone(),
            partial: AssistantMessage::partial(model),
            block_index: HashMap::new(),
            tool_json: HashMap::new(),
            closed: HashSet::new(),
            stop_reason: None,
            usage: Usage::default(),
        }
    }

    fn handle_event(&mut self, value: &Value) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            return events;
        };

        match kind {
            "message_start" => {
                if let Some(usage) = value.get("message").and_then(|m| m.get("usage")) {
                    self.usage.input = json_u64(usage, "input_tokens");
                    self.usage.cache_read = json_u64(usage, "cache_read_input_tokens");
                    self.usage.cache_write = json_u64(usage, "cache_creation_input_tokens");
                }
            }
            "content_block_start" => self.start_block(value, &mut events),
            "content_block_delta" => self.delta_block(value, &mut events),
            "content_block_stop" => {
                let index = block_index(value);
                if let Some(&content_index) = self.block_index.get(&index) {
                    self.emit_end(index, content_index, &mut events);
                }
            }
            "message_delta" => {
                if let Some(reason) = value
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.stop_reason = Some(reason.to_string());
                }
                if let Some(output) = value.get("usage").and_then(|u| u.get("output_tokens")) {
                    self.usage.output = output.as_u64().unwrap_or(self.usage.output);
                }
            }
            "error" => {
                let detail = value
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("provider error");
                events.push(StreamEvent::Error {
                    reason: ErrorReason::Error,
                    error: AssistantMessage::error(&self.model, detail),
                });
            }
            _ => {}
        }

        events
    }

    fn start_block(&mut self, value: &Value, events: &mut Vec<StreamEvent>) {
        let index = block_index(value);
        let Some(block) = value.get("content_block") else {
            return;
        };
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
        let content_index = self.partial.content.len();

        match block_type {
            "text" => {
                self.partial.content.push(ContentBlock::Text {
                    text: String::new(),
                });
                events.push(StreamEvent::TextStart { content_index });
            }
            "thinking" => {
                self.partial.content.push(ContentBlock::Thinking {
                    thinking: String::new(),
                    signature: None,
                });
                events.push(StreamEvent::ThinkingStart { content_index });
            }
            "tool_use" => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .map_or_else(|| format!("call_{content_index}"), str::to_string);
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                self.partial.content.push(ContentBlock::ToolCall {
                    id,
                    name,
                    arguments: json!({}),
                });
                self.tool_json.insert(index, String::new());
                events.push(StreamEvent::ToolCallStart { content_index });
            }
            _ => return,
        }

        self.block_index.insert(index, content_index);
    }

    fn delta_block(&mut self, value: &Value, events: &mut Vec<StreamEvent>) {
        let index = block_index(value);
        let Some(&content_index) = self.block_index.get(&index) else {
            return;
        };
        let Some(delta) = value.get("delta") else {
            return;
        };

        match delta.get("type").and_then(Value::as_str).unwrap_or("") {
            "text_delta" => {
                if let Some(text) = delta.get("text").and_then(Value::as_str) {
                    if let Some(ContentBlock::Text { text: slot }) =
                        self.partial.content.get_mut(content_index)
                    {
                        slot.push_str(text);
                    }
                    events.push(StreamEvent::TextDelta {
                        content_index,
                        delta: text.to_string(),
                    });
                }
            }
            "thinking_delta" => {
                if let Some(thinking) = delta.get("thinking").and_then(Value::as_str) {
                    if let Some(ContentBlock::Thinking { thinking: slot, .. }) =
                        self.partial.content.get_mut(content_index)
                    {
                        slot.push_str(thinking);
                    }
                    events.push(StreamEvent::ThinkingDelta {
                        content_index,
                        delta: thinking.to_string(),
                    });
                }
            }
            "input_json_delta" => {
                if let Some(partial) = delta.get("partial_json").and_then(Value::as_str) {
                    if let Some(buffer) = self.tool_json.get_mut(&index) {
                        buffer.push_str(partial);
                    }
                    events.push(StreamEvent::ToolCallDelta {
                        content_index,
                        delta: partial.to_string(),
                    });
                }
            }
            "signature_delta" => {
                if let Some(signature) = delta.get("signature").and_then(Value::as_str) {
                    if let Some(ContentBlock::Thinking {
                        signature: slot, ..
                    }) = self.partial.content.get_mut(content_index)
                    {
                        *slot = Some(signature.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    /// Emit the matching `*_end` event for one content block, finalizing a tool call's
    /// accumulated arguments first.
    fn emit_end(&mut self, index: usize, content_index: usize, events: &mut Vec<StreamEvent>) {
        self.closed.insert(index);
        match self.partial.content.get(content_index).cloned() {
            Some(ContentBlock::Text { text }) => events.push(StreamEvent::TextEnd {
                content_index,
                content: text,
            }),
            Some(ContentBlock::Thinking { thinking, .. }) => {
                events.push(StreamEvent::ThinkingEnd {
                    content_index,
                    content: thinking,
                });
            }
            Some(ContentBlock::ToolCall { .. }) => {
                let raw = self.tool_json.get(&index).cloned().unwrap_or_default();
                let arguments = if raw.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str::<Value>(&raw).unwrap_or_else(|_| json!({}))
                };
                if let Some(ContentBlock::ToolCall {
                    arguments: slot, ..
                }) = self.partial.content.get_mut(content_index)
                {
                    *slot = arguments;
                }
                if let Some(tool_call) = self.partial.content.get(content_index).cloned() {
                    events.push(StreamEvent::ToolCallEnd {
                        content_index,
                        tool_call,
                    });
                }
            }
            _ => {}
        }
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();

        // Close any block the stream ended without a `content_block_stop` for.
        let open: Vec<(usize, usize)> = self
            .block_index
            .iter()
            .filter(|(index, _)| !self.closed.contains(index))
            .map(|(index, content_index)| (*index, *content_index))
            .collect();
        for (index, content_index) in open {
            self.emit_end(index, content_index, &mut events);
        }

        self.partial.stop_reason = stop_reason_from_anthropic(self.stop_reason.as_deref());
        self.partial.usage = self.usage.clone();
        self.partial.timestamp = now_millis();

        events.push(StreamEvent::Done {
            reason: self.partial.stop_reason,
            message: self.partial.clone(),
        });

        events
    }
}

/// The `index` field of an Anthropic stream event, defaulting to 0.
fn block_index(value: &Value) -> usize {
    usize::try_from(value.get("index").and_then(Value::as_u64).unwrap_or(0)).unwrap_or(0)
}

/// Read a `u64` field, defaulting to 0 when missing.
fn json_u64(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_model() -> Model {
        messages_model("test", "claude-test", "http://localhost")
    }

    #[test]
    fn parses_streamed_text_and_tool_use() {
        let model = test_model();
        let mut parser = AnthropicStreamParser::new(&model);

        parser.handle_event(&json!({
            "type": "message_start",
            "message": { "usage": { "input_tokens": 12, "output_tokens": 1 } }
        }));
        parser.handle_event(&json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "text", "text": "" }
        }));
        parser.handle_event(&json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "Hel" }
        }));
        parser.handle_event(&json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "lo" }
        }));
        parser.handle_event(&json!({ "type": "content_block_stop", "index": 0 }));
        parser.handle_event(&json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": { "type": "tool_use", "id": "toolu_1", "name": "read_file", "input": {} }
        }));
        parser.handle_event(&json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": { "type": "input_json_delta", "partial_json": "{\"path\":" }
        }));
        parser.handle_event(&json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": { "type": "input_json_delta", "partial_json": "\"a.txt\"}" }
        }));
        parser.handle_event(&json!({ "type": "content_block_stop", "index": 1 }));
        parser.handle_event(&json!({
            "type": "message_delta",
            "delta": { "stop_reason": "tool_use" },
            "usage": { "output_tokens": 25 }
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
        assert_eq!(done.usage.input, 12);
        assert_eq!(done.usage.output, 25);
        let calls = done.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "read_file");
        assert_eq!(calls[0].2["path"], "a.txt");
    }
}
