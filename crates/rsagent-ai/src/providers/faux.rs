//! An in-memory, scripted provider for tests and offline demos.
//!
//! Modeled on pi's `fauxProvider()`: it consumes a queue of pre-scripted responses and
//! emits the same normalized events a real provider would.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_stream::stream;

use crate::event::{ErrorReason, StreamEvent};
use crate::message::{AssistantMessage, ContentBlock, Context};
use crate::provider::{EventStream, Provider, StreamOptions};
use crate::types::{now_millis, Modality, Model, StopReason, Usage};

/// One scripted content block.
#[derive(Debug, Clone)]
pub enum FauxBlock {
    /// A text block.
    Text(String),
    /// A reasoning/thinking block.
    Thinking(String),
    /// A tool call with the given name and arguments.
    ToolCall {
        /// The tool name.
        name: String,
        /// The parsed tool arguments.
        arguments: serde_json::Value,
    },
}

/// One scripted assistant response.
#[derive(Debug, Clone)]
pub struct FauxResponse {
    /// The content blocks, emitted in order.
    pub blocks: Vec<FauxBlock>,
    /// Why the response stops.
    pub stop_reason: StopReason,
}

impl FauxResponse {
    /// A plain text reply.
    pub fn text(text: impl Into<String>) -> Self {
        FauxResponse {
            blocks: vec![FauxBlock::Text(text.into())],
            stop_reason: StopReason::Stop,
        }
    }

    /// A reply that requests one or more tool calls.
    pub fn tool_calls(calls: Vec<(impl Into<String>, serde_json::Value)>) -> Self {
        FauxResponse {
            blocks: calls
                .into_iter()
                .map(|(name, arguments)| FauxBlock::ToolCall {
                    name: name.into(),
                    arguments,
                })
                .collect(),
            stop_reason: StopReason::ToolUse,
        }
    }
}

/// A provider that replays scripted responses in request order.
///
/// Cheap to clone: clones share the same response queue and counters, so a caller can
/// register one clone and keep another to script it.
#[derive(Debug, Clone)]
pub struct FauxProvider {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    model: Model,
    responses: Mutex<VecDeque<FauxResponse>>,
    /// Delay between streamed chunks, in nanoseconds; zero for deterministic tests.
    chunk_delay_nanos: AtomicU64,
    call_count: Mutex<usize>,
}

impl FauxProvider {
    /// Create a provider with an empty response queue.
    pub fn new() -> Self {
        let model = Model {
            id: "faux-1".to_string(),
            name: "Faux 1".to_string(),
            provider: "faux".to_string(),
            api: "faux".to_string(),
            base_url: String::new(),
            context_window: 128_000,
            max_tokens: 4_096,
            reasoning: true,
            input: vec![Modality::Text],
        };
        FauxProvider {
            inner: Arc::new(Inner {
                model,
                responses: Mutex::new(VecDeque::new()),
                chunk_delay_nanos: AtomicU64::new(0),
                call_count: Mutex::new(0),
            }),
        }
    }

    /// The default faux model.
    pub fn model(&self) -> Model {
        self.inner.model.clone()
    }

    /// Replace the remaining response queue.
    ///
    /// # Panics
    ///
    /// Panics if the internal response lock is poisoned.
    pub fn set_responses(&self, responses: Vec<FauxResponse>) {
        *self
            .inner
            .responses
            .lock()
            .expect("faux response lock poisoned") = responses.into();
    }

    /// Append responses to the queue.
    ///
    /// # Panics
    ///
    /// Panics if the internal response lock is poisoned.
    pub fn append_responses(&self, responses: Vec<FauxResponse>) {
        self.inner
            .responses
            .lock()
            .expect("faux response lock poisoned")
            .extend(responses);
    }

    /// How many responses are still queued.
    ///
    /// # Panics
    ///
    /// Panics if the internal response lock is poisoned.
    pub fn pending_count(&self) -> usize {
        self.inner
            .responses
            .lock()
            .expect("faux response lock poisoned")
            .len()
    }

