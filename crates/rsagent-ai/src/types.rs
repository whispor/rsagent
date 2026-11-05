//! Core value types shared across the whole crate.

use serde::{Deserialize, Serialize};

/// Current wall-clock time in milliseconds since the Unix epoch.
///
/// Values beyond `i64::MAX` (year 292 million) saturate instead of wrapping.
pub fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    i64::try_from(millis).unwrap_or(i64::MAX)
}

/// A kind of content a model can accept or produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    /// Plain text.
    Text,
    /// An image, delivered inline as base64-encoded data.
    Image,
}

/// Unified reasoning/thinking budget level, matching pi's vocabulary.
///
/// `XHigh` and `Max` are model-specific, opt-in levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    /// No extended reasoning.
    #[default]
    Off,
    /// A minimal reasoning budget.
    Minimal,
    /// A small reasoning budget.
    Low,
    /// A balanced reasoning budget.
    Medium,
    /// A large reasoning budget.
    High,
    /// A model-specific extra-high reasoning budget.
    XHigh,
    /// A model-specific maximum reasoning budget.
    Max,
}

/// Why generation stopped. Mirrors pi's `stopReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum StopReason {
    /// Only present on partial messages while the outcome is unknown.
    Pending,
    /// The final message this turn.
    Stop,
    /// Output hit the maximum token limit.
    Length,
    /// The model is calling tools and expects tool results.
    ToolUse,
    /// Generation failed; see `AssistantMessage::error_message`.
    Error,
    /// The request was cancelled.
    Aborted,
}

/// Token and (estimated) cost accounting for a single model call.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Input (prompt) tokens billed for this call.
    #[serde(default)]
    pub input: u64,
    /// Output (completion) tokens billed for this call.
    #[serde(default)]
    pub output: u64,
    /// Input tokens served from the provider's prompt cache.
    #[serde(default)]
    pub cache_read: u64,
    /// Input tokens written to the provider's prompt cache.
    #[serde(default)]
    pub cache_write: u64,
}

impl Usage {
    /// Total input plus output tokens, excluding cache accounting.
    pub fn total_tokens(&self) -> u64 {
        self.input + self.output
    }
}

/// Static metadata describing a concrete model. Plain serializable data: no behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    /// Provider-unique id used on the wire, e.g. `gpt-4o-mini`.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Owning provider id, used by [`crate::Models`] to route requests.
    pub provider: String,
    /// Wire protocol id, e.g. `openai-completions`.
    pub api: String,
    /// Base URL of the API endpoint (without the operation path).
    pub base_url: String,
    /// Maximum context window in tokens.
    #[serde(default)]
    pub context_window: u64,
    /// Maximum tokens the model may generate per response.
    #[serde(default)]
    pub max_tokens: u64,
    /// Whether the model exposes a reasoning/thinking channel.
    #[serde(default)]
    pub reasoning: bool,
    /// Modalities the model accepts as input.
    #[serde(default)]
    pub input: Vec<Modality>,
}

impl Model {
    /// Convenience constructor for an OpenAI-compatible chat model.
    pub fn openai_compatible(
        provider: impl Into<String>,
        id: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        let id = id.into();
        Model {
            name: id.clone(),
            id,
            provider: provider.into(),
            api: "openai-completions".to_string(),
            base_url: base_url.into(),
            context_window: 128_000,
            max_tokens: 16_384,
            reasoning: false,
            input: vec![Modality::Text],
        }
    }

    /// Enable the reasoning/thinking channel.
    #[must_use]
    pub fn with_reasoning(mut self) -> Self {
        self.reasoning = true;
        self
    }

    /// Disable the reasoning/thinking channel.
    #[must_use]
    pub fn without_reasoning(mut self) -> Self {
        self.reasoning = false;
        self
    }

    /// Set the context window, in tokens.
    #[must_use]
    pub fn with_context_window(mut self, window: u64) -> Self {
        self.context_window = window;
        self
    }
}
