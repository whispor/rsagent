//! The fields and helpers every HTTP provider shares.
//!
//! `OpenAiProvider` and `AnthropicProvider` differ only in their wire protocol;
//! their identity, model catalog, API-key lookup, and base-URL override are
//! identical. That common skeleton lives here so the two providers stay small
//! and a third one is trivial to add.

use std::time::Duration;

use crate::provider::Auth;
use crate::types::Model;

/// How long to wait for a TCP connection to the provider before giving up.
///
/// Only the *connect* phase is bounded; a request that has already connected is left
/// alone, because streaming responses are long-lived and must not be cut off mid-stream.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a single read from the provider may stall before the stream is failed.
///
/// This bounds each *read*, not the whole request: a streaming answer may legitimately take
/// minutes, but it should never go silent for this long. It catches a half-open connection —
/// or a gateway that accepts the request and then stops sending — so a turn fails instead of
/// waiting forever. Set well above the normal gap between tokens.
const READ_TIMEOUT: Duration = Duration::from_secs(120);

/// Build the provider's HTTP client: a bounded connect, a bounded idle read, and no
/// whole-request timeout.
///
/// A builder with only timeouts cannot fail to build; fall back to the default client rather
/// than panicking if it ever does.
fn build_client(read_timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(read_timeout)
        .build()
        .unwrap_or_default()
}

/// Identity, model catalog, and key/base-URL resolution for a provider.
#[derive(Debug)]
pub(super) struct ProviderCore {
    id: String,
    models: Vec<Model>,
    env_keys: Vec<String>,
    base_url_override: Option<String>,
    /// One HTTP client per provider, so its connection pool is reused across requests
    /// instead of rebuilt on every call. Deliberately built with no *whole-request* timeout:
    /// streaming responses are long-lived and must not be cut off mid-stream. A short
    /// *connect* timeout and a generous *read* (idle) timeout are set instead, so a server
    /// that accepts TCP but never answers — or stalls mid-stream — cannot hang a turn.
    client: reqwest::Client,
}

impl ProviderCore {
    /// Build a core from a provider id, its models, and its API-key env vars.
    pub(super) fn new(id: String, models: Vec<Model>, env_keys: Vec<String>) -> Self {
        ProviderCore {
            id,
            models,
            env_keys,
            base_url_override: None,
            client: build_client(READ_TIMEOUT),
        }
    }

    /// Replace the per-read (idle) timeout, rebuilding the client.
    ///
    /// Used to tolerate an endpoint that is slow between tokens, and by tests that need the
    /// timeout to fire quickly.
    pub(super) fn with_read_timeout(mut self, read_timeout: Duration) -> Self {
        self.client = build_client(read_timeout);
        self
    }

    /// The provider's shared HTTP client (cheap to clone; clones share the pool).
    pub(super) fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Point every model at `base_url` instead of its catalog value.
    pub(super) fn set_base_url(&mut self, base_url: String) {
        self.base_url_override = Some(base_url);
    }

    /// The provider id.
    pub(super) fn id(&self) -> &str {
        &self.id
    }

    /// The provider's model catalog.
    pub(super) fn models(&self) -> Vec<Model> {
        self.models.clone()
    }

    /// The first non-empty API key found among the configured env vars.
    pub(super) fn resolve_api_key(&self) -> Option<String> {
        self.env_keys
            .iter()
            .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
    }

    /// The message to report when no API key could be resolved.
    ///
    /// Names the environment variable(s) the provider looks for, so the failure is
    /// actionable — "missing API key" alone leaves the user guessing which key to set.
    pub(super) fn missing_key_message(&self) -> String {
        match self.env_keys.as_slice() {
            [] => "missing API key".to_string(),
            keys => format!(
                "missing API key (set {} or pass one explicitly)",
                keys.join(" / ")
            ),
        }
    }

    /// The base URL to use for `model`: the override if set, else the model's own.
    pub(super) fn effective_base_url(&self, model: &Model) -> String {
        self.base_url_override
            .clone()
            .unwrap_or_else(|| model.base_url.clone())
    }

    /// Resolve the authentication a request should carry.
    pub(super) fn resolve_auth(&self) -> Auth {
        Auth {
            api_key: self.resolve_api_key(),
            headers: Vec::new(),
            base_url: self.base_url_override.clone(),
        }
    }
}