    /// How many requests have been made so far.
    ///
    /// # Panics
    ///
    /// Panics if the internal call-count lock is poisoned.
    pub fn call_count(&self) -> usize {
        *self
            .inner
            .call_count
            .lock()
            .expect("faux call-count lock poisoned")
    }

    /// Pace chunk delivery in real time (useful for UI demos).
    #[must_use]
    pub fn with_chunk_delay(self, delay: Duration) -> Self {
        self.inner.chunk_delay_nanos.store(
            u64::try_from(delay.as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self
    }
}

impl Default for FauxProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for FauxProvider {
    fn id(&self) -> &'static str {
        "faux"
    }

    fn models(&self) -> Vec<Model> {
        vec![self.inner.model.clone()]
    }

    fn stream(&self, model: Model, _context: Context, _options: StreamOptions) -> EventStream {
        *self
            .inner
            .call_count
            .lock()
            .expect("faux call-count lock poisoned") += 1;

        let next = self
            .inner
            .responses
            .lock()
            .expect("faux response lock poisoned")
            .pop_front();
        let delay = Duration::from_nanos(self.inner.chunk_delay_nanos.load(Ordering::Relaxed));

        Box::pin(stream! {
            let mut partial = AssistantMessage::partial(&model);
            yield StreamEvent::Start { partial: partial.clone() };

            let Some(response) = next else {
                let mut error = AssistantMessage::error(&model, "No more faux responses queued");
                error.timestamp = now_millis();
                yield StreamEvent::Error { reason: ErrorReason::Error, error };
                return;
            };

            for block in response.blocks {
                let index = partial.content.len();
                match block {
                    FauxBlock::Text(text) => {
                        partial.content.push(ContentBlock::Text { text: String::new() });
                        yield StreamEvent::TextStart { content_index: index };
                        for chunk in text.chars().map(|c| c.to_string()) {
                            if !delay.is_zero() {
                                tokio::time::sleep(delay).await;
                            }
                            if let Some(ContentBlock::Text { text }) = partial.content.get_mut(index) {
                                text.push_str(&chunk);
                            }
                            yield StreamEvent::TextDelta { content_index: index, delta: chunk };
                        }
                        let content = partial.content[index].as_text().unwrap_or("").to_string();
                        yield StreamEvent::TextEnd { content_index: index, content };
                    }
                    FauxBlock::Thinking(thinking) => {
                        partial.content.push(ContentBlock::Thinking { thinking: String::new(), signature: None });
                        yield StreamEvent::ThinkingStart { content_index: index };
                        for chunk in thinking.chars().map(|c| c.to_string()) {
                            if !delay.is_zero() {
                                tokio::time::sleep(delay).await;
                            }
                            if let Some(ContentBlock::Thinking { thinking, .. }) = partial.content.get_mut(index) {
                                thinking.push_str(&chunk);
                            }
                            yield StreamEvent::ThinkingDelta { content_index: index, delta: chunk };
                        }
                        let content = match &partial.content[index] {
                            ContentBlock::Thinking { thinking, .. } => thinking.clone(),
                            _ => String::new(),
                        };
                        yield StreamEvent::ThinkingEnd { content_index: index, content };
                    }
                    FauxBlock::ToolCall { name, arguments } => {
                        let id = format!("call_{index}");
                        partial.content.push(ContentBlock::ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            arguments: serde_json::json!({}),
                        });
                        yield StreamEvent::ToolCallStart { content_index: index };
                        let rendered = serde_json::to_string(&arguments).unwrap_or_default();
                        yield StreamEvent::ToolCallDelta { content_index: index, delta: rendered.clone() };
                        partial.content[index] = ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                        };
                        yield StreamEvent::ToolCallEnd {
                            content_index: index,
                            tool_call: partial.content[index].clone(),
                        };
                    }
                }
            }

            partial.stop_reason = response.stop_reason;
            partial.usage = Usage {
                input: 10,
                output: 20,
                ..Usage::default()
            };
            partial.timestamp = now_millis();
            yield StreamEvent::Done { reason: response.stop_reason, message: partial };
        })
    }
}
