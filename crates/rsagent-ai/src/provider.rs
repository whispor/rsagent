//! Providers, auth, request options, and the [`Models`] collection that routes requests.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{self, Stream};
use futures::StreamExt;

use crate::event::{ErrorReason, StreamEvent};
use crate::message::{AssistantMessage, Context};
use crate::types::{Model, ThinkingLevel};

/// A boxed, owned stream of normalized events. Providers own everything they need so
/// the stream is `'static` and can be moved across tasks.
pub type EventStream = Pin<Box<dyn Stream<Item = StreamEvent> + Send>>;

/// Resolved request auth. Explicit per-request values always win over these.
#[derive(Debug, Clone, Default)]
pub struct Auth {
    /// Provider-resolved API key, if any.
    pub api_key: Option<String>,
    /// Extra headers applied to every request.
    pub headers: Vec<(String, String)>,
    /// Provider-resolved base URL override.
    pub base_url: Option<String>,
}

/// Per-request options passed to a provider.
#[derive(Debug, Clone)]
pub struct StreamOptions {
    /// Explicit API key; overrides provider-resolved auth.
    pub api_key: Option<String>,
    /// Extra headers merged on top of provider-resolved auth.
    pub headers: Vec<(String, String)>,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Maximum tokens to generate.
    pub max_tokens: Option<u32>,
    /// Requested reasoning/thinking budget.
    pub thinking: ThinkingLevel,
    /// Session id used for provider-side prompt caching.
    pub session_id: Option<String>,
    /// How many times to retry a request that fails before the stream starts.
    ///
    /// Retries apply to connection errors and transient HTTP statuses (408, 429,
    /// and 5xx); a request that fails mid-stream is not retried.
    pub max_retries: u32,
}

impl Default for StreamOptions {
    fn default() -> Self {
        StreamOptions {
            api_key: None,
            headers: Vec::new(),
            temperature: None,
            max_tokens: None,
            thinking: ThinkingLevel::default(),
            session_id: None,
            max_retries: 2,
        }
    }
}

/// The runtime unit that owns a model catalog, auth, and stream behavior.
///
/// `stream` is intentionally *not* async: it returns immediately and reports all
/// failures as events, matching pi's contract.
pub trait Provider: Send + Sync {
    /// Stable provider id, e.g. `openai`.
    fn id(&self) -> &str;

    /// The provider's chat model catalog.
    fn models(&self) -> Vec<Model>;

    /// Look up one model by id.
    fn get_model(&self, id: &str) -> Option<Model> {
        self.models().into_iter().find(|m| m.id == id)
    }

    /// Resolve auth for a model. Default: nothing configured.
    fn resolve_auth(&self, _model: &Model) -> Auth {
        Auth::default()
    }

    /// Start a streaming request. Errors surface as events, never as panics.
    fn stream(&self, model: Model, context: Context, options: StreamOptions) -> EventStream;
}

/// A collection of providers that routes each request to the provider owning the model.
///
/// Cheap to clone: clones share the same registry.
#[derive(Clone, Default)]
pub struct Models {
    providers: Arc<HashMap<String, Arc<dyn Provider>>>,
}

impl std::fmt::Debug for Models {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Models")
            .field("providers", &self.providers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Models {
    /// Create an empty registry.
    pub fn new() -> Self {
        Models::default()
    }

    /// Register a provider, taking ownership.
    ///
    /// To keep a handle to a shared provider, give it a cheap `Clone` and register a
    /// clone; clones observe the same state.
    pub fn register(&mut self, provider: impl Provider + 'static) -> &mut Self {
        let provider: Arc<dyn Provider> = Arc::new(provider);
        Arc::make_mut(&mut self.providers).insert(provider.id().to_string(), provider);
        self
    }

    /// Look up a registered provider by id.
    pub fn get_provider(&self, id: &str) -> Option<&dyn Provider> {
        self.providers.get(id).map(AsRef::as_ref)
    }

    /// Every registered provider.
    pub fn providers(&self) -> Vec<&dyn Provider> {
        self.providers.values().map(AsRef::as_ref).collect()
    }

    /// Look up a model within a named provider.
    pub fn get_model(&self, provider: &str, id: &str) -> Option<Model> {
        self.providers.get(provider).and_then(|p| p.get_model(id))
    }

    /// Route a request through the owning provider, merging resolved auth.
    pub fn stream(
        &self,
        model: &Model,
        context: Context,
        mut options: StreamOptions,
    ) -> EventStream {
        let Some(provider) = self.providers.get(&model.provider).map(Arc::clone) else {
            return error_stream(
                model,
                format!("unknown provider: {}", model.provider),
                ErrorReason::Error,
            );
        };

        let auth = provider.resolve_auth(model);
        if options.api_key.is_none() {
            options.api_key = auth.api_key;
        }
        if !auth.headers.is_empty() {
            let mut merged = auth.headers;
            merged.append(&mut options.headers);
            options.headers = merged;
        }

        provider.stream(model.clone(), context, options)
    }

    /// Run a request to completion and return the final assistant message.
    pub async fn complete(
        &self,
        model: &Model,
        context: Context,
        options: StreamOptions,
    ) -> AssistantMessage {
        let mut stream = self.stream(model, context, options);
        let mut result: Option<AssistantMessage> = None;
        while let Some(event) = stream.next().await {
            if let Some(message) = event.terminal_message() {
                result = Some(message);
            }
        }
        result.unwrap_or_else(|| AssistantMessage::error(model, "stream ended without a result"))
    }
}

/// Build a stream that immediately reports an error.
pub(crate) fn error_stream(model: &Model, message: String, reason: ErrorReason) -> EventStream {
    let error = AssistantMessage::error(model, message);
    Box::pin(stream::iter(vec![StreamEvent::Error { reason, error }]))
}
